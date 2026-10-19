//! Room endpoints: requests, join/leave, settings, invites, members, mutes,
//! messages, direct rooms, ICE servers, media tokens.

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{self, active_member_room, AppState, AuthUser, MaybeAuth};
use crate::domain::models::*;
use crate::error::{ApiError, ApiResult};
use crate::util;
use axum::response::{IntoResponse, Response};

// ------------------------------------------------------- room requests

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRoomRequestReq {
    /// "create_room" | "change_media_mode"
    pub kind: String,
    pub note: Option<String>,
    /// Only honored for create_room requests and when the requester is super_admin.
    pub desired_code: Option<String>,
    /// Target room code, required for change_media_mode requests.
    pub room_code: Option<String>,
    /// "p2p" | "sfu"
    pub media_mode: String,
    /// 音频码率（kbps，0=自动）；仅建房申请携带。门槛由 voice.bitrate_tiers 配置决定。
    pub audio_bitrate: Option<u32>,
}

pub async fn create_room_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    me: AuthUser,
    Json(req): Json<CreateRoomRequestReq>,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    if me.user.role == UserRole::Guest {
        return Err(ApiError::forbidden("访客不能申请建房"));
    }
    if !state.rl.allow(
        "room_request",
        &me.user.user_id,
        std::time::Duration::from_secs(3600),
        state.cfg.rate_limit.room_request_per_hour_per_user,
    ) {
        return Err(ApiError::rate_limited("建房申请过于频繁"));
    }
    let note = req.note.unwrap_or_default();
    if note.chars().count() > state.cfg.rooms.max_note_chars {
        return Err(ApiError::validation("说明过长"));
    }
    if util::has_control_chars(&note) {
        return Err(ApiError::validation("说明包含控制字符"));
    }
    let media_mode = MediaMode::from_str(&req.media_mode)
        .ok_or_else(|| ApiError::validation("media_mode 必须为 \"p2p\" 或 \"sfu\""))?;
    // 码率门槛：按申请人（将任房主）的账号角色校验。
    let audio_bitrate = req.audio_bitrate.unwrap_or(0);
    if !state
        .cfg
        .voice
        .can_set_bitrate(me.user.role.as_str(), audio_bitrate)
    {
        return Err(ApiError::validation(
            "该音频码率档位不可用（权限不足或档位不存在）",
        ));
    }
    let request_id = util::random_id();

    match req.kind.as_str() {
        "create_room" => {
            if req.room_code.is_some() {
                return Err(ApiError::validation("room_code 仅适用于媒体模式变更申请"));
            }
            let desired_code = match req.desired_code {
                Some(code) if !code.trim().is_empty() => {
                    if !me.user.role.is_super_admin() {
                        return Err(ApiError::forbidden("只有超级管理员可以指定自定义房间号"));
                    }
                    Some(
                        crate::domain::codes::validate_custom_code(&state.cfg, &code)
                            .map_err(|e| crate::error::ApiError::validation(e))?,
                    )
                }
                _ => None,
            };
            let req_row = RoomRequest {
                request_id: request_id.clone(),
                requester_id: me.user.user_id.clone(),
                request_kind: "create_room".into(),
                target_room_id: None,
                media_mode,
                audio_bitrate,
                note,
                desired_code,
                status: "pending".into(),
                created_at: util::now(),
                decided_at: None,
                decided_by: None,
                decision_reason: None,
                created_room_id: None,
            };
            state.db.with_sync(|conn| -> ApiResult<()> {
                crate::infra::repo::insert_room_request(conn, &req_row)?;
                api::audited(
                    conn,
                    Some(&me.user.user_id),
                    "room_request.created",
                    Some("room_request"),
                    Some(&request_id),
                    json!({ "kind": "create_room", "media_mode": media_mode.as_str() }),
                )?;
                Ok(())
            })?;
            // 超级管理员免审批：提交即建房
            if me.user.role.is_super_admin() {
                let uid = me.user.user_id.clone();
                let rid = request_id.clone();
                let st = state.clone();
                let st2 = st.clone();
                let room = st
                    .db
                    .with(move |conn| create_room_for_request(&st2, conn, &rid, &uid))
                    .await?;
                // 申请状态已变为 approved，响应回读最新状态而非插入时的 pending。
                let approved = st
                    .db
                    .with_sync(|conn| crate::infra::repo::get_room_request(conn, &request_id))
                    .ok()
                    .flatten()
                    .map(|r| api::room_request_json(&r))
                    .unwrap_or_else(|| api::room_request_json(&req_row));
                return Ok(Json(json!({
                    "request": approved,
                    "room": api::room_json(&room, Some("owner")),
                    "auto_approved": true,
                })));
            }
            Ok(Json(json!({ "request": api::room_request_json(&req_row) })))
        }
        "change_media_mode" => {
            if req.desired_code.is_some() {
                return Err(ApiError::validation("desired_code 仅适用于建房申请"));
            }
            let code = req.room_code.clone().ok_or_else(|| {
                ApiError::validation("room_code is required for change_media_mode requests")
            })?;
            let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
            if room.kind != RoomKind::Standard {
                return Err(ApiError::validation(
                    "媒体模式变更仅适用于普通房间; direct rooms inherit the parent room mode",
                ));
            }
            if member.room_role != RoomRole::Owner {
                return Err(ApiError::forbidden("只有房主可以申请媒体模式变更"));
            }
            if room.media_mode == media_mode {
                return Err(ApiError::validation("房间已使用该媒体模式"));
            }
            // 码率仅对 SFU 目标有意义：切到 P2P 时强制 0。
            let change_bitrate = if media_mode == MediaMode::Sfu {
                audio_bitrate
            } else {
                0
            };
            let target_room_id = room.room_id.clone();
            let req_row = RoomRequest {
                request_id: request_id.clone(),
                requester_id: me.user.user_id.clone(),
                request_kind: "change_media_mode".into(),
                target_room_id: Some(target_room_id.clone()),
                media_mode,
                audio_bitrate: change_bitrate,
                note,
                desired_code: None,
                status: "pending".into(),
                created_at: util::now(),
                decided_at: None,
                decided_by: None,
                decision_reason: None,
                created_room_id: None,
            };
            state.db.with_sync(|conn| -> ApiResult<()> {
                if crate::infra::repo::pending_media_change(conn, &target_room_id)?.is_some() {
                    return Err(ApiError::conflict("该房间已有待审批的媒体模式变更申请"));
                }
                crate::infra::repo::insert_room_request(conn, &req_row)?;
                api::audited(
                    conn,
                    Some(&me.user.user_id),
                    "room_request.created",
                    Some("room_request"),
                    Some(&request_id),
                    json!({
                        "kind": "change_media_mode",
                        "room_code": code,
                        "media_mode": media_mode.as_str(),
                    }),
                )?;
                Ok(())
            })?;
            let mut req_json = api::room_request_json(&req_row);
            if let Some(obj) = req_json.as_object_mut() {
                obj.insert("target_room_code".into(), json!(code));
            }
            // 超级管理员免审批：提交即生效
            if me.user.role.is_super_admin() {
                let rid = request_id.clone();
                let uid = me.user.user_id.clone();
                let st = state.clone();
                let st2 = st.clone();
                let room = st
                    .db
                    .with(move |conn| apply_media_mode_change_for_request(&st2, conn, &rid, &uid))
                    .await?;
                // 申请状态已变为 approved，响应回读最新状态。
                let approved = st
                    .db
                    .with_sync(|conn| crate::infra::repo::get_room_request(conn, &request_id))
                    .ok()
                    .flatten()
                    .map(|r| api::room_request_json(&r))
                    .unwrap_or_else(|| req_json.clone());
                return Ok(Json(json!({
                    "request": approved,
                    "room": api::room_json(&room, Some("owner")),
                    "auto_approved": true,
                })));
            }
            Ok(Json(json!({ "request": req_json })))
        }
        other => Err(ApiError::validation(format!(
            "kind 必须为 \"create_room\" 或 \"change_media_mode\"，实际为 {other:?}"
        ))),
    }
}

