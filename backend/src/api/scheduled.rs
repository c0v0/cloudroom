//! Per-user scheduled messages: create/list/cancel; delivery is owned by the
//! sweeper task so scheduled sends obey the same permission/rate checks.

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::{self, AppState, AuthUser};
use crate::error::{ApiError, ApiResult};
use crate::util;

const MAX_PENDING: i64 = 20;
const MAX_AHEAD_DAYS: i64 = 30;

fn scheduled_json(m: &crate::infra::repo::ScheduledMessage) -> Value {
    json!({
        "id": m.schedule_id,
        "body": m.body,
        "send_at": m.send_at.to_rfc3339(),
        "status": m.status,
        "created_at": m.created_at.to_rfc3339(),
        "sent_message_id": m.sent_message_id,
        "error": m.error,
    })
}

#[derive(Deserialize)]
pub struct ScheduleReq {
    pub body: String,
    pub send_at: String,
}

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
    me: AuthUser,
    Json(req): Json<ScheduleReq>,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    if !state.rl.allow(
        "schedule",
        &me.user.user_id,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.schedule_per_minute_per_user,
    ) {
        return Err(ApiError::rate_limited("定时消息创建过于频繁"));
    }
    let body = req.body.trim();
    if body.is_empty() || body.chars().count() > state.cfg.rooms.max_message_chars {
        return Err(ApiError::validation("消息内容为空或过长"));
    }
    if util::has_forbidden_controls(body) || util::has_bidi_controls(body) {
        return Err(ApiError::validation("消息包含控制字符"));
    }
    let send_at = chrono::DateTime::parse_from_rfc3339(req.send_at.trim())
        .map_err(|_| ApiError::validation("send_at 必须是 RFC3339 时间"))?
        .with_timezone(&chrono::Utc);
    let now = util::now();
    if send_at <= now {
        return Err(ApiError::validation("send_at 必须晚于当前时间"));
    }
    if send_at > now + chrono::Duration::days(MAX_AHEAD_DAYS) {
        return Err(ApiError::validation("定时消息最多提前 30 天"));
    }
    let (room, _member) =
        crate::app::access::active_member_room(&state, &code, &me.user.user_id).await?;
    if !room.text_enabled {
        return Err(ApiError::conflict("文字频道已关闭"));
    }
    let room_id = room.room_id.clone();
    let uid = me.user.user_id.clone();
    let body_owned = body.to_string();
    let scheduled = crate::infra::repo::ScheduledMessage {
        schedule_id: util::random_id(),
        room_id: room_id.clone(),
        user_id: uid.clone(),
        body: body_owned,
        thread_root_id: None,
        send_at,
        status: "pending".into(),
        created_at: now,
        sent_message_id: None,
        error: None,
    };
    let s2 = crate::infra::repo::ScheduledMessage {
        schedule_id: scheduled.schedule_id.clone(),
        room_id: room_id.clone(),
        user_id: uid.clone(),
        body: scheduled.body.clone(),
        thread_root_id: scheduled.thread_root_id.clone(),
        send_at,
        status: "pending".into(),
        created_at: now,
        sent_message_id: None,
        error: None,
    };
    state.db.with_sync(move |conn| {
        if crate::infra::repo::scheduled_pending_count(conn, &room_id, &uid)? >= MAX_PENDING {
            return Err(ApiError::conflict("待发送的定时消息已达上限"));
        }
        crate::infra::repo::insert_scheduled(conn, &s2)
    })?;
    Ok(Json(json!({ "scheduled": scheduled_json(&scheduled) })))
}

pub async fn list(
    State(state): State<AppState>,
    Path(code): Path<String>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    let (room, _member) =
        crate::app::access::active_member_room(&state, &code, &me.user.user_id).await?;
    let room_id = room.room_id.clone();
    let uid = me.user.user_id.clone();
    let rows = state
        .db
        .read(move |conn| crate::infra::repo::list_scheduled(conn, &room_id, &uid))
        .await?;
    Ok(Json(json!({
        "scheduled": rows.iter().map(scheduled_json).collect::<Vec<_>>()
    })))
}

pub async fn cancel(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((code, schedule_id)): Path<(String, String)>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    api::require_csrf(&state, &headers, &me.session)?;
    let (room, _member) =
        crate::app::access::active_member_room(&state, &code, &me.user.user_id).await?;
    let room_id = room.room_id.clone();
    let uid = me.user.user_id.clone();
    let ok = state.db.with_sync(move |conn| {
        crate::infra::repo::cancel_scheduled(conn, &room_id, &uid, &schedule_id)
    })?;
    if !ok {
        return Err(ApiError::not_found("定时消息不存在或已发送"));
    }
    Ok(Json(json!({ "ok": true })))
}
