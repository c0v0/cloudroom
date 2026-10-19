//! HTTP API: shared state, auth extractors, CSRF/Origin enforcement, JSON
//! serialization helpers, and the router.

pub mod admin;
pub mod auth_routes;
pub mod me;
pub mod rooms;
pub mod ws;

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{FromRef, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use crate::auth::{self, Auth};
use crate::config::Config;
use crate::db::Db;
use crate::error::{ApiError, ApiResult};
use crate::models::*;
use crate::rate_limit::RateLimiter;
use crate::ws::Hub;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub cfg: Arc<Config>,
    pub auth: Arc<Auth>,
    pub hub: Arc<Hub>,
    pub rl: Arc<RateLimiter>,
    pub avatars_dir: PathBuf,
}

impl FromRef<AppState> for Arc<Config> {
    fn from_ref(s: &AppState) -> Self {
        s.cfg.clone()
    }
}

impl FromRef<AppState> for Arc<Auth> {
    fn from_ref(s: &AppState) -> Self {
        s.auth.clone()
    }
}

impl FromRef<AppState> for Db {
    fn from_ref(s: &AppState) -> Self {
        s.db.clone()
    }
}

impl FromRef<AppState> for Arc<Hub> {
    fn from_ref(s: &AppState) -> Self {
        s.hub.clone()
    }
}

impl FromRef<AppState> for Arc<RateLimiter> {
    fn from_ref(s: &AppState) -> Self {
        s.rl.clone()
    }
}

/// Resolved session + user.
#[derive(Clone)]
pub struct AuthUser {
    pub user: User,
    pub session: Session,
}

pub struct MaybeAuth(pub Option<AuthUser>);

fn resolve_session(state: &AppState, parts: &Parts) -> ApiResult<Option<AuthUser>> {
    let raw = auth::cookie_value(&parts.headers, auth::cookie_name(&state.cfg));
    let Some(raw) = raw else { return Ok(None) };
    if raw.len() > 256 {
        return Ok(None);
    }
    let token_hash = crate::util::sha256_hex(raw.as_bytes());
    let db = state.db.clone();
    let found = tokio::task::block_in_place(|| {
        db.with_sync(|conn| -> ApiResult<Option<(Session, User)>> {
            let Some(session) = crate::repo::get_session_by_token_hash(conn, &token_hash)? else {
                return Ok(None);
            };
            if session.expires_at <= chrono::Utc::now() {
                return Ok(None);
            }
            let Some(user) = crate::repo::get_user(conn, &session.user_id)? else {
                return Ok(None);
            };
            if user.disabled {
                return Ok(None);
            }
            Ok(Some((session, user)))
        })
    })
    .map_err(|e| ApiError::internal(format!("身份校验失败: {e:?}")))?;

    Ok(found.map(|(session, user)| AuthUser { user, session }))
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        async move { resolve_session(state, parts)?.ok_or_else(|| ApiError::unauthorized("未登录")) }
    }
}

impl FromRequestParts<AppState> for MaybeAuth {
    type Rejection = ApiError;

    fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        async move { Ok(MaybeAuth(resolve_session(state, parts)?)) }
    }
}

/// Client IP from X-Forwarded-For. The backend only listens on loopback in
/// production, so the real client is always the trusted proxy's header.
pub fn client_ip(state: &AppState, headers: &HeaderMap) -> String {
    if state.cfg.server.trusted_proxy.is_some() {
        if let Some(xff) = headers.get("x-forwarded-for") {
            if let Ok(v) = xff.to_str() {
                if let Some(first) = v.split(',').next() {
                    let t = first.trim();
                    if !t.is_empty() {
                        return t.to_string();
                    }
                }
            }
        }
    }
    "unknown".to_string()
}

/// CSRF + Origin enforcement for cookie-authenticated state changes.
pub fn require_csrf(state: &AppState, headers: &HeaderMap, session: &Session) -> ApiResult<()> {
    // Origin check: if present it must be allowed; if absent, Fetch Metadata
    // must not claim cross-site.
    if let Some(origin) = headers.get("origin") {
        let origin = origin
            .to_str()
            .map_err(|_| ApiError::bad_request("invalid_origin", "invalid Origin header"))?;
        if !state.cfg.allowed_origins().iter().any(|o| o == origin) {
            return Err(ApiError::forbidden("跨域请求被拒绝"));
        }
    } else if let Some(sfs) = headers.get("sec-fetch-site") {
        if let Ok(v) = sfs.to_str() {
            if v != "same-origin" && v != "none" {
                return Err(ApiError::forbidden("跨域请求被拒绝"));
            }
        }
    }
    let token = headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if token.is_empty() || token != session.csrf_token {
        return Err(ApiError::forbidden("缺少或无效的 CSRF 令牌"));
    }
    Ok(())
}