pub async fn my_room_requests(
    State(state): State<AppState>,
    me: AuthUser,
    Query(params): Query<PagingQuery>,
) -> ApiResult<impl axum::response::IntoResponse> {
    let uid = me.user.user_id.clone();
    let (limit, offset) = paging(&params);
    let (rows, total) = state
        .db
        .with(move |conn| {
            crate::infra::repo::list_room_requests(conn, Some(&uid), None, offset, limit)
        })
        .await?;
    let requests = api::requests_json(&state, &rows).await;
    Ok(Json(json!({ "requests": requests, "total": total })))
}

pub async fn cancel_room_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let uid = me.user.user_id.clone();
    state.db.with_sync(|conn| -> ApiResult<()> {
        crate::infra::repo::cancel_room_request(conn, &request_id, &uid)?;
        api::audited(
            conn,
            Some(&uid),
            "room_request.cancelled",
            Some("room_request"),
            Some(&request_id),
            json!({}),
        )?;
        Ok(())
    })?;
    Ok(Json(json!({ "ok": true })))
}

// ------------------------------------------------------------- room info

#[derive(Deserialize)]
pub struct PagingQuery {
    pub offset: Option<i64>,
    pub limit: Option<i64>,
}

pub fn paging(q: &PagingQuery) -> (i64, i64) {
    (
        q.limit.unwrap_or(50).clamp(1, 200),
        q.offset.unwrap_or(0).max(0),
    )
}

pub async fn get_room(
    State(state): State<AppState>,
    Path(code): Path<String>,
    me: MaybeAuth,
) -> ApiResult<impl axum::response::IntoResponse> {
    // 未登录也返回 403（而不是 401）：前端据此展示"凭邀请加入"流程，
    // 未注册用户同样可以进房。真正的鉴权在 join / ws / 消息等端点。
    let uid =
        me.0.map(|m| m.user.user_id)
            .ok_or_else(|| ApiError::forbidden("不是房间成员 of this room"))?;
    let (room, member) = active_member_room(&state, &code, &uid).await?;
    let room_id = room.room_id.clone();
    let is_owner = member.room_role == RoomRole::Owner;
    let caps = capabilities(&member);
    let can_see_mutes =
        member_is_capable(&member, "mute_member") || member_is_capable(&member, "mute_room");
    let room_muted = room.room_muted;
    let (members, mutes, speak) = state
        .db
        .with(move |conn| -> ApiResult<(Vec<Value>, Vec<Value>, Value)> {
            let ms = crate::infra::repo::list_members(conn, &room_id)?;
            let mut members = Vec::new();
            for m in ms {
                let u = crate::infra::repo::get_user(conn, &m.user_id)?;
                members.push(api::member_json(&m, u.as_ref()));
            }
            let mutes = if can_see_mutes {
                crate::infra::repo::list_active_mutes(conn, &room_id, &util::now())?
                    .iter()
                    .map(|m| {
                        json!({
                            "mute_id": m.mute_id,
                            "scope": m.scope,
                            "target_user_id": m.target_user_id,
                            "muted_by": m.muted_by,
                            "created_at": m.created_at.to_rfc3339(),
                            "expires_at": m.expires_at.map(|d| d.to_rfc3339()),
                        })
                    })
                    .collect()
            } else {
                vec![]
            };
            let granted = crate::infra::repo::has_speak_grant(conn, &room_id, &uid)?;
            let pending = crate::infra::repo::list_speak_requests(conn, &room_id)?
                .iter()
                .any(|(u, _)| u == &uid);
            let mut speak = json!({
                // 禁言中房主同样需申请（申请自动通过）。
                "can_speak": !room_muted || granted,
                "granted": granted,
                "pending": pending,
            });
            // 房主可见申请/获权名单（含昵称）。
            if is_owner {
                let grants = crate::infra::repo::list_speak_grants(conn, &room_id)?;
                let requests = crate::infra::repo::list_speak_requests(conn, &room_id)?;
                let nick = |conn: &mut rusqlite::Connection, uid: &str| -> String {
                    crate::infra::repo::get_member(conn, &room_id, uid)
                        .ok()
                        .flatten()
                        .map(|m| m.nickname)
                        .unwrap_or_else(|| uid.chars().take(8).collect())
                };
                let g: Vec<Value> = grants
                    .iter()
                    .map(|(u, at)| json!({ "user_id": u, "nickname": nick(conn, u), "granted_at": at }))
                    .collect();
                let r: Vec<Value> = requests
                    .iter()
                    .map(|(u, at)| json!({ "user_id": u, "nickname": nick(conn, u), "created_at": at }))
                    .collect();
                speak["grants"] = json!(g);
                speak["requests"] = json!(r);
            }
            Ok((members, mutes, speak))
        })
        .await?;
    Ok(Json(json!({
        "room": api::room_json(&room, Some(member.room_role.as_str())),
        "members": members,
        "mutes": mutes,
        "capabilities": caps,
        "voice": speak,
    })))
}

fn member_is_capable(member: &RoomMember, _cap: &str) -> bool {
    member.room_role == RoomRole::Owner
}

pub fn capabilities(member: &RoomMember) -> Vec<&'static str> {
    match member.room_role {
        RoomRole::Owner => vec![
            "manage_room",
            "manage_members",
            "manage_invites",
            "manage_nicknames",
            "mute_member",
            "mute_room",
            "delete_room",
            "send_text",
            "join_voice",
        ],
        RoomRole::Member => vec!["send_text", "join_voice"],
    }
}

// ---------------------------------------------------------- join / leave

#[derive(Deserialize)]
pub struct JoinReq {
    pub invite_token: Option<String>,
    pub nickname: Option<String>,
}

