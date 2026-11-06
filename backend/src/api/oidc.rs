//! OIDC authorization-code login (PKCE, one-time state, verified ID token).

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::api::{self, auth_routes, AppState};
use crate::error::{ApiError, ApiResult};
use crate::infra::oidc;
use crate::util;

#[derive(Deserialize)]
pub struct LoginQuery {
    pub next: Option<String>,
}

fn safe_next(raw: Option<&str>) -> String {
    raw.filter(|s| {
        s.starts_with('/')
            && !s.starts_with("//")
            && !s.contains('\\')
            && !s.chars().any(char::is_control)
    })
    .unwrap_or("/")
    .to_string()
}

const OIDC_STATE_COOKIE: &str = "cloudroom_oidc_state";

fn state_cookie(state: &AppState, value: &str) -> String {
    let secure = if state.cfg.server.insecure_cookies {
        ""
    } else {
        "; Secure"
    };
    format!(
        "{OIDC_STATE_COOKIE}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age=600{secure}"
    )
}

fn clear_state_cookie(state: &AppState) -> String {
    let secure = if state.cfg.server.insecure_cookies {
        ""
    } else {
        "; Secure"
    };
    format!(
        "{OIDC_STATE_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{secure}"
    )
}

fn append_cookie(resp: &mut Response, value: String) {
    if let Ok(v) = HeaderValue::from_str(&value) {
        resp.headers_mut().append(header::SET_COOKIE, v);
    }
}

fn frontend_url(state: &AppState, path: &str) -> String {
    format!(
        "{}{}",
        state.cfg.server.frontend_origin.trim_end_matches('/'),
        path
    )
}

fn internal_oidc(e: String) -> ApiError {
    tracing::warn!(error = %e, "OIDC request failed");
    ApiError::internal("SSO 登录失败")
}

pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: api::PeerAddr,
    Query(q): Query<LoginQuery>,
) -> ApiResult<Response> {
    if !state.cfg.auth.oidc.enabled {
        return Err(ApiError::not_found("SSO 未启用"));
    }
    let ip = api::client_ip(&state, &headers, peer.0);
    if !state.rl.allow(
        "oidc_login",
        &ip,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.oidc_login_per_minute_per_ip,
    ) {
        return Err(ApiError::rate_limited("SSO 登录过于频繁"));
    }
    let provider = oidc::discover(&state.cfg.auth.oidc.issuer)
        .await
        .map_err(internal_oidc)?;
    let auth_state = util::random_token();
    let nonce = util::random_token();
    let verifier = util::random_token();
    let next = safe_next(q.next.as_deref());
    let expires = util::now() + chrono::Duration::minutes(10);
    let (s, n, v, next2) = (
        auth_state.clone(),
        nonce.clone(),
        verifier.clone(),
        next.clone(),
    );
    state.db.with_sync(move |conn| {
        let _ = crate::infra::repo::purge_expired_oidc_states(conn);
        crate::infra::repo::create_oidc_state(conn, &s, &n, &v, &next2, &expires)
    })?;
    let redirect_uri = state.cfg.oidc_redirect_url();
    let url = oidc::authorization_url(
        &provider,
        &state.cfg.auth.oidc,
        &redirect_uri,
        &auth_state,
        &nonce,
        &verifier,
    )
    .map_err(internal_oidc)?;
    let mut resp = Redirect::to(&url).into_response();
    append_cookie(&mut resp, state_cookie(&state, &auth_state));
    Ok(resp)
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    pub code: String,
    pub state: String,
}

pub async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: api::PeerAddr,
    Query(q): Query<CallbackQuery>,
) -> Response {
    let frontend_origin = state
        .cfg
        .server
        .frontend_origin
        .trim_end_matches('/')
        .to_string();
    let clear = clear_state_cookie(&state);
    let mut resp = match callback_inner(state, headers, peer, q).await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::warn!(error = %e.message, code = %e.code, "OIDC callback failed");
            Redirect::to(&format!("{frontend_origin}/login?sso_error=1")).into_response()
        }
    };
    append_cookie(&mut resp, clear);
    resp
}

