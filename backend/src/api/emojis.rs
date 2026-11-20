//! Custom emoji registry: admins upload small images; messages render `:name:`.

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use crate::api::{self, AppState, AuthUser};
use crate::domain::models::CustomEmoji;
use crate::error::{ApiError, ApiResult};
use crate::util;

const MAX_EMOJI_BYTES: usize = 256 * 1024;
const MAX_EMOJI_DIM: u32 = 128;

fn emoji_json(e: &CustomEmoji) -> Value {
    json!({
        "id": e.emoji_id,
        "name": e.name,
        "url": format!("/api/v1/emojis/{}/image", e.emoji_id),
    })
}

fn validate_name(raw: &str) -> ApiResult<String> {
    let name = raw.trim().to_ascii_lowercase();
    if name.len() < 2
        || name.len() > 32
        || !name.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'+' | b'-')
        })
    {
        return Err(ApiError::validation(
            "emoji 名称需为 2–32 位小写字母/数字/_-+",
        ));
    }
    Ok(name)
}

pub async fn list(State(state): State<AppState>, _me: AuthUser) -> ApiResult<impl IntoResponse> {
    let rows = state.db.read(crate::infra::repo::list_emojis).await?;
    Ok(Json(json!({
        "emojis": rows.iter().map(emoji_json).collect::<Vec<_>>()
    })))
}

pub async fn image(
    State(state): State<AppState>,
    Path(emoji_id): Path<String>,
    _me: AuthUser,
) -> ApiResult<Response> {
    let id = emoji_id.clone();
    let emoji = state
        .db
        .read(move |conn| crate::infra::repo::get_emoji(conn, &id))
        .await?
        .ok_or_else(|| ApiError::not_found("emoji 不存在"))?;
    let path = state.cfg.data.dir.join("emojis").join(&emoji.emoji_id);
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| ApiError::not_found("emoji 文件不存在"))?;
    let mut resp =
        axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(file)).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&emoji.mime)
            .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
    );
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400"),
    );
    resp.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(resp)
}

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    me: AuthUser,
    mut multipart: axum::extract::Multipart,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    if !me.user.role.is_admin() {
        return Err(ApiError::forbidden("需要管理员权限"));
    }
    let mut name = String::new();
    let mut bytes: Option<Vec<u8>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request("invalid_upload", e.to_string()))?
    {
        match field.name() {
            Some("name") => {
                name = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request("invalid_upload", e.to_string()))?;
            }
            Some("file") => {
                let b = field
                    .bytes()
                    .await
                    .map_err(|e| ApiError::bad_request("invalid_upload", e.to_string()))?;
                bytes = Some(b.to_vec());
            }
            _ => {}
        }
    }
    let name = validate_name(&name)?;
    let bytes = bytes.ok_or_else(|| ApiError::bad_request("invalid_upload", "缺少 file"))?;
    if bytes.len() > MAX_EMOJI_BYTES {
        return Err(ApiError::bad_request(
            "emoji_too_large",
            "emoji 不能超过 256 KiB",
        ));
    }
    let (mime, _ext) = crate::api::rooms::sniff_attachment(&bytes)
        .filter(|(m, _)| m.starts_with("image/"))
        .ok_or_else(|| ApiError::bad_request("emoji_unsupported", "仅接受 PNG/JPEG/WebP/GIF"))?;
    let dims = image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|_| ApiError::bad_request("emoji_corrupt", "图片无法读取"))?
        .into_dimensions()
        .map_err(|_| ApiError::bad_request("emoji_corrupt", "图片无法读取"))?;
    if dims.0 > MAX_EMOJI_DIM || dims.1 > MAX_EMOJI_DIM {
        return Err(ApiError::bad_request(
            "emoji_too_large",
            format!("emoji 尺寸不能超过 {MAX_EMOJI_DIM}x{MAX_EMOJI_DIM}"),
        ));
    }
    let emoji = CustomEmoji {
        emoji_id: util::random_id(),
        name,
        mime: mime.to_string(),
        size: bytes.len() as i64,
        created_by: me.user.user_id.clone(),
        created_at: util::now(),
        disabled: false,
    };
    let dir = state.cfg.data.dir.join("emojis");
    let path = dir.join(&emoji.emoji_id);
    let bytes2 = bytes.clone();
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, bytes2)
    })
    .await
    .map_err(|e| ApiError::internal(format!("emoji 写入任务失败: {e}")))?
    .map_err(|e| ApiError::internal(format!("emoji 写入失败: {e}")))?;
    let e2 = emoji.clone();
    let actor = me.user.user_id.clone();
    if let Err(e) = state.db.with_sync(move |conn| {
        crate::infra::repo::insert_emoji(conn, &e2)?;
        crate::app::audit::audited(
            conn,
            Some(&actor),
            "emoji.created",
            Some("emoji"),
            Some(&e2.emoji_id),
            json!({ "name": e2.name }),
        )?;
        Ok(())
    }) {
        let _ = std::fs::remove_file(state.cfg.data.dir.join("emojis").join(&emoji.emoji_id));
        return Err(e);
    }
    Ok(Json(json!({ "emoji": emoji_json(&emoji) })))
}

pub async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(emoji_id): Path<String>,
    me: AuthUser,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    if !me.user.role.is_admin() {
        return Err(ApiError::forbidden("需要管理员权限"));
    }
    if emoji_id.len() != 32 || !emoji_id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ApiError::not_found("emoji 不存在"));
    }
    let id = emoji_id.clone();
    let actor = me.user.user_id.clone();
    let ok = state.db.with_sync(move |conn| -> ApiResult<bool> {
        let ok = crate::infra::repo::delete_emoji(conn, &id)?;
        if ok {
            crate::app::audit::audited(
                conn,
                Some(&actor),
                "emoji.deleted",
                Some("emoji"),
                Some(&id),
                json!({}),
            )?;
        }
        Ok(ok)
    })?;
    if !ok {
        return Err(ApiError::not_found("emoji 不存在"));
    }
    let _ = std::fs::remove_file(state.cfg.data.dir.join("emojis").join(&emoji_id));
    Ok(Json(json!({ "ok": true })))
}
