//! Profile endpoints: /me, password change, avatar upload/serve, my rooms.

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{self, AppState, AuthUser};
use crate::app::revocation;
use crate::error::{ApiError, ApiResult};
use crate::util;

pub async fn me(State(state): State<AppState>, me: AuthUser) -> ApiResult<impl IntoResponse> {
    let uid = me.user.user_id.clone();
    let (_secret, totp_enabled) = state
        .db
        .with_sync(move |conn| crate::infra::repo::totp_state(conn, &uid))?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(json!({
            "user": api::user_json(&me.user, &state.cfg),
            "csrf_token": me.session.csrf_token,
            "session_expires_at": me.session.expires_at.to_rfc3339(),
            "totp_enabled": totp_enabled,
            "features": {
                "webhooks": state.cfg.security.webhooks_enabled,
            },
        })),
    ))
}

#[derive(Deserialize)]
pub struct ChangePasswordReq {
    pub current_password: String,
    pub new_password: String,
}

pub async fn change_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    me: AuthUser,
    Json(req): Json<ChangePasswordReq>,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    if !state.rl.allow(
        "password_change",
        &me.user.user_id,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.password_change_per_minute_per_user,
    ) {
        return Err(ApiError::rate_limited("密码修改尝试过于频繁"));
    }
    let Some(hash) = &me.user.password_hash else {
        return Err(ApiError::forbidden("访客账号不能修改密码"));
    };
    let verified = crate::infra::auth::verify_password_blocking(
        state.auth.clone(),
        req.current_password.clone(),
        hash.clone(),
    )
    .await?;
    if !verified {
        return Err(ApiError::forbidden("当前密码不正确"));
    }
    let pw = req.new_password.chars().count();
    if pw < state.cfg.auth.password_min_bytes || pw > state.cfg.auth.password_max_bytes {
        return Err(ApiError::validation("新密码长度不符合要求"));
    }
    if req.new_password.contains('\0') {
        return Err(ApiError::validation("密码包含 NUL 字符"));
    }
    let new_hash = crate::infra::auth::hash_password_blocking(
        state.auth.clone(),
        req.new_password.clone(),
    )
    .await?;
    state.db.with_sync(|conn| -> ApiResult<()> {
        crate::infra::repo::set_user_password(conn, &me.user.user_id, &new_hash)?;
        // Revoke all other sessions.
        conn.execute(
            "DELETE FROM sessions WHERE user_id = ?1 AND session_id != ?2",
            rusqlite::params![me.user.user_id, me.session.session_id],
        )
        .map_err(ApiError::from)?;
        api::audited(
            conn,
            Some(&me.user.user_id),
            "user.password_changed",
            Some("user"),
            Some(&me.user.user_id),
            json!({}),
        )?;
        Ok(())
    })?;
    state.hub.unregister_user(&me.user.user_id);
    // Other sessions are gone; also terminate any LiveKit participants they
    // still hold. The current session reconnects if the user wants voice again.
    revocation::schedule_user(state.clone(), me.user.user_id.clone()).await;
    Ok(Json(json!({ "ok": true })))
}

