//! Room endpoints: requests, join/leave, settings, invites, members, mutes,
//! messages, direct rooms, ICE servers, media tokens.

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use futures_util::StreamExt;

use crate::api::{self, AuthUser, MaybeAuth};
use crate::app::access::active_member_room;
use crate::app::events::broadcast_room_event;
use crate::app::revocation;
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
    /// 建房申请：是否申请附件能力（默认否）。
    pub attachments: Option<bool>,
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
        attachments: req.attachments.unwrap_or(false),
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
        // 媒体模式变更即时生效：广播给房间内客户端，其他端据此刷新并退出旧语音。
        broadcast_room_event(
            &state,
            &room.room_id,
            &json!({ "type": "room_settings", "room": api::room_json(&room, None) }),
        );
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
        if !crate::infra::repo::cancel_room_request(conn, &request_id, &uid)? {
            // 只对本人可见的申请区分“已处理”；他人或不存在的申请统一 404。
            return match crate::infra::repo::get_room_request(conn, &request_id)? {
                Some(r) if r.requester_id == uid => {
                    Err(ApiError::conflict("该申请已处理，无法取消"))
                }
                _ => Err(ApiError::not_found("申请不存在")),
            };
        }
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
    let can_see_mutes = member_is_capable(&member, "mute_member");
    let room_muted = room.room_muted;
    let (members, mutes, speak) = state
        .db
        .read(move |conn| -> ApiResult<(Vec<Value>, Vec<Value>, Value)> {
            let ms = crate::infra::repo::list_members(conn, &room_id)?;
            // 批量取用户，避免每个成员一次查询（N+1）。
            let uids: Vec<String> = ms.iter().map(|m| m.user_id.clone()).collect();
            let users = crate::infra::repo::users_by_ids(conn, &uids)?;
            let members: Vec<Value> = ms
                .iter()
                .map(|m| api::member_json(m, users.get(&m.user_id)))
                .collect();
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
                // 昵称一次批量取，避免每个申请/授权一次查询。
                let mut nick_ids: Vec<String> = Vec::new();
                for (u, _) in grants.iter().chain(requests.iter()) {
                    if !nick_ids.contains(u) {
                        nick_ids.push(u.clone());
                    }
                }
                let nicks = crate::infra::repo::member_nicknames(conn, &room_id, &nick_ids)?;
                let nick = |uid: &str| -> String {
                    nicks
                        .get(uid)
                        .cloned()
                        .unwrap_or_else(|| uid.chars().take(8).collect())
                };
                let g: Vec<Value> = grants
                    .iter()
                    .map(|(u, at)| json!({ "user_id": u, "nickname": nick(u), "granted_at": at }))
                    .collect();
                let r: Vec<Value> = requests
                    .iter()
                    .map(|(u, at)| json!({ "user_id": u, "nickname": nick(u), "created_at": at }))
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

fn member_is_capable(member: &RoomMember, cap: &str) -> bool {
    capabilities(member).contains(&cap)
}

pub fn capabilities(member: &RoomMember) -> Vec<&'static str> {
    match member.room_role {
        RoomRole::Owner => vec![
            "manage_room",
            "manage_members",
            "manage_invites",
            "manage_nicknames",
            "mute_member",
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
    peer: api::PeerAddr,
    Path(code): Path<String>,
    me: MaybeAuth,
    Json(req): Json<JoinReq>,
) -> ApiResult<Response> {
    // 防高频进出：短时间反复加入会被限流。
    if let Some(m) = &me.0 {
        // 已登录用户用 Cookie 加入房间属于状态变更；匿名访客凭邀请码加入。
        api::require_csrf(&state, &headers, &m.session)?;
    }
    let rl_key =
        me.0.as_ref()
            .map(|m| m.user.user_id.clone())
            .unwrap_or_else(|| format!("ip:{}", api::client_ip(&state, &headers, peer.0)));
    if !state.rl.allow(
        "room_join",
        &rl_key,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.room_join_per_minute_per_user,
    ) {
        return Err(ApiError::rate_limited("加入房间过于频繁，请稍后再试"));
    }
    if req.invite_token.is_some() {
        let invite_ip = api::client_ip(&state, &headers, peer.0);
        if !state.rl.allow(
            "invite_redeem",
            &invite_ip,
            std::time::Duration::from_secs(3600),
            state.cfg.rate_limit.invite_redeem_per_hour_per_ip,
        ) {
            return Err(ApiError::rate_limited("邀请兑换过于频繁"));
        }
    }
    // 邀请加入不要求 CSRF/登录：未注册用户凭邀请直接进房（创建 guest 会话）。
    // 邀请 token 本身就是授权凭证；已登录用户同样走此流程。
    // 注册与否只影响能否申请建房，不影响进房。
    // Anonymous joins get a guest identity only after join_member has
    // validated the room/invite. join_member itself creates the guest row
    // inside its transaction; no orphan user/session is written on failure.
    let (uid, new_session_raw): (String, Option<String>) = match me.0 {
        Some(m) => (m.user.user_id, None),
        None => (util::random_id(), Some(util::random_token())),
    };
    let code2 = code.clone();
    let join_uid = uid.clone();
    let st = state.clone();
    let st2 = st.clone();
    let (room, member, sys) = st
        .db
        .with(move |conn| {
            join_member(
                &st2,
                conn,
                &code2,
                &join_uid,
                req.invite_token.as_deref(),
                req.nickname.as_deref(),
            )
        })
        .await?;
    if let Some(raw) = new_session_raw.as_ref() {
        let mut session = state.auth.new_session(
            &uid,
            Some(api::client_ip(&state, &headers, peer.0)),
            headers
                .get(axum::http::header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .map(|x| x.to_string()),
        );
        // 匿名访客会话不沿用正式账号的 30 天，降低闲置访客会话/账号堆积。
        session.expires_at = util::now()
            + chrono::Duration::from_std(state.cfg.guest_session_ttl()).unwrap_or_default();
        session.token_hash = util::sha256_hex(raw.as_bytes());
        state
            .db
            .with_sync(|conn| crate::infra::repo::create_session(conn, &session))?;
    }
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
    revocation::schedule_participant(
        state.cfg.clone(),
        state.revocation.clone(),
        room.room_code.clone(),
        uid.clone(),
    );
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
    /// 媒体加密（服务器持钥，非端到端；仅 SFU 房间生效；开启时服务端生成并下发共享密钥）。
    pub e2ee: Option<bool>,
    /// 房主为管理员时可开启：房间内访客可直接注册为正式用户。
    pub guest_upgrade_open: Option<bool>,
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
    let announcement = match req.announcement {
        Some(s) => {
            if crate::util::has_bidi_controls(&s) {
                return Err(ApiError::validation("公告包含不可见控制字符"));
            }
            // 允许换行，其余控制字符剔除；保持 500 字截断。
            let cleaned: String = s
                .chars()
                .filter(|c| !c.is_control() || *c == '\n')
                .collect();
            cleaned.chars().take(500).collect()
        }
        None => room.announcement.clone(),
    };
    let max_members = req.max_members.unwrap_or(room.max_members).clamp(0, 500);
    let e2ee = req.e2ee.unwrap_or(room.e2ee);
    if e2ee && room.media_mode != MediaMode::Sfu {
        return Err(ApiError::validation("媒体加密仅支持 SFU 房间"));
    }
    // 访客转正开关：仅房主本人且账号为管理员时可开启；关闭不受限。
    if req.guest_upgrade_open == Some(true)
        && (member.room_role != RoomRole::Owner || !me.user.role.is_admin())
    {
        return Err(ApiError::forbidden("只有管理员房主可以开启访客转正"));
    }
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
            e2ee,
            guest_upgrade_open: req.guest_upgrade_open,
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
    terminate_room(&state, &room, "owner_delete", &me.user.user_id, false)?;
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

// -------------------------------------------------------------- invites

const MAX_INVITE_TTL_SECS: u64 = 365 * 24 * 60 * 60;
/// 只允许修正最近的输入错误；历史消息不允许改写。
const EDIT_WINDOW_SECS: i64 = 5 * 60;

fn invite_expiry(
    now: chrono::DateTime<chrono::Utc>,
    ttl: std::time::Duration,
) -> ApiResult<chrono::DateTime<chrono::Utc>> {
    if ttl.is_zero() || ttl.as_secs() > MAX_INVITE_TTL_SECS {
        return Err(ApiError::bad_request(
            "invalid_invite_ttl",
            format!("邀请有效期需为 1..={MAX_INVITE_TTL_SECS} 秒"),
        ));
    }
    let delta = chrono::Duration::from_std(ttl)
        .map_err(|_| ApiError::bad_request("invalid_invite_ttl", "邀请有效期无效"))?;
    now.checked_add_signed(delta)
        .ok_or_else(|| ApiError::bad_request("invalid_invite_ttl", "邀请有效期无效"))
}

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
    if room.kind != RoomKind::Standard {
        return Err(ApiError::validation("私聊房间不接受邀请"));
    }
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
        expires_at: invite_expiry(now, ttl)?,
        consumed_at: None,
        consumed_by: None,
        revoked_at: None,
        guest_user_id: None,
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
    revocation::schedule_participant(
        state.cfg.clone(),
        state.revocation.clone(),
        room.room_code.clone(),
        user_id.clone(),
    );
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
    if req.scope != "member" {
        return Err(ApiError::validation(
            "禁言接口仅支持 scope=member；全员禁言请使用房间设置中的开关",
        ));
    }
    let cap = "mute_member";
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
    let target = remove_mute_workflow(&state, &room_id, &me.user.user_id, &mute_id)?;
    broadcast_room_event(
        &state,
        &room_id,
        &json!({
            "type": "unmuted",
            "mute_id": mute_id,
            "user_id": target,
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
    /// 附件 id（先上传再发送，正文可为空）。
    pub attachment_id: Option<String>,
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
        req.attachment_id.as_deref(),
    )
    .await?;
    Ok(Json(json!({ "message": msg })))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn send_message_inner(
    state: &AppState,
    code: &str,
    user_id: &str,
    body: &str,
    reply_to: Option<&str>,
    attachment_id: Option<&str>,
) -> ApiResult<Value> {
    let sent = crate::app::messages::send(
        state,
        code,
        user_id,
        body,
        reply_to,
        attachment_id,
    )
    .await?;
    state.metrics.message_sent();
    let mut msg = api::message_json(
        &sent.message,
        sent.sender.as_ref(),
        sent.nickname.as_deref(),
    );
    if let Some(aid) = sent.message.attachment_id.as_deref() {
        let att = state
            .db
            .with_sync(|conn| crate::infra::repo::get_attachment(conn, aid))
            .ok()
            .flatten();
        if let Some(a) = att {
            msg["attachment"] = api::attachment_json(&a, code);
        }
    }
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
    pub before: Option<i64>,
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
    let before = q.before.filter(|b| *b > 0);
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let code_for_json = code.clone();
    let rows = state
        .db
        .read(move |conn| -> ApiResult<(Vec<Value>, i64)> {
            let ms = crate::infra::repo::list_messages(conn, &room_id, after, before, limit)?;
            let uid = me_user_id.clone();
            let ids: Vec<String> = ms.iter().map(|m| m.message_id.clone()).collect();
            let reactions = crate::infra::repo::message_reactions(conn, &room_id, &ids, &uid)?;
            // 批量取发送者与昵称：避免每条消息两次查询（N+1）。
            let senders: std::collections::HashSet<String> =
                ms.iter().filter_map(|m| m.sender_user_id.clone()).collect();
            let senders: Vec<String> = senders.into_iter().collect();
            let users = crate::infra::repo::users_by_ids(conn, &senders)?;
            let nicks = crate::infra::repo::member_nicknames(conn, &room_id, &senders)?;
            // 附件元数据一次批量取，避免每条消息一次查询。
            let att_ids: Vec<String> = ms.iter().filter_map(|m| m.attachment_id.clone()).collect();
            let atts = crate::infra::repo::attachments_by_ids(conn, &att_ids)?;
            let pinned = crate::infra::repo::pinned_message_ids(conn, &room_id, &ids)?;
            let mut out = Vec::new();
            for m in &ms {
                let sender = m.sender_user_id.as_ref().and_then(|uid| users.get(uid));
                let nick = m
                    .sender_user_id
                    .as_ref()
                    .and_then(|uid| nicks.get(uid))
                    .map(|s| s.as_str());
                let mut v = api::message_json(m, sender, nick);
                v["pinned"] = json!(pinned.contains(&m.message_id));
                if let Some(list) = reactions.get(&m.message_id) {
                    v["reactions"] = json!(list
                        .iter()
                        .map(|(e, c, me)| json!({ "emoji": e, "count": c, "me": me }))
                        .collect::<Vec<_>>());
                }
                if let Some(aid) = m.attachment_id.as_deref() {
                    if let Some(a) = atts.get(aid) {
                        v["attachment"] = api::attachment_json(a, &code_for_json);
                    }
                }
                out.push(v);
            }
            Ok((
                out,
                crate::infra::repo::room_last_seq(conn, &room_id).unwrap_or(0),
            ))
        })
        .await?;
    Ok(Json(json!({ "messages": rows.0, "last_seq": rows.1 })))
}

// ------------------------------------------------- message edit/delete/search

#[derive(Deserialize)]
pub struct EditMessageReq {
    pub body: String,
}

fn validate_message_body(state: &AppState, raw: &str) -> ApiResult<String> {
    let body = raw.trim();
    if body.is_empty() {
        return Err(ApiError::validation("消息内容为空"));
    }
    if body.chars().count() > state.cfg.rooms.max_message_chars {
        return Err(ApiError::validation("消息过长"));
    }
    if util::has_forbidden_controls(body) || util::has_bidi_controls(body) {
        return Err(ApiError::validation("消息包含控制字符"));
    }
    Ok(body.to_string())
}

pub async fn edit_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((code, message_id)): Path<(String, String)>,
    me: AuthUser,
    Json(req): Json<EditMessageReq>,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let body = validate_message_body(&state, &req.body)?;
    let (room, _member) = active_member_room(&state, &code, &me.user.user_id).await?;
    let room_id = room.room_id.clone();
    let room_id2 = room_id.clone();
    let uid = me.user.user_id.clone();
    let mid = message_id.clone();
    let (msg, sender, nick) = state
        .db
        .with(
            move |conn| -> ApiResult<(Message, Option<User>, Option<String>)> {
                let m = crate::infra::repo::get_message(conn, &mid)?
                    .filter(|m| m.room_id == room_id2)
                    .ok_or_else(|| ApiError::not_found("消息不存在"))?;
                if m.sender_user_id.as_deref() != Some(uid.as_str()) {
                    return Err(ApiError::forbidden("只能编辑自己的消息"));
                }
                if m.deleted_at.is_some() {
                    return Err(ApiError::conflict("消息已删除"));
                }
                if util::now()
                    .signed_duration_since(m.created_at)
                    .num_seconds()
                    > EDIT_WINDOW_SECS
                {
                    return Err(ApiError::forbidden("消息发送超过 5 分钟，无法编辑"));
                }
                crate::infra::repo::update_message_body(conn, &mid, &body)?;
                let m = crate::infra::repo::get_message(conn, &mid)?
                    .ok_or_else(|| ApiError::not_found("消息不存在"))?;
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
                Ok((m, sender, nick))
            },
        )
        .await?;
    let v = api::message_json(&msg, sender.as_ref(), nick.as_deref());
    broadcast_room_event(
        &state,
        &room_id,
        &json!({ "type": "message_updated", "message": v }),
    );
    Ok(Json(json!({ "message": v })))
}

pub async fn delete_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((code, message_id)): Path<(String, String)>,
    me: AuthUser,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    let room_id = room.room_id.clone();
    let room_id2 = room_id.clone();
    let uid = me.user.user_id.clone();
    let is_owner = member.room_role == RoomRole::Owner;
    let is_admin = matches!(me.user.role, UserRole::Admin | UserRole::SuperAdmin);
    let mid = message_id.clone();
    let (msg, orphan, sender, nick) = state
        .db
        .with(move |conn| -> ApiResult<(Message, Option<String>, Option<User>, Option<String>)> {
            let m = crate::infra::repo::get_message(conn, &mid)?
                .filter(|m| m.room_id == room_id2)
                .ok_or_else(|| ApiError::not_found("消息不存在"))?;
            if m.sender_user_id.is_none() {
                return Err(ApiError::forbidden("系统消息不能删除"));
            }
            let mine = m.sender_user_id.as_deref() == Some(uid.as_str());
            if !mine && !is_owner && !is_admin {
                return Err(ApiError::forbidden("只能删除自己的消息"));
            }
            if m.deleted_at.is_some() {
                return Err(ApiError::conflict("消息已删除"));
            }
            // 附件只被这条消息引用时回收：删记录 + 归还配额 + 事务外删文件。
            let mut orphan = None;
            if let Some(aid) = m.attachment_id.clone() {
                if !crate::infra::repo::attachment_referenced(conn, &aid, &mid)? {
                    if let Some(a) = crate::infra::repo::get_attachment(conn, &aid)? {
                        crate::infra::repo::delete_attachment(conn, &aid)?;
                        crate::infra::repo::sub_room_attachment_bytes(conn, &m.room_id, a.size)?;
                        orphan = Some(aid);
                    }
                }
            }
            crate::infra::repo::soft_delete_message(conn, &mid)?;
            let msg = crate::infra::repo::get_message(conn, &mid)?
                .ok_or_else(|| ApiError::not_found("消息不存在"))?;
            let sender = match msg.sender_user_id.as_deref() {
                Some(uid) => crate::infra::repo::get_user(conn, uid)?,
                None => None,
            };
            let nick = match msg.sender_user_id.as_deref() {
                Some(uid) => {
                    crate::infra::repo::get_member(conn, &room_id2, uid)?.map(|m| m.nickname)
                }
                None => None,
            };
            Ok((msg, orphan, sender, nick))
        })
        .await?;
    if let Some(aid) = orphan {
        let _ = std::fs::remove_file(state.attachments_dir.join(aid));
    }
    let v = api::message_json(&msg, sender.as_ref(), nick.as_deref());
    broadcast_room_event(
        &state,
        &room_id,
        &json!({ "type": "message_deleted", "message_id": msg.message_id, "message": v }),
    );
    Ok(Json(json!({ "message": v })))
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub q: String,
    pub limit: Option<i64>,
}

pub async fn search_messages(
    State(state): State<AppState>,
    Path(code): Path<String>,
    Query(q): Query<SearchQuery>,
    me: AuthUser,
) -> ApiResult<impl IntoResponse> {
    let needle = q.q.trim();
    if needle.is_empty() {
        return Err(ApiError::validation("搜索词为空"));
    }
    if needle.chars().count() > 100 {
        return Err(ApiError::validation("搜索词过长"));
    }
    let limit = q.limit.unwrap_or(20).clamp(1, 50);
    if !state.rl.allow(
        "room_search",
        &me.user.user_id,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.search_per_minute_per_user,
    ) {
        return Err(ApiError::rate_limited("搜索过于频繁，请稍后再试"));
    }
    let (room, _) = active_member_room(&state, &code, &me.user.user_id).await?;
    let room_id = room.room_id.clone();
    let me_id = me.user.user_id.clone();
    let needle = needle.to_string();
    let needle_out = needle.clone();
    let code2 = code.clone();
    let rows = state
        .db
        .read(move |conn| -> ApiResult<Vec<Value>> {
            let ms = crate::infra::repo::search_messages(conn, &room_id, &needle, limit)?;
            let ids: Vec<String> = ms.iter().map(|m| m.message_id.clone()).collect();
            let reactions = crate::infra::repo::message_reactions(conn, &room_id, &ids, &me_id)?;
            let senders: Vec<String> = ms
                .iter()
                .filter_map(|m| m.sender_user_id.clone())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            let users = crate::infra::repo::users_by_ids(conn, &senders)?;
            let nicks = crate::infra::repo::member_nicknames(conn, &room_id, &senders)?;
            // 附件一次批量取，避免每条命中一次查询。
            let att_ids: Vec<String> = ms.iter().filter_map(|m| m.attachment_id.clone()).collect();
            let atts = crate::infra::repo::attachments_by_ids(conn, &att_ids)?;
            let mut out = Vec::new();
            for m in &ms {
                let sender = m.sender_user_id.as_ref().and_then(|u| users.get(u));
                let nick = m
                    .sender_user_id
                    .as_ref()
                    .and_then(|u| nicks.get(u))
                    .map(|s| s.as_str());
                let mut v = api::message_json(m, sender, nick);
                if let Some(list) = reactions.get(&m.message_id) {
                    v["reactions"] = json!(list
                        .iter()
                        .map(|(e, c, me)| json!({ "emoji": e, "count": c, "me": me }))
                        .collect::<Vec<_>>());
                }
                if let Some(aid) = m.attachment_id.as_deref() {
                    if let Some(a) = atts.get(aid) {
                        v["attachment"] = api::attachment_json(a, &code2);
                    }
                }
                out.push(v);
            }
            Ok(out)
        })
        .await?;
    Ok(Json(json!({ "messages": rows, "query": needle_out })))
}

#[derive(Deserialize)]
pub struct GlobalSearchQuery {
    pub q: String,
    pub limit: Option<i64>,
}

/// Cross-room search over active rooms the caller is currently a member of.
pub async fn global_search(
    State(state): State<AppState>,
    Query(q): Query<GlobalSearchQuery>,
    me: AuthUser,
) -> ApiResult<impl IntoResponse> {
    let needle = q.q.trim();
    if needle.is_empty() {
        return Err(ApiError::validation("搜索词为空"));
    }
    if needle.chars().count() > 100 {
        return Err(ApiError::validation("搜索词过长"));
    }
    let limit = q.limit.unwrap_or(20).clamp(1, 50);
    if !state.rl.allow(
        "global_search",
        &me.user.user_id,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.search_per_minute_per_user,
    ) {
        return Err(ApiError::rate_limited("搜索过于频繁，请稍后再试"));
    }
    let uid = me.user.user_id.clone();
    let needle_owned = needle.to_string();
    let needle_out = needle.to_string();
    let hits = state
        .db
        .read(move |conn| {
            crate::infra::repo::search_messages_global(conn, &uid, &needle_owned, limit)
        })
        .await?;
    let results: Vec<Value> = hits
        .iter()
        .map(|h| {
            json!({
                "message_id": h.message_id,
                "room_code": h.room_code,
                "seq": h.seq,
                "body": h.body,
                "created_at": h.created_at,
                "sender_user_id": h.sender_user_id,
                "sender_nickname": h.sender_nickname,
            })
        })
        .collect();
    Ok(Json(json!({ "results": results, "query": needle_out })))
}

// ------------------------------------------------------------- pins / forward

pub async fn pin_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((code, message_id)): Path<(String, String)>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    api::require_cap(&state, &member, &room, "manage_room")?;
    let room_id = room.room_id.clone();
    let mid = message_id.clone();
    let actor = me.user.user_id.clone();
    let changed = state.db.with_sync(move |conn| -> ApiResult<bool> {
        let msg = crate::infra::repo::get_message(conn, &mid)?
            .filter(|m| m.room_id == room_id && m.deleted_at.is_none())
            .ok_or_else(|| ApiError::not_found("消息不存在"))?;
        let changed = crate::infra::repo::pin_message(conn, &room_id, &msg.message_id, &actor)?;
        if changed {
            crate::app::audit::audited(
                conn,
                Some(&actor),
                "message.pinned",
                Some("room"),
                Some(&room_id),
                json!({ "message_id": msg.message_id }),
            )?;
        }
        Ok(changed)
    })?;
    if changed {
        broadcast_room_event(
            &state,
            &room.room_id,
            &json!({ "type": "message_pinned", "message_id": message_id }),
        );
    }
    Ok(Json(json!({ "ok": true })))
}

pub async fn unpin_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((code, message_id)): Path<(String, String)>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    api::require_cap(&state, &member, &room, "manage_room")?;
    let room_id = room.room_id.clone();
    let mid = message_id.clone();
    let actor = me.user.user_id.clone();
    let changed = state.db.with_sync(move |conn| -> ApiResult<bool> {
        let changed = crate::infra::repo::unpin_message(conn, &room_id, &mid)?;
        if changed {
            crate::app::audit::audited(
                conn,
                Some(&actor),
                "message.unpinned",
                Some("room"),
                Some(&room_id),
                json!({ "message_id": mid }),
            )?;
        }
        Ok(changed)
    })?;
    if changed {
        broadcast_room_event(
            &state,
            &room.room_id,
            &json!({ "type": "message_unpinned", "message_id": message_id }),
        );
    }
    Ok(Json(json!({ "ok": true })))
}

pub async fn list_pins(
    State(state): State<AppState>,
    Path(code): Path<String>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    let (room, _member) = active_member_room(&state, &code, &me.user.user_id).await?;
    let room_id = room.room_id.clone();
    let code2 = code.clone();
    let msgs = state
        .db
        .read(move |conn| -> ApiResult<Vec<Value>> {
            let ms = crate::infra::repo::list_pinned_messages(conn, &room_id)?;
            let senders: Vec<String> = ms
                .iter()
                .filter_map(|m| m.sender_user_id.clone())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            let users = crate::infra::repo::users_by_ids(conn, &senders)?;
            let nicks = crate::infra::repo::member_nicknames(conn, &room_id, &senders)?;
            let att_ids: Vec<String> = ms.iter().filter_map(|m| m.attachment_id.clone()).collect();
            let atts = crate::infra::repo::attachments_by_ids(conn, &att_ids)?;
            Ok(ms
                .iter()
                .map(|m| {
                    let sender = m.sender_user_id.as_ref().and_then(|u| users.get(u));
                    let nick = m
                        .sender_user_id
                        .as_ref()
                        .and_then(|u| nicks.get(u))
                        .map(|s| s.as_str());
                    let mut v = api::message_json(m, sender, nick);
                    v["pinned"] = json!(true);
                    if let Some(aid) = m.attachment_id.as_deref() {
                        if let Some(a) = atts.get(aid) {
                            v["attachment"] = api::attachment_json(a, &code2);
                        }
                    }
                    v
                })
                .collect())
        })
        .await?;
    Ok(Json(json!({ "messages": msgs })))
}

// -------------------------------------------------------------- attachments

pub(crate) fn sniff_attachment(data: &[u8]) -> Option<(&'static str, &'static str)> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(("image/png", "png"));
    }
    if data.starts_with(b"\xff\xd8\xff") {
        return Some(("image/jpeg", "jpg"));
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return Some(("image/gif", "gif"));
    }
    if data.len() > 12 && &data[0..4] == b"RIFF" {
        if &data[8..12] == b"WEBP" {
            return Some(("image/webp", "webp"));
        }
        if &data[8..12] == b"WAVE" {
            return Some(("audio/wav", "wav"));
        }
    }
    if data.starts_with(b"OggS") {
        return Some(("audio/ogg", "ogg"));
    }
    if data.starts_with(b"ID3") {
        return Some(("audio/mpeg", "mp3"));
    }
    if data.len() > 1 && data[0] == 0xff && (data[1] & 0xe0) == 0xe0 {
        return Some(("audio/mpeg", "mp3"));
    }
    if data.len() > 12 && &data[4..8] == b"ftyp" {
        let brand = &data[8..12];
        if brand == b"M4A " || brand == b"M4B " {
            return Some(("audio/mp4", "m4a"));
        }
        return Some(("video/mp4", "mp4"));
    }
    if data.starts_with(&[0x1a, 0x45, 0xdf, 0xa3]) {
        return Some(("video/webm", "webm"));
    }
    if data.starts_with(b"%PDF-") {
        return Some(("application/pdf", "pdf"));
    }
    None
}

pub async fn upload_attachment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
    me: AuthUser,
    mut multipart: axum::extract::Multipart,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    if !state.rl.allow(
        "attachment",
        &me.user.user_id,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.attachment_per_minute_per_user,
    ) {
        return Err(ApiError::rate_limited("附件上传过于频繁"));
    }
    let (room, _member) = active_member_room(&state, &code, &me.user.user_id).await?;
    if !room.text_enabled {
        return Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            "text_disabled",
            "文字频道已关闭",
        ));
    }
    // 附件能力在申请建房时确定；未开启的房间一律拒绝。
    if !room.attachments_enabled {
        return Err(ApiError::new(
            axum::http::StatusCode::FORBIDDEN,
            "attachments_disabled",
            "该房间未开启附件",
        ));
    }
    let is_admin = matches!(me.user.role, UserRole::Admin | UserRole::SuperAdmin);
    let mut data: Option<Vec<u8>> = None;
    let mut filename = String::from("attachment");
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request("invalid_upload", e.to_string()))?
    {
        if field.name() == Some("file") {
            if let Some(name) = field.file_name() {
                let clean: String = name.chars().filter(|c| !c.is_control()).take(120).collect();
                if !clean.is_empty() {
                    filename = clean;
                }
            }
            let bytes = field
                .bytes()
                .await
                .map_err(|e| ApiError::bad_request("invalid_upload", e.to_string()))?;
            data = Some(bytes.to_vec());
        }
    }
    let Some(bytes) = data else {
        return Err(ApiError::bad_request("invalid_upload", "缺少 file 字段"));
    };
    let (mime, _ext) = sniff_attachment(&bytes).ok_or_else(|| {
        ApiError::bad_request(
            "attachment_unsupported",
            "仅接受图片、音频、视频或 PDF",
        )
    })?;
    let is_media = mime.starts_with("audio/") || mime.starts_with("video/");
    let cap = if is_media {
        state.cfg.security.max_media_attachment_bytes
    } else if is_admin {
        state.cfg.security.max_attachment_bytes
    } else {
        state.cfg.security.max_user_attachment_bytes
    };
    if bytes.len() > cap {
        return Err(ApiError::bad_request(
            "attachment_too_large",
            format!("附件超过 {cap} 字节"),
        ));
    }
    if !is_admin && !(mime.starts_with("image/") || is_media) {
        return Err(ApiError::forbidden("普通成员只能发送图片、音频或视频"));
    }
    let quota = state.cfg.security.max_room_attachment_bytes as i64;
    if room.attachment_bytes + bytes.len() as i64 > quota {
        return Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            "attachment_quota_exceeded",
            "房间附件空间已满，请联系管理员清理",
        ));
    }
    let attachment_id = util::random_id();
    let att = Attachment {
        attachment_id: attachment_id.clone(),
        room_id: room.room_id.clone(),
        uploader_id: me.user.user_id.clone(),
        filename,
        mime: mime.to_string(),
        size: bytes.len() as i64,
        created_at: util::now(),
    };
    let dir = state.attachments_dir.clone();
    let path = dir.join(&attachment_id);
    let thumb_path = dir.join(format!("{attachment_id}.thumb.jpg"));
    let id2 = attachment_id.clone();
    let mime2 = mime.to_string();
    let bytes_len = bytes.len();
    let make_thumb = mime.starts_with("image/");
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, &bytes)?;
        if make_thumb {
            if let Err(e) = crate::infra::thumbnails::write_thumbnail(&bytes, &thumb_path) {
                tracing::warn!(error = %e, "attachment thumbnail generation failed");
                let _ = std::fs::remove_file(&thumb_path);
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| ApiError::internal(format!("附件写入任务失败: {e}")))?
    .map_err(|e| ApiError::internal(format!("附件写入失败: {e}")))?;
    let att2 = att.clone();
    let room_id_for_quota = att.room_id.clone();
    let size_for_quota = att.size;
    let quota_for_db = quota;
    if let Err(e) = state.db.with_sync(move |conn| {
        crate::infra::repo::insert_attachment(conn, &att2)?;
        // 配额校验与累加在同一条 UPDATE 内完成：并发上传无法越过上限。
        if !crate::infra::repo::add_room_attachment_bytes(
            conn,
            &room_id_for_quota,
            size_for_quota,
            quota_for_db,
        )? {
            return Err(ApiError::new(
                axum::http::StatusCode::CONFLICT,
                "attachment_quota_exceeded",
                "房间附件空间已满，请联系管理员清理",
            ));
        }
        Ok(())
    }) {
        // 落库失败：回收刚写入的文件，避免孤儿。
        let _ = std::fs::remove_file(state.attachments_dir.join(&id2));
        let _ = std::fs::remove_file(
            state
                .attachments_dir
                .join(format!("{id2}.thumb.jpg")),
        );
        return Err(e);
    }
    tracing::info!(attachment_id = %attachment_id, mime = %mime2, size = bytes_len, "attachment stored");
    Ok(Json(
        json!({ "attachment": api::attachment_json(&att, &code) }),
    ))
}

