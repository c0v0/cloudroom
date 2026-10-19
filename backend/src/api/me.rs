//! Profile endpoints: /me, password change, avatar upload/serve, my rooms.

use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{self, AppState, AuthUser};
use crate::error::{ApiError, ApiResult};

pub async fn me(State(state): State<AppState>, me: AuthUser) -> impl IntoResponse {
    Json(json!({
        "user": api::user_json(&me.user, &state.cfg),
        "csrf_token": me.session.csrf_token,
        "session_expires_at": me.session.expires_at.to_rfc3339(),
    }))
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
    let Some(hash) = &me.user.password_hash else {
        return Err(ApiError::forbidden("访客账号不能修改密码"));
    };
    if !state.auth.verify_password(&req.current_password, hash) {
        return Err(ApiError::forbidden("当前密码不正确"));
    }
    let pw = req.new_password.len();
    if pw < state.cfg.auth.password_min_bytes || pw > state.cfg.auth.password_max_bytes {
        return Err(ApiError::validation("新密码长度不符合要求"));
    }
    if req.new_password.contains('\0') {
        return Err(ApiError::validation("密码包含 NUL 字符"));
    }
    let new_hash = state.auth.hash_password(&req.new_password)?;
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
    let (encoded, _ext) = crate::infra::avatars::process_avatar(&bytes, &state.cfg)?;
    std::fs::create_dir_all(&state.avatars_dir)
        .map_err(|e| ApiError::internal(format!("无法创建头像目录: {e}")))?;
    let avatar_id = crate::infra::avatars::store_avatar(&state.avatars_dir, &encoded)?;

    let old = me.user.avatar_id.clone();
    state.db.with_sync(|conn| -> ApiResult<()> {
        crate::infra::repo::set_user_avatar(conn, &me.user.user_id, Some(&avatar_id))?;
        api::audited(
            conn,
            Some(&me.user.user_id),
            "user.avatar_updated",
            Some("user"),
            Some(&me.user.user_id),
            json!({ "avatar_id": avatar_id }),
        )?;
        Ok(())
    })?;
    if let Some(old_id) = old {
        if old_id != avatar_id {
            crate::infra::avatars::delete_avatar(&state.avatars_dir, &old_id);
        }
    }
    // 头像变更同步到用户所在的所有房间（活跃成员关系的房间）。
    let avatar_url = format!("/api/v1/avatars/{avatar_id}");
    let rooms = state
        .db
        .with_sync(|conn| -> ApiResult<Vec<String>> {
            Ok(
                crate::infra::repo::list_member_rooms(conn, &me.user.user_id)?
                    .into_iter()
                    .filter(|r| r.state == crate::domain::models::RoomState::Active)
                    .map(|r| r.room_id)
                    .collect(),
            )
        })
        .unwrap_or_default();
    for room_id in rooms {
        crate::api::rooms::broadcast_room_event(
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
    let old = me.user.avatar_id.clone();
    state.db.with_sync(|conn| -> ApiResult<()> {
        crate::infra::repo::set_user_avatar(conn, &me.user.user_id, None)?;
        Ok(())
    })?;
    if let Some(old_id) = old {
        crate::infra::avatars::delete_avatar(&state.avatars_dir, &old_id);
    }
    // 头像移除同样同步到用户所在的所有房间。
    let rooms = state
        .db
        .with_sync(|conn| -> ApiResult<Vec<String>> {
            Ok(
                crate::infra::repo::list_member_rooms(conn, &me.user.user_id)?
                    .into_iter()
                    .filter(|r| r.state == crate::domain::models::RoomState::Active)
                    .map(|r| r.room_id)
                    .collect(),
            )
        })
        .unwrap_or_default();
    for room_id in rooms {
        crate::api::rooms::broadcast_room_event(
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
    _me: AuthUser,
    Path(avatar_id): Path<String>,
) -> ApiResult<Response> {
    let path = crate::infra::avatars::avatar_path(&state.avatars_dir, &avatar_id)?;
    let bytes =
        std::fs::read(&path).map_err(|e| ApiError::internal(format!("cannot read avatar: {e}")))?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
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
        .with(move |conn| -> ApiResult<Vec<serde_json::Value>> {
            let mut stmt = conn
                .prepare(
                    "SELECT r.room_id, r.room_code, r.kind, r.owner_user_id, r.parent_room_id, r.state,
                            r.created_at, r.expires_at, r.deleted_at, r.allow_direct_messages, r.text_enabled,
                            r.media_mode, r.audio_bitrate, r.room_muted, r.last_seq, r.version, m.room_role
                     FROM room_members m JOIN rooms r ON r.room_id = m.room_id
                     WHERE m.user_id = ?1 AND m.left_at IS NULL
                     ORDER BY m.joined_at DESC",
                )
                .map_err(ApiError::from)?;
            let rows = stmt
                .query_map([&uid], |row| {
                    Ok((
                        crate::infra::repo::row_to_room(row)?,
                        row.get::<_, String>(16)?,
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
