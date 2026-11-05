//! Room incoming webhooks: owner-managed tokens that post text messages into
//! one room without a browser session. Tokens are stored hashed and shown once.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{self, AppState, AuthUser};
use crate::app::access::active_member_room;
use crate::app::events::broadcast_room_event;
use crate::domain::models::*;
use crate::error::{ApiError, ApiResult};
use crate::util;

#[derive(Deserialize)]
pub struct CreateWebhookReq {
    pub name: String,
}

fn webhook_json(w: &RoomWebhook) -> Value {
    json!({
        "id": w.webhook_id,
        "name": w.name,
        "created_at": w.created_at.to_rfc3339(),
        "revoked_at": w.revoked_at.map(|d| d.to_rfc3339()),
        "last_used_at": w.last_used_at.map(|d| d.to_rfc3339()),
    })
}

fn validate_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 || util::has_forbidden_controls(name) {
        return Err(ApiError::validation("Webhook 名称需为 1–64 个字符"));
    }
    Ok(name.to_string())
}

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
    me: AuthUser,
    Json(req): Json<CreateWebhookReq>,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    api::require_cap(&state, &member, &room, "manage_room")?;
    if room.kind != RoomKind::Standard {
        return Err(ApiError::validation("只有普通房间支持 Webhook"));
    }
    let name = validate_name(&req.name)?;
    let token = util::random_token();
    let token_hash = util::sha256_hex(token.as_bytes());
    let webhook = RoomWebhook {
        webhook_id: util::random_id(),
        room_id: room.room_id.clone(),
        created_by: me.user.user_id.clone(),
        name,
        created_at: util::now(),
        revoked_at: None,
        last_used_at: None,
    };
    let w2 = webhook.clone();
    let hash = token_hash.clone();
    state.db.with_sync(move |conn| {
        crate::infra::repo::insert_webhook(conn, &w2, &hash)?;
        crate::app::audit::audited(
            conn,
            Some(&w2.created_by),
            "webhook.created",
            Some("room"),
            Some(&w2.room_id),
            json!({ "webhook_id": w2.webhook_id, "name": w2.name }),
        )?;
        Ok(())
    })?;
    let url = format!(
        "{}/api/v1/webhooks/{token}",
        state.cfg.server.public_origin.trim_end_matches('/')
    );
    Ok(Json(json!({
        "webhook": webhook_json(&webhook),
        // Token is returned exactly once; it is stored as a hash.
        "token": token,
        "url": url,
    })))
}

pub async fn list(
    State(state): State<AppState>,
    Path(code): Path<String>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    api::require_cap(&state, &member, &room, "manage_room")?;
    let room_id = room.room_id.clone();
    let rows = state
        .db
        .read(move |conn| crate::infra::repo::list_webhooks(conn, &room_id))
        .await?;
    Ok(Json(json!({
        "webhooks": rows.iter().map(webhook_json).collect::<Vec<_>>()
    })))
}

pub async fn revoke(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((code, webhook_id)): Path<(String, String)>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    api::require_cap(&state, &member, &room, "manage_room")?;
    let room_id = room.room_id.clone();
    let actor = me.user.user_id.clone();
    let wid = webhook_id.clone();
    let revoked = state.db.with_sync(move |conn| -> ApiResult<bool> {
        let ok = crate::infra::repo::revoke_webhook(conn, &room_id, &wid)?;
        if ok {
            crate::app::audit::audited(
                conn,
                Some(&actor),
                "webhook.revoked",
                Some("room"),
                Some(&room_id),
                json!({ "webhook_id": wid }),
            )?;
        }
        Ok(ok)
    })?;
    if !revoked {
        return Err(ApiError::not_found("Webhook 不存在或已撤销"));
    }
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct IngestReq {
    pub body: String,
}

/// Public endpoint: `POST /api/v1/webhooks/{token}` with `{"body":"..."}`.
pub async fn ingest(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Json(req): Json<IngestReq>,
) -> ApiResult<impl axum::response::IntoResponse> {
    let token_hash = util::sha256_hex(token.as_bytes());
    if !state.rl.allow(
        "webhook",
        &token_hash,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.webhook_per_minute_per_token,
    ) {
        return Err(ApiError::rate_limited("Webhook 调用过于频繁"));
    }
    let body = req.body.trim();
    if body.is_empty() || body.chars().count() > state.cfg.rooms.max_message_chars {
        return Err(ApiError::validation("消息内容为空或过长"));
    }
    if util::has_forbidden_controls(body) || util::has_bidi_controls(body) {
        return Err(ApiError::validation("消息包含控制字符"));
    }
    let hash = token_hash.clone();
    let body_owned = body.to_string();
    let result = state
        .db
        .with(move |conn| -> ApiResult<(Room, Message, RoomWebhook)> {
            let webhook = crate::infra::repo::get_webhook_by_hash(conn, &hash)?
                .ok_or_else(|| ApiError::not_found("Webhook 不存在或已撤销"))?;
            let room = crate::infra::repo::get_room_by_id(conn, &webhook.room_id)?
                .filter(|r| r.state == RoomState::Active)
                .ok_or_else(|| ApiError::not_found("房间不存在或已失效"))?;
            if !room.text_enabled {
                return Err(ApiError::conflict("房间文字频道已关闭"));
            }
            if room.kind != RoomKind::Standard {
                return Err(ApiError::validation("该房间不支持 Webhook"));
            }
            let sender = room
                .owner_user_id
                .clone()
                .ok_or_else(|| ApiError::internal("房间没有房主"))?;
            let seq = crate::infra::repo::bump_room_seq(conn, &room.room_id)?;
            let message = Message {
                message_id: util::random_id(),
                room_id: room.room_id.clone(),
                seq,
                sender_user_id: Some(sender),
                body: body_owned,
                created_at: util::now(),
                reply_snapshot: None,
                edited_at: None,
                deleted_at: None,
                attachment_id: None,
                thread_root_id: None,
                forwarded_from: None,
            };
            crate::infra::repo::insert_message(conn, &message)?;
            crate::infra::repo::touch_webhook(conn, &webhook.webhook_id)?;
            Ok((room, message, webhook))
        })
        .await?;
    let (room, message, _webhook) = result;
    let mut v = api::message_json(&message, None, None);
    v["webhook"] = json!(true);
    broadcast_room_event(
        &state,
        &room.room_id,
        &json!({ "type": "message", "message": v }),
    );
    Ok(Json(json!({ "ok": true, "message": v })))
}