pub async fn upload_avatar(
    State(state): State<AppState>,
    headers: HeaderMap,
    me: AuthUser,
    mut multipart: axum::extract::Multipart,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    if me.user.role == crate::domain::models::UserRole::Guest {
        return Err(ApiError::forbidden("访客不能上传头像"));
    }
    if !state.rl.allow(
        "avatar",
        &me.user.user_id,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.avatar_per_minute_per_user,
    ) {
        return Err(ApiError::rate_limited("头像上传过于频繁"));
    }
    let mut data: Option<Vec<u8>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request("invalid_upload", e.to_string()))?
    {
        if field.name() == Some("avatar") {
            let bytes = field
                .bytes()
                .await
                .map_err(|e| ApiError::bad_request("invalid_upload", e.to_string()))?;
            data = Some(bytes.to_vec());
        }
    }
    let Some(bytes) = data else {
        return Err(ApiError::bad_request("invalid_upload", "缺少头像文件字段"));
    };
    let st = state.clone();
    let uid = me.user.user_id.clone();
    let old_avatar = me.user.avatar_id.clone();
    let update = tokio::task::spawn_blocking(move || {
        crate::app::avatar::update_avatar(&st, &uid, old_avatar.as_deref(), &bytes)
    })
    .await
    .map_err(|e| ApiError::internal(format!("头像处理任务失败: {e}")))??;
    // 头像变更同步到用户所在的所有房间（活跃成员关系的房间）。
    let avatar_url = format!("/api/v1/avatars/{}", update.avatar_id);
    for room_id in update.rooms {
        crate::app::events::broadcast_room_event(
            &state,
            &room_id,
            &json!({
                "type": "member_updated",
                "user_id": me.user.user_id,
                "avatar_url": avatar_url,
            }),
        );
    }
    Ok(Json(json!({ "avatar_url": avatar_url })))
}

pub async fn delete_avatar(
    State(state): State<AppState>,
    headers: HeaderMap,
    me: AuthUser,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let rooms =
        crate::app::avatar::remove_avatar(&state, &me.user.user_id, me.user.avatar_id.as_deref())?;
    // 头像移除同样同步到用户所在的所有房间。
    for room_id in rooms {
        crate::app::events::broadcast_room_event(
            &state,
            &room_id,
            &json!({
                "type": "member_updated",
                "user_id": me.user.user_id,
                "avatar_url": Value::Null,
            }),
        );
    }
    Ok(Json(json!({ "ok": true })))
}

/// Avatar serving: authenticated, nosniff, immutable caching. Avatars live
/// outside the webroot and are never served as arbitrary paths.
pub async fn get_avatar(
    State(state): State<AppState>,
    me: AuthUser,
    Path(avatar_id): Path<String>,
) -> ApiResult<Response> {
    if !state.rl.allow(
        "avatar_read",
        &me.user.user_id,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.avatar_read_per_minute_per_user,
    ) {
        return Err(ApiError::rate_limited("头像读取过于频繁"));
    }
    let path = crate::infra::avatars::avatar_path(&state.avatars_dir, &avatar_id)?;
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| ApiError::internal(format!("cannot read avatar: {e}")))?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/png"),
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable",
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    )
        .into_response())
}

pub async fn my_rooms(State(state): State<AppState>, me: AuthUser) -> ApiResult<impl IntoResponse> {
    let uid = me.user.user_id.clone();
    let rooms = state
        .db
        .read(move |conn| -> ApiResult<Vec<serde_json::Value>> {
            let sql = format!(
                "SELECT {}, m.room_role
                 FROM room_members m JOIN rooms r ON r.room_id = m.room_id
                 WHERE m.user_id = ?1 AND m.left_at IS NULL
                 ORDER BY m.joined_at DESC",
                crate::infra::repo::room_cols("r.")
            );
            let mut stmt = conn.prepare(&sql).map_err(ApiError::from)?;
            let rows = stmt
                .query_map([&uid], |row| {
                    Ok((
                        crate::infra::repo::row_to_room(row)?,
                        row.get::<_, String>("room_role")?,
                    ))
                })
                .map_err(ApiError::from)?;
            let mut out = Vec::new();
            for r in rows {
                let (room, role) = r.map_err(ApiError::from)?;
                out.push(crate::api::room_json(&room, Some(&role)));
            }
            Ok(out)
        })
        .await?;
    Ok(Json(json!({ "rooms": rooms })))
}

// ------------------------------------------------------- notification prefs

#[derive(Deserialize)]
pub struct NotificationPrefsReq {
    pub mode: String,
    pub dnd_until: Option<String>,
}

fn prefs_json(mode: &str, dnd_until: Option<&str>) -> Value {
    json!({ "mode": mode, "dnd_until": dnd_until })
}