#[derive(Deserialize)]
pub struct DownloadQuery {
    /// `?download=1` forces attachment disposition even for inline media/PDF.
    pub download: Option<String>,
}

pub async fn download_attachment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((code, attachment_id)): Path<(String, String)>,
    Query(q): Query<DownloadQuery>,
    me: AuthUser,
) -> ApiResult<Response> {
    if !state.rl.allow(
        "attachment_download",
        &me.user.user_id,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.attachment_download_per_minute_per_user,
    ) {
        return Err(ApiError::rate_limited("下载过于频繁，请稍后再试"));
    }
    let (room, _member) = active_member_room(&state, &code, &me.user.user_id).await?;
    let permit = state
        .attachment_downloads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::rate_limited("附件下载并发过高，请稍后再试"))?;
    let room_id = room.room_id.clone();
    let aid = attachment_id.clone();
    let att = state
        .db
        .read(move |conn| crate::infra::repo::get_attachment(conn, &aid))
        .await?
        .filter(|a| a.room_id == room_id)
        .ok_or_else(|| ApiError::not_found("附件不存在"))?;
    let path = state.attachments_dir.join(&att.attachment_id);
    let size = att.size.max(0) as u64;
    let range = headers
        .get(axum::http::header::RANGE)
        .and_then(|v| v.to_str().ok())
        .map(|raw| parse_range(raw, size));
    let (start, end, partial) = match range {
        Some(Ok((s, e))) => (s, e, true),
        Some(Err(())) => {
            let mut resp = axum::response::Response::new(axum::body::Body::empty());
            *resp.status_mut() = axum::http::StatusCode::RANGE_NOT_SATISFIABLE;
            resp.headers_mut().insert(
                axum::http::header::CONTENT_RANGE,
                axum::http::HeaderValue::from_str(&format!("bytes */{size}"))
                    .unwrap_or_else(|_| axum::http::HeaderValue::from_static("bytes */*")),
            );
            return Ok(resp);
        }
        None => (0, size.saturating_sub(1), false),
    };
    let len = end.saturating_sub(start).saturating_add(1);
    let mut file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| ApiError::not_found("附件文件不存在"))?;
    if start > 0 {
        use tokio::io::AsyncSeekExt;
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(|_| ApiError::internal("附件读取失败"))?;
    }
    use tokio::io::AsyncReadExt;
    let stream = tokio_util::io::ReaderStream::new(file.take(len)).map(move |item| {
        let _ = &permit;
        item
    });
    let mut resp = axum::body::Body::from_stream(stream).into_response();
    if partial {
        *resp.status_mut() = axum::http::StatusCode::PARTIAL_CONTENT;
        resp.headers_mut().insert(
            axum::http::header::CONTENT_RANGE,
            axum::http::HeaderValue::from_str(&format!("bytes {start}-{end}/{size}"))
                .unwrap_or_else(|_| axum::http::HeaderValue::from_static("bytes */*")),
        );
    }
    let force_download = q
        .download
        .as_deref()
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let disposition = if !force_download
        && (att.mime.starts_with("image/")
            || att.mime.starts_with("audio/")
            || att.mime.starts_with("video/")
            || att.mime == "application/pdf")
    {
        "inline"
    } else {
        "attachment"
    };
    let safe_name: String = att
        .filename
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    resp.headers_mut().insert(
        axum::http::header::CONTENT_LENGTH,
        axum::http::HeaderValue::from_str(&len.to_string())
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("0")),
    );
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_str(&att.mime)
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("application/octet-stream")),
    );
    resp.headers_mut().insert(
        axum::http::header::CONTENT_DISPOSITION,
        axum::http::HeaderValue::from_str(&format!("{disposition}; filename=\"{safe_name}\""))
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("attachment")),
    );
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, max-age=3600"),
    );
    resp.headers_mut().insert(
        axum::http::header::ACCEPT_RANGES,
        axum::http::HeaderValue::from_static("bytes"),
    );
    resp.headers_mut().insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        axum::http::HeaderValue::from_static("nosniff"),
    );
    Ok(resp)
}

