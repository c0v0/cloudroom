//! Application workflows for room lifecycle and voice access.

use serde_json::json;

use crate::app::state::AppState;
use crate::domain::models::*;
use crate::error::{ApiError, ApiResult};
use crate::util;

/// Shared termination path: mark deleted; the code is released once the row
/// is physically purged after the grace period.
pub(crate) fn terminate_room(
    state: &AppState,
    room: &Room,
    reason: &str,
    actor: &str,
    release_code: bool,
) -> ApiResult<()> {
    state.db.with_sync(|conn| -> ApiResult<()> {
        crate::infra::repo::update_room_state(
            conn,
            &room.room_id,
            RoomState::Deleted,
            Some(&util::now()),
        )?;
        crate::app::audit::audited(
            conn,
            Some(actor),
            "room.deleted",
            Some("room"),
            Some(&room.room_id),
            json!({ "reason": reason, "room_code": room.room_code }),
        )?;
        if release_code {
            // Admin deletion must free the public code immediately while
            // retaining the room row/history for the grace period.
            let archived = format!("{}-deleted-{}", room.room_code, room.room_id);
            crate::infra::repo::archive_room_code(conn, &room.room_id, &archived)?;
        }
        Ok(())
    })?;
    state.hub.close_room(&room.room_id);
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
            // 号码只要求当前未被占用；删除/过期的房间在宽限期清理后号码可再次分配。
            if crate::infra::repo::get_room_by_code(conn, &normalized)?.is_some() {
                return Err(ApiError::conflict(format!(
                    "房间号 {normalized:?} 已被使用"
                )));
            }
            normalized
        }
        None => {
            let mut code = None;
            for _ in 0..8 {
                let c = crate::domain::codes::generate_code(&state.cfg);
                if crate::infra::repo::get_room_by_code(conn, &c)?.is_some() {
                    continue;
                }
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
        e2ee: false,
        e2ee_key: None,
        attachments_enabled: req.attachments,
        attachment_bytes: 0,
        guest_upgrade_open: false,
    };
    crate::infra::repo::insert_room(conn, &room)?;
    let nick = requester
        .username
        .clone()
        .unwrap_or_else(|| format!("guest-{}", crate::util::short_id(&requester.user_id)));
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
    crate::infra::repo::cancel_other_pending_create_requests(
        conn,
        &req.requester_id,
        request_id,
        actor,
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

/// Join or rejoin a room. The API keeps guest-session creation, rate limiting,
/// and response cookies; this function owns membership/invite consistency.
pub(crate) fn join_member(
    state: &AppState,
    conn: &mut rusqlite::Connection,
    code: &str,
    uid: &str,
    invite_token: Option<&str>,
    requested_nickname: Option<&str>,
) -> ApiResult<(Room, RoomMember, Option<Message>)> {
    let room =
        crate::infra::repo::get_room_by_code(conn, code)?.ok_or_else(ApiError::room_not_found)?;
    if room.state != RoomState::Active {
        return Err(ApiError::room_unavailable());
    }
    // 被移出的成员需要新邀请才能回来；有效邀请在兑换成功后解除封禁。
    if invite_token.is_none() && crate::infra::repo::is_room_banned(conn, &room.room_id, uid)? {
        return Err(ApiError::forbidden(
            "您已被移出该房间，需要新邀请才能重新加入",
        ));
    }
    let mut removed_left_at = None;
    if let Some(member) = crate::infra::repo::get_member(conn, &room.room_id, uid)? {
        if member.left_at.is_none() {
            return Err(ApiError::conflict("您已是该房间成员"));
        }
        if member.left_reason.as_deref() != Some("removed") {
            crate::infra::repo::restore_member(conn, &room.room_id, uid)?;
            crate::infra::repo::bump_room_seq(conn, &room.room_id)?;
            let member = crate::infra::repo::get_member(conn, &room.room_id, uid)?.unwrap();
            let sys = crate::infra::repo::insert_system_message(
                conn,
                &room.room_id,
                &format!("{} 加入了房间", member.nickname),
            )
            .ok();
            return Ok((room, member, sys));
        }
        // Removed members must present a fresh invite and pass the normal
        // capacity check; do not restore them here.
        removed_left_at = member.left_at;
    }
    if Some(uid) == room.owner_user_id.as_deref() {
        let nickname = default_nickname(conn, uid, &room.room_id)?;
        crate::infra::repo::add_member(conn, &room.room_id, uid, RoomRole::Owner, &nickname)?;
        crate::infra::repo::bump_room_seq(conn, &room.room_id)?;
        let member = crate::infra::repo::get_member(conn, &room.room_id, uid)?.unwrap();
        let sys = crate::infra::repo::insert_system_message(
            conn,
            &room.room_id,
            &format!("{} 加入了房间", member.nickname),
        )
        .ok();
        return Ok((room, member, sys));
    }
    if room.max_members > 0 {
        let active = crate::infra::repo::count_active_members(conn, &room.room_id)?;
        if active >= room.max_members {
            return Err(ApiError::new(
                axum::http::StatusCode::CONFLICT,
                "room_full",
                "房间成员已满",
            ));
        }
    }
    let token = invite_token.ok_or_else(|| ApiError::forbidden("加入该房间需要有效邀请"))?;
    redeem_invite(
        state,
        conn,
        token,
        &room,
        uid,
        requested_nickname,
        removed_left_at,
    )?;
    // 有效邀请 = 解除封禁（房主重新接纳）。
    crate::infra::repo::remove_room_ban(conn, &room.room_id, uid)?;
    crate::infra::repo::bump_room_seq(conn, &room.room_id)?;
    let member = crate::infra::repo::get_member(conn, &room.room_id, uid)?.unwrap();
    let sys = crate::infra::repo::insert_system_message(
        conn,
        &room.room_id,
        &format!("{} 加入了房间", member.nickname),
    )
    .ok();
    Ok((room, member, sys))
}

fn default_nickname(
    conn: &mut rusqlite::Connection,
    uid: &str,
    room_id: &str,
) -> ApiResult<String> {
    if let Some(m) = crate::infra::repo::get_member(conn, room_id, uid)? {
        if !m.nickname.is_empty() {
            return Ok(m.nickname);
        }
    }
    Ok(crate::infra::repo::get_user(conn, uid)?
        .and_then(|u| u.username)
        .unwrap_or_else(|| format!("guest-{}", crate::util::short_id(uid))))
}

fn redeem_invite(
    state: &AppState,
    conn: &mut rusqlite::Connection,
    raw: &str,
    room: &Room,
    uid: &str,
    requested_nickname: Option<&str>,
    min_created_at: Option<chrono::DateTime<chrono::Utc>>,
) -> ApiResult<()> {
    let token_hash = util::sha256_hex(raw.as_bytes());
    let inv = crate::infra::repo::get_invite_by_hash(conn, &token_hash)?
        .ok_or_else(|| ApiError::bad_request("invite_invalid", "邀请不存在"))?;
    if inv.room_id.as_deref() != Some(room.room_id.as_str()) || inv.kind != "room" {
        return Err(ApiError::bad_request("invite_invalid", "邀请不属于该房间"));
    }
    if !inv.active(util::now()) {
        return Err(ApiError::bad_request(
            "invite_invalid",
            "邀请已过期、已撤销或已被使用",
        ));
    }
    if let Some(after) = min_created_at {
        if inv.created_at <= after {
            return Err(ApiError::bad_request(
                "invite_invalid",
                "被移出后需要新邀请",
            ));
        }
    }
    let role = RoomRole::Member;
    let nickname = match requested_nickname {
        Some(n) if util::validate_nickname(n, state.cfg.security.max_nickname_graphemes) => {
            n.trim().to_string()
        }
        _ => {
            let user = crate::infra::repo::get_user(conn, uid)?;
            user.and_then(|x| x.username)
                .unwrap_or_else(|| format!("guest-{}", crate::util::short_id(uid)))
        }
    };
    let user = crate::infra::repo::get_user(conn, uid)?;
    if user.is_none() {
        crate::infra::repo::create_user(conn, uid, None, None, UserRole::Guest, None)?;
    }
    if !crate::infra::repo::consume_invite(conn, &inv.invite_id, uid)? {
        return Err(ApiError::bad_request("invite_invalid", "邀请已被使用"));
    }
    if let Some(existing) = crate::infra::repo::get_member(conn, &room.room_id, uid)? {
        if existing.left_at.is_none() {
            return Err(ApiError::conflict("您已是该房间成员"));
        }
        crate::infra::repo::restore_member_with_nickname(conn, &room.room_id, uid, &nickname)?;
    } else {
        crate::infra::repo::add_member(conn, &room.room_id, uid, role, &nickname)?;
    }
    crate::app::audit::audited(
        conn,
        Some(uid),
        "invite.redeemed",
        Some("invite"),
        Some(&inv.invite_id),
        json!({ "room_code": room.room_code }),
    )?;
    Ok(())
}

pub(crate) struct SettingsUpdate {
    pub text_enabled: bool,
    pub allow_direct_messages: bool,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
    pub audio_bitrate: u32,
    pub announcement: String,
    pub max_members: i64,
    pub e2ee: bool,
    pub guest_upgrade_open: Option<bool>,
}

pub(crate) fn update_settings(
    state: &AppState,
    room_id: &str,
    actor: &str,
    update: SettingsUpdate,
) -> ApiResult<()> {
    state.db.with_sync(|conn| -> ApiResult<()> {
        crate::infra::repo::update_room_settings(
            conn,
            room_id,
            update.text_enabled,
            update.allow_direct_messages,
            update.expires_at.as_ref(),
            update.audio_bitrate,
            &update.announcement,
            update.max_members,
            update.e2ee,
        )?;
        crate::infra::repo::bump_room_seq(conn, room_id)?;
        if let Some(open) = update.guest_upgrade_open {
            crate::infra::repo::set_room_guest_upgrade_open(conn, room_id, open)?;
        }
        crate::app::audit::audited(
            conn,
            Some(actor),
            "room.settings_updated",
            Some("room"),
            Some(room_id),
            json!({
                "text_enabled": update.text_enabled,
                "allow_direct_messages": update.allow_direct_messages,
                "expires_at": update.expires_at.map(|d| d.to_rfc3339()),
                "e2ee": update.e2ee,
            }),
        )?;
        Ok(())
    })
}

pub(crate) fn leave_member(state: &AppState, room_id: &str, uid: &str) -> ApiResult<()> {
    state.db.with_sync(|conn| -> ApiResult<()> {
        crate::infra::repo::leave_room(conn, room_id, uid)?;
        crate::infra::repo::bump_room_seq(conn, room_id)?;
        crate::app::audit::audited(
            conn,
            Some(uid),
            "room.left",
            Some("room"),
            Some(room_id),
            json!({}),
        )?;
        Ok(())
    })
}

pub(crate) fn set_member_role(
    state: &AppState,
    room_id: &str,
    actor: &str,
    target_user_id: &str,
    role: RoomRole,
) -> ApiResult<()> {
    state.db.with_sync(|conn| -> ApiResult<()> {
        let target = crate::infra::repo::get_member(conn, room_id, target_user_id)?
            .ok_or_else(|| ApiError::not_found("member not found"))?;
        if target.room_role == RoomRole::Owner {
            return Err(ApiError::validation("不能修改房主角色"));
        }
        if target_user_id == actor {
            return Err(ApiError::validation("不能修改自己的角色"));
        }
        crate::infra::repo::set_member_role(conn, room_id, target_user_id, role)?;
        crate::infra::repo::bump_room_seq(conn, room_id)?;
        crate::app::audit::audited(
            conn,
            Some(actor),
            "member.role_changed",
            Some("room"),
            Some(room_id),
            json!({ "user_id": target_user_id, "role": role.as_str() }),
        )?;
        Ok(())
    })
}

pub(crate) fn set_member_nickname(
    state: &AppState,
    room_id: &str,
    actor: &str,
    target_user_id: &str,
    nickname: &str,
) -> ApiResult<()> {
    state.db.with_sync(|conn| -> ApiResult<()> {
        let target = crate::infra::repo::get_member(conn, room_id, target_user_id)?
            .ok_or_else(|| ApiError::not_found("member not found"))?;
        if target.room_role == RoomRole::Owner && target.user_id != actor {
            return Err(ApiError::validation("不能修改房主昵称"));
        }
        crate::infra::repo::set_member_nickname(conn, room_id, target_user_id, nickname)?;
        crate::infra::repo::bump_room_seq(conn, room_id)?;
        crate::app::audit::audited(
            conn,
            Some(actor),
            "member.nickname_changed",
            Some("room"),
            Some(room_id),
            json!({ "user_id": target_user_id }),
        )?;
        Ok(())
    })
}

pub(crate) fn remove_member(
    state: &AppState,
    room_id: &str,
    actor: &str,
    target_user_id: &str,
) -> ApiResult<()> {
    state.db.with_sync(|conn| -> ApiResult<()> {
        let target = crate::infra::repo::get_member(conn, room_id, target_user_id)?
            .ok_or_else(|| ApiError::not_found("member not found"))?;
        if target.room_role == RoomRole::Owner && target.user_id != actor {
            return Err(ApiError::validation("不能移出房主"));
        }
        if target_user_id == actor {
            return Err(ApiError::validation("请使用离开房间接口退出"));
        }
        crate::infra::repo::remove_member(conn, room_id, target_user_id)?;
        // 移出即封禁：无新邀请不能自行重进。
        crate::infra::repo::add_room_ban(conn, room_id, target_user_id, Some(actor))?;
        crate::infra::repo::bump_room_seq(conn, room_id)?;
        crate::app::audit::audited(
            conn,
            Some(actor),
            "member.removed",
            Some("room"),
            Some(room_id),
            json!({ "user_id": target_user_id }),
        )?;
        Ok(())
    })
}

pub(crate) fn insert_system_message(
    state: &AppState,
    room_id: &str,
    body: &str,
) -> ApiResult<Message> {
    state
        .db
        .with_sync(|conn| crate::infra::repo::insert_system_message(conn, room_id, body))
}

pub(crate) fn revoke_invite(
    state: &AppState,
    room_id: &str,
    actor: &str,
    invite_id: &str,
) -> ApiResult<()> {
    state.db.with_sync(|conn| -> ApiResult<()> {
        let inv = crate::infra::repo::get_invite_by_id(conn, invite_id)?
            .ok_or_else(|| ApiError::not_found("邀请不存在"))?;
        if inv.room_id.as_deref() != Some(room_id) {
            return Err(ApiError::not_found("邀请不存在"));
        }
        crate::app::invites::ensure_revocable(&inv)?;
        crate::infra::repo::revoke_invite(conn, invite_id)?;
        crate::app::audit::audited(
            conn,
            Some(actor),
            "invite.revoked",
            Some("invite"),
            Some(invite_id),
            json!({}),
        )?;
        Ok(())
    })
}

pub(crate) fn create_mute(
    state: &AppState,
    mute: &Mute,
    duration_seconds: Option<i64>,
) -> ApiResult<()> {
    state.db.with_sync(|conn| -> ApiResult<()> {
        if mute.scope == "member" {
            let target = mute
                .target_user_id
                .as_deref()
                .ok_or_else(|| ApiError::validation("成员禁言需要指定用户"))?;
            let member = crate::infra::repo::get_member(conn, &mute.room_id, target)?
                .ok_or_else(|| ApiError::not_found("member not found"))?;
            if member.room_role == RoomRole::Owner {
                return Err(ApiError::validation("不能禁言房主"));
            }
        }
        crate::infra::repo::insert_mute(conn, mute)?;
        crate::infra::repo::bump_room_seq(conn, &mute.room_id)?;
        crate::app::audit::audited(
            conn,
            Some(&mute.muted_by),
            "mute.created",
            Some("room"),
            Some(&mute.room_id),
            json!({
                "mute_id": mute.mute_id,
                "scope": mute.scope,
                "target_user_id": mute.target_user_id,
                "duration_seconds": duration_seconds,
            }),
        )?;
        Ok(())
    })
}

pub(crate) fn remove_mute(
    state: &AppState,
    room_id: &str,
    actor: &str,
    mute_id: &str,
) -> ApiResult<Option<String>> {
    state.db.with_sync(|conn| -> ApiResult<Option<String>> {
        let mute = crate::infra::repo::get_mute(conn, mute_id)?
            .ok_or_else(|| ApiError::not_found("禁言记录不存在"))?;
        if mute.room_id != room_id {
            return Err(ApiError::not_found("禁言记录不存在"));
        }
        crate::infra::repo::delete_mute(conn, mute_id)?;
        crate::infra::repo::bump_room_seq(conn, room_id)?;
        crate::app::audit::audited(
            conn,
            Some(actor),
            "mute.removed",
            Some("room"),
            Some(room_id),
            json!({ "mute_id": mute_id }),
        )?;
        Ok(mute.target_user_id)
    })
}

pub(crate) struct CreatedDirectRoom {
    pub room: Room,
    pub parent: Option<Room>,
}

pub(crate) async fn create_direct_room(
    state: &AppState,
    parent_id: &str,
    uid: &str,
    peer: &str,
) -> ApiResult<CreatedDirectRoom> {
    let cfg = state.cfg.clone();
    let parent_id = parent_id.to_string();
    let uid = uid.to_string();
    let peer = peer.to_string();
    let room = state
        .db
        .with(move |conn| -> ApiResult<Room> {
            // Both must be current members of the parent room.
            let me = crate::infra::repo::get_member(conn, &parent_id, &uid)?
                .ok_or_else(|| ApiError::forbidden("不是房间成员 of this room"))?;
            if me.left_at.is_some() {
                return Err(ApiError::forbidden("不是房间成员 of this room"));
            }
            let peer_member = crate::infra::repo::get_member(conn, &parent_id, &peer)?
                .ok_or_else(|| ApiError::not_found("peer is 不是房间成员 of this room"))?;
            if peer_member.left_at.is_some() {
                return Err(ApiError::not_found("peer is 不是房间成员 of this room"));
            }
            // Reuse only an active direct room; a deleted/expired room is recreated.
            if let Some(existing_id) =
                crate::infra::repo::direct_room_by_pair(conn, &parent_id, &uid, &peer)?
            {
                return crate::infra::repo::get_room_by_id(conn, &existing_id)?
                    .ok_or_else(|| ApiError::internal("私聊房间不存在"));
            }
            // Create: claim a fresh code (retry on collision).
            // 直接房间继承父标准房间当前的媒体模式，不能单独申请切换。
            let parent_mode = crate::infra::repo::get_room_by_id(conn, &parent_id)?
                .map(|p| p.media_mode)
                .unwrap_or(MediaMode::Sfu);
            for _ in 0..8 {
                let code = crate::domain::codes::generate_code(&cfg);
                if crate::infra::repo::get_room_by_code(conn, &code)?.is_some() {
                    continue;
                }
                let now = util::now();
                let room = Room {
                    room_id: util::random_id(),
                    room_code: code,
                    kind: RoomKind::Direct,
                    owner_user_id: None,
                    parent_room_id: Some(parent_id.clone()),
                    state: RoomState::Active,
                    created_at: now,
                    expires_at: None,
                    deleted_at: None,
                    allow_direct_messages: false,
                    text_enabled: true,
                    media_mode: parent_mode,
                    audio_bitrate: 0,
                    room_muted: false,
                    announcement: String::new(),
                    max_members: 0,
                    last_seq: 0,
                    version: 1,
                    e2ee: false,
                    e2ee_key: None,
                    attachments_enabled: false,
                    attachment_bytes: 0,
                    guest_upgrade_open: false,
                };
                crate::infra::repo::insert_room(conn, &room)?;
                crate::infra::repo::create_direct_pair(
                    conn,
                    &room.room_id,
                    &parent_id,
                    &uid,
                    &peer,
                )?;
                let nick_me = crate::infra::repo::get_user(conn, &uid)?
                    .and_then(|u| u.username)
                    .unwrap_or_else(|| format!("guest-{}", crate::util::short_id(&uid)));
                let nick_peer = crate::infra::repo::get_user(conn, &peer)?
                    .and_then(|u| u.username)
                    .unwrap_or_else(|| format!("guest-{}", crate::util::short_id(&peer)));
                crate::infra::repo::add_member(
                    conn,
                    &room.room_id,
                    &uid,
                    RoomRole::Owner,
                    &nick_me,
                )?;
                crate::infra::repo::add_member(
                    conn,
                    &room.room_id,
                    &peer,
                    RoomRole::Owner,
                    &nick_peer,
                )?;
                return Ok(room);
            }
            Err(ApiError::internal("无法分配房间号"))
        })
        .await?;
    let parent_id = room.parent_room_id.clone().unwrap_or_default();
    let parent = state
        .db
        .with_sync(|conn| crate::infra::repo::get_room_by_id(conn, &parent_id))
        .unwrap_or(None);
    Ok(CreatedDirectRoom { room, parent })
}