pub async fn get_notification_prefs(
    State(state): State<AppState>,
    me: AuthUser,
) -> ApiResult<impl IntoResponse> {
    let uid = me.user.user_id.clone();
    let (mode, dnd) = state
        .db
        .read(move |conn| crate::infra::repo::notification_prefs(conn, &uid))
        .await?;
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(prefs_json(&mode, dnd.as_deref())),
    ))
}

pub async fn set_notification_prefs(
    State(state): State<AppState>,
    headers: HeaderMap,
    me: AuthUser,
    Json(req): Json<NotificationPrefsReq>,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    if !matches!(req.mode.as_str(), "all" | "mentions" | "none") {
        return Err(ApiError::validation("无效的通知模式"));
    }
    let dnd_until = match req.dnd_until.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(raw) => {
            let dt = chrono::DateTime::parse_from_rfc3339(raw)
                .map_err(|_| ApiError::validation("无效的免打扰截止时间"))?;
            Some(dt.with_timezone(&chrono::Utc).to_rfc3339())
        }
    };
    let uid = me.user.user_id.clone();
    let mode = req.mode.clone();
    let dnd = dnd_until.clone();
    state
        .db
        .with(move |conn| crate::infra::repo::set_notification_prefs(conn, &uid, &mode, dnd.as_deref()))
        .await?;
    Ok(Json(prefs_json(&req.mode, dnd_until.as_deref())))
}

pub async fn bookmarks(
    State(state): State<AppState>,
    Query(q): Query<BookmarksQuery>,
    me: AuthUser,
) -> ApiResult<impl IntoResponse> {
    let limit = q.limit.unwrap_or(30).clamp(1, 100);
    let needle = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if needle.is_some_and(|s| s.chars().count() > 100) {
        return Err(ApiError::validation("搜索词过长"));
    }
    let tag = q.tag.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if tag.is_some_and(|s| s.chars().count() > 24) {
        return Err(ApiError::validation("标签过长"));
    }
    let uid = me.user.user_id.clone();
    let needle = needle.map(str::to_string);
    let tag = tag.map(str::to_string);
    let rows = state
        .db
        .read(move |conn| {
            crate::infra::repo::list_bookmarks(
                conn,
                &uid,
                limit,
                needle.as_deref(),
                tag.as_deref(),
            )
        })
        .await?;
    let out: Vec<Value> = rows
        .iter()
        .map(|b| {
            json!({
                "message_id": b.message_id,
                "room_code": b.room_code,
                "seq": b.seq,
                "body": b.body,
                "created_at": b.created_at,
                "sender_username": b.sender_username,
                "sender_nickname": b.sender_nickname,
                "tags": b.tags,
            })
        })
        .collect();
    Ok(Json(json!({ "bookmarks": out })))
}

#[derive(Deserialize)]
pub struct BookmarksQuery {
    pub q: Option<String>,
    pub tag: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Deserialize)]
pub struct BatchBookmarksReq {
    pub message_ids: Vec<String>,
}

pub async fn delete_bookmarks(
    State(state): State<AppState>,
    headers: HeaderMap,
    me: AuthUser,
    Json(req): Json<BatchBookmarksReq>,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    if req.message_ids.is_empty() || req.message_ids.len() > 100 {
        return Err(ApiError::validation("一次最多删除 100 条收藏"));
    }
    let uid = me.user.user_id.clone();
    let ids = req.message_ids;
    let deleted = state
        .db
        .with(move |conn| crate::infra::repo::delete_bookmarks(conn, &uid, &ids))
        .await?;
    Ok(Json(json!({ "deleted": deleted })))
}