/// Parse a single `bytes=` range (start-end / start- / -suffix).
fn parse_range(raw: &str, size: u64) -> Result<(u64, u64), ()> {
    let spec = raw.trim().strip_prefix("bytes=").ok_or(())?;
    if spec.contains(',') || size == 0 {
        return Err(());
    }
    let (s, e) = spec.split_once('-').ok_or(())?;
    let (start, end) = if s.is_empty() {
        let suffix: u64 = e.parse().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        (size.saturating_sub(suffix), size - 1)
    } else {
        let start: u64 = s.parse().map_err(|_| ())?;
        let end: u64 = if e.is_empty() {
            size - 1
        } else {
            let parsed: u64 = e.parse().map_err(|_| ())?;
            parsed.min(size - 1)
        };
        (start, end)
    };
    if start >= size || start > end {
        return Err(());
    }
    Ok((start, end))
}

/// Serve the generated JPEG thumbnail; 404 when the image is not a supported
/// raster format or thumbnail generation failed (client falls back to `url`).
pub async fn download_thumbnail(
    State(state): State<AppState>,
    Path((code, attachment_id)): Path<(String, String)>,
    me: AuthUser,
) -> ApiResult<Response> {
    if !state.rl.allow(
        "attachment_thumb",
        &me.user.user_id,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.attachment_thumbnail_per_minute_per_user,
    ) {
        return Err(ApiError::rate_limited("缩略图读取过于频繁，请稍后再试"));
    }
    let (room, _member) = active_member_room(&state, &code, &me.user.user_id).await?;
    let room_id = room.room_id.clone();
    let aid = attachment_id.clone();
    let att = state
        .db
        .read(move |conn| crate::infra::repo::get_attachment(conn, &aid))
        .await?
        .filter(|a| a.room_id == room_id)
        .ok_or_else(|| ApiError::not_found("附件不存在"))?;
    if !att.mime.starts_with("image/") {
        return Err(ApiError::not_found("缩略图不存在"));
    }
    let path = state
        .attachments_dir
        .join(format!("{}.thumb.jpg", att.attachment_id));
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| ApiError::not_found("缩略图不存在"))?;
    let size = file
        .metadata()
        .await
        .map_err(|_| ApiError::not_found("缩略图不存在"))?
        .len();
    let mut resp =
        axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file)).into_response();
    resp.headers_mut().insert(
        axum::http::header::CONTENT_LENGTH,
        axum::http::HeaderValue::from_str(&size.to_string())
            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("0")),
    );
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("image/jpeg"),
    );
    resp.headers_mut().insert(
        axum::http::header::CONTENT_DISPOSITION,
        axum::http::HeaderValue::from_static("inline"),
    );
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("private, max-age=86400"),
    );
    resp.headers_mut().insert(
        axum::http::header::X_CONTENT_TYPE_OPTIONS,
        axum::http::HeaderValue::from_static("nosniff"),
    );
    Ok(resp)
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
    if !state.rl.allow(
        "direct_room",
        &me.user.user_id,
        std::time::Duration::from_secs(3600),
        state.cfg.rate_limit.direct_room_per_hour_per_user,
    ) {
        return Err(ApiError::rate_limited("创建私聊过于频繁"));
    }
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
    // Notify only the two participants; do not leak the DM code to other room members.
    if let Some(parent) = created.parent {
        let event = json!({
            "type": "direct_room_created",
            "room": api::room_json(&created.room, None),
            "peer_user_id": req.user_id,
        });
        if let Ok(payload) = serde_json::to_string(&event) {
            state
                .hub
                .send_to(&parent.room_id, &me.user.user_id, &payload);
            state.hub.send_to(&parent.room_id, &req.user_id, &payload);
        }
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
    if !state.rl.allow(
        "ice_servers",
        &me.user.user_id,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.ice_servers_per_minute_per_user,
    ) {
        return Err(ApiError::rate_limited("媒体凭证领取过于频繁"));
    }
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
    if !state.rl.allow(
        "media_token",
        &me.user.user_id,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.media_token_per_minute_per_user,
    ) {
        return Err(ApiError::rate_limited("媒体 token 领取过于频繁"));
    }
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
        // 仅 SFU + 房间开启时下发；客户端用它初始化 LiveKit E2EE。
        "e2ee_key": if room.e2ee { room.e2ee_key.clone() } else { None },
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
    #[test]
    fn invite_ttl_is_bounded_and_checked() {
        assert!(invite_expiry(now(), std::time::Duration::from_secs(3600)).is_ok());
        assert_eq!(
            invite_expiry(now(), std::time::Duration::ZERO)
                .unwrap_err()
                .status,
            axum::http::StatusCode::BAD_REQUEST
        );
        assert_eq!(
            invite_expiry(
                now(),
                std::time::Duration::from_secs(MAX_INVITE_TTL_SECS + 1),
            )
            .unwrap_err()
            .status,
            axum::http::StatusCode::BAD_REQUEST
        );
    }
}

