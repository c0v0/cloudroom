//! Global inbox: per-room unread/mention counts and recent mentions.

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::api::{AppState, AuthUser};
use crate::error::{ApiError, ApiResult};

#[derive(Deserialize)]
pub struct InboxQuery {
    pub limit: Option<i64>,
}

pub async fn inbox(
    State(state): State<AppState>,
    Query(q): Query<InboxQuery>,
    me: AuthUser,
) -> ApiResult<impl axum::response::IntoResponse> {
    if !state.rl.allow(
        "inbox",
        &me.user.user_id,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.search_per_minute_per_user,
    ) {
        return Err(ApiError::rate_limited("收件箱刷新过于频繁"));
    }
    let limit = q.limit.unwrap_or(20).clamp(1, 50);
    let uid = me.user.user_id.clone();
    let (rooms, mentions) = state
        .db
        .read(move |conn| {
            let rooms = crate::infra::repo::inbox_rooms(conn, &uid)?;
            let mentions = crate::infra::repo::inbox_mentions(conn, &uid, limit)?;
            Ok((rooms, mentions))
        })
        .await?;
    let total_unread: i64 = rooms.iter().map(|r| r.unread).sum();
    let total_mentions: i64 = rooms.iter().map(|r| r.mentions).sum();
    let room_json: Vec<_> = rooms
        .iter()
        .map(|r| {
            json!({
                "room_code": r.room_code,
                "kind": r.kind,
                "unread": r.unread,
                "mentions": r.mentions,
            })
        })
        .collect();
    let mention_json: Vec<_> = mentions
        .iter()
        .map(|m| {
            json!({
                "message_id": m.message_id,
                "room_code": m.room_code,
                "body": m.body,
                "created_at": m.created_at,
                "sender_nickname": m.sender_nickname,
                "unread": m.unread,
            })
        })
        .collect();
    Ok(Json(json!({
        "total_unread": total_unread,
        "total_mentions": total_mentions,
        "rooms": room_json,
        "mentions": mention_json,
    })))
}