pub async fn join_room(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
    me: MaybeAuth,
    Json(req): Json<JoinReq>,
) -> ApiResult<Response> {
    // 防高频进出：短时间反复加入会被限流。
    let rl_key =
        me.0.as_ref()
            .map(|m| m.user.user_id.clone())
            .unwrap_or_else(|| format!("ip:{}", api::client_ip(&state, &headers)));
    if !state.rl.allow(
        "room_join",
        &rl_key,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.room_join_per_minute_per_user,
    ) {
        return Err(ApiError::rate_limited("加入房间过于频繁，请稍后再试"));
    }
    // 邀请加入不要求 CSRF/登录：未注册用户凭邀请直接进房（创建 guest 会话）。
    // 邀请 token 本身就是授权凭证；已登录用户同样走此流程。
    // 注册与否只影响能否申请建房，不影响进房。
    let (uid, new_session_raw): (String, Option<String>) = match me.0 {
        Some(m) => (m.user.user_id, None),
        None => {
            let guest_id = util::random_id();
            let raw = util::random_token();
            let mut session = state.auth.new_session(
                &guest_id,
                Some(api::client_ip(&state, &headers)),
                headers
                    .get(axum::http::header::USER_AGENT)
                    .and_then(|v| v.to_str().ok())
                    .map(|x| x.to_string()),
            );
            session.token_hash = util::sha256_hex(raw.as_bytes());
            state
                .db
                .with_sync(|conn| -> ApiResult<()> {
                    crate::infra::repo::create_user(conn, &guest_id, None, None, UserRole::Guest)?;
                    crate::infra::repo::create_session(conn, &session)?;
                    Ok(())
                })
                .map_err(|e| ApiError::internal(format!("guest 会话创建失败: {}", e.message)))?;
            (guest_id, Some(raw))
        }
    };
    let code2 = code.clone();
    let cfg = state.cfg.clone();
    let outcome = state
        .db
        .with(move |conn| -> ApiResult<(Room, RoomMember, bool)> {
            let room = crate::infra::repo::get_room_by_code(conn, &code2)?
                .ok_or_else(ApiError::room_not_found)?;
            if room.state != RoomState::Active {
                return Err(ApiError::room_unavailable());
            }
            // Already a member (or rejoining after leaving)?
            if let Some(m) = crate::infra::repo::get_member(conn, &room.room_id, &uid)? {
                if m.left_at.is_none() {
                    return Err(ApiError::conflict("您已是该房间成员"));
                }
                // Rejoin: restore membership.
                crate::infra::repo::leave_room(conn, &room.room_id, &uid).ok(); // no-op guard
                conn.execute(
                    "UPDATE room_members SET left_at = NULL, joined_at = ?1 WHERE room_id = ?2 AND user_id = ?3",
                    rusqlite::params![util::now_rfc3339(), room.room_id, uid],
                )
                .map_err(ApiError::from)?;
                let m = crate::infra::repo::get_member(conn, &room.room_id, &uid)?.unwrap();
                let seq = crate::infra::repo::bump_room_seq(conn, &room.room_id)?;
                let _ = seq;
                return Ok((room, m, true));
            }
            // Owner rejoins without invite; everyone else needs an invite.
            if Some(uid.as_str()) == room.owner_user_id.as_deref() {
                let nickname = default_nickname(&cfg, conn, &uid, &room.room_id)?;
                crate::infra::repo::add_member(conn, &room.room_id, &uid, RoomRole::Owner, &nickname)?;
                crate::infra::repo::bump_room_seq(conn, &room.room_id)?;
                let m = crate::infra::repo::get_member(conn, &room.room_id, &uid)?.unwrap();
                return Ok((room, m, false));
            }
            // 成员上限校验（房主/已是成员不受限，仅限新成员加入）。
            if room.max_members > 0 {
                let active = crate::infra::repo::count_active_members(conn, &room.room_id)?;
                if active >= room.max_members {
                    return Err(ApiError::new(
                        axum::http::StatusCode::CONFLICT,
                        "room_full",
                        "房间成员已满",
                    ));
                }
            }
            let token = req.invite_token.as_deref().ok_or_else(|| {
                ApiError::forbidden("加入该房间需要有效邀请")
            })?;
            redeem_invite(conn, &cfg, token, &room, &uid, req.nickname.as_deref())?;
            crate::infra::repo::bump_room_seq(conn, &room.room_id)?;
            let m = crate::infra::repo::get_member(conn, &room.room_id, &uid)?.unwrap();
            Ok((room, m, false))
        })
        .await?;
    let (room, member, _rejoined) = outcome;
    broadcast_room_event(
        &state,
        &room.room_id,
        &json!({
            "type": "member_joined",
            "member": api::member_json(&member, None),
        }),
    );
    // 文字频道播报：成员进出，优先显示房间昵称
    let nick = member.nickname.clone();
    let room_id_b = room.room_id.clone();
    let sys = state
        .db
        .with_sync(|conn| -> ApiResult<Value> {
            let m = crate::infra::repo::insert_system_message(
                conn,
                &room_id_b,
                &format!("{nick} 加入了房间"),
            )?;
            Ok(api::message_json(&m, None, None))
        })
        .unwrap_or(Value::Null);
    if !sys.is_null() {
        state.hub.broadcast(
            &room.room_id,
            &serde_json::to_string(&json!({ "type": "message", "message": sys }))
                .unwrap_or_default(),
        );
    }
    let mut resp = Json(json!({
        "room": api::room_json(&room, Some(member.room_role.as_str())),
        "member": api::member_json(&member, None),
    }))
    .into_response();
    if let Some(raw) = new_session_raw {
        resp.headers_mut().insert(
            axum::http::header::SET_COOKIE,
            state.auth.session_cookie(&raw).parse().unwrap(),
        );
    }
    Ok(resp)
}

fn default_nickname(
    _cfg: &crate::config::Config,
    conn: &mut rusqlite::Connection,
    uid: &str,
    room_id: &str,
) -> ApiResult<String> {
    // Reuse a previous nickname if one exists.
    if let Some(m) = crate::infra::repo::get_member(conn, room_id, uid)? {
        if !m.nickname.is_empty() {
            return Ok(m.nickname);
        }
    }
    Ok(crate::infra::repo::get_user(conn, uid)?
        .and_then(|u| u.username)
        .unwrap_or_else(|| format!("guest-{}", &uid[..8])))
}

fn redeem_invite(
    conn: &mut rusqlite::Connection,
    cfg: &crate::config::Config,
    raw: &str,
    room: &Room,
    uid: &str,
    requested_nickname: Option<&str>,
) -> ApiResult<()> {
    let token_hash = util::sha256_hex(raw.as_bytes());
    let inv = crate::infra::repo::get_invite_by_hash(conn, &token_hash)?
        .ok_or_else(|| ApiError::bad_request("invite_invalid", "邀请不存在"))?;
    if inv.room_id.as_deref() != Some(room.room_id.as_str()) || inv.kind != "room" {
        return Err(ApiError::bad_request("invite_invalid", "邀请不属于该房间"));
    }
    if !inv.active(util::now()) {
        return Err(ApiError::bad_request(
            "invite_invalid",
            "邀请已过期、已撤销或已被使用",
        ));
    }
    let role = RoomRole::Member;
    let nickname = match requested_nickname {
        Some(n) if util::validate_nickname(n, cfg.security.max_nickname_graphemes) => n.to_string(),
        _ => {
            // Guest nickname: derived from invite; registered user: username.
            let u = crate::infra::repo::get_user(conn, uid)?;
            u.and_then(|x| x.username)
                .unwrap_or_else(|| format!("guest-{}", &uid[..8]))
        }
    };
    // Atomic consumption; guests get a user row first (FK on consumed_by).
    let user = crate::infra::repo::get_user(conn, uid)?;
    if user.is_none() {
        crate::infra::repo::create_user(conn, uid, None, None, UserRole::Guest)?;
    }
    if !crate::infra::repo::consume_invite(conn, &inv.invite_id, uid)? {
        return Err(ApiError::bad_request("invite_invalid", "邀请已被使用"));
    }
    crate::infra::repo::add_member(conn, &room.room_id, uid, role, &nickname)?;
    api::audited(
        conn,
        Some(uid),
        "invite.redeemed",
        Some("invite"),
        Some(&inv.invite_id),
        json!({ "room_code": room.room_code }),
    )?;
    Ok(())
}

pub async fn leave_room(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    if !state.rl.allow(
        "room_leave",
        &me.user.user_id,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.room_leave_per_minute_per_user,
    ) {
        return Err(ApiError::rate_limited("离开房间过于频繁，请稍后再试"));
    }
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    let room_id = room.room_id.clone();
    let uid = me.user.user_id.clone();
    state.db.with_sync(|conn| -> ApiResult<()> {
        crate::infra::repo::leave_room(conn, &room_id, &uid)?;
        crate::infra::repo::bump_room_seq(conn, &room_id)?;
        api::audited(
            conn,
            Some(&uid),
            "room.left",
            Some("room"),
            Some(&room_id),
            json!({}),
        )?;
        Ok(())
    })?;
    state.hub.unregister_all(&room_id, &uid);
    broadcast_room_event(
        &state,
        &room_id,
        &json!({
            "type": "member_left",
            "user_id": uid,
            "member": api::member_json(&member, Some(&me.user)),
        }),
    );
    let nick = member.nickname.clone();
    let room_id_b = room_id.clone();
    let sys = state
        .db
        .with_sync(|conn| -> ApiResult<Value> {
            let m = crate::infra::repo::insert_system_message(
                conn,
                &room_id_b,
                &format!("{nick} 离开了房间"),
            )?;
            Ok(api::message_json(&m, None, None))
        })
        .unwrap_or(Value::Null);
    if !sys.is_null() {
        state.hub.broadcast(
            &room_id,
            &serde_json::to_string(&json!({ "type": "message", "message": sys }))
                .unwrap_or_default(),
        );
    }
    Ok(Json(json!({ "ok": true })))
}