// ------------------------------------------------- guest upgrade requests

#[derive(Deserialize)]
pub struct GuestUpgradeRequestBody {
    pub user_id: String,
}

/// 房主为某个访客发起转正申请（管理员审批后自动生成绑定邀请）。
pub async fn request_guest_upgrade(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
    me: AuthUser,
    Json(req): Json<GuestUpgradeRequestBody>,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    if member.room_role != RoomRole::Owner {
        return Err(ApiError::forbidden("只有房主可以申请转正"));
    }
    if room.kind != RoomKind::Standard {
        return Err(ApiError::validation("私聊房间不支持转正申请"));
    }
    if !state.rl.allow(
        "guest_upgrade_request",
        &me.user.user_id,
        std::time::Duration::from_secs(3600),
        state.cfg.rate_limit.room_request_per_hour_per_user,
    ) {
        return Err(ApiError::rate_limited("转正申请过于频繁"));
    }
    let room_id = room.room_id.clone();
    let guest_id = req.user_id.clone();
    let requester = me.user.user_id.clone();
    let created = state.db.with_sync(|conn| -> ApiResult<GuestUpgradeRequest> {
        let target = crate::infra::repo::get_member(conn, &room_id, &guest_id)?
            .ok_or_else(|| ApiError::not_found("成员不存在"))?;
        if target.left_at.is_some() {
            return Err(ApiError::validation("该成员已离开房间"));
        }
        let user = crate::infra::repo::get_user(conn, &guest_id)?
            .ok_or_else(|| ApiError::not_found("用户不存在"))?;
        if user.role != UserRole::Guest {
            return Err(ApiError::conflict("该成员已是正式账号"));
        }
        if crate::infra::repo::pending_guest_upgrade_request(conn, &room_id, &guest_id)?.is_some() {
            return Err(ApiError::conflict("该成员已有待审批的转正申请"));
        }
        let r = GuestUpgradeRequest {
            request_id: util::random_id(),
            room_id: room_id.clone(),
            guest_user_id: guest_id.clone(),
            requested_by: requester.clone(),
            status: "pending".into(),
            note: String::new(),
            created_at: util::now(),
            decided_at: None,
            decided_by: None,
            invite_id: None,
        };
        crate::infra::repo::insert_guest_upgrade_request(conn, &r)?;
        api::audited(
            conn,
            Some(&requester),
            "guest_upgrade.requested",
            Some("user"),
            Some(&guest_id),
            json!({ "room_id": room_id }),
        )?;
        Ok(r)
    })?;
    let v = api::guest_upgrade_request_json(&state, &created).await;
    Ok(Json(json!({ "request": v })))
}

