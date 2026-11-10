//! Room request creation: validation, persistence, and auto-approval decision.
//! Transport concerns (CSRF, rate limit, response JSON) stay in the API.

use serde_json::json;

use crate::app::access::active_member_room;
use crate::app::state::AppState;
use crate::domain::models::*;
use crate::error::{ApiError, ApiResult};
use crate::util;

pub(crate) struct CreateInput {
    pub kind: String,
    pub note: Option<String>,
    pub desired_code: Option<String>,
    pub media_mode: String,
    pub audio_bitrate: Option<u32>,
    pub room_code: Option<String>,
    /// 建房申请：是否申请附件能力（仅 create_room 有意义）。
    pub attachments: bool,
}

pub(crate) struct CreatedRequest {
    pub request: RoomRequest,
    pub room: Option<Room>,
    pub auto_approved: bool,
    pub target_room_code: Option<String>,
}

pub(crate) async fn create(
    state: &AppState,
    actor: &str,
    actor_role: UserRole,
    input: CreateInput,
) -> ApiResult<CreatedRequest> {
    let note = input.note.unwrap_or_default();
    if note.chars().count() > state.cfg.rooms.max_note_chars {
        return Err(ApiError::validation("说明过长"));
    }
    if util::has_control_chars(&note) || util::has_bidi_controls(&note) {
        return Err(ApiError::validation("说明包含控制字符"));
    }
    let media_mode = MediaMode::from_str(&input.media_mode)
        .ok_or_else(|| ApiError::validation("media_mode 必须为 \"p2p\" 或 \"sfu\""))?;
    let audio_bitrate = input.audio_bitrate.unwrap_or(0);
    if !state
        .cfg
        .voice
        .can_set_bitrate(actor_role.as_str(), audio_bitrate)
    {
        return Err(ApiError::validation(
            "该音频码率档位不可用（权限不足或档位不存在）",
        ));
    }
    let request_id = util::random_id();
    match input.kind.as_str() {
        "create_room" => {
            if input.room_code.is_some() {
                return Err(ApiError::validation("room_code 仅适用于媒体模式变更申请"));
            }
            let desired_code = match input.desired_code {
                Some(code) if !code.trim().is_empty() => {
                    if !actor_role.is_super_admin() {
                        return Err(ApiError::forbidden("只有超级管理员可以指定自定义房间号"));
                    }
                    Some(
                        crate::domain::codes::validate_custom_code(&state.cfg, &code)
                            .map_err(ApiError::validation)?,
                    )
                }
                _ => None,
            };
            let request = RoomRequest {
                request_id: request_id.clone(),
                requester_id: actor.to_string(),
                request_kind: "create_room".into(),
                target_room_id: None,
                media_mode,
                audio_bitrate,
                note,
                desired_code,
                status: "pending".into(),
                created_at: util::now(),
                decided_at: None,
                decided_by: None,
                decision_reason: None,
                created_room_id: None,
                attachments: input.attachments,
            };
            state.db.with_sync(|conn| -> ApiResult<()> {
                if crate::infra::repo::has_pending_create_room_request(conn, actor)? {
                    return Err(ApiError::conflict("您已有待审批的建房申请，请等待处理"));
                }
                crate::infra::repo::insert_room_request(conn, &request)?;
                crate::app::audit::audited(
                    conn,
                    Some(actor),
                    "room_request.created",
                    Some("room_request"),
                    Some(&request_id),
                    json!({ "kind": "create_room", "media_mode": media_mode.as_str() }),
                )?;
                Ok(())
            })?;
            if actor_role.is_super_admin() {
                let uid = actor.to_string();
                let rid = request_id.clone();
                let st = state.clone();
                let room = state
                    .db
                    .with(move |conn| {
                        crate::app::rooms::create_room_for_request(&st, conn, &rid, &uid)
                    })
                    .await?;
                let approved = state
                    .db
                    .with_sync(|conn| crate::infra::repo::get_room_request(conn, &request_id))
                    .ok()
                    .flatten()
                    .unwrap_or(request);
                return Ok(CreatedRequest {
                    request: approved,
                    room: Some(room),
                    auto_approved: true,
                    target_room_code: None,
                });
            }
            Ok(CreatedRequest {
                request,
                room: None,
                auto_approved: false,
                target_room_code: None,
            })
        }
        "change_media_mode" => {
            if input.desired_code.is_some() {
                return Err(ApiError::validation("desired_code 仅适用于建房申请"));
            }
            let code = input.room_code.ok_or_else(|| {
                ApiError::validation("room_code is required for change_media_mode requests")
            })?;
            let (room, member) = active_member_room(state, &code, actor).await?;
            if room.kind != RoomKind::Standard {
                return Err(ApiError::validation(
                    "媒体模式变更仅适用于普通房间; direct rooms inherit the parent room mode",
                ));
            }
            if member.room_role != RoomRole::Owner {
                return Err(ApiError::forbidden("只有房主可以申请媒体模式变更"));
            }
            if room.media_mode == media_mode {
                return Err(ApiError::validation("房间已使用该媒体模式"));
            }
            let change_bitrate = if media_mode == MediaMode::Sfu {
                audio_bitrate
            } else {
                0
            };
            let target_room_id = room.room_id.clone();
            let request = RoomRequest {
                request_id: request_id.clone(),
                requester_id: actor.to_string(),
                request_kind: "change_media_mode".into(),
                target_room_id: Some(target_room_id.clone()),
                media_mode,
                audio_bitrate: change_bitrate,
                note,
                desired_code: None,
                status: "pending".into(),
                created_at: util::now(),
                decided_at: None,
                decided_by: None,
                decision_reason: None,
                created_room_id: None,
                attachments: false,
            };
            state.db.with_sync(|conn| -> ApiResult<()> {
                if crate::infra::repo::pending_media_change(conn, &target_room_id)?.is_some() {
                    return Err(ApiError::conflict("该房间已有待审批的媒体模式变更申请"));
                }
                crate::infra::repo::insert_room_request(conn, &request)?;
                crate::app::audit::audited(
                    conn,
                    Some(actor),
                    "room_request.created",
                    Some("room_request"),
                    Some(&request_id),
                    json!({
                        "kind": "change_media_mode",
                        "room_code": code,
                        "media_mode": media_mode.as_str(),
                    }),
                )?;
                Ok(())
            })?;
            if actor_role.is_super_admin() {
                let rid = request_id.clone();
                let uid = actor.to_string();
                let st = state.clone();
                let room = state
                    .db
                    .with(move |conn| {
                        crate::app::rooms::apply_media_mode_change_for_request(
                            &st, conn, &rid, &uid,
                        )
                    })
                    .await?;
                let approved = state
                    .db
                    .with_sync(|conn| crate::infra::repo::get_room_request(conn, &request_id))
                    .ok()
                    .flatten()
                    .unwrap_or(request);
                return Ok(CreatedRequest {
                    request: approved,
                    room: Some(room),
                    auto_approved: true,
                    target_room_code: None,
                });
            }
            Ok(CreatedRequest {
                request,
                room: None,
                auto_approved: false,
                target_room_code: Some(code),
            })
        }
        other => Err(ApiError::validation(format!(
            "kind 必须为 \"create_room\" 或 \"change_media_mode\"，实际为 {other:?}"
        ))),
    }
}
