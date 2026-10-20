//! Request guards: client IP, CSRF/Origin checks for state changes and
//! WebSocket upgrades.

use axum::http::HeaderMap;

use crate::api::AppState;
use crate::domain::models::Session;
use crate::error::{ApiError, ApiResult};

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