// ------------------------------------------------------------- bookmarks

pub async fn room_bookmarks(
    State(state): State<AppState>,
    Path(code): Path<String>,
    me: AuthUser,
) -> ApiResult<impl IntoResponse> {
    let (room, _member) = active_member_room(&state, &code, &me.user.user_id).await?;
    let uid = me.user.user_id.clone();
    let room_id = room.room_id.clone();
    let ids = state
        .db
        .read(move |conn| crate::infra::repo::bookmarked_message_ids(conn, &uid, &room_id))
        .await?;
    Ok(Json(json!({ "message_ids": ids })))
}

pub async fn bookmark_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((code, message_id)): Path<(String, String)>,
    me: AuthUser,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, _member) = active_member_room(&state, &code, &me.user.user_id).await?;
    let uid = me.user.user_id.clone();
    let room_id = room.room_id.clone();
    let mid = message_id.clone();
    state
        .db
        .with(move |conn| {
            let msg = crate::infra::repo::get_message(conn, &mid)?
                .filter(|m| m.room_id == room_id && m.deleted_at.is_none())
                .ok_or_else(|| ApiError::not_found("消息不存在"))?;
            crate::infra::repo::add_bookmark(conn, &uid, &msg.message_id)
        })
        .await?;
    Ok(Json(json!({ "bookmarked": true })))
}

