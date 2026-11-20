//! Admin write workflows: request decisions, ownership, users, invites.

use serde_json::json;

use crate::app::state::AppState;
use crate::domain::models::*;
use crate::error::{ApiError, ApiResult};

pub(crate) fn reject_room_request(
    state: &AppState,
    request_id: &str,
    actor: &str,
    reason: &str,
) -> ApiResult<()> {
    state.db.with_sync(|conn| -> ApiResult<()> {
        let request = crate::infra::repo::get_room_request(conn, request_id)?
            .ok_or_else(|| ApiError::not_found("申请不存在"))?;
        if request.status != "pending" {
            return Err(ApiError::conflict("该申请不在待审批状态"));
        }
        crate::infra::repo::update_request_decision(
            conn,
            request_id,
            "rejected",
            actor,
            Some(reason),
            None,
        )?;
        crate::app::audit::audited(
            conn,
            Some(actor),
            "room_request.rejected",
            Some("room_request"),
            Some(request_id),
            json!({ "reason": reason }),
        )?;
        Ok(())
    })
}

pub(crate) async fn transfer_owner(
    state: &AppState,
    code: &str,
    target: &str,
    actor: &str,
) -> ApiResult<String> {
    let code = code.to_string();
    let target = target.to_string();
    let actor = actor.to_string();
    state
        .db
        .with(move |conn| -> ApiResult<String> {
            let room = crate::infra::repo::get_room_by_code(conn, &code)?
                .ok_or_else(ApiError::room_not_found)?;
            if room.kind != RoomKind::Standard {
                return Err(ApiError::validation("房主转让仅适用于普通房间"));
            }
            if room.state != crate::domain::models::RoomState::Active {
                return Err(ApiError::room_unavailable());
            }
            let target_user = crate::infra::repo::get_user(conn, &target)?
                .ok_or_else(|| ApiError::not_found("target user not found"))?;
            if target_user.role == UserRole::Guest {
                return Err(ApiError::validation("不能把房主转让给访客"));
            }
            // Target becomes owner member; every other owner role is demoted.
            crate::infra::repo::demote_other_owners(conn, &room.room_id, &target)?;
            if crate::infra::repo::get_member(conn, &room.room_id, &target)?.is_some() {
                crate::infra::repo::set_member_role(conn, &room.room_id, &target, RoomRole::Owner)?;
            } else {
                let nick = target_user
                    .username
                    .unwrap_or_else(|| format!("guest-{}", crate::util::short_id(&target)));
                crate::infra::repo::add_member(
                    conn,
                    &room.room_id,
                    &target,
                    RoomRole::Owner,
                    &nick,
                )?;
            }
            crate::infra::repo::set_room_owner(conn, &room.room_id, &target)?;
            crate::infra::repo::bump_room_seq(conn, &room.room_id)?;
            crate::app::audit::audited(
                conn,
                Some(&actor),
                "room.owner_transferred",
                Some("room"),
                Some(&room.room_id),
                json!({ "new_owner": target, "room_code": room.room_code }),
            )?;
            Ok(room.room_id)
        })
        .await
}

pub(crate) fn set_user_disabled(
    state: &AppState,
    user_id: &str,
    actor: &str,
    actor_role: UserRole,
    disabled: bool,
) -> ApiResult<()> {
    state.db.with_sync(|conn| -> ApiResult<()> {
        let target = crate::infra::repo::get_user(conn, user_id)?
            .ok_or_else(|| ApiError::not_found("user not found"))?;
        if target.role.is_super_admin() {
            return Err(ApiError::forbidden("不能禁用超级管理员"));
        }
        if target.role == UserRole::Admin && !actor_role.is_super_admin() {
            return Err(ApiError::forbidden("只有超级管理员可以禁用管理员"));
        }
        crate::infra::repo::set_user_disabled(conn, user_id, disabled)?;
        if disabled {
            // Revoke all sessions so the disabled user is logged out everywhere.
            crate::infra::repo::delete_sessions_for_user(conn, user_id)?;
            crate::infra::repo::disable_totp(conn, user_id)?;
        }
        crate::app::audit::audited(
            conn,
            Some(actor),
            if disabled {
                "user.disabled"
            } else {
                "user.enabled"
            },
            Some("user"),
            Some(user_id),
            json!({}),
        )?;
        Ok(())
    })
}