// ------------------------------------------------------------ settings

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsReq {
    pub text_enabled: Option<bool>,
    pub allow_direct_messages: Option<bool>,
    pub expires_at: Option<String>, // RFC3339 or null to clear
    /// 音频码率（kbps，0=自动）。门槛由 voice.bitrate_tiers 配置决定。
    pub audio_bitrate: Option<u32>,
    /// 房间公告（空串清除）。
    pub announcement: Option<String>,
    /// 成员上限（0=不限）。
    pub max_members: Option<i64>,
}

pub async fn update_settings(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
    me: AuthUser,
    Json(req): Json<SettingsReq>,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    if room.kind == RoomKind::Standard {
        api::require_cap(&state, &member, &room, "manage_room")?;
    } else if member.room_role != RoomRole::Owner {
        return Err(ApiError::forbidden("只有私聊房主可以修改设置"));
    }
    // 媒体模式只能通过房间申请/管理员审批通道变更（change_media_mode）。
    let expires_at = match req.expires_at.as_deref() {
        None => room.expires_at,
        Some("") | Some("null") => None,
        Some(s) => Some(
            chrono::DateTime::parse_from_rfc3339(s)
                .map_err(|_| ApiError::validation("过期时间无效（需为 RFC3339 UTC 格式）"))?
                .with_timezone(&chrono::Utc),
        ),
    };
    let room_id = room.room_id.clone();
    let text_enabled = req.text_enabled.unwrap_or(room.text_enabled);
    let adm = req
        .allow_direct_messages
        .unwrap_or(room.allow_direct_messages);
    let audio_bitrate = req.audio_bitrate.unwrap_or(room.audio_bitrate);
    let announcement = req
        .announcement
        .map(|s| s.chars().take(500).collect::<String>())
        .unwrap_or_else(|| room.announcement.clone());
    if announcement.chars().count() > 500 {
        return Err(ApiError::validation("公告过长（最多 500 字）"));
    }
    let max_members = req.max_members.unwrap_or(room.max_members).clamp(0, 500);
    // 码率门槛按当前房主（owner）账号角色校验。
    if !state
        .cfg
        .voice
        .can_set_bitrate(me.user.role.as_str(), audio_bitrate)
    {
        return Err(ApiError::validation(
            "该音频码率档位不可用（权限不足或档位不存在）",
        ));
    }
    state.db.with_sync(|conn| -> ApiResult<()> {
        crate::infra::repo::update_room_settings(
            conn,
            &room_id,
            text_enabled,
            adm,
            expires_at.as_ref(),
            audio_bitrate,
            &announcement,
            max_members,
        )?;
        crate::infra::repo::bump_room_seq(conn, &room_id)?;
        api::audited(
            conn,
            Some(&me.user.user_id),
            "room.settings_updated",
            Some("room"),
            Some(&room_id),
            json!({
                "text_enabled": text_enabled,
                "allow_direct_messages": adm,
                "expires_at": expires_at.map(|d| d.to_rfc3339()),
            }),
        )?;
        Ok(())
    })?;
    let (room2, _) = active_member_room(&state, &code, &me.user.user_id).await?;
    broadcast_room_event(
        &state,
        &room2.room_id,
        &json!({
            "type": "room_settings",
            "room": api::room_json(&room2, None),
        }),
    );
    Ok(Json(
        json!({ "room": api::room_json(&room2, Some(member.room_role.as_str())) }),
    ))
}

// --------------------------------------------------------------- delete

pub async fn delete_room(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    if member.room_role != RoomRole::Owner {
        return Err(ApiError::forbidden("只有房主可以删除房间"));
    }
    terminate_room(&state, &room, "owner_delete", &me.user.user_id)?;
    state
        .hub
        .broadcast(&room.room_id, r#"{"type":"room_deleted"}"#);
    Ok(Json(json!({ "ok": true })))
}

/// Shared termination path: mark deleted, tombstone the code forever.
pub(crate) fn terminate_room(
    state: &AppState,
    room: &Room,
    reason: &str,
    actor: &str,
) -> ApiResult<()> {
    state.db.with_sync(|conn| -> ApiResult<()> {
        crate::infra::repo::update_room_state(
            conn,
            &room.room_id,
            RoomState::Deleted,
            Some(&util::now()),
        )?;
        crate::infra::repo::insert_tombstone(conn, &room.room_code)?;
        api::audited(
            conn,
            Some(actor),
            "room.deleted",
            Some("room"),
            Some(&room.room_id),
            json!({ "reason": reason, "room_code": room.room_code }),
        )?;
        Ok(())
    })?;
    Ok(())
}

// -------------------------------------------------------------- invites

#[derive(Deserialize)]
pub struct CreateInviteReq {
    pub max_role: Option<String>,
    pub expires_in: Option<String>, // humantime, default config invite_ttl
}

pub async fn create_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
    me: AuthUser,
    Json(req): Json<CreateInviteReq>,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    api::require_cap(&state, &member, &room, "manage_invites")?;
    // 只有一种房间邀请码：加入者一律成为普通成员。
    // 未注册用户凭邀请直接进房（创建 guest 会话），注册与否不影响进房，
    // 注册只影响能否申请建房。max_role 字段保留兼容但不再生效。
    let role = RoomRole::Member;
    let ttl = match req.expires_in.as_deref() {
        None => state.cfg.invite_ttl(),
        Some(s) => humantime::parse_duration(s).map_err(|_| ApiError::validation("有效期无效"))?,
    };
    let raw = util::random_token();
    let now = util::now();
    let inv = Invite {
        invite_id: util::random_id(),
        kind: "room".into(),
        token_hash: util::sha256_hex(raw.as_bytes()),
        room_id: Some(room.room_id.clone()),
        created_by: me.user.user_id.clone(),
        max_role: role.as_str().into(),
        created_at: now,
        expires_at: now + chrono::Duration::from_std(ttl).unwrap_or_default(),
        consumed_at: None,
        consumed_by: None,
        revoked_at: None,
    };
    let room_id = room.room_id.clone();
    state.db.with_sync(|conn| -> ApiResult<()> {
        crate::infra::repo::insert_invite(conn, &inv)?;
        api::audited(
            conn,
            Some(&me.user.user_id),
            "invite.created",
            Some("invite"),
            Some(&inv.invite_id),
            json!({ "room_code": room.room_code, "max_role": inv.max_role }),
        )?;
        Ok(())
    })?;
    let _ = room_id;
    Ok(Json(json!({
        "invite": api::invite_json(&inv, Some(&raw)),
        "invite_url": format!("/{}?invite={raw}", room.room_code),
    })))
}

pub async fn list_invites(
    State(state): State<AppState>,
    Path(code): Path<String>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    api::require_cap(&state, &member, &room, "manage_invites")?;
    let room_id = room.room_id.clone();
    let rows = state
        .db
        .with(move |conn| crate::infra::repo::list_room_invites(conn, &room_id))
        .await?;
    Ok(Json(
        json!({ "invites": rows.iter().map(|i| api::invite_json(i, None)).collect::<Vec<_>>() }),
    ))
}

