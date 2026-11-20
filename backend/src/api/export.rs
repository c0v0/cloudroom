//! Room chat export (owner or admin): JSON with members and live messages.

use axum::extract::{Path, State};
use axum::http::{header, HeaderValue};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::api::{self, AppState, AuthUser};
use crate::app::access::active_member_room;
use crate::domain::models::RoomRole;
use crate::error::{ApiError, ApiResult};

const MAX_EXPORT_MESSAGES: i64 = 20_000;

pub async fn export_room(
    State(state): State<AppState>,
    Path(code): Path<String>,
    me: AuthUser,
) -> ApiResult<Response> {
    let (room, member) = active_member_room(&state, &code, &me.user.user_id).await?;
    if member.room_role != RoomRole::Owner && !me.user.role.is_admin() {
        return Err(ApiError::forbidden("只有房主或管理员可以导出"));
    }
    let room_id = room.room_id.clone();
    let code2 = code.clone();
    let role = member.room_role.as_str().to_string();
    let payload = state
        .db
        .read(move |conn| -> ApiResult<serde_json::Value> {
            let count = crate::infra::repo::message_count(conn, &room_id)?;
            if count > MAX_EXPORT_MESSAGES {
                return Err(ApiError::new(
                    axum::http::StatusCode::PAYLOAD_TOO_LARGE,
                    "export_too_large",
                    format!("消息超过 {MAX_EXPORT_MESSAGES} 条，暂不支持一次导出"),
                ));
            }
            let msgs = crate::infra::repo::all_messages(conn, &room_id)?;
            let members = crate::infra::repo::list_members(conn, &room_id)?;
            let uids: Vec<String> = members.iter().map(|m| m.user_id.clone()).collect();
            let users = crate::infra::repo::users_by_ids(conn, &uids)?;
            let senders: Vec<String> = msgs
                .iter()
                .filter_map(|m| m.sender_user_id.clone())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            let senders_users = crate::infra::repo::users_by_ids(conn, &senders)?;
            let nicks = crate::infra::repo::member_nicknames(conn, &room_id, &senders)?;
            let att_ids: Vec<String> = msgs
                .iter()
                .filter_map(|m| m.attachment_id.clone())
                .collect();
            let atts = crate::infra::repo::attachments_by_ids(conn, &att_ids)?;
            let member_json: Vec<_> = members
                .iter()
                .map(|m| api::member_json(m, users.get(&m.user_id)))
                .collect();
            let message_json: Vec<_> = msgs
                .iter()
                .map(|m| {
                    let sender = m.sender_user_id.as_ref().and_then(|u| senders_users.get(u));
                    let nick = m
                        .sender_user_id
                        .as_ref()
                        .and_then(|u| nicks.get(u))
                        .map(|s| s.as_str());
                    let mut v = api::message_json(m, sender, nick);
                    if let Some(aid) = m.attachment_id.as_deref() {
                        if let Some(a) = atts.get(aid) {
                            v["attachment"] = api::attachment_json(a, &code2);
                        }
                    }
                    v
                })
                .collect();
            Ok(json!({
                "exported_at": crate::util::now_rfc3339(),
                "room": api::room_json(&room_for(&room_id, conn)?, Some(role.as_str())),
                "members": member_json,
                "messages": message_json,
            }))
        })
        .await?;
    let body = serde_json::to_vec_pretty(&payload)
        .map_err(|e| ApiError::internal(format!("导出序列化失败: {e}")))?;
    let mut resp = body.into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"cloudroom-{code}.json\""))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

fn room_for(
    room_id: &str,
    conn: &mut rusqlite::Connection,
) -> ApiResult<crate::domain::models::Room> {
    crate::infra::repo::get_room_by_id(conn, room_id)?
        .ok_or_else(|| ApiError::internal("房间不存在"))
}
