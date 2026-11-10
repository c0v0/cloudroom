//! TOTP two-factor setup/disable and second-factor verification.

use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::api::{self, AppState, AuthUser};
use crate::error::{ApiError, ApiResult};
use crate::infra::totp;

fn setup_limited(state: &AppState, user_id: &str) -> ApiResult<()> {
    if !state.rl.allow(
        "totp_setup",
        user_id,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.totp_setup_per_minute_per_user,
    ) {
        return Err(ApiError::rate_limited("两步验证操作过于频繁"));
    }
    Ok(())
}

/// True when `code` is a valid TOTP or an unused recovery code (consumed).
pub(crate) async fn verify_factor(
    state: &AppState,
    user_id: &str,
    secret: &str,
    code: &str,
) -> ApiResult<bool> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Some(step) = totp::verify_step(secret, code, now) {
        let uid = user_id.to_string();
        return state.db.with_sync(move |conn| {
            crate::infra::repo::consume_totp_step(conn, &uid, step)
        });
    }
    let hash = totp::hash_recovery_code(code);
    state
        .db
        .with_sync(move |conn| {
            crate::infra::repo::consume_recovery_code(conn, user_id, &hash)
        })
}

pub async fn setup(
    State(state): State<AppState>,
    headers: HeaderMap,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    setup_limited(&state, &me.user.user_id)?;
    let uid = me.user.user_id.clone();
    let (_secret, enabled) = state
        .db
        .with_sync(move |conn| crate::infra::repo::totp_state(conn, &uid))?;
    if enabled {
        return Err(ApiError::conflict("两步验证已启用"));
    }
    let secret = totp::generate_secret();
    let secret2 = secret.clone();
    let uid = me.user.user_id.clone();
    state
        .db
        .with_sync(move |conn| crate::infra::repo::set_totp_secret(conn, &uid, &secret2))?;
    let account = me.user.username.clone().unwrap_or_else(|| "user".into());
    let otpauth = totp::otpauth_url(&account, &secret);
    Ok(Json(json!({ "secret": secret, "otpauth_url": otpauth })))
}

#[derive(Deserialize)]
pub struct EnableReq {
    pub code: String,
    #[serde(default)]
    pub password: Option<String>,
}

pub async fn enable(
    State(state): State<AppState>,
    headers: HeaderMap,
    me: AuthUser,
    Json(req): Json<EnableReq>,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    setup_limited(&state, &me.user.user_id)?;
    if let Some(hash) = me.user.password_hash.clone() {
        let password = req.password.as_deref().unwrap_or("");
        if password.is_empty()
            || !crate::infra::auth::verify_password_blocking(
                state.auth.clone(),
                password.to_string(),
                hash,
            )
            .await?
        {
            return Err(ApiError::forbidden("启用两步验证需要当前密码"));
        }
    }
    let uid = me.user.user_id.clone();
    let (secret, enabled) = state
        .db
        .with_sync(move |conn| crate::infra::repo::totp_state(conn, &uid))?;
    if enabled {
        return Err(ApiError::conflict("两步验证已启用"));
    }
    let secret = secret.ok_or_else(|| ApiError::validation("请先开始设置两步验证"))?;
    if !verify_factor(&state, &me.user.user_id, &secret, &req.code).await? {
        return Err(ApiError::unauthorized("动态码或恢复码无效"));
    }
    let codes = totp::generate_recovery_codes();
    let hashes: Vec<String> = codes.iter().map(|c| totp::hash_recovery_code(c)).collect();
    let uid = me.user.user_id.clone();
    let actor = uid.clone();
    state.db.with_sync(move |conn| {
        crate::infra::repo::enable_totp(conn, &uid)?;
        crate::infra::repo::replace_recovery_codes(conn, &uid, &hashes)?;
        crate::app::audit::audited(
            conn,
            Some(&actor),
            "user.totp_enabled",
            Some("user"),
            Some(&actor),
            json!({}),
        )?;
        Ok(())
    })?;
    Ok(Json(json!({ "ok": true, "recovery_codes": codes })))
}

#[derive(Deserialize)]
pub struct DisableReq {
    /// Required when the account has a password.
    pub password: Option<String>,
    pub code: String,
}

pub async fn disable(
    State(state): State<AppState>,
    headers: HeaderMap,
    me: AuthUser,
    Json(req): Json<DisableReq>,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    setup_limited(&state, &me.user.user_id)?;
    let uid = me.user.user_id.clone();
    let (secret, enabled) = state
        .db
        .with_sync(move |conn| crate::infra::repo::totp_state(conn, &uid))?;
    if !enabled {
        return Err(ApiError::conflict("两步验证未启用"));
    }
    let secret = secret.ok_or_else(|| ApiError::internal("两步验证配置缺失"))?;
    if let Some(hash) = me.user.password_hash.as_ref() {
        let password = req.password.as_deref().unwrap_or("");
        if password.is_empty()
            || !crate::infra::auth::verify_password_blocking(
                state.auth.clone(),
                password.to_string(),
                hash.clone(),
            )
            .await?
        {
            return Err(ApiError::unauthorized("密码错误"));
        }
    }
    if !verify_factor(&state, &me.user.user_id, &secret, &req.code).await? {
        return Err(ApiError::unauthorized("动态码或恢复码无效"));
    }
    let uid = me.user.user_id.clone();
    let actor = uid.clone();
    state.db.with_sync(move |conn| {
        crate::infra::repo::disable_totp(conn, &uid)?;
        crate::app::audit::audited(
            conn,
            Some(&actor),
            "user.totp_disabled",
            Some("user"),
            Some(&actor),
            json!({}),
        )?;
        Ok(())
    })?;
    Ok(Json(json!({ "ok": true })))
}
