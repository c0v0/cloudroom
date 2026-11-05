//! Authenticated link-preview endpoint with a small server-side cache.

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::api::{AppState, AuthUser};
use crate::error::{ApiError, ApiResult};
use crate::infra::link_preview;
use crate::infra::repo::LinkPreviewRow;

#[derive(Deserialize)]
pub struct PreviewQuery {
    pub url: String,
}

pub async fn get(
    State(state): State<AppState>,
    Query(q): Query<PreviewQuery>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    if !state.rl.allow(
        "link_preview",
        &me.user.user_id,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.link_preview_per_minute_per_user,
    ) {
        return Err(ApiError::rate_limited("链接预览请求过于频繁"));
    }
    let raw = q.url.trim();
    if raw.is_empty() || raw.len() > 2048 {
        return Err(ApiError::validation("URL 不合法"));
    }
    let hash = crate::util::sha256_hex(raw.as_bytes());
    let hash_for_read = hash.clone();
    let cached = state
        .db
        .read(move |conn| crate::infra::repo::get_link_preview(conn, &hash_for_read))
        .await?;
    if let Some(row) = cached {
        let ttl = if row.ok { 24 * 3600 } else { 3600 };
        if (crate::util::now() - row.fetched_at).num_seconds() < ttl {
            if row.ok {
                return Ok(Json(json!({ "preview": preview_json(&row) })));
            }
            return Err(ApiError::validation("无法生成链接预览"));
        }
    }
    let fetched = link_preview::fetch(raw).await;
    let now = crate::util::now();
    match fetched {
        Ok(p) => {
            let row = LinkPreviewRow {
                url: p.url,
                title: p.title,
                description: p.description,
                site_name: p.site_name,
                ok: true,
                error: None,
                fetched_at: now,
            };
            let h = hash.clone();
            let r2 = row.clone();
            state
                .db
                .with_sync(move |conn| crate::infra::repo::upsert_link_preview(conn, &h, &r2))?;
            Ok(Json(json!({ "preview": preview_json(&row) })))
        }
        Err(e) => {
            tracing::info!(error = %e, url = %raw, "link preview failed");
            let row = LinkPreviewRow {
                url: raw.to_string(),
                title: String::new(),
                description: String::new(),
                site_name: String::new(),
                ok: false,
                error: Some(e),
                fetched_at: now,
            };
            let h = hash.clone();
            let r2 = row.clone();
            let _ = state
                .db
                .with_sync(move |conn| crate::infra::repo::upsert_link_preview(conn, &h, &r2));
            Err(ApiError::validation("无法生成链接预览"))
        }
    }
}

fn preview_json(row: &LinkPreviewRow) -> serde_json::Value {
    json!({
        "url": row.url,
        "title": row.title,
        "description": row.description,
        "site_name": row.site_name,
    })
}