pub async fn revoke_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((code, invite_id)): Path<(String, String)>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    api::require_cap(&state, &member, &room, "manage_invites")?;
    let room_id = room.room_id.clone();
    state.db.with_sync(|conn| -> ApiResult<()> {
        let inv = crate::infra::repo::get_invite_by_id(conn, &invite_id)?
            .ok_or_else(|| ApiError::not_found("邀请不存在"))?;
        if inv.room_id.as_deref() != Some(room_id.as_str()) {
            return Err(ApiError::not_found("邀请不存在"));
        }
        crate::infra::repo::revoke_invite(conn, &invite_id)?;
        api::audited(
            conn,
            Some(&me.user.user_id),
            "invite.revoked",
            Some("invite"),
            Some(&invite_id),
            json!({}),
        )?;
        Ok(())
    })?;
    Ok(Json(json!({ "ok": true })))
}

// -------------------------------------------------------------- members

pub async fn set_member_role(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((code, user_id)): Path<(String, String)>,
    me: AuthUser,
    Json(req): Json<RoleReq>,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    if member.room_role != RoomRole::Owner {
        return Err(ApiError::forbidden("只有房主可以修改成员角色"));
    }
    if req.role != "member" {
        return Err(ApiError::validation("房间角色只能设置为普通成员"));
    }
    let new_role = RoomRole::Member;
    let room_id = room.room_id.clone();
    state.db.with_sync(|conn| -> ApiResult<()> {
        let target = crate::infra::repo::get_member(conn, &room_id, &user_id)?
            .ok_or_else(|| ApiError::not_found("member not found"))?;
        if target.room_role == RoomRole::Owner {
            return Err(ApiError::validation("不能修改房主角色"));
        }
        if user_id == me.user.user_id {
            return Err(ApiError::validation("不能修改自己的角色"));
        }
        crate::infra::repo::set_member_role(conn, &room_id, &user_id, new_role)?;
        crate::infra::repo::bump_room_seq(conn, &room_id)?;
        api::audited(
            conn,
            Some(&me.user.user_id),
            "member.role_changed",
            Some("room"),
            Some(&room_id),
            json!({ "user_id": user_id, "role": new_role.as_str() }),
        )?;
        Ok(())
    })?;
    broadcast_room_event(
        &state,
        &room.room_id,
        &json!({
            "type": "member_updated",
            "user_id": user_id,
            "room_role": new_role.as_str(),
        }),
    );
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct RoleReq {
    pub role: String,
}

pub async fn set_nickname(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((code, user_id)): Path<(String, String)>,
    me: AuthUser,
    Json(req): Json<NicknameReq>,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    // 昵称自助：任何成员（含未注册 guest）可修改自己的昵称；
    // 房主可修改他人昵称（manage_nicknames）。
    let is_self = user_id == me.user.user_id;
    if !is_self {
        api::require_cap(&state, &member, &room, "manage_nicknames")?;
    }
    let nick = req.nickname.trim().to_string();
    if !util::validate_nickname(&nick, state.cfg.security.max_nickname_graphemes) {
        return Err(ApiError::validation(format!(
            "昵称长度需为 1-{} 个字符且不含控制字符",
            state.cfg.security.max_nickname_graphemes
        )));
    }
    let room_id = room.room_id.clone();
    state.db.with_sync(|conn| -> ApiResult<()> {
        let target = crate::infra::repo::get_member(conn, &room_id, &user_id)?
            .ok_or_else(|| ApiError::not_found("member not found"))?;
        if target.room_role == RoomRole::Owner && target.user_id != me.user.user_id {
            return Err(ApiError::validation("不能修改房主昵称"));
        }
        crate::infra::repo::set_member_nickname(conn, &room_id, &user_id, &nick)?;
        crate::infra::repo::bump_room_seq(conn, &room_id)?;
        api::audited(
            conn,
            Some(&me.user.user_id),
            "member.nickname_changed",
            Some("room"),
            Some(&room_id),
            json!({ "user_id": user_id }),
        )?;
        Ok(())
    })?;
    broadcast_room_event(
        &state,
        &room.room_id,
        &json!({
            "type": "member_updated",
            "user_id": user_id,
            "nickname": nick,
        }),
    );
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct NicknameReq {
    pub nickname: String,
}

pub async fn remove_member(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((code, user_id)): Path<(String, String)>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    api::require_cap(&state, &member, &room, "manage_members")?;
    let room_id = room.room_id.clone();
    state.db.with_sync(|conn| -> ApiResult<()> {
        let target = crate::infra::repo::get_member(conn, &room_id, &user_id)?
            .ok_or_else(|| ApiError::not_found("member not found"))?;
        if target.room_role == RoomRole::Owner && target.user_id != me.user.user_id {
            return Err(ApiError::validation("不能移出房主"));
        }
        if user_id == me.user.user_id {
            return Err(ApiError::validation("请使用离开房间接口退出"));
        }
        crate::infra::repo::leave_room(conn, &room_id, &user_id)?;
        crate::infra::repo::bump_room_seq(conn, &room_id)?;
        api::audited(
            conn,
            Some(&me.user.user_id),
            "member.removed",
            Some("room"),
            Some(&room_id),
            json!({ "user_id": user_id }),
        )?;
        Ok(())
    })?;
    state.hub.unregister_all(&room.room_id, &user_id);
    broadcast_room_event(
        &state,
        &room.room_id,
        &json!({
            "type": "member_left",
            "user_id": user_id,
        }),
    );
    let room_id_b = room.room_id.clone();
    let sys = state
        .db
        .with_sync(|conn| -> ApiResult<Value> {
            let m = crate::infra::repo::insert_system_message(
                conn,
                &room_id_b,
                "一名成员被移出了房间",
            )?;
            Ok(api::message_json(&m, None, None))
        })
        .unwrap_or(Value::Null);
    if !sys.is_null() {
        state.hub.broadcast(
            &room.room_id,
            &serde_json::to_string(&json!({ "type": "message", "message": sys }))
                .unwrap_or_default(),
        );
    }
    Ok(Json(json!({ "ok": true })))
}

// ---------------------------------------------------------------- mutes

#[derive(Deserialize)]
pub struct MuteReq {
    pub scope: String, // "member" | "room"
    pub user_id: Option<String>,
    pub duration_seconds: Option<i64>,
}

pub async fn create_mute(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
    me: AuthUser,
    Json(req): Json<MuteReq>,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    let cap = if req.scope == "room" {
        "mute_room"
    } else {
        "mute_member"
    };
    api::require_cap(&state, &member, &room, cap)?;
    let room_id = room.room_id.clone();
    let expires_at = req
        .duration_seconds
        .map(|s| util::now() + chrono::Duration::seconds(s));
    let mute = Mute {
        mute_id: util::random_id(),
        room_id: room_id.clone(),
        scope: req.scope.clone(),
        target_user_id: req.user_id.clone(),
        muted_by: me.user.user_id.clone(),
        created_at: util::now(),
        expires_at,
    };
    state.db.with_sync(|conn| -> ApiResult<()> {
        if req.scope == "member" {
            let Some(target) = req.user_id.as_deref() else {
                return Err(ApiError::validation("成员禁言需要指定用户"));
            };
            let t = crate::infra::repo::get_member(conn, &room_id, target)?
                .ok_or_else(|| ApiError::not_found("member not found"))?;
            if t.room_role == RoomRole::Owner {
                return Err(ApiError::validation("不能禁言房主"));
            }
        }
        crate::infra::repo::insert_mute(conn, &mute)?;
        crate::infra::repo::bump_room_seq(conn, &room_id)?;
        api::audited(
            conn,
            Some(&me.user.user_id),
            "mute.created",
            Some("room"),
            Some(&room_id),
            json!({
                "mute_id": mute.mute_id,
                "scope": mute.scope,
                "target_user_id": mute.target_user_id,
                "duration_seconds": req.duration_seconds,
            }),
        )?;
        Ok(())
    })?;
    broadcast_room_event(
        &state,
        &room_id,
        &json!({
            "type": "muted",
            "mute_id": mute.mute_id,
            "scope": mute.scope,
            "target_user_id": mute.target_user_id,
        }),
    );
    Ok(Json(json!({ "mute": {
        "mute_id": mute.mute_id,
        "scope": mute.scope,
        "target_user_id": mute.target_user_id,
        "created_at": mute.created_at.to_rfc3339(),
        "expires_at": mute.expires_at.map(|d| d.to_rfc3339()),
    } })))
}

pub async fn list_mutes(
    State(state): State<AppState>,
    Path(code): Path<String>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    api::require_cap(&state, &member, &room, "mute_member")?;
    let room_id = room.room_id.clone();
    let rows = state
        .db
        .with(move |conn| crate::infra::repo::list_active_mutes(conn, &room_id, &util::now()))
        .await?;
    Ok(Json(json!({ "mutes": rows.iter().map(|m| json!({
        "mute_id": m.mute_id,
        "scope": m.scope,
        "target_user_id": m.target_user_id,
        "muted_by": m.muted_by,
        "created_at": m.created_at.to_rfc3339(),
        "expires_at": m.expires_at.map(|d| d.to_rfc3339()),
    })).collect::<Vec<_>>() })))
}

pub async fn remove_mute(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((code, mute_id)): Path<(String, String)>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    api::require_cap(&state, &member, &room, "mute_member")?;
    let room_id = room.room_id.clone();
    state.db.with_sync(|conn| -> ApiResult<()> {
        let m = crate::infra::repo::get_mute(conn, &mute_id)?
            .ok_or_else(|| ApiError::not_found("禁言记录不存在"))?;
        if m.room_id != room_id {
            return Err(ApiError::not_found("禁言记录不存在"));
        }
        crate::infra::repo::delete_mute(conn, &mute_id)?;
        crate::infra::repo::bump_room_seq(conn, &room_id)?;
        api::audited(
            conn,
            Some(&me.user.user_id),
            "mute.removed",
            Some("room"),
            Some(&room_id),
            json!({ "mute_id": mute_id }),
        )?;
        Ok(())
    })?;
    broadcast_room_event(
        &state,
        &room_id,
        &json!({
            "type": "unmuted",
            "mute_id": mute_id,
        }),
    );
    Ok(Json(json!({ "ok": true })))
}

/// 依据已提交的建房申请创建房间（批准或 super_admin 免审批共用）。
pub(crate) fn create_room_for_request(
    state: &AppState,
    conn: &mut rusqlite::Connection,
    request_id: &str,
    actor: &str,
) -> ApiResult<Room> {
    let req = crate::infra::repo::get_room_request(conn, request_id)?
        .ok_or_else(|| ApiError::not_found("申请不存在"))?;
    if req.status != "pending" {
        return Err(ApiError::conflict("该申请不在待审批状态"));
    }
    if req.request_kind != "create_room" {
        return Err(ApiError::conflict("该申请不是建房申请"));
    }
    let media_mode = req.media_mode;
    let requester = crate::infra::repo::get_user(conn, &req.requester_id)?
        .ok_or_else(|| ApiError::internal("申请人不存在"))?;
    let code = match req.desired_code {
        Some(code) => {
            let normalized = crate::domain::codes::validate_custom_code(&state.cfg, &code)
                .map_err(|e| ApiError::validation(e))?;
            if crate::infra::repo::get_room_by_code(conn, &normalized)?.is_some()
                || crate::infra::repo::code_tombstoned(conn, &normalized)?
            {
                return Err(ApiError::conflict(format!(
                    "房间号 {normalized:?} 已被使用"
                )));
            }
            crate::infra::repo::claim_code(conn, &normalized)?;
            normalized
        }
        None => {
            let mut code = None;
            for _ in 0..8 {
                let c = crate::domain::codes::generate_code(&state.cfg);
                if crate::infra::repo::get_room_by_code(conn, &c)?.is_some()
                    || crate::infra::repo::code_tombstoned(conn, &c)?
                {
                    continue;
                }
                crate::infra::repo::claim_code(conn, &c)?;
                code = Some(c);
                break;
            }
            code.ok_or_else(|| ApiError::internal("无法分配房间号"))?
        }
    };
    let now = crate::util::now();
    let room = Room {
        room_id: crate::util::random_id(),
        room_code: code,
        kind: RoomKind::Standard,
        owner_user_id: Some(requester.user_id.clone()),
        parent_room_id: None,
        state: RoomState::Active,
        created_at: now,
        expires_at: None,
        deleted_at: None,
        allow_direct_messages: true,
        text_enabled: true,
        media_mode,
        audio_bitrate: req.audio_bitrate,
        room_muted: false,
        announcement: String::new(),
        max_members: 0,
        last_seq: 0,
        version: 1,
    };
    crate::infra::repo::insert_room(conn, &room)?;
    let nick = requester
        .username
        .clone()
        .unwrap_or_else(|| format!("guest-{}", &requester.user_id[..8]));
    crate::infra::repo::add_member(
        conn,
        &room.room_id,
        &requester.user_id,
        RoomRole::Owner,
        &nick,
    )?;
    crate::infra::repo::update_request_decision(
        conn,
        request_id,
        "approved",
        actor,
        None,
        Some(&room.room_id),
    )?;
    api::audited(
        conn,
        Some(actor),
        "room_request.approved",
        Some("room_request"),
        Some(request_id),
        serde_json::json!({ "room_code": room.room_code, "owner": requester.user_id }),
    )?;
    Ok(room)
}

/// 依据已提交的模式变更申请更新房间媒体模式（批准或 super_admin 免审批共用）。
/// 审批时重新校验：申请仍 pending、目标标准房间仍 active、申请人仍是房主、
/// 目标模式与当前模式不同；任何一项失效都整体失败，不部分应用。
pub(crate) fn apply_media_mode_change_for_request(
    _state: &AppState,
    conn: &mut rusqlite::Connection,
    request_id: &str,
    actor: &str,
) -> ApiResult<Room> {
    let req = crate::infra::repo::get_room_request(conn, request_id)?
        .ok_or_else(|| ApiError::not_found("申请不存在"))?;
    if req.status != "pending" {
        return Err(ApiError::conflict("该申请不在待审批状态"));
    }
    if req.request_kind != "change_media_mode" {
        return Err(ApiError::conflict("该申请不是媒体模式变更申请"));
    }
    let room_id = req
        .target_room_id
        .clone()
        .ok_or_else(|| ApiError::conflict("媒体模式变更申请缺少目标房间"))?;
    let room = crate::infra::repo::get_room_by_id(conn, &room_id)?
        .ok_or_else(|| ApiError::conflict("目标房间已不存在"))?;
    if room.state != RoomState::Active {
        return Err(ApiError::conflict("target 房间未处于正常状态"));
    }
    if room.kind != RoomKind::Standard {
        return Err(ApiError::conflict("媒体模式变更仅适用于普通房间"));
    }
    let member = crate::infra::repo::get_member(conn, &room_id, &req.requester_id)?
        .ok_or_else(|| ApiError::conflict("申请人已不再是目标房间成员"))?;
    if member.left_at.is_some() || member.room_role != RoomRole::Owner {
        return Err(ApiError::conflict("申请人已不再是房主"));
    }
    if room.media_mode == req.media_mode {
        return Err(ApiError::conflict("目标房间已使用该媒体模式"));
    }
    crate::infra::repo::update_room_media_mode(conn, &room_id, req.media_mode)?;
    // 模式变更申请可携带目标码率（SFU 目标）：审批时一并应用。
    if req.media_mode == MediaMode::Sfu && req.audio_bitrate != room.audio_bitrate {
        crate::infra::repo::update_room_audio_bitrate(conn, &room_id, req.audio_bitrate)?;
    }
    crate::infra::repo::update_request_decision(
        conn,
        request_id,
        "approved",
        actor,
        None,
        Some(&room_id),
    )?;
    api::audited(
        conn,
        Some(actor),
        "room_media_mode.changed",
        Some("room"),
        Some(&room_id),
        json!({
            "room_code": room.room_code,
            "from": room.media_mode.as_str(),
            "to": req.media_mode.as_str(),
            "request_id": request_id,
        }),
    )?;
    crate::infra::repo::get_room_by_id(conn, &room_id)?
        .ok_or_else(|| ApiError::internal("媒体模式变更后房间不存在"))
}

// ------------------------------------------------------------- messages

#[derive(Deserialize)]
pub struct SendMessageReq {
    pub body: String,
    /// 回复引用的消息 id。
    pub reply_to: Option<String>,
}

pub async fn send_message_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
    me: AuthUser,
    Json(req): Json<SendMessageReq>,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let msg = send_message_inner(
        &state,
        &code,
        &me.user.user_id,
        &req.body,
        req.reply_to.as_deref(),
    )
    .await?;
    Ok(Json(json!({ "message": msg })))
}

pub(crate) async fn send_message_inner(
    state: &AppState,
    code: &str,
    user_id: &str,
    body: &str,
    reply_to: Option<&str>,
) -> ApiResult<Value> {
    let body = body.trim();
    if body.is_empty() {
        return Err(ApiError::validation("消息内容为空"));
    }
    if body.chars().count() > state.cfg.rooms.max_message_chars {
        return Err(ApiError::validation("消息过长"));
    }
    if util::has_control_chars(body) {
        return Err(ApiError::validation("消息包含控制字符"));
    }
    if !state.rl.allow(
        "message",
        user_id,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.message_per_minute_per_user,
    ) {
        return Err(ApiError::rate_limited("消息发送过于频繁"));
    }
    let (room, member) = active_member_room(state, code, user_id).await?;
    if !room.text_enabled {
        return Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            "text_disabled",
            "文字频道已关闭",
        ));
    }
    // 全员禁言下文字频道同步禁用：仅获发言权用户可发消息（房主同样需先申请）。
    if room.room_muted {
        let granted = state
            .db
            .with_sync(|conn| crate::infra::repo::has_speak_grant(conn, &room.room_id, user_id))
            .unwrap_or(false);
        if !granted {
            return Err(ApiError::new(
                axum::http::StatusCode::FORBIDDEN,
                "room_muted",
                "房间已开启全员禁言，仅获发言权用户可发言",
            ));
        }
    }
    let now = util::now();
    let uid = user_id.to_string();
    let room_id = room.room_id.clone();
    let body2 = body.to_string();
    let room_id2 = room_id.clone();
    let reply_to_owned = reply_to.map(|s| s.to_string());
    let seq = state
        .db
        .with(move |conn| -> ApiResult<i64> {
            // 回复快照：引用原消息的发送者昵称与正文前 80 字（防原消息删除丢失）。
            let reply_snapshot = match reply_to_owned.as_deref() {
                Some(rid) => {
                    let orig = crate::infra::repo::get_message(conn, rid)?
                        .filter(|m| m.room_id == room_id);
                    match orig {
                        Some(m) => {
                            let sender = m
                                .sender_user_id
                                .as_deref()
                                .and_then(|s| {
                                    crate::infra::repo::get_member(conn, &room_id, s)
                                        .ok()
                                        .flatten()
                                })
                                .map(|x| x.nickname)
                                .unwrap_or_else(|| "成员".into());
                            let preview: String = m.body.chars().take(80).collect();
                            Some(
                                serde_json::to_string(&json!({
                                    "id": m.message_id,
                                    "sender": sender,
                                    "uid": m.sender_user_id,
                                    "body": preview,
                                }))
                                .unwrap_or_default(),
                            )
                        }
                        None => None,
                    }
                }
                None => None,
            };
            // Mute enforcement (owner exempt from room-scope mute).
            let member_row = crate::infra::repo::get_member(conn, &room_id, &uid)?.unwrap();
            if member_row.room_role != RoomRole::Owner
                && crate::infra::repo::member_is_muted(conn, &room_id, &uid, &now)?
            {
                return Err(ApiError::new(
                    axum::http::StatusCode::FORBIDDEN,
                    "muted",
                    "您已被禁言",
                ));
            }
            let seq = crate::infra::repo::bump_room_seq(conn, &room_id)?;
            crate::infra::repo::insert_message(
                conn,
                &Message {
                    message_id: util::random_id(),
                    room_id: room_id.clone(),
                    seq,
                    sender_user_id: Some(uid.clone()),
                    body: body2.clone(),
                    created_at: now,
                    reply_snapshot,
                },
            )?;
            Ok(seq)
        })
        .await?;
    let msg = state
        .db
        .with(move |conn| -> ApiResult<Value> {
            let m = crate::infra::repo::list_messages(conn, &room_id2, seq - 1, 1)?
                .pop()
                .unwrap();
            let sender = match &m.sender_user_id {
                Some(uid) => crate::infra::repo::get_user(conn, uid)?,
                None => None,
            };
            let nick = match &m.sender_user_id {
                Some(uid) => {
                    crate::infra::repo::get_member(conn, &room_id2, uid)?.map(|x| x.nickname)
                }
                None => None,
            };
            Ok(api::message_json(&m, sender.as_ref(), nick.as_deref()))
        })
        .await?;
    state.hub.broadcast(
        &room.room_id,
        &serde_json::to_string(&json!({ "type": "message", "message": msg })).unwrap_or_default(),
    );
    let _ = member;
    Ok(msg)
}

