//! Registration (bootstrap + invite-only), login, logout.

use axum::extract::State;
use axum::http::{header, HeaderMap};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::api::{self, AppState, AuthUser, MaybeAuth};
use crate::domain::models::UserRole;
use crate::error::{ApiError, ApiResult};
use crate::infra::auth;

#[derive(Deserialize)]
pub struct RegisterReq {
    pub username: String,
    pub password: String,
    /// bootstrap token (first user) or registration invite token.
    pub token: Option<String>,
}

#[derive(Deserialize)]
pub struct LoginReq {
    pub username: String,
    pub password: String,
}

pub async fn register(
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: api::PeerAddr,
    MaybeAuth(_): MaybeAuth,
    Json(req): Json<RegisterReq>,
) -> ApiResult<Response> {
    let ip = api::client_ip(&state, &headers, peer.0);
    if !state.rl.allow(
        "register",
        &ip,
        std::time::Duration::from_secs(3600),
        state.cfg.rate_limit.register_per_hour_per_ip,
    ) {
        return Err(ApiError::rate_limited("注册尝试过于频繁"));
    }

    let username = req.username.trim().to_string();
    if !crate::util::validate_username(&username) {
        return Err(ApiError::validation("用户名需为 3-32 个字符且不含控制字符"));
    }
    let pw_bytes = req.password.len();
    if pw_bytes < state.cfg.auth.password_min_bytes {
        return Err(ApiError::validation("密码过短"));
    }
    if pw_bytes > state.cfg.auth.password_max_bytes {
        return Err(ApiError::validation("密码过长"));
    }
    if req.password.contains('\0') {
        return Err(ApiError::validation("密码包含 NUL 字符"));
    }

    let has_super = state
        .db
        .with_sync(crate::infra::repo::super_admin_exists)
        .unwrap_or(false);

    let mut role = UserRole::User;
    let mut invite_id: Option<String> = None;
    if !has_super {
        // Bootstrap path: token required, first success becomes super_admin.
        let token = req
            .token
            .as_deref()
            .ok_or_else(|| ApiError::bad_request("bootstrap_required", "首次注册需要引导令牌"))?;
        let token_hash = crate::util::sha256_hex(token.as_bytes());
        let winner = state
            .db
            .with_sync(|conn| -> ApiResult<bool> {
                let avail = crate::infra::repo::bootstrap_token_available(
                    conn,
                    &token_hash,
                    &chrono::Utc::now(),
                )?;
                if !avail {
                    return Ok(false);
                }
                crate::infra::repo::consume_bootstrap_token(conn, &token_hash)
            })
            .unwrap_or(false);
        if !winner {
            return Err(ApiError::bad_request(
                "bootstrap_invalid",
                "引导令牌无效、已过期或已被使用",
            ));
        }
        role = UserRole::SuperAdmin;
        tracing::info!(username = %username, "bootstrap registration: first user became super_admin");
    } else if state.cfg.auth.registration_mode == "invite_only" {
        let token = req
            .token
            .as_deref()
            .ok_or_else(|| ApiError::bad_request("invite_required", "注册需要管理员邀请"))?;
        let token_hash = crate::util::sha256_hex(token.as_bytes());
        let id = state.db.with_sync(|conn| -> ApiResult<String> {
            let inv = crate::infra::repo::get_invite_by_hash(conn, &token_hash)?
                .ok_or_else(|| ApiError::bad_request("invite_invalid", "邀请不存在"))?;
            if inv.kind != "registration" || !inv.active(chrono::Utc::now()) {
                return Err(ApiError::bad_request(
                    "invite_invalid",
                    "邀请已过期、已撤销或已被使用",
                ));
            }
            Ok(inv.invite_id)
        })?;
        invite_id = Some(id);
    }
    // registration_mode == "open": public registration allowed.

    let password_hash =
        auth::hash_password_blocking(state.auth.clone(), req.password.clone()).await?;
    let user_id = crate::util::random_id();
    let username_for_db = username.clone();
    state
        .db
        .with_sync(|conn| -> ApiResult<()> {
            crate::infra::repo::create_user(
                conn,
                &user_id,
                Some(&username_for_db),
                Some(&password_hash),
                role,
            )?;
            // Consume the registration invite after the user exists (FK).
            if let Some(id) = invite_id.as_deref() {
                if !crate::infra::repo::consume_invite(conn, id, &user_id)? {
                    return Err(ApiError::bad_request("invite_invalid", "邀请无法兑换"));
                }
            }
            api::audited(
                conn,
                Some(&user_id),
                "user.registered",
                Some("user"),
                Some(&user_id),
                json!({}),
            )?;
            Ok(())
        })
        .map_err(|e: ApiError| {
            if e.code == "conflict" {
                ApiError::conflict("用户名已被占用")
            } else {
                e
            }
        })?;

    let raw = new_session(&state, &user_id, &headers, &ip)?;
    let user = state
        .db
        .with_sync(|conn| crate::infra::repo::get_user(conn, &user_id))?
        .ok_or_else(|| ApiError::internal("user vanished after registration"))?;
    let mut resp = Json(json!({ "user": api::user_json(&user, &state.cfg) })).into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        state.auth.session_cookie(&raw).parse().unwrap(),
    );
    Ok(resp)
}

pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: api::PeerAddr,
    Json(req): Json<LoginReq>,
) -> ApiResult<Response> {
    let ip = api::client_ip(&state, &headers, peer.0);
    if !state.rl.allow(
        "login_ip",
        &ip,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.login_per_minute_per_ip,
    ) {
        return Err(ApiError::rate_limited("登录尝试过于频繁"));
    }
    let username = req.username.trim().to_string();
    let account_key = if username.is_empty() || username.chars().count() > 32 {
        "invalid".to_string()
    } else {
        username.clone()
    };
    if !state.rl.allow(
        "login_acct",
        &account_key,
        std::time::Duration::from_secs(3600),
        state.cfg.rate_limit.login_per_hour_per_account,
    ) {
        return Err(ApiError::rate_limited("登录尝试过于频繁 for this account"));
    }
    if username.is_empty()
        || username.chars().count() > 32
        || req.password.len() > state.cfg.auth.password_max_bytes
    {
        let _ = auth::verify_dummy_blocking(state.auth.clone(), req.password.clone()).await;
        return Err(ApiError::unauthorized("用户名或密码错误"));
    }

    let user = state
        .db
        .with_sync(|conn| crate::infra::repo::get_user_by_username(conn, &username))
        .unwrap_or(None);
    let Some(user) = user else {
        let _ = auth::verify_dummy_blocking(state.auth.clone(), req.password.clone()).await;
        return Err(ApiError::unauthorized("用户名或密码错误"));
    };

    let Some(hash) = &user.password_hash else {
        let _ = auth::verify_dummy_blocking(state.auth.clone(), req.password.clone()).await;
        return Err(ApiError::unauthorized("用户名或密码错误"));
    };
    if !auth::verify_password_blocking(state.auth.clone(), req.password.clone(), hash.clone())
        .await?
    {
        return Err(ApiError::unauthorized("用户名或密码错误"));
    }
    if user.disabled {
        return Err(ApiError::forbidden("账号已被禁用"));
    }
    if user.role == UserRole::Guest {
        return Err(ApiError::forbidden("访客账号不能登录"));
    }
    state.rl.reset("login_acct", &account_key);

    let raw = new_session(&state, &user.user_id, &headers, &ip)?;
    let mut resp = Json(json!({ "user": api::user_json(&user, &state.cfg) })).into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        state.auth.session_cookie(&raw).parse().unwrap(),
    );
    Ok(resp)
}

pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    me: AuthUser,
) -> ApiResult<Response> {
    api::require_csrf(&state, &headers, &me.session)?;
    state
        .db
        .with_sync(|conn| crate::infra::repo::delete_session(conn, &me.session.session_id))?;
    state.hub.unregister_user(&me.user.user_id);
    let mut resp = Json(json!({ "ok": true })).into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        state.auth.clear_cookie().parse().unwrap(),
    );
    Ok(resp)
}

