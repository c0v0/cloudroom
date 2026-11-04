//! Session extractors: resolve the session cookie into an optional or
//! required authenticated user.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use crate::api::AppState;
use crate::domain::models::{Session, User};
use crate::error::{ApiError, ApiResult};
use crate::infra::auth;

/// Resolved session + user.
#[derive(Clone)]
pub struct AuthUser {
    pub user: User,
    pub session: Session,
}

pub struct MaybeAuth(pub Option<AuthUser>);

async fn resolve_session(state: &AppState, parts: &Parts) -> ApiResult<Option<AuthUser>> {
    let raw = auth::cookie_value(&parts.headers, auth::cookie_name(&state.cfg));
    let Some(raw) = raw else { return Ok(None) };
    if raw.len() > 256 {
        return Ok(None);
    }
    let token_hash = crate::util::sha256_hex(raw.as_bytes());
    let db = state.db.clone();
    let found = tokio::task::spawn_blocking(move || {
        db.read_sync(|conn| -> ApiResult<Option<(Session, User)>> {
            let Some(session) = crate::infra::repo::get_session_by_token_hash(conn, &token_hash)?
            else {
                return Ok(None);
            };
            if session.expires_at <= chrono::Utc::now() {
                return Ok(None);
            }
            let Some(user) = crate::infra::repo::get_user(conn, &session.user_id)? else {
                return Ok(None);
            };
            if user.disabled {
                return Ok(None);
            }
            Ok(Some((session, user)))
        })
    })
    .await
    .map_err(|e| ApiError::internal(format!("身份校验失败: {e:?}")))?
    .map_err(|e| ApiError::internal(format!("身份校验失败: {e:?}")))?;

    Ok(found.map(|(session, user)| AuthUser { user, session }))
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    // Existing trait signature uses explicit impl Future; async fn would change the impl signature.
    #[allow(clippy::manual_async_fn)]
    fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            resolve_session(state, parts)
                .await?
                .ok_or_else(|| ApiError::unauthorized("未登录"))
        }
    }
}

impl FromRequestParts<AppState> for MaybeAuth {
    type Rejection = ApiError;

    // Existing trait signature uses explicit impl Future; async fn would change the impl signature.
    #[allow(clippy::manual_async_fn)]
    fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        async move { Ok(MaybeAuth(resolve_session(state, parts).await?)) }
    }
}