pub async fn export_bookmarks(
    State(state): State<AppState>,
    Query(q): Query<BookmarksQuery>,
    me: AuthUser,
) -> ApiResult<Response> {
    let needle = q.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if needle.is_some_and(|s| s.chars().count() > 100) {
        return Err(ApiError::validation("搜索词过长"));
    }
    let tag = q.tag.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if tag.is_some_and(|s| s.chars().count() > 24) {
        return Err(ApiError::validation("标签过长"));
    }
    let uid = me.user.user_id.clone();
    let needle = needle.map(str::to_string);
    let tag = tag.map(str::to_string);
    let rows = state
        .db
        .read(move |conn| {
            crate::infra::repo::list_bookmarks(
                conn,
                &uid,
                10_000,
                needle.as_deref(),
                tag.as_deref(),
            )
        })
        .await?;
    let bookmarks: Vec<Value> = rows
        .iter()
        .map(|b| {
            json!({
                "message_id": b.message_id,
                "room_code": b.room_code,
                "seq": b.seq,
                "body": b.body,
                "created_at": b.created_at,
                "sender_username": b.sender_username,
                "sender_nickname": b.sender_nickname,
                "tags": b.tags,
            })
        })
        .collect();
    let body = serde_json::to_vec(&json!({
        "exported_at": util::now_rfc3339(),
        "bookmarks": bookmarks,
    }))
    .map_err(|e| ApiError::internal(format!("cannot serialize bookmarks: {e}")))?;
    let filename = format!("cloudroom-bookmarks-{}.json", chrono::Utc::now().format("%Y%m%d"));
    let mut resp = Response::new(axum::body::Body::from(body));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json; charset=utf-8"),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        header::HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .unwrap_or_else(|_| header::HeaderValue::from_static("attachment")),
    );
    Ok(resp)
}

#[derive(Deserialize)]
pub struct ImportBookmarksReq {
    pub bookmarks: Vec<ImportBookmark>,
}

#[derive(Deserialize)]
pub struct ImportBookmark {
    pub message_id: String,
}

pub async fn import_bookmarks(
    State(state): State<AppState>,
    headers: HeaderMap,
    me: AuthUser,
    Json(req): Json<ImportBookmarksReq>,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    if req.bookmarks.is_empty() || req.bookmarks.len() > 1000 {
        return Err(ApiError::validation("一次最多导入 1000 条收藏"));
    }
    let mut seen = std::collections::HashSet::new();
    let ids: Vec<String> = req
        .bookmarks
        .into_iter()
        .map(|b| b.message_id)
        .filter(|id| !id.is_empty() && seen.insert(id.clone()))
        .collect();
    if ids.is_empty() {
        return Err(ApiError::validation("没有可导入的消息 ID"));
    }
    let uid = me.user.user_id.clone();
    let (imported, skipped, invalid) = state
        .db
        .with(move |conn| crate::infra::repo::import_bookmarks(conn, &uid, &ids))
        .await?;
    Ok(Json(json!({
        "imported": imported,
        "skipped": skipped,
        "invalid": invalid,
    })))
}

// ------------------------------------------------------ bookmark backups

fn bookmark_backup_dir(state: &AppState) -> std::path::PathBuf {
    state.cfg.data.dir.join("bookmark-backups")
}

#[derive(Deserialize)]
pub struct BookmarkBackupReq {
    pub enabled: bool,
}

pub async fn bookmark_backup_status(
    State(state): State<AppState>,
    me: AuthUser,
) -> ApiResult<impl IntoResponse> {
    let uid = me.user.user_id.clone();
    let (enabled, last_backup_at) = state
        .db
        .read(move |conn| crate::infra::repo::bookmark_backup_status(conn, &uid))
        .await?;
    let available = crate::app::bookmark_backup::latest_backup_path(
        &bookmark_backup_dir(&state),
        &me.user.user_id,
    )
    .is_some();
    Ok(Json(json!({
        "enabled": enabled,
        "last_backup_at": last_backup_at,
        "available": available,
    })))
}