fn new_session(
    state: &AppState,
    user_id: &str,
    headers: &HeaderMap,
    ip: &str,
) -> ApiResult<String> {
    let raw = crate::util::random_token();
    let mut session = state.auth.new_session(
        user_id,
        Some(ip.to_string()),
        headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string()),
    );
    session.token_hash = crate::util::sha256_hex(raw.as_bytes());
    state
        .db
        .with_sync(|conn| crate::infra::repo::create_session(conn, &session))?;
    Ok(raw)
}

/// 未注册访客在房间内当场注册：凭有效邀请码把 guest 账号升级为正式用户，
/// 已加入的房间成员关系（角色/昵称/加入时间）原样保留并绑定到新账号。
#[derive(Deserialize)]
pub struct UpgradeGuestReq {
    pub username: String,
    pub password: String,
    /// 房间邀请码或注册邀请码（注册必需；房间邀请码需属于该访客已加入的房间）
    pub invite_token: String,
}

pub async fn upgrade_guest(
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: api::PeerAddr,
    me: AuthUser,
    Json(req): Json<UpgradeGuestReq>,
) -> ApiResult<Response> {
    api::require_csrf(&state, &headers, &me.session)?;
    let guest = me;
    if guest.user.role != UserRole::Guest {
        return Err(ApiError::conflict("当前账号不是访客，无需升级"));
    }
    let username = req.username.trim().to_string();
    if !crate::util::validate_username(&username) {
        return Err(ApiError::validation("用户名需为 3-32 个字符且不含控制字符"));
    }
    let pw = req.password.len();
    if pw < state.cfg.auth.password_min_bytes || pw > state.cfg.auth.password_max_bytes {
        return Err(ApiError::validation("密码长度不符合要求"));
    }
    if req.password.contains('\0') {
        return Err(ApiError::validation("密码包含 NUL"));
    }
    let guest_id = guest.user.user_id.clone();
    let token_hash = crate::util::sha256_hex(req.invite_token.as_bytes());
    let ip = api::client_ip(&state, &headers, peer.0);
    if !state.rl.allow(
        "invite_redeem",
        &ip,
        std::time::Duration::from_secs(3600),
        state.cfg.rate_limit.invite_redeem_per_hour_per_ip,
    ) {
        return Err(ApiError::rate_limited("邀请兑换过于频繁"));
    }
    // Validate before paying the Argon2 cost; the transaction below repeats
    // the checks so a concurrent redemption still fails atomically.
    state.db.with_sync(|conn| -> ApiResult<()> {
        let inv = crate::infra::repo::get_invite_by_hash(conn, &token_hash)?
            .ok_or_else(|| ApiError::bad_request("invite_invalid", "邀请码无效"))?;
        if !inv.active(crate::util::now()) {
            return Err(ApiError::bad_request(
                "invite_invalid",
                "邀请码已过期或已使用",
            ));
        }
        if inv.kind == "room" {
            let room_id = inv
                .room_id
                .as_deref()
                .ok_or_else(|| ApiError::bad_request("invite_invalid", "邀请码无效"))?;
            let member =
                crate::infra::repo::get_member(conn, room_id, &guest_id)?.ok_or_else(|| {
                    ApiError::bad_request("invite_invalid", "该邀请不属于您所在的房间")
                })?;
            if member.left_at.is_some() {
                return Err(ApiError::bad_request(
                    "invite_invalid",
                    "该邀请不属于您所在的房间",
                ));
            }
        } else if inv.kind != "registration" {
            return Err(ApiError::bad_request("invite_invalid", "邀请码类型无效"));
        }
        Ok(())
    })?;
    let password_hash =
        auth::hash_password_blocking(state.auth.clone(), req.password.clone()).await?;
    let new_user_id = crate::util::random_id();

    let upgraded = state
        .db
        .with_sync(|conn| -> ApiResult<Vec<String>> {
            // 校验邀请：注册邀请，或属于该访客已加入房间的房间邀请
            let inv = crate::infra::repo::get_invite_by_hash(conn, &token_hash)?
                .ok_or_else(|| ApiError::bad_request("invite_invalid", "邀请码无效"))?;
            if !inv.active(crate::util::now()) {
                return Err(ApiError::bad_request(
                    "invite_invalid",
                    "邀请码已过期或已使用",
                ));
            }
            if inv.kind == "room" {
                let room_id = inv
                    .room_id
                    .as_deref()
                    .ok_or_else(|| ApiError::bad_request("invite_invalid", "邀请码无效"))?;
                let member =
                    crate::infra::repo::get_member(conn, room_id, &guest_id)?.ok_or_else(|| {
                        ApiError::bad_request("invite_invalid", "该邀请不属于您所在的房间")
                    })?;
                if member.left_at.is_some() {
                    return Err(ApiError::bad_request(
                        "invite_invalid",
                        "该邀请不属于您所在的房间",
                    ));
                }
            } else if inv.kind != "registration" {
                return Err(ApiError::bad_request("invite_invalid", "邀请码类型无效"));
            }
            // 创建正式账号
            crate::infra::repo::create_user(
                conn,
                &new_user_id,
                Some(&username),
                Some(&password_hash),
                UserRole::User,
            )?;
            if !crate::infra::repo::consume_invite(conn, &inv.invite_id, &new_user_id)? {
                return Err(ApiError::bad_request("invite_invalid", "邀请码无法兑换"));
            }
            // 保留房间状态：把访客的所有成员关系绑定到新账号
            let mut bound_rooms: Vec<String> = Vec::new();
            {
                let mut stmt = conn
                    .prepare(
                        "SELECT room_id FROM room_members WHERE user_id = ?1 AND left_at IS NULL",
                    )
                    .map_err(ApiError::from)?;
                let rows = stmt
                    .query_map([&guest_id], |r| r.get::<_, String>(0))
                    .map_err(ApiError::from)?;
                for r in rows {
                    bound_rooms.push(r.map_err(ApiError::from)?);
                }
            }
            conn.execute(
                "UPDATE room_members SET user_id = ?1 WHERE user_id = ?2",
                rusqlite::params![new_user_id, guest_id],
            )
            .map_err(ApiError::from)?;
            // 禁言状态与审计归属一并转移到新账号（否则 DELETE 会撞 FK）
            conn.execute(
                "UPDATE mutes SET target_user_id = ?1 WHERE target_user_id = ?2",
                rusqlite::params![new_user_id, guest_id],
            )
            .map_err(ApiError::from)?;
            conn.execute(
                "UPDATE audit_log SET actor_user_id = ?1 WHERE actor_user_id = ?2",
                rusqlite::params![new_user_id, guest_id],
            )
            .map_err(ApiError::from)?;
            // 邀请兑换记录归属新账号
            conn.execute(
                "UPDATE invites SET consumed_by = ?1 WHERE consumed_by = ?2",
                rusqlite::params![new_user_id, guest_id],
            )
            .map_err(ApiError::from)?;
            // 访客发的历史消息归属新账号（保持同一个人）
            conn.execute(
                "UPDATE messages SET sender_user_id = ?1 WHERE sender_user_id = ?2",
                rusqlite::params![new_user_id, guest_id],
            )
            .map_err(ApiError::from)?;
            // 删除访客账号（级联清理其旧会话）
            conn.execute("DELETE FROM users WHERE user_id = ?1", [&guest_id])
                .map_err(ApiError::from)?;
            api::audited(
                conn,
                Some(&new_user_id),
                "guest.upgraded",
                Some("user"),
                Some(&new_user_id),
                serde_json::json!({ "bound_rooms": bound_rooms.len() }),
            )?;
            Ok(bound_rooms)
        })
        .map_err(|e: ApiError| {
            if e.code == "conflict" {
                ApiError::conflict("用户名已被占用")
            } else {
                e
            }
        })?;

    // The old guest identity is gone; close its sockets before issuing the
    // new account's session.
    state.hub.unregister_user(&guest_id);
    let raw = new_session(
        &state,
        &new_user_id,
        &headers,
        &api::client_ip(&state, &headers, peer.0),
    )?;
    let user = state
        .db
        .with_sync(|conn| crate::infra::repo::get_user(conn, &new_user_id))?
        .ok_or_else(|| ApiError::internal("user vanished"))?;
    let mut resp = Json(json!({
        "user": api::user_json(&user, &state.cfg),
        "bound_rooms": upgraded,
    }))
    .into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        state.auth.session_cookie(&raw).parse().unwrap(),
    );
    Ok(resp)
}
