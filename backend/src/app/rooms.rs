//! Application workflows for room lifecycle and voice access.

use serde_json::json;

use crate::app::state::AppState;
use crate::domain::models::*;
use crate::error::{ApiError, ApiResult};
use crate::util;

/// Shared termination path: mark deleted, tombstone the code forever.
pub(crate) fn terminate_room(
    state: &AppState,
    room: &Room,
    reason: &str,
    actor: &str,
) -> ApiResult<()> {
    state.db.with_sync(|conn| -> ApiResult<()> {
        crate::infra::repo::update_room_state(
            conn,
            &room.room_id,
            RoomState::Deleted,
            Some(&util::now()),
        )?;
        crate::infra::repo::insert_tombstone(conn, &room.room_code)?;
        crate::app::audit::audited(
            conn,
            Some(actor),
            "room.deleted",
            Some("room"),
            Some(&room.room_id),
            json!({ "reason": reason, "room_code": room.room_code }),
        )?;
        Ok(())
    })?;
    Ok(())
}

/// 依据已提交的建房申请创建房间（批准或 super_admin 免审批共用）。
pub(crate) fn create_room_for_request(
    state: &AppState,
    conn: &mut rusqlite::Connection,
    request_id: &str,
    actor: &str,
) -> ApiResult<Room> {
    let req = crate::infra::repo::get_room_request(conn, request_id)?
        .ok_or_else(|| ApiError::not_found("申请不存在"))?;
    if req.status != "pending" {
        return Err(ApiError::conflict("该申请不在待审批状态"));
    }
    if req.request_kind != "create_room" {
        return Err(ApiError::conflict("该申请不是建房申请"));
    }
    let media_mode = req.media_mode;
    let requester = crate::infra::repo::get_user(conn, &req.requester_id)?
        .ok_or_else(|| ApiError::internal("申请人不存在"))?;
    let code = match req.desired_code {
        Some(code) => {
            let normalized = crate::domain::codes::validate_custom_code(&state.cfg, &code)
                .map_err(ApiError::validation)?;
            if crate::infra::repo::get_room_by_code(conn, &normalized)?.is_some()
                || crate::infra::repo::code_tombstoned(conn, &normalized)?
            {
                return Err(ApiError::conflict(format!(
                    "房间号 {normalized:?} 已被使用"
                )));
            }
            crate::infra::repo::claim_code(conn, &normalized)?;
            normalized
        }
        None => {
            let mut code = None;
            for _ in 0..8 {
                let c = crate::domain::codes::generate_code(&state.cfg);
                if crate::infra::repo::get_room_by_code(conn, &c)?.is_some()
                    || crate::infra::repo::code_tombstoned(conn, &c)?
                {
                    continue;
                }
                crate::infra::repo::claim_code(conn, &c)?;
                code = Some(c);
                break;
            }
            code.ok_or_else(|| ApiError::internal("无法分配房间号"))?
        }
    };
    let now = crate::util::now();
    let room = Room {
        room_id: crate::util::random_id(),
        room_code: code,
        kind: RoomKind::Standard,
        owner_user_id: Some(requester.user_id.clone()),
        parent_room_id: None,
        state: RoomState::Active,
        created_at: now,
        expires_at: None,
        deleted_at: None,
        allow_direct_messages: true,
        text_enabled: true,
        media_mode,
        audio_bitrate: req.audio_bitrate,
        room_muted: false,
        announcement: String::new(),
        max_members: 0,
        last_seq: 0,
        version: 1,
    };
    crate::infra::repo::insert_room(conn, &room)?;
    let nick = requester
        .username
        .clone()
        .unwrap_or_else(|| format!("guest-{}", &requester.user_id[..8]));
    crate::infra::repo::add_member(
        conn,
        &room.room_id,
        &requester.user_id,
        RoomRole::Owner,
        &nick,
    )?;
    crate::infra::repo::update_request_decision(
        conn,
        request_id,
        "approved",
        actor,
        None,
        Some(&room.room_id),
    )?;
    crate::app::audit::audited(
        conn,
        Some(actor),
        "room_request.approved",
        Some("room_request"),
        Some(request_id),
        serde_json::json!({ "room_code": room.room_code, "owner": requester.user_id }),
    )?;
    Ok(room)
}