/// Origin gate for WebSocket upgrades.
pub fn check_ws_origin(state: &AppState, headers: &HeaderMap) -> ApiResult<()> {
    let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) else {
        // Non-browser clients (tests) may omit Origin.
        return Ok(());
    };
    if state.cfg.allowed_origins().iter().any(|o| o == origin) {
        Ok(())
    } else {
        Err(ApiError::forbidden("WebSocket 来源不被允许"))
    }
}

// ----------------------------------------------------------- JSON helpers

pub fn user_json(u: &User, _cfg: &Config) -> Value {
    json!({
        "user_id": u.user_id,
        "username": u.username,
        "role": u.role.as_str(),
        "avatar_url": u.avatar_id.as_ref().map(|a| format!("/api/v1/avatars/{a}")),
        "created_at": u.created_at.to_rfc3339(),
        "disabled": u.disabled,
    })
}

pub fn member_json(m: &RoomMember, user: Option<&User>) -> Value {
    json!({
        "user_id": m.user_id,
        "username": user.and_then(|u| u.username.as_ref()),
        "avatar_url": user.and_then(|u| u.avatar_id.as_ref()).map(|id| format!("/api/v1/avatars/{id}")),
        "nickname": m.nickname,
        "room_role": m.room_role.as_str(),
        "joined_at": m.joined_at.to_rfc3339(),
        "last_online_at": m.last_online_at.map(|d| d.to_rfc3339()),
    })
}

pub fn room_json(r: &Room, my_role: Option<&str>) -> Value {
    json!({
        "code": r.room_code,
        "kind": r.kind.as_str(),
        "state": r.state.as_str(),
        "owner_user_id": r.owner_user_id,
        "created_at": r.created_at.to_rfc3339(),
        "expires_at": r.expires_at.map(|d| d.to_rfc3339()),
        "allow_direct_messages": r.allow_direct_messages,
        "text_enabled": r.text_enabled,
        "media_mode": r.media_mode.as_str(),
        "audio_bitrate": r.audio_bitrate,
        "room_muted": r.room_muted,
        "announcement": r.announcement,
        "max_members": r.max_members,
        "last_seq": r.last_seq,
        "my_role": my_role,
    })
}

pub fn message_json(m: &Message, sender: Option<&User>, nickname: Option<&str>) -> Value {
    json!({
        "id": m.message_id,
        "seq": m.seq,
        "sender": {
            "user_id": m.sender_user_id,
            "username": sender.and_then(|u| u.username.as_ref()),
            "nickname": nickname,
        },
        "body": m.body,
        "reply": m.reply_snapshot.as_ref().and_then(|s| serde_json::from_str(s).ok()).unwrap_or(Value::Null),
        "created_at": m.created_at.to_rfc3339(),
    })
}

pub fn room_request_json(r: &RoomRequest) -> Value {
    json!({
        "request_id": r.request_id,
        "requester_id": r.requester_id,
        "kind": r.request_kind,
        "note": r.note,
        "desired_code": r.desired_code,
        "target_room_code": Value::Null,
        "media_mode": r.media_mode.as_str(),
        "audio_bitrate": r.audio_bitrate,
        "status": r.status,
        "created_at": r.created_at.to_rfc3339(),
        "decided_at": r.decided_at.map(|d| d.to_rfc3339()),
        "decided_by": r.decided_by,
        "decision_reason": r.decision_reason,
        "created_room_code": Value::Null,
    })
}

/// 请求列表展示：补充申请人用户名（admin 视图）与切换申请的目标房间码。
pub async fn requests_json(state: &AppState, rows: &[RoomRequest]) -> Vec<Value> {
    let mut out = Vec::new();
    for r in rows {
        let mut v = room_request_json(r);
        if let Some(obj) = v.as_object_mut() {
            if let Some(rid) = &r.target_room_id {
                let code = state
                    .db
                    .with_sync(|conn| crate::repo::get_room_by_id(conn, rid))
                    .ok()
                    .flatten()
                    .map(|room| room.room_code);
                if let Some(code) = code {
                    obj.insert("target_room_code".into(), json!(code));
                }
            }
            let username = state
                .db
                .with_sync(|c| crate::repo::get_user(c, &r.requester_id))
                .ok()
                .flatten()
                .and_then(|u| u.username)
                .unwrap_or_default();
            obj.insert("requester_username".into(), json!(username));
            // 房间号：自定义码或自动生成占位
            if obj
                .get("desired_code")
                .and_then(|d| d.as_str())
                .map(|s| s.is_empty())
                .unwrap_or(true)
            {
                obj.insert("desired_code".into(), Value::Null);
            }
        }
        out.push(v);
    }
    out
}