#[derive(Deserialize)]
pub struct MessagesQuery {
    pub after: Option<i64>,
    pub limit: Option<i64>,
}

pub async fn get_messages(
    State(state): State<AppState>,
    Path(code): Path<String>,
    Query(q): Query<MessagesQuery>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    let (room, _) = active_member_room(&state, &code, &me.user.user_id).await?;
    let room_id = room.room_id.clone();
    let me_user_id = me.user.user_id.clone();
    let after = q.after.unwrap_or(0).max(0);
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let rows = state
        .db
        .with(move |conn| -> ApiResult<(Vec<Value>, i64)> {
            let ms = crate::infra::repo::list_messages(conn, &room_id, after, limit)?;
            let uid = me_user_id.clone();
            let ids: Vec<String> = ms.iter().map(|m| m.message_id.clone()).collect();
            let reactions = crate::infra::repo::message_reactions(conn, &room_id, &ids, &uid)?;
            let mut out = Vec::new();
            for m in &ms {
                let sender = match &m.sender_user_id {
                    Some(uid) => crate::infra::repo::get_user(conn, uid)?,
                    None => None,
                };
                let nick = match &m.sender_user_id {
                    Some(uid) => {
                        crate::infra::repo::get_member(conn, &room_id, uid)?.map(|x| x.nickname)
                    }
                    None => None,
                };
                let mut v = api::message_json(m, sender.as_ref(), nick.as_deref());
                if let Some(list) = reactions.get(&m.message_id) {
                    v["reactions"] = json!(list
                        .iter()
                        .map(|(e, c, me)| json!({ "emoji": e, "count": c, "me": me }))
                        .collect::<Vec<_>>());
                }
                out.push(v);
            }
            Ok((
                out,
                crate::infra::repo::get_room_by_id(conn, &room_id)?
                    .map(|r| r.last_seq)
                    .unwrap_or(0),
            ))
        })
        .await?;
    Ok(Json(json!({ "messages": rows.0, "last_seq": rows.1 })))
}

