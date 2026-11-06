//! Admin panel API: room requests, all rooms (list/detail/delete/transfer),
//! users, registration invites, audit log, stats. All endpoints require
//! role `admin` or `super_admin`; super_admin-only actions are marked.

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{self, AppState, AuthUser};
use crate::app::revocation;
use crate::domain::models::*;
use crate::error::{ApiError, ApiResult};

fn require_admin(me: &AuthUser) -> ApiResult<()> {
    if me.user.role.is_admin() {
        Ok(())
    } else {
        Err(ApiError::forbidden("需要管理员权限"))
    }
}

fn require_super(me: &AuthUser) -> ApiResult<()> {
    if me.user.role.is_super_admin() {
        Ok(())
    } else {
        Err(ApiError::forbidden("需要超级管理员权限"))
    }
}

// --------------------------------------------------------- room requests

#[derive(Deserialize)]
pub struct RequestListQuery {
    pub status: Option<String>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

pub async fn list_room_requests(
    State(state): State<AppState>,
    Query(q): Query<RequestListQuery>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    require_admin(&me)?;
    let status = q.status.clone();
    let offset = q.offset.unwrap_or(0).max(0);
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let (rows, total) = state
        .db
        .with(move |conn| {
            crate::infra::repo::list_room_requests(conn, None, status.as_deref(), offset, limit)
        })
        .await?;
    let requests = api::requests_json(&state, &rows).await;
    Ok(Json(json!({
        "requests": requests,
        "total": total,
    })))
}

pub async fn approve_room_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    require_admin(&me)?;
    let actor = me.user.user_id.clone();
    let st = state.clone();
    let st2 = st.clone();
    let (req, room) = st
        .db
        .with(move |conn| -> ApiResult<(RoomRequest, Room)> {
            let r = crate::infra::repo::get_room_request(conn, &request_id)?
                .ok_or_else(|| ApiError::not_found("申请不存在"))?;
            if r.status != "pending" {
                return Err(ApiError::conflict("该申请不在待审批状态"));
            }
            // 申请人不能批准自己的请求（super_admin 提交时已直接生效）。
            if r.requester_id == actor {
                return Err(ApiError::forbidden("您不能批准自己的申请"));
            }
            let room = if r.request_kind == "change_media_mode" {
                crate::app::rooms::apply_media_mode_change_for_request(
                    &st2,
                    conn,
                    &request_id,
                    &actor,
                )?
            } else {
                crate::app::rooms::create_room_for_request(&st2, conn, &request_id, &actor)?
            };
            Ok((r, room))
        })
        .await?;
    // 媒体模式变更：广播给房间内所有客户端，让它们刷新并退出旧模式的语音。
    if req.request_kind == "change_media_mode" {
        crate::app::events::broadcast_room_event(
            &state,
            &room.room_id,
            &json!({ "type": "room_settings", "room": api::room_json(&room, None) }),
        );
    }
    Ok(Json(json!({
        "request": api::room_request_json(&req),
        "room": api::room_json(&room, None),
    })))
}

#[derive(Deserialize)]
pub struct RejectReq {
    pub reason: String,
}

pub async fn reject_room_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    me: AuthUser,
    Json(req): Json<RejectReq>,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    require_admin(&me)?;
    let reason = req.reason.trim().to_string();
    if reason.is_empty() || reason.chars().count() > 500 {
        return Err(ApiError::validation("拒绝原因需为 1-500 个字符"));
    }
    let actor = me.user.user_id.clone();
    crate::app::admin::reject_room_request(&state, &request_id, &actor, &reason)?;
    Ok(Json(json!({ "ok": true })))
}

// ----------------------------------------------------------------- rooms

