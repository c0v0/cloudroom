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
    if !state.rl.allow(
        "push_subscribe",
        &me.user.user_id,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.push_subscribe_per_minute_per_user,
    ) {
        return Err(ApiError::rate_limited("推送订阅过于频繁"));
    }
    if !state.cfg.push.enabled() {
        return Err(ApiError::bad_request("push_disabled", "推送未启用"));
    }
    let endpoint = req.endpoint.trim();
    if endpoint.is_empty()
        || endpoint.len() > 1024
        || !valid_push_endpoint(endpoint)
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
        .with(move |conn| -> ApiResult<()> {
            const MAX_PUSH_SUBSCRIPTIONS_PER_USER: i64 = 10;
            let exists = crate::infra::repo::push_subscription_exists(conn, &uid, &ep)?;
            if !exists
                && crate::infra::repo::count_push_subscriptions(conn, &uid)?
                    >= MAX_PUSH_SUBSCRIPTIONS_PER_USER
            {
                return Err(ApiError::validation("推送订阅数量已达上限"));
            }
            if !crate::infra::repo::upsert_push_subscription(conn, &id, &uid, &ep, &p256dh, &auth)?
            {
                return Err(ApiError::validation("该推送端点已被其他账号注册"));
            }
            Ok(())
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

/// 只允许公网 HTTPS 443 的推送端点，阻止内网 SSRF（环回/私网/链路本地等）。
fn valid_push_endpoint(endpoint: &str) -> bool {
    let Ok(url) = url::Url::parse(endpoint) else {
        return false;
    };
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    match url.host() {
        Some(url::Host::Ipv4(ip)) => crate::util::is_public_ip(ip.into()),
        Some(url::Host::Ipv6(ip)) => crate::util::is_public_ip(ip.into()),
        Some(url::Host::Domain(host)) => {
            let host = host.to_ascii_lowercase();
            !(host == "localhost"
                || host.ends_with(".localhost")
                || host.ends_with(".local")
                || host.ends_with(".internal")
                || host.ends_with(".home.arpa"))
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::valid_push_endpoint;

    #[test]
    fn push_endpoint_validation_blocks_internal_targets() {
        assert!(valid_push_endpoint(
            "https://fcm.googleapis.com/fcm/send/abc"
        ));
        assert!(valid_push_endpoint(
            "https://updates.push.services.mozilla.com/wpush/v2/abc"
        ));
        assert!(!valid_push_endpoint("http://example.com/push"));
        assert!(!valid_push_endpoint("https://example.com:8443/push"));
        assert!(!valid_push_endpoint("https://127.0.0.1/push"));
        assert!(!valid_push_endpoint("https://[::1]/push"));
        assert!(!valid_push_endpoint("https://10.1.2.3/push"));
        assert!(!valid_push_endpoint("https://169.254.1.1/push"));
        assert!(!valid_push_endpoint("https://localhost/push"));
        assert!(!valid_push_endpoint("https://push.internal/push"));
    }
}