pub async fn set_bookmark_backup(
    State(state): State<AppState>,
    headers: HeaderMap,
    me: AuthUser,
    Json(req): Json<BookmarkBackupReq>,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let uid = me.user.user_id.clone();
    state
        .db
        .with(move |conn| crate::infra::repo::set_bookmark_backup_enabled(conn, &uid, req.enabled))
        .await?;
    let mut last_backup_at = None;
    if req.enabled {
        crate::app::bookmark_backup::backup_user(
            &state.db,
            &bookmark_backup_dir(&state),
            &me.user.user_id,
        )
        .await?;
        let uid = me.user.user_id.clone();
        let (_, at) = state
            .db
            .read(move |conn| crate::infra::repo::bookmark_backup_status(conn, &uid))
            .await?;
        last_backup_at = at;
    }
    Ok(Json(json!({
        "enabled": req.enabled,
        "last_backup_at": last_backup_at,
    })))
}

pub async fn download_bookmark_backup(
    State(state): State<AppState>,
    me: AuthUser,
) -> ApiResult<Response> {
    let body = crate::app::bookmark_backup::read_latest_backup(
        &bookmark_backup_dir(&state),
        &me.user.user_id,
    )?;
    let filename = format!(
        "cloudroom-bookmarks-backup-{}.json",
        chrono::Utc::now().format("%Y%m%d")
    );
    let mut resp = Response::new(axum::body::Body::from(body));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json; charset=utf-8"),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        header::HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .unwrap_or_else(|_| header::HeaderValue::from_static("attachment")),
    );
    Ok(resp)
}

pub async fn bookmark_backup_versions(
    State(state): State<AppState>,
    me: AuthUser,
) -> ApiResult<impl IntoResponse> {
    let versions = crate::app::bookmark_backup::list_versions(
        &bookmark_backup_dir(&state),
        &me.user.user_id,
    );
    let out: Vec<Value> = versions
        .iter()
        .map(|v| json!({ "id": v.id, "created_at": v.created_at, "size": v.size }))
        .collect();
    Ok(Json(json!({ "versions": out })))
}

pub async fn download_bookmark_backup_version(
    State(state): State<AppState>,
    Path(version_id): Path<String>,
    me: AuthUser,
) -> ApiResult<Response> {
    let body = crate::app::bookmark_backup::read_version(
        &bookmark_backup_dir(&state),
        &me.user.user_id,
        &version_id,
    )?;
    let filename = format!("cloudroom-bookmarks-{version_id}");
    let mut resp = Response::new(axum::body::Body::from(body));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json; charset=utf-8"),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        header::HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .unwrap_or_else(|_| header::HeaderValue::from_static("attachment")),
    );
    Ok(resp)
}

#[derive(Deserialize)]
pub struct RestoreBookmarkBackupReq {
    pub version_id: String,
}

pub async fn restore_bookmark_backup(
    State(state): State<AppState>,
    headers: HeaderMap,
    me: AuthUser,
    Json(req): Json<RestoreBookmarkBackupReq>,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let bytes = crate::app::bookmark_backup::read_version(
        &bookmark_backup_dir(&state),
        &me.user.user_id,
        &req.version_id,
    )?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| ApiError::validation("备份文件格式无效"))?;
    let raw = value["bookmarks"]
        .as_array()
        .ok_or_else(|| ApiError::validation("备份文件缺少 bookmarks"))?;
    let mut seen = std::collections::HashSet::new();
    let ids: Vec<String> = raw
        .iter()
        .filter_map(|b| b["message_id"].as_str())
        .filter(|id| !id.is_empty() && seen.insert((*id).to_string()))
        .map(str::to_string)
        .take(10_000)
        .collect();
    let uid = me.user.user_id.clone();
    let (imported, skipped, invalid) = state
        .db
        .with(move |conn| crate::infra::repo::import_bookmarks(conn, &uid, &ids))
        .await?;
    Ok(Json(json!({
        "imported": imported,
        "skipped": skipped,
        "invalid": invalid,
    })))
}

pub async fn bookmark_tags(
    State(state): State<AppState>,
    me: AuthUser,
) -> ApiResult<impl IntoResponse> {
    let uid = me.user.user_id.clone();
    let rows = state
        .db
        .read(move |conn| crate::infra::repo::list_bookmark_tags(conn, &uid))
        .await?;
    let tags: Vec<Value> = rows
        .iter()
        .map(|(tag, count)| json!({ "tag": tag, "count": count }))
        .collect();
    Ok(Json(json!({ "tags": tags })))
}

