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
    let stored = state
        .db
        .with(move |conn| {
            crate::infra::repo::upsert_push_subscription(conn, &id, &uid, &ep, &p256dh, &auth)
        })
        .await?;
    if !stored {
        return Err(ApiError::validation("该推送端点已被其他账号注册"));
    }
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
        Some(url::Host::Ipv4(ip)) => is_public_ip(ip.into()),
        Some(url::Host::Ipv6(ip)) => is_public_ip(ip.into()),
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

fn is_public_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (64..=127).contains(&o[1])))
        }
        std::net::IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(v4.into());
            }
            let seg = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || seg[0] & 0xfe00 == 0xfc00
                || seg[0] & 0xffc0 == 0xfe80)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::valid_push_endpoint;

    #[test]
    fn push_endpoint_validation_blocks_internal_targets() {
        assert!(valid_push_endpoint("https://fcm.googleapis.com/fcm/send/abc"));
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