// ---------------------------------------------------------- direct rooms

#[derive(Deserialize)]
pub struct DirectReq {
    pub user_id: String,
}

pub async fn create_direct_room(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
    me: AuthUser,
    Json(req): Json<DirectReq>,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    if room.kind != RoomKind::Standard {
        return Err(ApiError::validation("只能从普通房间创建私聊"));
    }
    if !room.allow_direct_messages {
        return Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            "direct_messages_disabled",
            "该房间不允许私聊",
        ));
    }
    if req.user_id == me.user.user_id {
        return Err(ApiError::validation("不能与自己创建私聊"));
    }
    let parent_id = room.room_id.clone();
    let parent_code = room.room_code.clone();
    let uid = me.user.user_id.clone();
    let peer = req.user_id.clone();
    let cfg = state.cfg.clone();
    let result = state
        .db
        .with(move |conn| -> ApiResult<Option<Room>> {
            // Both must be current members of the parent room.
            let me_m = crate::infra::repo::get_member(conn, &parent_id, &uid)?
                .ok_or_else(|| ApiError::forbidden("不是房间成员 of this room"))?;
            if me_m.left_at.is_some() {
                return Err(ApiError::forbidden("不是房间成员 of this room"));
            }
            let peer_m = crate::infra::repo::get_member(conn, &parent_id, &peer)?
                .ok_or_else(|| ApiError::not_found("peer is 不是房间成员 of this room"))?;
            if peer_m.left_at.is_some() {
                return Err(ApiError::not_found("peer is 不是房间成员 of this room"));
            }
            // Reuse an existing direct room if one exists (active or not).
            if let Some(existing_id) =
                crate::infra::repo::direct_room_by_pair(conn, &parent_id, &uid, &peer)?
            {
                let room = crate::infra::repo::get_room_by_id(conn, &existing_id)?.unwrap();
                return Ok(Some(room));
            }
            // Create: claim a fresh code (retry on collision).
            // 直接房间继承父标准房间当前的媒体模式，不能单独申请切换。
            let parent_mode = crate::infra::repo::get_room_by_id(conn, &parent_id)?
                .map(|p| p.media_mode)
                .unwrap_or(MediaMode::Sfu);
            let mut room_row = None;
            for _ in 0..8 {
                let code = crate::domain::codes::generate_code(&cfg);
                if crate::infra::repo::get_room_by_code(conn, &code)?.is_some()
                    || crate::infra::repo::code_tombstoned(conn, &code)?
                {
                    continue;
                }
                crate::infra::repo::claim_code(conn, &code)?;
                let now = util::now();
                let r = Room {
                    room_id: util::random_id(),
                    room_code: code,
                    kind: RoomKind::Direct,
                    owner_user_id: None,
                    parent_room_id: Some(parent_id.clone()),
                    state: RoomState::Active,
                    created_at: now,
                    expires_at: None,
                    deleted_at: None,
                    allow_direct_messages: false,
                    text_enabled: true,
                    media_mode: parent_mode,
                    audio_bitrate: 0,
                    room_muted: false,
                    announcement: String::new(),
                    max_members: 0,
                    last_seq: 0,
                    version: 1,
                };
                crate::infra::repo::insert_room(conn, &r)?;
                crate::infra::repo::create_direct_pair(conn, &r.room_id, &parent_id, &uid, &peer)?;
                let nick_me = crate::infra::repo::get_user(conn, &uid)?
                    .and_then(|u| u.username)
                    .unwrap_or_else(|| format!("guest-{}", &uid[..8]));
                let nick_peer = crate::infra::repo::get_user(conn, &peer)?
                    .and_then(|u| u.username)
                    .unwrap_or_else(|| format!("guest-{}", &peer[..8]));
                crate::infra::repo::add_member(conn, &r.room_id, &uid, RoomRole::Owner, &nick_me)?;
                crate::infra::repo::add_member(
                    conn,
                    &r.room_id,
                    &peer,
                    RoomRole::Owner,
                    &nick_peer,
                )?;
                room_row = Some(r);
                break;
            }
            Ok(Some(
                room_row.ok_or_else(|| ApiError::internal("无法分配房间号"))?,
            ))
        })
        .await?;
    let room = result.unwrap();
    // Notify the parent room about the new DM.
    if let Some(parent) = state
        .db
        .with_sync(|conn| {
            crate::infra::repo::get_room_by_id(
                conn,
                &room.parent_room_id.clone().unwrap_or_default(),
            )
        })
        .unwrap_or(None)
    {
        broadcast_room_event(
            &state,
            &parent.room_id,
            &json!({
                "type": "direct_room_created",
                "room": api::room_json(&room, None),
                "peer_user_id": req.user_id,
            }),
        );
    }
    let _ = member;
    let _ = parent_code;
    Ok(Json(
        json!({ "room": api::room_json(&room, Some("owner")) }),
    ))
}