#[derive(Deserialize)]
pub struct BookmarkTagsReq {
    pub tags: Vec<String>,
}

pub async fn set_bookmark_tags(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(message_id): Path<String>,
    me: AuthUser,
    Json(req): Json<BookmarkTagsReq>,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    if req.tags.len() > 5 {
        return Err(ApiError::validation("最多 5 个标签"));
    }
    let mut seen = std::collections::HashSet::new();
    let mut tags = Vec::new();
    for raw in req.tags {
        let tag = raw.trim();
        if tag.is_empty() {
            continue;
        }
        if tag.chars().count() > 24 || tag.chars().any(char::is_control) {
            return Err(ApiError::validation("标签无效"));
        }
        if seen.insert(tag.to_string()) {
            tags.push(tag.to_string());
        }
    }
    let uid = me.user.user_id.clone();
    let mid = message_id.clone();
    let exists = state
        .db
        .read(move |conn| crate::infra::repo::bookmark_exists(conn, &uid, &mid))
        .await?;
    if !exists {
        return Err(ApiError::not_found("请先收藏该消息"));
    }
    let uid = me.user.user_id.clone();
    let response_tags = tags.clone();
    state
        .db
        .with(move |conn| crate::infra::repo::set_bookmark_tags(conn, &uid, &message_id, &tags))
        .await?;
    Ok(Json(json!({ "tags": response_tags })))
}

fn clean_tag(raw: &str) -> ApiResult<String> {
    let tag = raw.trim();
    if tag.is_empty() || tag.chars().count() > 24 || tag.chars().any(char::is_control) {
        return Err(ApiError::validation("标签无效"));
    }
    Ok(tag.to_string())
}

#[derive(Deserialize)]
pub struct RenameTagReq {
    pub name: String,
}

pub async fn rename_bookmark_tag(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(tag): Path<String>,
    me: AuthUser,
    Json(req): Json<RenameTagReq>,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let old = clean_tag(&tag)?;
    let new = clean_tag(&req.name)?;
    let new_for_repo = new.clone();
    let uid = me.user.user_id.clone();
    let changed = state
        .db
        .with(move |conn| {
            crate::infra::repo::rename_bookmark_tag(conn, &uid, &old, &new_for_repo)
        })
        .await?;
    Ok(Json(json!({ "renamed": changed, "tag": new })))
}

pub async fn delete_bookmark_tag(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(tag): Path<String>,
    me: AuthUser,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let tag = clean_tag(&tag)?;
    let uid = me.user.user_id.clone();
    let removed = state
        .db
        .with(move |conn| crate::infra::repo::delete_bookmark_tag(conn, &uid, &tag))
        .await?;
    Ok(Json(json!({ "removed": removed })))
}

#[derive(Deserialize)]
pub struct BatchTagReq {
    pub message_ids: Vec<String>,
    pub tag: String,
    pub remove: bool,
}

pub async fn batch_bookmark_tag(
    State(state): State<AppState>,
    headers: HeaderMap,
    me: AuthUser,
    Json(req): Json<BatchTagReq>,
) -> ApiResult<impl IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    if req.message_ids.is_empty() || req.message_ids.len() > 100 {
        return Err(ApiError::validation("一次最多处理 100 条收藏"));
    }
    let tag = clean_tag(&req.tag)?;
    let mut seen = std::collections::HashSet::new();
    let ids: Vec<String> = req
        .message_ids
        .into_iter()
        .filter(|id| !id.is_empty() && seen.insert(id.clone()))
        .collect();
    let uid = me.user.user_id.clone();
    let changed = state
        .db
        .with(move |conn| {
            crate::infra::repo::tag_bookmarks(conn, &uid, &ids, &tag, req.remove)
        })
        .await?;
    Ok(Json(json!({ "changed": changed })))
}