pub(crate) fn password_reset_target(
    state: &AppState,
    user_id: &str,
    actor_role: UserRole,
) -> ApiResult<()> {
    // 密码重置等同于账号接管：只允许超级管理员，避免 admin 越过“看不到房间内容”的边界。
    if !actor_role.is_super_admin() {
        return Err(ApiError::forbidden("只有超级管理员可以重置密码"));
    }
    state.db.with_sync(|conn| -> ApiResult<()> {
        let target = crate::infra::repo::get_user(conn, user_id)?
            .ok_or_else(|| ApiError::not_found("user not found"))?;
        if target.role.is_super_admin() {
            return Err(ApiError::forbidden("不能重置超级管理员密码"));
        }
        Ok(())
    })
}

pub(crate) fn apply_password_reset(
    state: &AppState,
    user_id: &str,
    actor: &str,
    password_hash: &str,
) -> ApiResult<()> {
    state.db.with_sync(|conn| -> ApiResult<()> {
        crate::infra::repo::set_user_password_reset(conn, user_id, password_hash)?;
        crate::infra::repo::delete_sessions_for_user(conn, user_id)?;
        crate::infra::repo::disable_totp(conn, user_id)?;
        crate::app::audit::audited(
            conn,
            Some(actor),
            "user.password_reset",
            Some("user"),
            Some(user_id),
            json!({}),
        )?;
        Ok(())
    })
}

pub(crate) fn set_user_role(
    state: &AppState,
    user_id: &str,
    role: UserRole,
    actor: &str,
) -> ApiResult<()> {
    state.db.with_sync(|conn| -> ApiResult<()> {
        let target = crate::infra::repo::get_user(conn, user_id)?
            .ok_or_else(|| ApiError::not_found("user not found"))?;
        if target.role.is_super_admin() {
            return Err(ApiError::forbidden("不能修改超级管理员角色"));
        }
        if target.role == UserRole::Guest || role == UserRole::Guest {
            return Err(ApiError::forbidden(
                "不能把访客账号提升为管理角色，也不能把账号降级为访客",
            ));
        }
        crate::infra::repo::set_user_role(conn, user_id, role)?;
        // 房主失去 admin：其名下所有房间的访客转正开关立即置 0。
        if target.role.is_admin() && !role.is_admin() {
            let closed = crate::infra::repo::disable_guest_upgrade_for_owner(conn, user_id)?;
            if closed > 0 {
                crate::app::audit::audited(
                    conn,
                    Some(actor),
                    "room.guest_upgrade_closed",
                    Some("user"),
                    Some(user_id),
                    json!({ "rooms": closed }),
                )?;
            }
        }
        crate::app::audit::audited(
            conn,
            Some(actor),
            "user.role_changed",
            Some("user"),
            Some(user_id),
            json!({ "role": role.as_str() }),
        )?;
        Ok(())
    })
}

pub(crate) fn revoke_registration_invite(
    state: &AppState,
    invite_id: &str,
    actor: &str,
) -> ApiResult<()> {
    state.db.with_sync(|conn| -> ApiResult<()> {
        let invite = crate::infra::repo::get_invite_by_id(conn, invite_id)?
            .ok_or_else(|| ApiError::not_found("邀请不存在"))?;
        if invite.kind != "registration" {
            return Err(ApiError::not_found("邀请不存在"));
        }
        crate::app::invites::ensure_revocable(&invite)?;
        crate::infra::repo::revoke_invite(conn, invite_id)?;
        crate::app::audit::audited(
            conn,
            Some(actor),
            "invite.registration_revoked",
            Some("invite"),
            Some(invite_id),
            json!({}),
        )?;
        Ok(())
    })
}