pub async fn unbookmark_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((code, message_id)): Path<(String, String)>,
    me: AuthUser,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, _member) = active_member_room(&state, &code, &me.user.user_id).await?;
    let uid = me.user.user_id.clone();
    let room_id = room.room_id.clone();
    let mid = message_id.clone();
    state
        .db
        .with(move |conn| {
            let msg = crate::infra::repo::get_message(conn, &mid)?
                .filter(|m| m.room_id == room_id)
                .ok_or_else(|| ApiError::not_found("消息不存在"))?;
            crate::infra::repo::remove_bookmark(conn, &uid, &msg.message_id)
        })
        .await?;
    Ok(Json(json!({ "bookmarked": false })))
}

// ------------------------------------------------- room notification prefs

#[derive(Deserialize)]
pub struct RoomNotificationPrefsReq {
    pub muted: bool,
}

pub async fn room_notification_prefs(
    State(state): State<AppState>,
    Path(code): Path<String>,
    me: AuthUser,
) -> ApiResult<impl IntoResponse> {
    let (room, _member) = active_member_room(&state, &code, &me.user.user_id).await?;
    let uid = me.user.user_id.clone();
    let room_id = room.room_id.clone();
    let muted = state
        .db
        .read(move |conn| crate::infra::repo::room_notification_muted(conn, &uid, &room_id))
        .await?;
    Ok(Json(json!({ "muted": muted })))
}

pub async fn set_room_notification_prefs(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
    me: AuthUser,
    Json(req): Json<RoomNotificationPrefsReq>,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, _member) = active_member_room(&state, &code, &me.user.user_id).await?;
    let uid = me.user.user_id.clone();
    let room_id = room.room_id.clone();
    state
        .db
        .with(move |conn| {
            crate::infra::repo::set_room_notification_muted(conn, &uid, &room_id, req.muted)
        })
        .await?;
    Ok(Json(json!({ "muted": req.muted })))
}