pub fn invite_json(i: &Invite, raw_token: Option<&str>) -> Value {
    json!({
        "invite_id": i.invite_id,
        "kind": i.kind,
        "room_code": None::<String>,
        "max_role": i.max_role,
        "created_at": i.created_at.to_rfc3339(),
        "expires_at": i.expires_at.to_rfc3339(),
        "consumed_at": i.consumed_at.map(|d| d.to_rfc3339()),
        "consumed_by": i.consumed_by,
        "revoked_at": i.revoked_at.map(|d| d.to_rfc3339()),
        "active": i.active(chrono::Utc::now()),
        "token": raw_token,
    })
}

pub fn audit_json(a: &AuditEntry) -> Value {
    json!({
        "entry_id": a.entry_id,
        "actor_user_id": a.actor_user_id,
        "action": a.action,
        "target_type": a.target_type,
        "target_id": a.target_id,
        "detail": serde_json::from_str::<Value>(&a.detail).unwrap_or(Value::Null),
        "created_at": a.created_at.to_rfc3339(),
    })
}

/// Look up a room by code, require active state, and require membership.
/// Returns (room, member).
pub async fn active_member_room(
    state: &AppState,
    code: &str,
    user_id: &str,
) -> ApiResult<(Room, RoomMember)> {
    let db = state.db.clone();
    let code = code.to_string();
    let uid = user_id.to_string();
    db.with(move |conn| {
        let room =
            crate::repo::get_room_by_code(conn, &code)?.ok_or_else(ApiError::room_not_found)?;
        if room.state != RoomState::Active {
            return Err(ApiError::room_unavailable());
        }
        let member = crate::repo::get_member(conn, &room.room_id, &uid)?
            .ok_or_else(|| ApiError::forbidden("you are 不是房间成员 of this room"))?;
        Ok((room, member))
    })
    .await
}

/// Capability check helpers -------------------------------------------------

pub fn can(_state: &AppState, member: &RoomMember, room: &Room, cap: &str) -> bool {
    match member.room_role {
        RoomRole::Owner => true,
        // 房间权限只有 owner / member 两级；管理能力全部归房主。
        // 账号权限（user/admin/super_admin/guest）与房间权限无关。
        RoomRole::Member => {
            matches!(cap, "send_text" | "join_voice") && room.state == RoomState::Active
        }
    }
}

pub fn require_cap(state: &AppState, member: &RoomMember, room: &Room, cap: &str) -> ApiResult<()> {
    if can(state, member, room, cap) {
        Ok(())
    } else {
        Err(ApiError::forbidden(format!("缺少权限: {cap}")))
    }
}

// ------------------------------------------------------------------ router

