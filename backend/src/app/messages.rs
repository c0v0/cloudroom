//! Message sending use case: validation, persistence, sequence assignment,
//! reply snapshots, and mute/permission checks.

use serde_json::json;

use crate::app::access::active_member_room;
use crate::app::state::AppState;
use crate::domain::models::*;
use crate::error::{ApiError, ApiResult};
use crate::util;

pub(crate) struct SentMessage {
    pub room_id: String,
    pub message: Message,
    pub sender: Option<User>,
    pub nickname: Option<String>,
}

pub(crate) async fn send(
    state: &AppState,
    code: &str,
    user_id: &str,
    body: &str,
    reply_to: Option<&str>,
    attachment_id: Option<&str>,
) -> ApiResult<SentMessage> {
    let body = body.trim();
    if body.is_empty() && attachment_id.is_none() {
        return Err(ApiError::validation("消息内容为空"));
    }
    if body.chars().count() > state.cfg.rooms.max_message_chars {
        return Err(ApiError::validation("消息过长"));
    }
    if util::has_forbidden_controls(body) || util::has_bidi_controls(body) {
        return Err(ApiError::validation("消息包含控制字符"));
    }
    if !state.rl.allow(
        "message",
        user_id,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.message_per_minute_per_user,
    ) {
        return Err(ApiError::rate_limited("消息发送过于频繁"));
    }
    let (room, _member) = active_member_room(state, code, user_id).await?;
    if !room.text_enabled {
        return Err(ApiError::new(
            axum::http::StatusCode::CONFLICT,
            "text_disabled",
            "文字频道已关闭",
        ));
    }
    if room.room_muted {
        let granted = state
            .db
            .with_sync(|conn| crate::infra::repo::has_speak_grant(conn, &room.room_id, user_id))
            .unwrap_or(false);
        if !granted {
            return Err(ApiError::new(
                axum::http::StatusCode::FORBIDDEN,
                "room_muted",
                "房间已开启全员禁言，仅获发言权用户可发言",
            ));
        }
    }
    let now = util::now();
    let uid = user_id.to_string();
    let room_id = room.room_id.clone();
    let body2 = body.to_string();
    let reply_to_owned = reply_to.map(|s| s.to_string());
    let attachment_id_owned = attachment_id.map(|s| s.to_string());
    let sent = state
        .db
        .with(
            move |conn| -> ApiResult<(Message, Option<User>, Option<String>)> {
                let reply_snapshot = match reply_to_owned.as_deref() {
                    Some(rid) => {
                        let orig = crate::infra::repo::get_message(conn, rid)?
                            .filter(|m| m.room_id == room_id);
                        match orig {
                            Some(m) => {
                                let sender = m
                                    .sender_user_id
                                    .as_deref()
                                    .and_then(|s| {
                                        crate::infra::repo::get_member(conn, &room_id, s)
                                            .ok()
                                            .flatten()
                                    })
                                    .map(|x| x.nickname)
                                    .unwrap_or_else(|| "成员".into());
                                let preview: String = m.body.chars().take(80).collect();
                                Some(
                                    serde_json::to_string(&json!({
                                        "id": m.message_id,
                                        "sender": sender,
                                        "uid": m.sender_user_id,
                                        "body": preview,
                                    }))
                                    .unwrap_or_default(),
                                )
                            }
                            None => None,
                        }
                    }
                    None => None,
                };
                if let Some(aid) = attachment_id_owned.as_deref() {
                    let a = crate::infra::repo::get_attachment(conn, aid)?
                        .ok_or_else(|| ApiError::validation("附件不存在"))?;
                    if a.room_id != room_id {
                        return Err(ApiError::validation("附件不属于该房间"));
                    }
                    if a.uploader_id != uid {
                        return Err(ApiError::forbidden("只能发送自己上传的附件"));
                    }
                }
                let member_row = crate::infra::repo::get_member(conn, &room_id, &uid)?
                    .ok_or_else(|| ApiError::conflict("成员关系已变更，请重试"))?;
                if member_row.room_role != RoomRole::Owner
                    && crate::infra::repo::member_is_muted(conn, &room_id, &uid, &now)?
                {
                    return Err(ApiError::new(
                        axum::http::StatusCode::FORBIDDEN,
                        "muted",
                        "您已被禁言",
                    ));
                }
                let seq = crate::infra::repo::bump_room_seq(conn, &room_id)?;
                let message = Message {
                    message_id: util::random_id(),
                    room_id: room_id.clone(),
                    seq,
                    sender_user_id: Some(uid.clone()),
                    body: body2.clone(),
                    created_at: now,
                    reply_snapshot,
                    edited_at: None,
                    deleted_at: None,
                    attachment_id: attachment_id_owned.clone(),
                };
                crate::infra::repo::insert_message(conn, &message)?;
                // 同一事务内取回发送者与昵称，省掉一次数据库往返。
                let sender = match &message.sender_user_id {
                    Some(uid) => crate::infra::repo::get_user(conn, uid)?,
                    None => None,
                };
                let nick = match &message.sender_user_id {
                    Some(uid) => {
                        crate::infra::repo::get_member(conn, &room_id, uid)?.map(|x| x.nickname)
                    }
                    None => None,
                };
                Ok((message, sender, nick))
            },
        )
        .await?;
    let (message, sender, nickname) = sent;
    // Web Push：被 @ 或被回复的成员（排除发送者）。异步发送，不阻塞消息返回。
    if state.cfg.push.enabled() {
        let mut targets: Vec<String> = Vec::new();
        let self_id = user_id.to_string();
        let rid = reply_to.map(|s| s.to_string());
        let body_for_mentions = body.to_string();
        let room_for_mentions = room.room_id.clone();
        // 回复目标与成员列表一次取回，少一次数据库往返。
        let (reply_uid, members) = state
            .db
            .with(
                move |conn| -> ApiResult<(Option<String>, Vec<RoomMember>)> {
                    let reply_uid = match rid.as_deref() {
                        Some(r) => crate::infra::repo::get_message(conn, r)
                            .ok()
                            .flatten()
                            .and_then(|m| m.sender_user_id),
                        None => None,
                    };
                    let members = crate::infra::repo::list_members(conn, &room_for_mentions)?;
                    Ok((reply_uid, members))
                },
            )
            .await
            .unwrap_or((None, Vec::new()));
        if let Some(uid) = reply_uid {
            if uid != self_id {
                targets.push(uid);
            }
        }
        for m in members {
            if m.user_id == self_id {
                continue;
            }
            if body_for_mentions.contains(&format!("@{}", m.nickname)) {
                targets.push(m.user_id);
            }
        }
        targets.sort();
        targets.dedup();
        if !targets.is_empty() {
            let db = state.db.clone();
            let cfg = state.cfg.push.clone();
            let code = room.room_code.clone();
            let text = body.to_string();
            let from = nickname.clone().unwrap_or_default();
            tokio::spawn(async move {
                crate::infra::push::notify_users(
                    &db,
                    &cfg,
                    targets,
                    serde_json::json!({
                        "title": format!("房间 /{code}"),
                        "body": format!("{from}：{}", text.chars().take(60).collect::<String>()),
                        "url": format!("/{code}"),
                        "tag": format!("room-{code}"),
                    }),
                )
                .await;
            });
        }
    }

    Ok(SentMessage {
        room_id: room.room_id,
        message,
        sender,
        nickname,
    })
}
