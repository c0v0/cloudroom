//! WebSocket endpoint: `/api/v1/rooms/{room_code}/ws`.
//! Events are JSON; the server enforces room membership, text state, mute
//! state, voice limits and relays WebRTC signaling between members.

use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, watch};

use crate::api::{self, AppState, AuthUser};
use crate::domain::models::{MediaMode, RoomState};
use crate::error::ApiResult;
use crate::infra::hub::ServerMessage;
use crate::util;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(code): Path<String>,
    headers: axum::http::HeaderMap,
    me: AuthUser,
) -> axum::response::Response {
    if let Err(e) = api::check_ws_origin(&state, &headers) {
        return e.into_response();
    }
    if !state.rl.allow(
        "ws_upgrade",
        &me.user.user_id,
        std::time::Duration::from_secs(60),
        state.cfg.rate_limit.ws_upgrade_per_minute_per_user,
    ) {
        return crate::error::ApiError::rate_limited("WebSocket 连接过于频繁").into_response();
    }
    let max_message = state.cfg.security.ws_max_message_bytes.max(1024);
    ws.max_message_size(max_message)
        .max_frame_size(max_message)
        .on_upgrade(move |socket| handle_socket(state, socket, code, me))
}

pub async fn handle_socket(state: AppState, mut socket: WebSocket, code: String, me: AuthUser) {
    // Membership snapshot (authoritative).
    let snapshot = state.db.read_sync(
        |conn| -> ApiResult<(
            crate::domain::models::Room,
            crate::domain::models::RoomMember,
        )> {
            let room = crate::infra::repo::get_room_by_code(conn, &code)?
                .ok_or_else(crate::error::ApiError::room_not_found)?;
            if room.state != RoomState::Active {
                return Err(crate::error::ApiError::room_unavailable());
            }
            let member = crate::infra::repo::get_member(conn, &room.room_id, &me.user.user_id)?
                .ok_or_else(|| crate::error::ApiError::forbidden("不是房间成员"))?;
            if member.left_at.is_some() {
                return Err(crate::error::ApiError::forbidden("不是房间成员"));
            }
            Ok((room, member))
        },
    );
    let Ok((room, _member)) = snapshot else {
        drop(socket);
        return;
    };
    state.metrics.ws_connected();

    let (tx, mut rx) = mpsc::channel::<ServerMessage>(state.cfg.security.ws_queue_messages.max(1));
    let (close_tx, close_rx) = watch::channel(false);
    let room_id = room.room_id.clone();
    let uid = me.user.user_id.clone();
    if !state.hub.register(&room_id, &uid, tx.clone(), close_tx) {
        tracing::warn!(
            room_id,
            uid,
            "ws connection limit reached; closing new connection"
        );
        // 明确告知客户端（1008 policy violation），否则只能看到异常断开并立刻重连。
        let _ = socket
            .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                code: 1008,
                reason: "连接数已达上限".into(),
            })))
            .await;
        drop(socket);
        return;
    }

    // Initial snapshot.
    let hello = build_hello(&state, &room).await;
    let _ = socket.send(Message::Text(hello.into())).await;

    let (mut sink, mut stream) = socket.split();
    let mut last_db_write = Instant::now() - Duration::from_secs(3600);
    // 社交事件的每类型最小间隔，避免高频 typing/表情刷屏（全局每秒配额之外）。
    let mut last_social = [Instant::now() - Duration::from_secs(60); 3];
    let mut voice_member = false;

    let hub = state.hub.clone();
    let db = state.db.clone();
    let uid2 = uid.clone();
    let room_id2 = room_id.clone();

    // Presence must advance while a socket is connected, even when the client
    // is idle. The interval owns its DB clone and is aborted at teardown.
    let presence_db = db.clone();
    let presence_uid = uid.clone();
    let presence_room = room_id.clone();
    let heartbeat = state.cfg.heartbeat_interval();
    let presence_task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(heartbeat);
        loop {
            ticker.tick().await;
            let now = util::now();
            let _ = presence_db.with_sync(|conn| -> ApiResult<()> {
                crate::infra::repo::touch_user_online(conn, &presence_uid, &now)?;
                crate::infra::repo::touch_member_online(conn, &presence_room, &presence_uid, &now)?;
                Ok(())
            });
        }
    });

    // Writer task. The queue is bounded; Hub notifies `close` on overflow or
    // replacement, and the writer sends a close frame instead of growing RSS.
    let mut writer_close = close_rx.clone();
    let writer = tokio::spawn(async move {
        loop {
            tokio::select! {
                maybe = rx.recv() => {
                    match maybe {
                        Some(ServerMessage::Text(text)) => {
                            if sink.send(Message::Text(text.as_ref().into())).await.is_err() {
                                break;
                            }
                        }
                        Some(ServerMessage::Close) | None => {
                            let _ = sink.send(Message::Close(None)).await;
                            break;
                        }
                    }
                }
                _ = writer_close.changed() => {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
            }
        }
    });
    let mut event_rate = EventRate::new(state.cfg.security.ws_events_per_second);
    let mut read_close = close_rx;
    let session_remaining = me
        .session
        .expires_at
        .signed_duration_since(chrono::Utc::now())
        .to_std()
        .unwrap_or_default();
    let session_timer = tokio::time::sleep(session_remaining);
    tokio::pin!(session_timer);

    loop {
        let next = tokio::select! {
            _ = read_close.changed() => break,
            _ = &mut session_timer => {
                tracing::info!(room_id, uid, "ws session expired; closing connection");
                break;
            }
            next = stream.next() => next,
        };
        let Some(Ok(msg)) = next else { break };
        match msg {
            Message::Text(text) => {
                if !state.hub.is_current(&room_id, &uid, &tx) {
                    break;
                }
                if !event_rate.allow() {
                    tracing::warn!(
                        room_id,
                        uid,
                        "ws event rate limit exceeded; closing connection"
                    );
                    break;
                }
                let Ok(ev) = serde_json::from_str::<ClientEvent>(&text) else {
                    let _ = state.hub.send_to(
                        &room_id,
                        &uid,
                        r#"{"type":"error","code":"invalid_event","message":"invalid event"}"#,
                    );
                    continue;
                };
                match ev {
                    ClientEvent::Ping => {
                        let _ = state.hub.send_to(&room_id, &uid, r#"{"type":"pong"}"#);
                    }
                    ClientEvent::SendMessage {
                        body,
                        reply_to,
                        thread_root_id,
                        attachment_id,
                    } => {
                        match crate::api::rooms::send_message_inner(
                            &state,
                            &code,
                            &uid,
                            &body,
                            reply_to.as_deref(),
                            thread_root_id.as_deref(),
                            attachment_id.as_deref(),
                            None,
                        )
                        .await
                        {
                            Ok(_) => {}
                            Err(e) => {
                                let _ = state.hub.send_to(
                                    &room_id,
                                    &uid,
                                    &serde_json::to_string(&json!({
                                        "type": "error",
                                        "code": e.code,
                                        "message": e.message,
                                    }))
                                    .unwrap_or_default(),
                                );
                            }
                        }
                    }
                    ClientEvent::VoiceJoin => {
                        // 语音在场/信令只属于 p2p 房间；sfu 房间走 LiveKit。
                        if room.media_mode != MediaMode::P2p {
                            let _ = state.hub.send_to(
                                &room_id,
                                &uid,
                                r#"{"type":"error","code":"media_mode_mismatch","message":"媒体接口与房间媒体模式不匹配"}"#,
                            );
                            continue;
                        }
                        let allowed = check_voice(&state, &room, &uid).await;
                        match allowed {
                            Ok(_) => {
                                let joined = state.hub.voice_join(&room_id, &uid);
                                tracing::info!(uid, joined, "ws voice_join result");
                                // 幂等：重连/重复加入也广播，恢复客户端语音状态
                                voice_member = true;
                                broadcast_voice(&state, &room_id);
                            }
                            Err(e) => {
                                let _ = state.hub.send_to(
                                    &room_id,
                                    &uid,
                                    &serde_json::to_string(&json!({
                                        "type": "error",
                                        "code": e.code,
                                        "message": e.message,
                                    }))
                                    .unwrap_or_default(),
                                );
                            }
                        }
                    }
                    ClientEvent::VoiceLeave => {
                        if room.media_mode != MediaMode::P2p {
                            continue;
                        }
                        if state.hub.voice_leave(&room_id, &uid) {
                            voice_member = false;
                            broadcast_voice(&state, &room_id);
                        }
                    }
                    ClientEvent::Typing => {
                        if last_social[0].elapsed() < Duration::from_millis(1500) {
                            continue;
                        }
                        last_social[0] = Instant::now();
                        // 只广播，不落库；客户端 4 秒后自动清除。
                        let nickname = state
                            .db
                            .with_sync(|conn| {
                                crate::infra::repo::get_member(conn, &room_id, &uid)
                                    .map(|m| m.map(|m| m.nickname))
                            })
                            .ok()
                            .flatten()
                            .unwrap_or_default();
                        state.hub.broadcast(
                            &room_id,
                            &serde_json::to_string(&json!({
                                "type": "typing",
                                "user_id": uid,
                                "nickname": nickname,
                                "ttl_ms": 4000,
                            }))
                            .unwrap_or_default(),
                        );
                    }
                    ClientEvent::Read { seq } => {
                        // 用数据库里的最新 last_seq 钳制；连接时的快照会漏掉
                        // 本次连接期间新到的消息，导致长连接永远读不到新消息。
                        let capped = state
                            .db
                            .with_sync(|conn| -> ApiResult<i64> {
                                let last = crate::infra::repo::room_last_seq(conn, &room_id)?;
                                let capped = seq.clamp(0, last);
                                crate::infra::repo::set_member_read_seq(
                                    conn, &room_id, &uid, capped,
                                )?;
                                Ok(capped)
                            })
                            .unwrap_or_else(|_| seq.clamp(0, room.last_seq));
                        state.hub.broadcast(
                            &room_id,
                            &serde_json::to_string(&json!({
                                "type": "read",
                                "user_id": uid,
                                "seq": capped,
                            }))
                            .unwrap_or_default(),
                        );
                    }
                    ClientEvent::Hand { raised } => {
                        if last_social[1].elapsed() < Duration::from_millis(300) {
                            continue;
                        }
                        last_social[1] = Instant::now();
                        let hands = state.hub.set_hand(&room_id, &uid, raised);
                        state.hub.broadcast(
                            &room_id,
                            &serde_json::to_string(&json!({
                                "type": "hand",
                                "user_id": uid,
                                "raised": raised,
                                "hands": hands,
                            }))
                            .unwrap_or_default(),
                        );
                    }
                    ClientEvent::VoiceReaction { emoji } => {
                        if last_social[2].elapsed() < Duration::from_millis(500) {
                            continue;
                        }
                        last_social[2] = Instant::now();
                        const ALLOWED: &[&str] = &["👍", "👏", "🎉", "😂", "❤️", "🔥"];
                        let custom = emoji
                            .strip_prefix(':')
                            .and_then(|s| s.strip_suffix(':'))
                            .filter(|n| {
                                n.len() >= 2
                                    && n.len() <= 32
                                    && n.bytes().all(|b| {
                                        b.is_ascii_lowercase()
                                            || b.is_ascii_digit()
                                            || matches!(b, b'_' | b'+' | b'-')
                                    })
                            });
                        if !ALLOWED.contains(&emoji.as_str()) {
                            let Some(name) = custom else {
                                let _ = state.hub.send_to(
                                    &room_id,
                                    &uid,
                                    r#"{"type":"error","code":"validation","message":"不支持的表情"}"#,
                                );
                                continue;
                            };
                            let exists = state
                                .db
                                .with_sync(|conn| crate::infra::repo::custom_emoji_exists(conn, name))
                                .unwrap_or(false);
                            if !exists {
                                let _ = state.hub.send_to(
                                    &room_id,
                                    &uid,
                                    r#"{"type":"error","code":"validation","message":"不支持的表情"}"#,
                                );
                                continue;
                            }
                        }
                        let nickname = state
                            .db
                            .with_sync(|conn| {
                                crate::infra::repo::get_member(conn, &room_id, &uid)
                                    .map(|m| m.map(|m| m.nickname))
                            })
                            .ok()
                            .flatten()
                            .unwrap_or_default();
                        state.hub.broadcast(
                            &room_id,
                            &serde_json::to_string(&json!({
                                "type": "voice_reaction",
                                "user_id": uid,
                                "nickname": nickname,
                                "emoji": emoji,
                            }))
                            .unwrap_or_default(),
                        );
                    }
                    ClientEvent::MediaState {
                        mic_muted,
                        speaker_muted,
                    } => {
                        state.hub.broadcast(
                            &room_id,
                            &serde_json::to_string(&json!({
                                "type": "media_state",
                                "user_id": uid,
                                "mic_muted": mic_muted,
                                "speaker_muted": speaker_muted,
                            }))
                            .unwrap_or_default(),
                        );
                    }
                    ClientEvent::RoomMuteToggle { muted } => {
                        // 仅房主可操作；私聊房间为双方房主制。
                        // 权限校验、幂等判断与写入放在同一事务里，避免两次往返。
                        let (is_owner, changed) = state
                            .db
                            .with_sync(|conn| -> ApiResult<(bool, bool)> {
                                let is_owner =
                                    crate::infra::repo::get_member(conn, &room_id, &uid)?
                                        .map(|m| {
                                            m.room_role == crate::domain::models::RoomRole::Owner
                                        })
                                        .unwrap_or(false);
                                if !is_owner {
                                    return Ok((false, false));
                                }
                                // 幂等：状态未变化（连点/重复广播）不广播、不插系统消息，
                                // 避免"房主开启了全员禁言"消息堆叠。
                                // 发言权生命周期跟随禁言开关：每次开关都重新洗牌
                                // （清空全部获权与待审批申请）。
                                let current = crate::infra::repo::get_room_by_id(conn, &room_id)?
                                    .map(|r| r.room_muted)
                                    .unwrap_or(false);
                                if current == muted {
                                    return Ok((true, false));
                                }
                                crate::infra::repo::set_room_muted(conn, &room_id, muted)?;
                                crate::infra::repo::clear_speak_grants(conn, &room_id)?;
                                Ok((true, true))
                            })
                            .unwrap_or((false, false));
                        if !is_owner {
                            let _ = state.hub.send_to(
                                &room_id,
                                &uid,
                                r#"{"type":"error","code":"forbidden","message":"只有房主可以设置全员禁言"}"#,
                            );
                            continue;
                        }
                        if !changed {
                            continue;
                        }
                        state.hub.broadcast(
                            &room_id,
                            &serde_json::to_string(&json!({
                                "type": "room_muted",
                                "muted": muted,
                                "user_id": uid,
                            }))
                            .unwrap_or_default(),
                        );
                        let sys = state
                            .db
                            .with_sync(|conn| -> ApiResult<Value> {
                                let text = if muted {
                                    "房主开启了全员禁言"
                                } else {
                                    "房主关闭了全员禁言"
                                };
                                let m = crate::infra::repo::insert_system_message(
                                    conn, &room_id, text,
                                )?;
                                Ok(api::message_json(&m, None, None))
                            })
                            .unwrap_or(Value::Null);
                        if !sys.is_null() {
                            state.hub.broadcast(
                                &room_id,
                                &serde_json::to_string(
                                    &json!({ "type": "message", "message": sys }),
                                )
                                .unwrap_or_default(),
                            );
                        }
                    }
                    ClientEvent::SpeakRequest => {
                        // 一次拿齐申请判定所需的全部读数据，避免每个事件 4~6 次
                        // spawn_blocking + 连接锁竞争。
                        let (member, granted, room_muted, all_members) = state
                            .db
                            .with_sync(
                                |conn| -> ApiResult<(
                                    Option<crate::domain::models::RoomMember>,
                                    bool,
                                    bool,
                                    Vec<crate::domain::models::RoomMember>,
                                )> {
                                    let member =
                                        crate::infra::repo::get_member(conn, &room_id, &uid)?;
                                    let granted =
                                        crate::infra::repo::has_speak_grant(conn, &room_id, &uid)?;
                                    let muted = crate::infra::repo::get_room_by_id(conn, &room_id)?
                                        .map(|r| r.room_muted)
                                        .unwrap_or(false);
                                    let members = crate::infra::repo::list_members(conn, &room_id)?;
                                    Ok((member, granted, muted, members))
                                },
                            )
                            .unwrap_or((None, false, false, Vec::new()));
                        let is_owner = member
                            .as_ref()
                            .map(|m| m.room_role == crate::domain::models::RoomRole::Owner)
                            .unwrap_or(false);
                        let muted = room_muted;
                        if !muted {
                            // 房间未禁言时申请无意义；房主申请直接视为开启发言权占位。
                            let _ = state.hub.send_to(
                                &room_id,
                                &uid,
                                r#"{"type":"error","code":"validation","message":"房间未开启全员禁言，无需申请发言"}"#,
                            );
                            continue;
                        }
                        if granted {
                            continue;
                        }
                        // 房主申请自动通过（默认批准）。
                        if is_owner {
                            let ok = state
                                .db
                                .with_sync(|conn| {
                                    crate::infra::repo::grant_speak(conn, &room_id, &uid, &uid)
                                })
                                .unwrap_or(false);
                            if ok {
                                state.hub.broadcast(
                                    &room_id,
                                    &serde_json::to_string(&json!({
                                        "type": "speak_grant",
                                        "user_id": uid,
                                        "auto": true,
                                    }))
                                    .unwrap_or_default(),
                                );
                            }
                            continue;
                        }
                        let ok = state
                            .db
                            .with_sync(|conn| {
                                crate::infra::repo::insert_speak_request(conn, &room_id, &uid)
                            })
                            .unwrap_or(false);
                        if !ok {
                            // 重复申请：已挂起，静默忽略（不重复通知房主）。
                            continue;
                        }
                        // 通知房主（在线时）；离线时房主下次进房从 HTTP 快照取。
                        let nick = member
                            .as_ref()
                            .map(|m| m.nickname.clone())
                            .unwrap_or_else(|| uid.chars().take(8).collect());
                        let event = serde_json::to_string(&json!({
                            "type": "speak_request_notify",
                            "user_id": uid,
                            "nickname": nick,
                        }))
                        .unwrap_or_default();
                        // 通知所有房主（普通房间 1 位，私聊房间双方都是房主）。
                        let owners: Vec<String> = all_members
                            .into_iter()
                            .filter(|m| {
                                m.room_role == crate::domain::models::RoomRole::Owner
                                    && m.left_at.is_none()
                            })
                            .map(|m| m.user_id)
                            .collect();
                        for owner in owners {
                            let _ = state.hub.send_to(&room_id, &owner, &event);
                        }
                        // 申请人回执。
                        let _ = state
                            .hub
                            .send_to(&room_id, &uid, r#"{"type":"speak_requested"}"#);
                    }
                    ClientEvent::SpeakReply { user_id, approve } => {
                        let member = state
                            .db
                            .with_sync(|conn| crate::infra::repo::get_member(conn, &room_id, &uid))
                            .ok()
                            .flatten();
                        let is_owner = member
                            .map(|m| m.room_role == crate::domain::models::RoomRole::Owner)
                            .unwrap_or(false);
                        if !is_owner {
                            let _ = state.hub.send_to(
                                &room_id,
                                &uid,
                                r#"{"type":"error","code":"forbidden","message":"只有房主可以审批发言申请"}"#,
                            );
                            continue;
                        }
                        if approve {
                            // 幂等：已授权用户重复批准不广播、不插系统消息。
                            // 授权 + 昵称 + 系统消息在同一事务内完成。
                            let (ok, sys) = state
                                .db
                                .with_sync(|conn| -> ApiResult<(bool, Value)> {
                                    let ok = crate::infra::repo::grant_speak(
                                        conn, &room_id, &user_id, &uid,
                                    )?;
                                    if !ok {
                                        return Ok((false, Value::Null));
                                    }
                                    let nick =
                                        crate::infra::repo::get_member(conn, &room_id, &user_id)
                                            .ok()
                                            .flatten()
                                            .map(|m| m.nickname)
                                            .unwrap_or_else(|| user_id.chars().take(8).collect());
                                    let sys = crate::infra::repo::insert_system_message(
                                        conn,
                                        &room_id,
                                        &format!("{nick} 获得了发言权限"),
                                    )
                                    .map(|m| api::message_json(&m, None, None))
                                    .unwrap_or(Value::Null);
                                    Ok((true, sys))
                                })
                                .unwrap_or((false, Value::Null));
                            if ok {
                                state.hub.broadcast(
                                    &room_id,
                                    &serde_json::to_string(&json!({
                                        "type": "speak_grant",
                                        "user_id": user_id,
                                    }))
                                    .unwrap_or_default(),
                                );
                                if !sys.is_null() {
                                    state.hub.broadcast(
                                        &room_id,
                                        &serde_json::to_string(
                                            &json!({ "type": "message", "message": sys }),
                                        )
                                        .unwrap_or_default(),
                                    );
                                }
                            }
                        } else {
                            let ok = state
                                .db
                                .with_sync(|conn| -> ApiResult<()> {
                                    crate::infra::repo::delete_speak_request(
                                        conn, &room_id, &user_id,
                                    )
                                })
                                .is_ok();
                            if ok {
                                let _ = state.hub.send_to(
                                    &room_id,
                                    &user_id,
                                    r#"{"type":"speak_rejected"}"#,
                                );
                            }
                        }
                    }
                    ClientEvent::SpeakRevoke { user_id } => {
                        let member = state
                            .db
                            .with_sync(|conn| crate::infra::repo::get_member(conn, &room_id, &uid))
                            .ok()
                            .flatten();
                        let is_owner = member
                            .map(|m| m.room_role == crate::domain::models::RoomRole::Owner)
                            .unwrap_or(false);
                        if !is_owner {
                            let _ = state.hub.send_to(
                                &room_id,
                                &uid,
                                r#"{"type":"error","code":"forbidden","message":"只有房主可以取消发言权限"}"#,
                            );
                            continue;
                        }
                        if user_id == uid {
                            let _ = state.hub.send_to(
                                &room_id,
                                &uid,
                                r#"{"type":"error","code":"validation","message":"不能取消自己的发言权限"}"#,
                            );
                            continue;
                        }
                        // 幂等：目标本无发言权时不广播。
                        let ok = state
                            .db
                            .with_sync(|conn| {
                                crate::infra::repo::revoke_speak(conn, &room_id, &user_id)
                            })
                            .unwrap_or(false);
                        if ok {
                            state.hub.broadcast(
                                &room_id,
                                &serde_json::to_string(&json!({
                                    "type": "speak_revoked",
                                    "user_id": user_id,
                                }))
                                .unwrap_or_default(),
                            );
                            let nick = state
                                .db
                                .with_sync(|conn| {
                                    crate::infra::repo::get_member(conn, &room_id, &user_id)
                                })
                                .ok()
                                .flatten()
                                .map(|m| m.nickname)
                                .unwrap_or_else(|| user_id.chars().take(8).collect());
                            let sys = state
                                .db
                                .with_sync(|conn| {
                                    crate::infra::repo::insert_system_message(
                                        conn,
                                        &room_id,
                                        &format!("{nick} 的发言权限被取消"),
                                    )
                                    .map(|m| api::message_json(&m, None, None))
                                })
                                .unwrap_or(Value::Null);
                            if !sys.is_null() {
                                state.hub.broadcast(
                                    &room_id,
                                    &serde_json::to_string(
                                        &json!({ "type": "message", "message": sys }),
                                    )
                                    .unwrap_or_default(),
                                );
                            }
                        }
                    }
                    ClientEvent::SpeakRelinquish => {
                        // 用户主动放弃自己的发言权限（仅自己有权限时生效）。
                        let ok = state
                            .db
                            .with_sync(|conn| {
                                crate::infra::repo::revoke_speak(conn, &room_id, &uid)
                            })
                            .unwrap_or(false);
                        if !ok {
                            continue;
                        }
                        state.hub.broadcast(
                            &room_id,
                            &serde_json::to_string(&json!({
                                "type": "speak_revoked",
                                "user_id": uid,
                                "relinquished": true,
                            }))
                            .unwrap_or_default(),
                        );
                        let nick = state
                            .db
                            .with_sync(|conn| crate::infra::repo::get_member(conn, &room_id, &uid))
                            .ok()
                            .flatten()
                            .map(|m| m.nickname)
                            .unwrap_or_else(|| uid.chars().take(8).collect());
                        let sys = state
                            .db
                            .with_sync(|conn| {
                                crate::infra::repo::insert_system_message(
                                    conn,
                                    &room_id,
                                    &format!("{nick} 放弃了发言权限"),
                                )
                                .map(|m| api::message_json(&m, None, None))
                            })
                            .unwrap_or(Value::Null);
                        if !sys.is_null() {
                            state.hub.broadcast(
                                &room_id,
                                &serde_json::to_string(
                                    &json!({ "type": "message", "message": sys }),
                                )
                                .unwrap_or_default(),
                            );
                        }
                    }
                    ClientEvent::Reaction { message_id, emoji } => {
                        // 固定 unicode 白名单 + 已注册的 :custom: 表情。
                        const ALLOWED: &[&str] = &["👍", "❤️", "😂", "😮", "😢", "😡", "🎉"];
                        let custom = emoji
                            .strip_prefix(':')
                            .and_then(|s| s.strip_suffix(':'))
                            .filter(|n| {
                                n.len() >= 2
                                    && n.len() <= 32
                                    && n.bytes().all(|b| {
                                        b.is_ascii_lowercase()
                                            || b.is_ascii_digit()
                                            || matches!(b, b'_' | b'+' | b'-')
                                    })
                            });
                        if !ALLOWED.contains(&emoji.as_str()) {
                            let Some(name) = custom else {
                                let _ = state.hub.send_to(
                                    &room_id,
                                    &uid,
                                    r#"{"type":"error","code":"validation","message":"不支持的表情"}"#,
                                );
                                continue;
                            };
                            let exists = state
                                .db
                                .with_sync(|conn| {
                                    crate::infra::repo::custom_emoji_exists(conn, name)
                                })
                                .unwrap_or(false);
                            if !exists {
                                let _ = state.hub.send_to(
                                    &room_id,
                                    &uid,
                                    r#"{"type":"error","code":"validation","message":"不支持的表情"}"#,
                                );
                                continue;
                            }
                        }
                        let added = state
                            .db
                            .with_sync(|conn| -> ApiResult<bool> {
                                if !crate::infra::repo::message_belongs_to_room(
                                    conn,
                                    &message_id,
                                    &room_id,
                                )? {
                                    return Err(crate::error::ApiError::not_found("消息不存在"));
                                }
                                crate::infra::repo::toggle_reaction(conn, &message_id, &uid, &emoji)
                            })
                            .unwrap_or(false);
                        // 广播最新聚合（事件自带，前端直接替换，避免计数漂移）。
                        let reactions = state
                            .db
                            .with_sync(|conn| {
                                let map = crate::infra::repo::message_reactions(
                                    conn,
                                    &room_id,
                                    std::slice::from_ref(&message_id),
                                    &uid,
                                )?;
                                let list: Vec<Value> = map
                                    .get(&message_id)
                                    .map(|v| {
                                        v.iter()
                                            .map(|(e, c, me)| json!({ "emoji": e, "count": c, "me": me }))
                                            .collect()
                                    })
                                    .unwrap_or_default();
                                Ok::<_, crate::error::ApiError>(list)
                            })
                            .unwrap_or_default();
                        state.hub.broadcast(
                            &room_id,
                            &serde_json::to_string(&json!({
                                "type": "reaction",
                                "message_id": message_id,
                                "user_id": uid,
                                "added": added,
                                "reactions": reactions,
                            }))
                            .unwrap_or_default(),
                        );
                    }
                    ClientEvent::Signal { to, data } => {
                        // P2P SDP/ICE 中继只属于 p2p 房间；sfu 房间不接受该事件。
                        if room.media_mode != MediaMode::P2p {
                            let _ = state.hub.send_to(
                                &room_id,
                                &uid,
                                r#"{"type":"error","code":"media_mode_mismatch","message":"媒体接口与房间媒体模式不匹配"}"#,
                            );
                            continue;
                        }
                        // Relay only between current members.
                        let target_ok = state
                            .db
                            .with_sync(|conn| -> ApiResult<bool> {
                                Ok(crate::infra::repo::get_member(conn, &room_id, &to)?
                                    .map(|m| m.left_at.is_none())
                                    .unwrap_or(false))
                            })
                            .unwrap_or(false);
                        if !target_ok {
                            continue;
                        }
                        let _ = state.hub.send_to(
                            &room_id,
                            &to,
                            &serde_json::to_string(&json!({
                                "type": "signal",
                                "from": uid,
                                "data": data,
                            }))
                            .unwrap_or_default(),
                        );
                    }
                }
                // Presence: throttle DB writes.
                if last_db_write.elapsed() >= state.cfg.heartbeat_interval() {
                    last_db_write = Instant::now();
                    let db2 = db.clone();
                    let uid3 = uid.clone();
                    let room_id3 = room_id.clone();
                    tokio::spawn(async move {
                        let now = util::now();
                        let _ = db2.with_sync(|conn| -> ApiResult<()> {
                            crate::infra::repo::touch_user_online(conn, &uid3, &now)?;
                            crate::infra::repo::touch_member_online(conn, &room_id3, &uid3, &now)?;
                            Ok(())
                        });
                    });
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Teardown.
    presence_task.abort();
    state.hub.unregister(&room_id, &uid, &tx);
    state.hub.connection_closed(&uid);
    if voice_member {
        broadcast_voice(&state, &room_id);
    }
    // 释放本地 sender，关闭 channel 让 writer task 退出；否则 writer.await
    // 永远挂起，后续的离线确认任务（spawn）永远不执行。
    drop(tx);
    let _ = writer.await;
    let _ = (hub, db, uid2, room_id2);

    // 关闭标签页/断网：延迟确认离线（刷新页面会很快重连，8 秒窗口避免
    // "离开/加入"消息刷屏）。确认离线后插入系统消息并广播 member_left。
    let st = state.clone();
    let room_id3 = room_id.clone();
    let uid3 = uid.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(8)).await;
        if st.hub.is_connected(&room_id3, &uid3) {
            return; // 已重连（刷新/SPA 切换回来）
        }
        let nick = st
            .db
            .with_sync(|conn| -> ApiResult<Option<String>> {
                // 主动离开后 left_at 已设置：不再重复播报。
                let m = crate::infra::repo::get_member(conn, &room_id3, &uid3)?;
                match &m {
                    Some(m) if m.left_at.is_none() => Ok(Some(m.nickname.clone())),
                    _ => Ok(None),
                }
            })
            .unwrap_or(None);
        let Some(nick) = nick else { return };
        let msg = st
            .db
            .with_sync(|conn| {
                crate::infra::repo::insert_system_message(
                    conn,
                    &room_id3,
                    &format!("{nick} 离开了房间"),
                )
                .map(|m| api::message_json(&m, None, None))
            })
            .unwrap_or(Value::Null);
        if !msg.is_null() {
            st.hub.broadcast(
                &room_id3,
                &serde_json::to_string(&json!({ "type": "message", "message": msg }))
                    .unwrap_or_default(),
            );
        }
        // 断连（关闭标签页）≠ 离开房间：成员关系保留，仅广播离线提示。
        st.hub.broadcast(
            &room_id3,
            &serde_json::to_string(&json!({ "type": "member_offline", "user_id": uid3 }))
                .unwrap_or_default(),
        );
    });
}

async fn check_voice(
    state: &AppState,
    room: &crate::domain::models::Room,
    uid: &str,
) -> ApiResult<()> {
    // The Room passed in is the connect-time snapshot. Re-read before every
    // voice_join so a mute/media-mode change made after the socket opened
    // cannot be bypassed by a long-lived connection.
    let room_id = room.room_id.clone();
    let current = state
        .db
        .with_sync(move |conn| crate::infra::repo::get_room_by_id(conn, &room_id))?
        .ok_or_else(crate::error::ApiError::room_not_found)?;
    if current.state != RoomState::Active {
        return Err(crate::error::ApiError::room_unavailable());
    }
    if current.media_mode != MediaMode::P2p {
        return Err(crate::error::ApiError::media_mode_mismatch());
    }
    let current_voice = state.hub.voice_members(&current.room_id).len();
    if current_voice + 1 > state.cfg.media.p2p_voice_max_participants {
        return Err(crate::error::ApiError::sfu_required());
    }
    // 房间级禁言：仅房主与获发言权用户可加入语音。
    state
        .db
        .with_sync(|conn| crate::app::rooms::check_speak_allowed(state, conn, &current, uid))?;
    Ok(())
}

fn broadcast_voice(state: &AppState, room_id: &str) {
    let members = state.hub.voice_members(room_id);
    let _ = serde_json::to_string(&json!({ "type": "voice_state", "members": members }))
        .map(|s| state.hub.broadcast(room_id, &s));
}

async fn build_hello(state: &AppState, room: &crate::domain::models::Room) -> String {
    let room_id = room.room_id.clone();
    let members = state
        .db
        .with_sync(|conn| -> ApiResult<Vec<Value>> {
            let ms = crate::infra::repo::list_members(conn, &room_id)?;
            // 用户一次批量取，避免每个成员一次查询。
            let uids: Vec<String> = ms.iter().map(|m| m.user_id.clone()).collect();
            let users = crate::infra::repo::users_by_ids(conn, &uids)?;
            Ok(ms
                .iter()
                .map(|m| api::member_json(m, users.get(&m.user_id)))
                .collect())
        })
        .unwrap_or_default();
    let voice = state.hub.voice_members(&room_id);
    let reads = state
        .db
        .with_sync(|conn| crate::infra::repo::room_read_seqs(conn, &room_id))
        .unwrap_or_default();
    let reads: serde_json::Map<String, Value> = reads
        .into_iter()
        .map(|(uid, seq)| (uid, json!(seq)))
        .collect();
    serde_json::to_string(&json!({
        "type": "hello",
        "room": api::room_json(room, None),
        "members": members,
        "voice_members": voice,
        "reads": reads,
        "hands": state.hub.hands(&room_id),
        "last_seq": room.last_seq,
        "now": util::now_rfc3339(),
    }))
    .unwrap_or_else(|_| r#"{"type":"hello"}"#.into())
}

/// One-second token bucket for inbound WS events. Once the budget is spent,
/// the caller closes the connection; it never panics or grows state.
struct EventRate {
    window_start: Instant,
    used: u32,
    max: u32,
}

impl EventRate {
    fn new(max: u32) -> Self {
        Self {
            window_start: Instant::now(),
            used: 0,
            max,
        }
    }

    fn allow(&mut self) -> bool {
        if self.window_start.elapsed() >= Duration::from_secs(1) {
            self.window_start = Instant::now();
            self.used = 0;
        }
        if self.used >= self.max {
            return false;
        }
        self.used += 1;
        true
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientEvent {
    Ping,
    SendMessage {
        body: String,
        reply_to: Option<String>,
        thread_root_id: Option<String>,
        attachment_id: Option<String>,
    },
    VoiceJoin,
    VoiceLeave,
    Signal {
        to: String,
        data: Value,
    },
    /// 麦克风/扬声器静音状态广播（供成员列表展示）。
    MediaState {
        mic_muted: bool,
        speaker_muted: bool,
    },
    /// 房主切换房间级全员禁言。
    RoomMuteToggle {
        muted: bool,
    },
    /// 禁言状态下申请发言（房主申请自动通过）。
    SpeakRequest,
    /// 房主审批发言申请。
    SpeakReply {
        user_id: String,
        approve: bool,
    },
    /// 房主取消某用户的发言权限。
    SpeakRevoke {
        user_id: String,
    },
    /// 获权用户主动放弃自己的发言权限。
    SpeakRelinquish,
    /// 消息 emoji 回应（切换：已有时移除）。
    Reaction {
        message_id: String,
        emoji: String,
    },
    /// 正在输入（不落库，仅广播，客户端 4 秒过期）。
    Typing,
    /// 已读游标（只增不减，持久化到 room_members）。
    Read {
        seq: i64,
    },
    /// 举手/放下。
    Hand {
        raised: bool,
    },
    /// 语音表情（房间内飘屏）。
    VoiceReaction {
        emoji: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_rate_caps_a_one_second_window() {
        let mut rate = EventRate::new(2);
        assert!(rate.allow());
        assert!(rate.allow());
        assert!(!rate.allow());
    }

    #[test]
    fn zero_event_budget_closes_immediately() {
        let mut rate = EventRate::new(0);
        assert!(!rate.allow());
    }
}
