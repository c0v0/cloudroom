//! Request guards: client IP, CSRF/Origin checks for state changes and
//! WebSocket upgrades.

use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use axum::http::HeaderMap;

use crate::api::AppState;
use crate::domain::models::Session;
use crate::error::{ApiError, ApiResult};

/// Actual TCP peer, when the server was started with `ConnectInfo`.
#[derive(Clone, Copy, Debug)]
pub struct PeerAddr(pub Option<IpAddr>);

impl<S: Send + Sync> FromRequestParts<S> for PeerAddr {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(PeerAddr(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|info| info.0.ip()),
        ))
    }
}

/// Client IP for rate limits and audit records.
///
/// `X-Forwarded-For` is trusted only when a trusted proxy is explicitly
/// configured and the TCP peer is that proxy (or there is no TCP peer, as with
/// a Unix socket reachable only by the local proxy). The rightmost hop is used
/// because the proxy should append/overwrite it; malformed values fall back to
/// the socket peer.
pub fn client_ip(state: &AppState, headers: &HeaderMap, peer: Option<IpAddr>) -> String {
    resolve_client_ip(state.cfg.server.trusted_proxy.as_deref(), headers, peer)
}

fn resolve_client_ip(
    trusted_proxy: Option<&str>,
    headers: &HeaderMap,
    peer: Option<IpAddr>,
) -> String {
    let configured = trusted_proxy.and_then(|value| value.trim().parse::<IpAddr>().ok());
    let peer_is_trusted = match (configured, peer) {
        (Some(proxy), Some(peer)) => proxy == peer,
        // Unix sockets have no SocketAddr. Only the local proxy can reach
        // them, so an explicit trusted_proxy still authorizes the header.
        (Some(_), None) => true,
        _ => false,
    };
    if peer_is_trusted {
        if let Some(ip) = forwarded_ip(headers) {
            return ip.to_string();
        }
    }
    peer.map(|ip| ip.to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn forwarded_ip(headers: &HeaderMap) -> Option<IpAddr> {
    let raw = headers.get("x-forwarded-for")?.to_str().ok()?;
    raw.rsplit(',').next()?.trim().parse().ok()
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(xff: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_str(xff).unwrap());
        headers
    }

    #[test]
    fn direct_client_ignores_forwarded_header() {
        let peer = "127.0.0.1".parse().unwrap();
        assert_eq!(
            resolve_client_ip(None, &headers("198.51.100.9"), Some(peer)),
            "127.0.0.1"
        );
    }

    #[test]
    fn trusted_proxy_uses_rightmost_forwarded_hop() {
        let peer = "127.0.0.1".parse().unwrap();
        assert_eq!(
            resolve_client_ip(
                Some("127.0.0.1"),
                &headers("203.0.113.7, 198.51.100.9"),
                Some(peer)
            ),
            "198.51.100.9"
        );
    }

    #[test]
    fn configured_proxy_must_match_tcp_peer() {
        let peer = "127.0.0.1".parse().unwrap();
        assert_eq!(
            resolve_client_ip(Some("10.0.0.1"), &headers("203.0.113.7"), Some(peer)),
            "127.0.0.1"
        );
    }

    #[test]
    fn malformed_forwarded_value_falls_back_to_peer() {
        let peer = "127.0.0.1".parse().unwrap();
        assert_eq!(
            resolve_client_ip(Some("127.0.0.1"), &headers("not-an-ip"), Some(peer)),
            "127.0.0.1"
        );
    }
}
