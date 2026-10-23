//! Room endpoints: requests, join/leave, settings, invites, members, mutes,
//! messages, direct rooms, ICE servers, media tokens.

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{self, AuthUser, MaybeAuth};
use crate::app::access::active_member_room;
use crate::app::events::broadcast_room_event;
use crate::app::rooms::{
    check_speak_allowed, create_direct_room as create_direct_room_workflow,
    create_mute as create_mute_workflow, insert_system_message, join_member, leave_member,
    remove_member as remove_member_workflow, remove_mute as remove_mute_workflow,
    revoke_invite as revoke_invite_workflow, set_member_nickname,
    set_member_role as set_member_role_workflow, terminate_room,
    update_settings as update_settings_workflow, SettingsUpdate,
};
use crate::app::state::AppState;
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
    let input = crate::app::requests::CreateInput {
        kind: req.kind,
        note: req.note,
        desired_code: req.desired_code,
        media_mode: req.media_mode,
        audio_bitrate: req.audio_bitrate,
        room_code: req.room_code,
    };
    let created =
        crate::app::requests::create(&state, &me.user.user_id, me.user.role, input).await?;
    let mut request_json = api::room_request_json(&created.request);
    if let Some(code) = created.target_room_code {
        if let Some(obj) = request_json.as_object_mut() {
            obj.insert("target_room_code".into(), json!(code));
        }
    }
    if created.auto_approved {
        let room = created
            .room
            .ok_or_else(|| ApiError::internal("自动审批缺少房间结果"))?;
        return Ok(Json(json!({
            "request": request_json,
            "room": api::room_json(&room, Some("owner")),
            "auto_approved": true,
        })));
    }
    Ok(Json(json!({ "request": request_json })))
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
    let st = state.clone();
    let st2 = st.clone();
    let (room, member, sys) = st
        .db
        .with(move |conn| {
            join_member(
                &st2,
                conn,
                &code2,
                &uid,
                req.invite_token.as_deref(),
                req.nickname.as_deref(),
            )
        })
        .await?;
    broadcast_room_event(
        &state,
        &room.room_id,
        &json!({
            "type": "member_joined",
            "member": api::member_json(&member, None),
        }),
    );
    if let Some(sys) = sys {
        broadcast_room_event(
            &state,
            &room.room_id,
            &json!({
                "type": "message",
                "message": api::message_json(&sys, None, None),
            }),
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
    leave_member(&state, &room_id, &uid)?;
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
    let sys = insert_system_message(&state, &room_id_b, &format!("{nick} 离开了房间"))
        .map(|m| api::message_json(&m, None, None))
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
    update_settings_workflow(
        &state,
        &room.room_id,
        &me.user.user_id,
        SettingsUpdate {
            text_enabled,
            allow_direct_messages: adm,
            expires_at,
            audio_bitrate,
            announcement,
            max_members,
        },
    )?;
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
    revoke_invite_workflow(&state, &room_id, &me.user.user_id, &invite_id)?;
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
    set_member_role_workflow(&state, &room.room_id, &me.user.user_id, &user_id, new_role)?;
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
    set_member_nickname(&state, &room_id, &me.user.user_id, &user_id, &nick)?;
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
    remove_member_workflow(&state, &room_id, &me.user.user_id, &user_id)?;
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
    let sys = insert_system_message(&state, &room_id_b, "一名成员被移出了房间")
        .map(|m| api::message_json(&m, None, None))
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

const MAX_MUTE_SECONDS: i64 = 365 * 24 * 60 * 60;

/// Validate a requested mute duration before any chrono arithmetic.
/// `None` keeps the existing permanent-mute semantics; `Some` must be 1 second
/// through one year. The checked API guarantees a hostile i64 cannot panic the
/// process (release builds use `panic = "abort"`).
fn mute_expiry(
    now: chrono::DateTime<chrono::Utc>,
    duration_seconds: Option<i64>,
) -> ApiResult<Option<chrono::DateTime<chrono::Utc>>> {
    match duration_seconds {
        None => Ok(None),
        Some(seconds) if (1..=MAX_MUTE_SECONDS).contains(&seconds) => {
            let delta = chrono::Duration::try_seconds(seconds)
                .ok_or_else(|| ApiError::bad_request("invalid_mute_duration", "禁言时长无效"))?;
            now.checked_add_signed(delta)
                .map(Some)
                .ok_or_else(|| ApiError::bad_request("invalid_mute_duration", "禁言时长无效"))
        }
        Some(_) => Err(ApiError::bad_request(
            "invalid_mute_duration",
            format!("禁言时长需为 1..={MAX_MUTE_SECONDS} 秒"),
        )),
    }
}

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
    let expires_at = mute_expiry(util::now(), req.duration_seconds)?;
    let mute = Mute {
        mute_id: util::random_id(),
        room_id: room_id.clone(),
        scope: req.scope.clone(),
        target_user_id: req.user_id.clone(),
        muted_by: me.user.user_id.clone(),
        created_at: util::now(),
        expires_at,
    };
    create_mute_workflow(&state, &mute, req.duration_seconds)?;
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
    remove_mute_workflow(&state, &room_id, &me.user.user_id, &mute_id)?;
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
    let sent = crate::app::messages::send(state, code, user_id, body, reply_to).await?;
    let msg = api::message_json(
        &sent.message,
        sent.sender.as_ref(),
        sent.nickname.as_deref(),
    );
    broadcast_room_event(
        state,
        &sent.room_id,
        &json!({ "type": "message", "message": msg }),
    );
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
    let (room, _member) = active_member_room(&state, &code, &me.user.user_id).await?;
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
    let created =
        create_direct_room_workflow(&state, &room.room_id, &me.user.user_id, &req.user_id).await?;
    // Notify the parent room about the new DM.
    if let Some(parent) = created.parent {
        broadcast_room_event(
            &state,
            &parent.room_id,
            &json!({
                "type": "direct_room_created",
                "room": api::room_json(&created.room, None),
                "peer_user_id": req.user_id,
            }),
        );
    }
    Ok(Json(json!({
        "room": api::room_json(&created.room, Some("owner")),
    })))
}

// ---------------------------------------------------- ICE / media tokens

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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    const MAX_MUTE_SECONDS: i64 = 365 * 24 * 60 * 60;

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 23, 12, 0, 0).unwrap()
    }

    #[test]
    fn mute_duration_none_is_permanent() {
        assert_eq!(mute_expiry(now(), None).unwrap(), None);
    }

    #[test]
    fn mute_duration_zero_is_rejected() {
        let err = mute_expiry(now(), Some(0)).unwrap_err();
        assert_eq!(err.status, axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn mute_duration_negative_is_rejected() {
        for seconds in [-1, -3600, i64::MIN] {
            let err = mute_expiry(now(), Some(seconds)).unwrap_err();
            assert_eq!(err.status, axum::http::StatusCode::BAD_REQUEST);
        }
    }

    #[test]
    fn mute_duration_extreme_is_rejected_without_panic() {
        for seconds in [MAX_MUTE_SECONDS + 1, 10_000_000_000, i64::MAX] {
            let err = mute_expiry(now(), Some(seconds)).unwrap_err();
            assert_eq!(err.status, axum::http::StatusCode::BAD_REQUEST);
        }
    }

    #[test]
    fn mute_duration_max_is_accepted() {
        let expires = mute_expiry(now(), Some(MAX_MUTE_SECONDS)).unwrap().unwrap();
        assert_eq!(expires, now() + chrono::Duration::seconds(MAX_MUTE_SECONDS));
    }
}
