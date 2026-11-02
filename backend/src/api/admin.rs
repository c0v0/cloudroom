//! Admin panel API: room requests, all rooms (list/detail/delete/transfer),
//! users, registration invites, audit log, stats. All endpoints require
//! role `admin` or `super_admin`; super_admin-only actions are marked.

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
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
        Err(ApiError::forbidden("super_需要管理员权限"))
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
            // 模式变更申请的申请人不能批准自己的请求（super_admin 提交时已直接生效）。
            if r.request_kind == "change_media_mode" && r.requester_id == actor {
                return Err(ApiError::forbidden("您不能批准自己的媒体模式变更申请"));
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
            let mut out = Vec::new();
            for r in &rows {
                let member_count = crate::infra::repo::count_members(conn, &r.room_id)?;
                let owner = r
                    .owner_user_id
                    .as_ref()
                    .map(|id| crate::infra::repo::get_user(conn, id))
                    .transpose()?
                    .flatten();
                out.push(json!({
                    "room": api::room_json(r, None),
                    "member_count": member_count,
                    "owner_username": owner.and_then(|u| u.username),
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
            let mut members = Vec::new();
            for m in ms {
                let u = crate::infra::repo::get_user(conn, &m.user_id)?;
                members.push(api::member_json(&m, u.as_ref()));
            }
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
    Ok(Json(json!({
        "users": users.iter().map(|u| api::user_json(u, &state.cfg)).collect::<Vec<_>>(),
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
    Ok(Json(json!({ "temporary_password": temp })))
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

// --------------------------------------------------------------- stats

pub async fn stats(
    State(state): State<AppState>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    require_admin(&me)?;
    let (users, rooms, pending, active, deleted, expired, messages) = state
        .db
        .with_sync(|conn| -> ApiResult<(i64, i64, i64, i64, i64, i64, i64)> {
            let users = crate::infra::repo::count_users(conn)?;
            let rooms = crate::infra::repo::count_rooms(conn)?;
            let pending = conn
                .query_row(
                    "SELECT COUNT(*) FROM room_requests WHERE status = 'pending'",
                    [],
                    |r| r.get(0),
                )
                .map_err(ApiError::from)?;
            let active = conn
                .query_row(
                    "SELECT COUNT(*) FROM rooms WHERE state = 'active'",
                    [],
                    |r| r.get(0),
                )
                .map_err(ApiError::from)?;
            let deleted = conn
                .query_row(
                    "SELECT COUNT(*) FROM rooms WHERE state = 'deleted'",
                    [],
                    |r| r.get(0),
                )
                .map_err(ApiError::from)?;
            let expired = conn
                .query_row(
                    "SELECT COUNT(*) FROM rooms WHERE state = 'expired'",
                    [],
                    |r| r.get(0),
                )
                .map_err(ApiError::from)?;
            let messages = conn
                .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
                .map_err(ApiError::from)?;
            Ok((users, rooms, pending, active, deleted, expired, messages))
        })
        .unwrap_or_default();
    Ok(Json(json!({
        "users": users,
        "rooms": rooms,
        "rooms_active": active,
        "rooms_deleted": deleted,
        "rooms_expired": expired,
        "room_requests_pending": pending,
        "messages": messages,
        "livekit_revocations_attempted": state.revocation.attempted(),
        "livekit_revocations_failed": state.revocation.failed(),
    })))
}