async fn callback_inner(
    state: AppState,
    headers: HeaderMap,
    peer: api::PeerAddr,
    q: CallbackQuery,
) -> ApiResult<Response> {
    if !state.cfg.auth.oidc.enabled {
        return Err(ApiError::not_found("SSO 未启用"));
    }
    // The state row alone is a bearer value: bind it to this browser so an
    // attacker cannot complete their own login in a victim's browser
    // (login CSRF). SameSite=Lax is sent on the IdP's top-level redirect.
    let cookie_state = crate::infra::auth::cookie_value(&headers, OIDC_STATE_COOKIE);
    if cookie_state.as_deref() != Some(q.state.as_str()) {
        return Err(ApiError::unauthorized("SSO 状态与当前浏览器不匹配"));
    }
    let ip = api::client_ip(&state, &headers, peer.0);
    if !state.rl.allow(
        "oidc_login",
        &ip,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.oidc_login_per_minute_per_ip,
    ) {
        return Err(ApiError::rate_limited("SSO 登录过于频繁"));
    }
    let (nonce, verifier, next) = state
        .db
        .with_sync(|conn| crate::infra::repo::take_oidc_state(conn, &q.state))?
        .ok_or_else(|| ApiError::unauthorized("SSO 状态无效或已过期"))?;
    let provider = oidc::discover(&state.cfg.auth.oidc.issuer)
        .await
        .map_err(internal_oidc)?;
    let token = oidc::exchange_code(
        &provider,
        &state.cfg.auth.oidc,
        &q.code,
        &verifier,
        &state.cfg.oidc_redirect_url(),
    )
    .await
    .map_err(internal_oidc)?;
    let claims = oidc::verify_id_token(&provider, &state.cfg.auth.oidc, &token.id_token, &nonce)
        .await
        .map_err(internal_oidc)?;
    if state.cfg.auth.oidc.require_verified_email && claims.email_verified != Some(true) {
        return Err(ApiError::forbidden("SSO 邮箱未验证"));
    }
    let issuer = provider.issuer.clone();
    let subject = claims.sub.clone();
    let email = claims
        .email
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase());
    let user_id = state
        .db
        .with_sync(|conn| -> ApiResult<String> {
            if let Some(uid) =
                crate::infra::repo::get_user_by_oidc(conn, &issuer, &subject)?
            {
                return Ok(uid);
            }
            if let Some(uid) = email
                .as_deref()
                .map(|e| crate::infra::repo::get_user_by_email(conn, e))
                .transpose()?
                .flatten()
            {
                if !crate::infra::repo::link_user_oidc(
                    conn,
                    &uid,
                    &issuer,
                    &subject,
                    email.as_deref(),
                )? {
                    return Err(ApiError::conflict("该账号已绑定其他 SSO 身份"));
                }
                return Ok(uid);
            }
            if !state.cfg.auth.oidc.auto_create {
                return Err(ApiError::forbidden("SSO 账号未开通，请联系管理员"));
            }
            let raw_name = claims
                .preferred_username
                .as_deref()
                .or(claims.name.as_deref())
                .or(email.as_deref().and_then(|e| e.split('@').next()))
                .unwrap_or("sso-user");
            let base: String = raw_name
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
                .take(32)
                .collect();
            let mut username = if util::validate_username(&base) {
                base
            } else {
                "sso-user".to_string()
            };
            for _ in 0..20 {
                if !crate::infra::repo::username_exists(conn, &username)? {
                    break;
                }
                let suffix = &util::random_id()[..4];
                username = format!("{}_{}", username.chars().take(27).collect::<String>(), suffix);
            }
            let uid = util::random_id();
            crate::infra::repo::create_oidc_user(
                conn,
                &uid,
                &username,
                email.as_deref(),
                &issuer,
                &subject,
            )?;
            crate::app::audit::audited(
                conn,
                Some(&uid),
                "user.oidc_created",
                Some("user"),
                Some(&uid),
                serde_json::json!({ "username": username }),
            )?;
            Ok(uid)
        })
        ?;
    let user = state
        .db
        .with_sync(|conn| crate::infra::repo::get_user(conn, &user_id))?
        .ok_or_else(|| ApiError::internal("SSO 用户不存在"))?;
    if user.disabled {
        return Err(ApiError::forbidden("账号已被禁用"));
    }
    if user.role == crate::domain::models::UserRole::Guest {
        return Err(ApiError::forbidden("访客账号不能使用 SSO"));
    }
    let raw = auth_routes::new_session(&state, &user_id, &headers, &ip)?;
    let next = safe_next(Some(&next));
    let mut resp = Redirect::to(&frontend_url(&state, &next)).into_response();
    if let Ok(cookie) = HeaderValue::from_str(&state.auth.session_cookie(&raw)) {
        resp.headers_mut().append(header::SET_COOKIE, cookie);
    }
    let _ = state.db.with_sync(|conn| {
        crate::app::audit::audited(
            conn,
            Some(&user_id),
            "user.oidc_login",
            Some("user"),
            Some(&user_id),
            serde_json::json!({ "issuer": issuer }),
        )
    });
    Ok(resp)
}