/// 依据已提交的模式变更申请更新房间媒体模式（批准或 super_admin 免审批共用）。
/// 审批时重新校验：申请仍 pending、目标标准房间仍 active、申请人仍是房主、
/// 目标模式与当前模式不同；任何一项失效都整体失败，不部分应用。
pub(crate) fn apply_media_mode_change_for_request(
    _state: &AppState,
    conn: &mut rusqlite::Connection,
    request_id: &str,
    actor: &str,
) -> ApiResult<Room> {
    let req = crate::infra::repo::get_room_request(conn, request_id)?
        .ok_or_else(|| ApiError::not_found("申请不存在"))?;
    if req.status != "pending" {
        return Err(ApiError::conflict("该申请不在待审批状态"));
    }
    if req.request_kind != "change_media_mode" {
        return Err(ApiError::conflict("该申请不是媒体模式变更申请"));
    }
    let room_id = req
        .target_room_id
        .clone()
        .ok_or_else(|| ApiError::conflict("媒体模式变更申请缺少目标房间"))?;
    let room = crate::infra::repo::get_room_by_id(conn, &room_id)?
        .ok_or_else(|| ApiError::conflict("目标房间已不存在"))?;
    if room.state != RoomState::Active {
        return Err(ApiError::conflict("target 房间未处于正常状态"));
    }
    if room.kind != RoomKind::Standard {
        return Err(ApiError::conflict("媒体模式变更仅适用于普通房间"));
    }
    let member = crate::infra::repo::get_member(conn, &room_id, &req.requester_id)?
        .ok_or_else(|| ApiError::conflict("申请人已不再是目标房间成员"))?;
    if member.left_at.is_some() || member.room_role != RoomRole::Owner {
        return Err(ApiError::conflict("申请人已不再是房主"));
    }
    if room.media_mode == req.media_mode {
        return Err(ApiError::conflict("目标房间已使用该媒体模式"));
    }
    crate::infra::repo::update_room_media_mode(conn, &room_id, req.media_mode)?;
    // 模式变更申请可携带目标码率（SFU 目标）：审批时一并应用。
    if req.media_mode == MediaMode::Sfu && req.audio_bitrate != room.audio_bitrate {
        crate::infra::repo::update_room_audio_bitrate(conn, &room_id, req.audio_bitrate)?;
    }
    crate::infra::repo::update_request_decision(
        conn,
        request_id,
        "approved",
        actor,
        None,
        Some(&room_id),
    )?;
    crate::app::audit::audited(
        conn,
        Some(actor),
        "room_media_mode.changed",
        Some("room"),
        Some(&room_id),
        json!({
            "room_code": room.room_code,
            "from": room.media_mode.as_str(),
            "to": req.media_mode.as_str(),
            "request_id": request_id,
        }),
    )?;
    crate::infra::repo::get_room_by_id(conn, &room_id)?
        .ok_or_else(|| ApiError::internal("媒体模式变更后房间不存在"))
}

/// 房间级禁言校验：禁言中不允许任何语音存在，仅获发言权用户可加入
/// （房主同样需申请——房主申请自动通过）。
pub(crate) fn check_speak_allowed(
    _state: &AppState,
    conn: &mut rusqlite::Connection,
    room: &Room,
    uid: &str,
) -> ApiResult<()> {
    if !room.room_muted {
        return Ok(());
    }
    if crate::infra::repo::has_speak_grant(conn, &room.room_id, uid)? {
        return Ok(());
    }
    Err(ApiError::new(
        axum::http::StatusCode::FORBIDDEN,
        "room_muted",
        "房间已开启全员禁言，需申请发言并获得房主批准",
    ))
}