pub fn build_router(state: AppState) -> Router {
    let public = Router::new()
        .route("/health", get(health))
        .route("/config/public", get(public_config))
        .route("/auth/register", post(auth_routes::register))
        .route("/auth/login", post(auth_routes::login))
        .route("/auth/logout", post(auth_routes::logout))
        .route("/auth/upgrade-guest", post(auth_routes::upgrade_guest))
        .route("/avatars/{avatar_id}", get(me::get_avatar));

    let authed = Router::new()
        .route("/me", get(me::me))
        .route("/me/password", post(me::change_password))
        .route(
            "/me/avatar",
            post(me::upload_avatar).delete(me::delete_avatar),
        )
        .route("/me/rooms", get(me::my_rooms))
        .route(
            "/room-requests",
            post(rooms::create_room_request).get(rooms::my_room_requests),
        )
        .route(
            "/room-requests/{id}/cancel",
            post(rooms::cancel_room_request),
        )
        .route("/rooms/{room_code}", get(rooms::get_room))
        .route("/rooms/{room_code}/join", post(rooms::join_room))
        .route("/rooms/{room_code}/leave", post(rooms::leave_room))
        .route("/rooms/{room_code}/settings", post(rooms::update_settings))
        .route("/rooms/{room_code}/delete", post(rooms::delete_room))
        .route(
            "/rooms/{room_code}/invites",
            post(rooms::create_invite).get(rooms::list_invites),
        )
        .route(
            "/rooms/{room_code}/invites/{invite_id}/revoke",
            post(rooms::revoke_invite),
        )
        .route(
            "/rooms/{room_code}/members/{user_id}/role",
            post(rooms::set_member_role),
        )
        .route(
            "/rooms/{room_code}/members/{user_id}/nickname",
            post(rooms::set_nickname),
        )
        .route(
            "/rooms/{room_code}/members/{user_id}/remove",
            post(rooms::remove_member),
        )
        .route(
            "/rooms/{room_code}/mutes",
            post(rooms::create_mute).get(rooms::list_mutes),
        )
        .route(
            "/rooms/{room_code}/mutes/{mute_id}",
            delete(rooms::remove_mute),
        )
        .route(
            "/rooms/{room_code}/messages",
            get(rooms::get_messages).post(rooms::send_message_http),
        )
        .route("/rooms/{room_code}/direct", post(rooms::create_direct_room))
        .route("/rooms/{room_code}/ice-servers", get(rooms::ice_servers))
        .route("/voice/options", get(rooms::voice_options))
        .route("/rooms/{room_code}/media-token", post(rooms::media_token))
        .route("/rooms/{room_code}/ws", get(ws::ws_handler))
        .route("/admin/room-requests", get(admin::list_room_requests))
        .route(
            "/admin/room-requests/{id}/approve",
            post(admin::approve_room_request),
        )
        .route(
            "/admin/room-requests/{id}/reject",
            post(admin::reject_room_request),
        )
        .route("/admin/rooms", get(admin::list_rooms))
        .route("/admin/rooms/{room_code}", get(admin::room_detail))
        .route(
            "/admin/rooms/{room_code}/delete",
            post(admin::force_delete_room),
        )
        .route(
            "/admin/rooms/{room_code}/transfer-owner",
            post(admin::transfer_owner),
        )
        .route("/admin/users", get(admin::list_users))
        .route(
            "/admin/users/{user_id}/disable",
            post(admin::set_user_disabled),
        )
        .route(
            "/admin/users/{user_id}/reset-password",
            post(admin::reset_password),
        )
        .route("/admin/users/{user_id}/role", post(admin::set_user_role))
        .route(
            "/admin/invites",
            post(admin::create_registration_invite).get(admin::list_registration_invites),
        )
        .route(
            "/admin/invites/{invite_id}/revoke",
            post(admin::revoke_registration_invite),
        )
        .route("/admin/audit", get(admin::list_audit))
        .route("/admin/stats", get(admin::stats));

    Router::new()
        .nest("/api/v1", public.merge(authed))
        .with_state(state)
        .layer(axum::middleware::from_fn(crate::api::trace_layer))
}

// ------------------------------------------------------------- health etc.

async fn health(State(state): axum::extract::State<AppState>) -> impl IntoResponse {
    let sqlite_ok = state
        .db
        .with_sync(|conn| crate::repo::count_users(conn).map(|_| true))
        .is_ok();
    let mut checks = json!({
        "sqlite": if sqlite_ok { "ok" } else { "error" },
    });
    // LiveKit 配置已通过启动校验；这里只报告配置状态，不做网络探测。
    checks["livekit"] = json!("configured");
    Json(json!({
        "status": if sqlite_ok { "ok" } else { "degraded" },
        "checks": checks,
        "time": chrono::Utc::now().to_rfc3339(),
    }))
}

async fn public_config(State(state): axum::extract::State<AppState>) -> impl IntoResponse {
    let bootstrap_pending = state
        .db
        .with_sync(|conn| crate::repo::super_admin_exists(conn).map(|b| !b))
        .unwrap_or(false);
    Json(json!({
        "registration_mode": state.cfg.auth.registration_mode,
        "bootstrap_pending": bootstrap_pending,
        "features": {
            "voice": true,
            "screen_share": false,
            "media_playback": false,
            "voice_modes": ["p2p", "sfu"],
            "p2p_voice_max_participants": state.cfg.media.p2p_voice_max_participants,
        },
        "limits": {
            "max_avatar_bytes": state.cfg.security.max_avatar_bytes,
            "max_avatar_dimension": state.cfg.security.max_avatar_dimension,
            "max_nickname_graphemes": state.cfg.security.max_nickname_graphemes,
            "max_message_chars": state.cfg.rooms.max_message_chars,
        },
        "rooms": {
            "generated_code_length": state.cfg.rooms.generated_code_length,
        },
    }))
}

pub fn audited(
    conn: &mut rusqlite::Connection,
    actor: Option<&str>,
    action: &str,
    target_type: Option<&str>,
    target_id: Option<&str>,
    detail: Value,
) -> ApiResult<()> {
    crate::repo::insert_audit(conn, actor, action, target_type, target_id, &detail)?;
    Ok(())
}

/// Access log with redaction: never log cookies, tokens, passwords, bodies.
async fn trace_layer(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let start = std::time::Instant::now();
    let resp = next.run(req).await;
    tracing::info!(
        method = %method,
        path = %path,
        status = resp.status().as_u16(),
        ms = start.elapsed().as_millis(),
        "http"
    );
    resp
}

pub fn use_state(state: AppState) -> Router {
    build_router(state)
}
