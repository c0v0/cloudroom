//! Web Push 订阅端点（VAPID 公钥 / 订阅 / 退订）。

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::api::{self, AppState, AuthUser};
use crate::error::{ApiError, ApiResult};

pub async fn public_key(
    State(state): State<AppState>,
    _me: AuthUser,
) -> ApiResult<impl IntoResponse> {
    let keys = crate::infra::push::PushKeys {
        public_key: state.cfg.push.vapid_public_key.clone(),
        private_key_pkcs8: state.cfg.push.vapid_private_key.clone(),
        subject: state.cfg.push.subject.clone(),
    };
    Ok(Json(json!({
        "enabled": state.cfg.push.enabled(),
        "key": keys.derived_public_key(),
    })))
}

#[derive(Deserialize)]
pub struct SubscribeKeys {
    pub p256dh: String,
    pub auth: String,
}

#[derive(Deserialize)]
pub struct SubscribeReq {
    pub endpoint: String,
    pub keys: SubscribeKeys,
}

pub async fn subscribe(
    State(state): State<AppState>,
    headers: HeaderMap,
    me: AuthUser,
    Json(req): Json<SubscribeReq>,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    if !state.cfg.push.enabled() {
        return Err(ApiError::bad_request("push_disabled", "推送未启用"));
    }
    let endpoint = req.endpoint.trim();
    if endpoint.is_empty()
        || endpoint.len() > 1024
        || !endpoint.starts_with("https://")
        || req.keys.p256dh.len() > 256
        || req.keys.auth.len() > 128
    {
        return Err(ApiError::validation("推送订阅参数不合法"));
    }
    let uid = me.user.user_id.clone();
    let id = crate::util::random_id();
    let (ep, p256dh, auth) = (
        endpoint.to_string(),
        req.keys.p256dh.clone(),
        req.keys.auth.clone(),
    );
    state
        .db
        .with(move |conn| {
            crate::infra::repo::upsert_push_subscription(conn, &id, &uid, &ep, &p256dh, &auth)
        })
        .await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct UnsubscribeReq {
    pub endpoint: String,
}

pub async fn unsubscribe(
    State(state): State<AppState>,
    headers: HeaderMap,
    me: AuthUser,
    Json(req): Json<UnsubscribeReq>,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let uid = me.user.user_id.clone();
    let endpoint = req.endpoint.trim().to_string();
    state
        .db
        .with(move |conn| crate::infra::repo::delete_push_subscription(conn, &uid, &endpoint))
        .await?;
    Ok(Json(json!({ "ok": true })))
}