#[derive(Deserialize)]
pub struct RoomListQuery {
    pub state: Option<String>,
    pub kind: Option<String>,
    pub search: Option<String>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

pub async fn list_rooms(
    State(state): State<AppState>,
    Query(q): Query<RoomListQuery>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    require_admin(&me)?;
    let state_ = q.state.clone();
    let kind = q.kind.clone();
    let search = q.search.clone();
    let offset = q.offset.unwrap_or(0).max(0);
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let (rooms, total) = state
        .db
        .with(move |conn| -> ApiResult<(Vec<Value>, i64)> {
            let (rows, total) = crate::infra::repo::list_rooms(
                conn,
                state_.as_deref(),
                kind.as_deref(),
                search.as_deref(),
                offset,
                limit,
            )?;
            // 批量取成员数与房主用户名，避免逐房两次查询（N+1）。
            let room_ids: Vec<String> = rows.iter().map(|r| r.room_id.clone()).collect();
            let counts = crate::infra::repo::count_members_by_room(conn, &room_ids)?;
            let owner_ids: Vec<String> = rows
                .iter()
                .filter_map(|r| r.owner_user_id.clone())
                .collect();
            let owners = crate::infra::repo::users_by_ids(conn, &owner_ids)?;
            let mut out = Vec::new();
            for r in &rows {
                let member_count = counts.get(&r.room_id).copied().unwrap_or(0);
                let owner_username = r
                    .owner_user_id
                    .as_ref()
                    .and_then(|id| owners.get(id))
                    .and_then(|u| u.username.clone());
                out.push(json!({
                    "room": api::room_json(r, None),
                    "member_count": member_count,
                    "owner_username": owner_username,
                }));
            }
            Ok((out, total))
        })
        .await?;
    Ok(Json(json!({ "rooms": rooms, "total": total })))
}

pub async fn room_detail(
    State(state): State<AppState>,
    Path(code): Path<String>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    require_admin(&me)?;
    let code2 = code.clone();
    let (room, members) = state
        .db
        .with(move |conn| -> ApiResult<(Room, Vec<Value>)> {
            let room = crate::infra::repo::get_room_by_code(conn, &code2)?
                .ok_or_else(ApiError::room_not_found)?;
            let ms = crate::infra::repo::list_members(conn, &room.room_id)?;
            let uids: Vec<String> = ms.iter().map(|m| m.user_id.clone()).collect();
            let users = crate::infra::repo::users_by_ids(conn, &uids)?;
            let members: Vec<Value> = ms
                .iter()
                .map(|m| api::member_json(m, users.get(&m.user_id)))
                .collect();
            Ok((room, members))
        })
        .await?;
    Ok(Json(json!({
        "room": api::room_json(&room, None),
        "members": members,
    })))
}

pub async fn force_delete_room(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    require_admin(&me)?;
    let room = state
        .db
        .with_sync(|conn| crate::infra::repo::get_room_by_code(conn, &code))
        .unwrap_or(None)
        .ok_or_else(ApiError::room_not_found)?;
    if room.state != RoomState::Active {
        return Err(ApiError::conflict("房间未处于正常状态"));
    }
    crate::app::rooms::terminate_room(&state, &room, "admin_delete", &me.user.user_id)?;
    revocation::schedule_room(
        state.cfg.clone(),
        state.revocation.clone(),
        room.room_code.clone(),
    );
    state
        .hub
        .broadcast(&room.room_id, r#"{"type":"room_deleted"}"#);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct TransferReq {
    pub user_id: String,
}

pub async fn transfer_owner(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
    me: AuthUser,
    Json(req): Json<TransferReq>,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    require_super(&me)?;
    let room_id =
        crate::app::admin::transfer_owner(&state, &code, &req.user_id, &me.user.user_id).await?;
    crate::app::events::broadcast_room_event(
        &state,
        &room_id,
        &json!({
            "type": "owner_transferred",
            "owner_user_id": req.user_id,
        }),
    );
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------- users

#[derive(Deserialize)]
pub struct UserListQuery {
    pub search: Option<String>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

pub async fn list_users(
    State(state): State<AppState>,
    Query(q): Query<UserListQuery>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    require_admin(&me)?;
    let search = q.search.clone();
    let offset = q.offset.unwrap_or(0).max(0);
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let (users, total) = state
        .db
        .with(move |conn| crate::infra::repo::list_users(conn, search.as_deref(), offset, limit))
        .await?;
    let ids: Vec<String> = users.iter().map(|u| u.user_id.clone()).collect();
    let inviters = state
        .db
        .with(move |conn| crate::infra::repo::inviter_usernames(conn, &ids))
        .await?;
    let users_json: Vec<serde_json::Value> = users
        .iter()
        .map(|u| {
            let mut v = api::user_json(u, &state.cfg);
            v["invited_by"] = json!(inviters.get(&u.user_id));
            v
        })
        .collect();
    Ok(Json(json!({
        "users": users_json,
        "total": total,
    })))
}

#[derive(Deserialize)]
pub struct DisableReq {
    pub disabled: bool,
}

pub async fn set_user_disabled(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    me: AuthUser,
    Json(req): Json<DisableReq>,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    require_admin(&me)?;
    if user_id == me.user.user_id {
        return Err(ApiError::validation("不能禁用自己"));
    }
    crate::app::admin::set_user_disabled(
        &state,
        &user_id,
        &me.user.user_id,
        me.user.role,
        req.disabled,
    )?;
    state.hub.unregister_user(&user_id);
    if req.disabled {
        revocation::schedule_user(state.clone(), user_id.clone()).await;
    }
    Ok(Json(json!({ "ok": true })))
}

pub async fn reset_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    require_admin(&me)?;
    crate::app::admin::password_reset_target(&state, &user_id, me.user.role)?;
    let temp = crate::util::random_token().replace(['-', '_'], "a"); // URL-safe base64url chars only
    let temp = format!("{temp}!"); // ensure a symbol is present
    let hash = crate::infra::auth::hash_password_blocking(state.auth.clone(), temp.clone()).await?;
    crate::app::admin::apply_password_reset(&state, &user_id, &me.user.user_id, &hash)?;
    state.hub.unregister_user(&user_id);
    revocation::schedule_user(state.clone(), user_id.clone()).await;
    Ok((
        [(axum::http::header::CACHE_CONTROL, "no-store")],
        Json(json!({ "temporary_password": temp })),
    ))
}

#[derive(Deserialize)]
pub struct RoleReq {
    pub role: String,
}

pub async fn set_user_role(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    me: AuthUser,
    Json(req): Json<RoleReq>,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    require_super(&me)?;
    if user_id == me.user.user_id {
        return Err(ApiError::validation("不能修改自己的角色"));
    }
    let role = match req.role.as_str() {
        "admin" => UserRole::Admin,
        "user" => UserRole::User,
        other => {
            return Err(ApiError::validation(format!(
                "role must be admin or user, got {other}"
            )))
        }
    };
    crate::app::admin::set_user_role(&state, &user_id, role, &me.user.user_id)?;
    Ok(Json(json!({ "ok": true })))
}

// ------------------------------------------------------ registration invites

pub async fn create_registration_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    require_admin(&me)?;
    let raw = crate::util::random_token();
    let now = crate::util::now();
    let inv = Invite {
        invite_id: crate::util::random_id(),
        kind: "registration".into(),
        token_hash: crate::util::sha256_hex(raw.as_bytes()),
        room_id: None,
        created_by: me.user.user_id.clone(),
        max_role: "user".into(),
        created_at: now,
        expires_at: now + chrono::Duration::from_std(state.cfg.invite_ttl()).unwrap_or_default(),
        consumed_at: None,
        consumed_by: None,
        revoked_at: None,
        guest_user_id: None,
    };
    state.db.with_sync(|conn| -> ApiResult<()> {
        crate::infra::repo::insert_invite(conn, &inv)?;
        api::audited(
            conn,
            Some(&me.user.user_id),
            "invite.registration_created",
            Some("invite"),
            Some(&inv.invite_id),
            json!({}),
        )?;
        Ok(())
    })?;
    Ok(Json(json!({
        "invite": api::invite_json(&inv, None),
        "token": raw,
        "register_url": format!("/register?invite={raw}"),
    })))
}

pub async fn list_registration_invites(
    State(state): State<AppState>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    require_admin(&me)?;
    let rows = state
        .db
        .with_sync(crate::infra::repo::list_registration_invites)
        .unwrap_or_default();
    Ok(Json(
        json!({ "invites": rows.iter().map(|i| api::invite_json(i, None)).collect::<Vec<_>>() }),
    ))
}

pub async fn revoke_registration_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(invite_id): Path<String>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    require_admin(&me)?;
    crate::app::admin::revoke_registration_invite(&state, &invite_id, &me.user.user_id)?;
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------- audit

#[derive(Deserialize)]
pub struct AuditQuery {
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

pub async fn list_audit(
    State(state): State<AppState>,
    Query(q): Query<AuditQuery>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    require_admin(&me)?;
    let offset = q.offset.unwrap_or(0).max(0);
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let (rows, total) = state
        .db
        .with(move |conn| crate::infra::repo::list_audit(conn, offset, limit))
        .await?;
    Ok(Json(json!({
        "entries": rows.iter().map(api::audit_json).collect::<Vec<_>>(),
        "total": total,
    })))
}

#[derive(Deserialize)]
pub struct AuditExportQuery {
    pub limit: Option<i64>,
}

/// CSV export of the most recent audit entries (newest first), capped at
/// 10,000 rows so a single admin request cannot exhaust memory.
pub async fn export_audit(
    State(state): State<AppState>,
    Query(q): Query<AuditExportQuery>,
    me: AuthUser,
) -> ApiResult<Response> {
    require_admin(&me)?;
    let limit = q.limit.unwrap_or(5_000).clamp(1, 10_000);
    let (rows, _total) = state
        .db
        .with(move |conn| crate::infra::repo::list_audit(conn, 0, limit))
        .await?;
    let mut out = String::with_capacity(rows.len().saturating_mul(160) + 64);
    out.push_str("entry_id,created_at,actor_user_id,action,target_type,target_id,detail\n");
    for e in &rows {
        let fields = [
            e.entry_id.to_string(),
            e.created_at.to_rfc3339(),
            e.actor_user_id.clone().unwrap_or_default(),
            e.action.clone(),
            e.target_type.clone().unwrap_or_default(),
            e.target_id.clone().unwrap_or_default(),
            e.detail.clone(),
        ];
        out.push_str(
            &fields
                .iter()
                .map(|f| csv_field(f))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
    }
    let mut resp = out.into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"cloudroom-audit.csv\""),
    );
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(resp)
}

fn csv_field(value: &str) -> String {
    // Prefix spreadsheet formula leads so exported audit text cannot execute
    // as a formula when opened in Excel/LibreOffice.
    let guarded = if value
        .trim_start_matches(|c| c == ' ' || c == '\t')
        .chars()
        .next()
        .is_some_and(|c| matches!(c, '=' | '+' | '-' | '@'))
        || value.starts_with('\t')
        || value.starts_with('\r')
    {
        format!("'{value}")
    } else {
        value.to_string()
    };
    if guarded
        .bytes()
        .any(|b| matches!(b, b',' | b'"' | b'\n' | b'\r'))
    {
        format!("\"{}\"", guarded.replace('"', "\"\""))
    } else {
        guarded
    }
}

// --------------------------------------------------------------- stats

pub async fn stats(
    State(state): State<AppState>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    require_admin(&me)?;
    let (users, rooms, pending, active, deleted, expired, messages) = state
        .db
        .with_sync(|conn| -> ApiResult<(i64, i64, i64, i64, i64, i64, i64)> {
            // 一次查询取全部计数，避免 6 次往返。
            conn.query_row(
                "SELECT
                   (SELECT COUNT(*) FROM users WHERE role != 'guest'),
                   (SELECT COUNT(*) FROM rooms),
                   (SELECT COUNT(*) FROM room_requests WHERE status = 'pending'),
                   (SELECT COUNT(*) FROM rooms WHERE state = 'active'),
                   (SELECT COUNT(*) FROM rooms WHERE state = 'deleted'),
                   (SELECT COUNT(*) FROM rooms WHERE state = 'expired'),
                   (SELECT COUNT(*) FROM messages)",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                },
            )
            .map_err(ApiError::from)
        })
        .unwrap_or_default();
    let (attachment_bytes, attachments, push_subscriptions) = state
        .db
        .with_sync(crate::infra::repo::storage_stats)
        .unwrap_or_default();
    Ok(Json(json!({
        "users": users,
        "rooms": rooms,
        "rooms_active": active,
        "rooms_deleted": deleted,
        "rooms_expired": expired,
        "room_requests_pending": pending,
        "messages": messages,
        "attachment_bytes": attachment_bytes,
        "attachments": attachments,
        "push_subscriptions": push_subscriptions,
        "ws_connections": state.hub.total_connections(),
        "livekit_revocations_attempted": state.revocation.attempted(),
        "livekit_revocations_failed": state.revocation.failed(),
    })))
}

// ------------------------------------------------ guest upgrade requests

#[derive(Deserialize)]
pub struct GuestUpgradeQuery {
    pub status: Option<String>,
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

pub async fn list_guest_upgrade_requests(
    State(state): State<AppState>,
    me: AuthUser,
    Query(q): Query<GuestUpgradeQuery>,
) -> ApiResult<impl axum::response::IntoResponse> {
    require_admin(&me)?;
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let offset = q.offset.unwrap_or(0).max(0);
    let (rows, total) = state
        .db
        .with(move |conn| {
            crate::infra::repo::list_guest_upgrade_requests(
                conn,
                q.status.as_deref(),
                offset,
                limit,
            )
        })
        .await?;
    let mut requests = Vec::with_capacity(rows.len());
    for r in &rows {
        requests.push(api::guest_upgrade_request_json(&state, r).await);
    }
    Ok(Json(json!({ "requests": requests, "total": total })))
}

pub async fn approve_guest_upgrade_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    require_admin(&me)?;
    let actor = me.user.user_id.clone();
    let now = crate::util::now();
    let invite_ttl = state.cfg.invite_ttl();
    state.db.with_sync(|conn| -> ApiResult<()> {
        let req = crate::infra::repo::get_guest_upgrade_request(conn, &request_id)?
            .ok_or_else(|| ApiError::not_found("申请不存在"))?;
        if req.status != "pending" {
            return Err(ApiError::conflict("该申请不在待审批状态"));
        }
        let room = crate::infra::repo::get_room_by_id(conn, &req.room_id)?
            .ok_or_else(|| ApiError::not_found("房间不存在"))?;
        if room.state != RoomState::Active {
            return Err(ApiError::conflict("房间已失效"));
        }
        let member = crate::infra::repo::get_member(conn, &req.room_id, &req.guest_user_id)?
            .ok_or_else(|| ApiError::not_found("访客已不在房间"))?;
        if member.left_at.is_some() {
            return Err(ApiError::conflict("访客已离开房间"));
        }
        let raw = crate::util::random_token();
        let inv = Invite {
            invite_id: crate::util::random_id(),
            kind: "registration".into(),
            token_hash: crate::util::sha256_hex(raw.as_bytes()),
            room_id: None,
            created_by: actor.clone(),
            max_role: "user".into(),
            created_at: now,
            expires_at: now + chrono::Duration::from_std(invite_ttl).unwrap_or_default(),
            consumed_at: None,
            consumed_by: None,
            revoked_at: None,
            guest_user_id: Some(req.guest_user_id.clone()),
        };
        crate::infra::repo::insert_invite(conn, &inv)?;
        if !crate::infra::repo::decide_guest_upgrade_request(
            conn,
            &request_id,
            "approved",
            &actor,
            Some(&inv.invite_id),
        )? {
            return Err(ApiError::conflict("该申请已被处理"));
        }
        crate::app::audit::audited(
            conn,
            Some(&actor),
            "guest_upgrade.approved",
            Some("user"),
            Some(&req.guest_user_id),
            json!({ "room_id": req.room_id, "invite_id": inv.invite_id }),
        )?;
        Ok(())
    })?;
    let updated = state
        .db
        .with_sync(|conn| crate::infra::repo::get_guest_upgrade_request(conn, &request_id))?;
    let v = match updated.as_ref() {
        Some(r) => api::guest_upgrade_request_json(&state, r).await,
        None => json!({ "request_id": request_id }),
    };
    Ok(Json(json!({ "ok": true, "request": v })))
}

pub async fn reject_guest_upgrade_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    require_admin(&me)?;
    let actor = me.user.user_id.clone();
    let req = state.db.with_sync(|conn| -> ApiResult<GuestUpgradeRequest> {
        let req = crate::infra::repo::get_guest_upgrade_request(conn, &request_id)?
            .ok_or_else(|| ApiError::not_found("申请不存在"))?;
        if !crate::infra::repo::decide_guest_upgrade_request(
            conn,
            &request_id,
            "rejected",
            &actor,
            None,
        )? {
            return Err(ApiError::conflict("该申请不在待审批状态"));
        }
        crate::app::audit::audited(
            conn,
            Some(&actor),
            "guest_upgrade.rejected",
            Some("user"),
            Some(&req.guest_user_id),
            json!({ "room_id": req.room_id }),
        )?;
        Ok(req)
    })?;
    let v = api::guest_upgrade_request_json(&state, &req).await;
    Ok(Json(json!({ "ok": true, "request": v })))
}

#[cfg(test)]
mod tests {
    use super::csv_field;

    #[test]
    fn csv_export_escapes_commas_quotes_and_newlines() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("a\"b"), "\"a\"\"b\"");
        assert_eq!(csv_field("a\nb"), "\"a\nb\"");
        assert_eq!(csv_field("=cmd"), "'=cmd");
        assert_eq!(csv_field("  =cmd"), "'  =cmd");
        assert_eq!(csv_field("+cmd"), "'+cmd");
    }
}
