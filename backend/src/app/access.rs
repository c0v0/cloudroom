//! Application-level access checks: membership lookup and capabilities.

use crate::app::state::AppState;
use crate::domain::models::*;
use crate::error::{ApiError, ApiResult};

/// Returns (room, member).
pub async fn active_member_room(
    state: &AppState,
    code: &str,
    user_id: &str,
) -> ApiResult<(Room, RoomMember)> {
    let db = state.db.clone();
    let code = code.to_string();
    let uid = user_id.to_string();
    db.with(move |conn| {
        let room = crate::infra::repo::get_room_by_code(conn, &code)?
            .ok_or_else(ApiError::room_not_found)?;
        if room.state != RoomState::Active {
            return Err(ApiError::room_unavailable());
        }
        let member = crate::infra::repo::get_member(conn, &room.room_id, &uid)?
            .ok_or_else(|| ApiError::forbidden("you are 不是房间成员 of this room"))?;
        Ok((room, member))
    })
    .await
}

/// Capability check helpers -------------------------------------------------
pub fn can(_state: &AppState, member: &RoomMember, room: &Room, cap: &str) -> bool {
    match member.room_role {
        RoomRole::Owner => true,
        // 房间权限只有 owner / member 两级；管理能力全部归房主。
        // 账号权限（user/admin/super_admin/guest）与房间权限无关。
        RoomRole::Member => {
            matches!(cap, "send_text" | "join_voice") && room.state == RoomState::Active
        }
    }
}

pub fn require_cap(state: &AppState, member: &RoomMember, room: &Room, cap: &str) -> ApiResult<()> {
    if can(state, member, room, cap) {
        Ok(())
    } else {
        Err(ApiError::forbidden(format!("缺少权限: {cap}")))
    }
}