// ---------------------------------------------------- ICE / media tokens

/// 房间级禁言校验：禁言中不允许任何语音存在，仅获发言权用户可加入
/// （房主同样需申请——房主申请自动通过）。
pub(crate) fn check_speak_allowed(
    _state: &AppState,
    conn: &mut rusqlite::Connection,
    room: &Room,
    uid: &str,
) -> ApiResult<()> {
    if !room.room_muted {
        return Ok(());
    }
    if crate::infra::repo::has_speak_grant(conn, &room.room_id, uid)? {
        return Ok(());
    }
    Err(ApiError::new(
        axum::http::StatusCode::FORBIDDEN,
        "room_muted",
        "房间已开启全员禁言，需申请发言并获得房主批准",
    ))
}

/// P2P 专用：签发 LiveKit 内嵌 TURN 的短期 ICE 凭证（无独立 coturn）。
pub async fn ice_servers(
    State(state): State<AppState>,
    Path(code): Path<String>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    api::require_cap(&state, &member, &room, "join_voice")?;
    if room.media_mode != MediaMode::P2p {
        return Err(ApiError::media_mode_mismatch());
    }
    state
        .db
        .with_sync(|conn| check_speak_allowed(&state, conn, &room, &me.user.user_id))?;
    let creds = crate::infra::livekit::livekit_turn_credentials(&state.cfg)?;
    Ok(Json(creds))
}

/// SFU 专用：签发短期 LiveKit JWT。
pub async fn media_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    api::require_cap(&state, &member, &room, "join_voice")?;
    if room.media_mode != MediaMode::Sfu {
        return Err(ApiError::media_mode_mismatch());
    }
    state
        .db
        .with_sync(|conn| check_speak_allowed(&state, conn, &room, &me.user.user_id))?;
    let token =
        crate::infra::livekit::livekit_token(&state.cfg, &room.room_code, &me.user.user_id)?;
    Ok(Json(json!({
        "token": token,
        "url": state.cfg.media.livekit_url,
        "room": room.room_code,
    })))
}

// --------------------------------------------------------------- helpers

pub(crate) fn broadcast_room_event(state: &AppState, room_id: &str, event: &Value) {
    if let Ok(s) = serde_json::to_string(event) {
        state.hub.broadcast(room_id, &s);
    }
}

/// 语音选项：码率档位及房主所需最低账号角色（前端按当前用户角色过滤可选项）。
pub async fn voice_options(
    State(state): State<AppState>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    let _ = me;
    let tiers: Vec<Value> = state
        .cfg
        .voice
        .bitrate_tiers_sorted()
        .iter()
        .map(|(kbps, role)| json!({ "kbps": kbps, "min_owner_role": role }))
        .collect();
    Ok(Json(json!({
        "default_kbps": 0,
        "bitrate_tiers": tiers,
    })))
}
