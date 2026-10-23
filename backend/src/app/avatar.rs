//! Avatar upload/removal workflow: validation, storage, user field, old file cleanup.

use serde_json::json;

use crate::app::state::AppState;
use crate::domain::models::RoomState;
use crate::error::{ApiError, ApiResult};

pub(crate) struct AvatarUpdate {
    pub avatar_id: String,
    pub rooms: Vec<String>,
}

pub(crate) fn update_avatar(
    state: &AppState,
    user_id: &str,
    old_avatar_id: Option<&str>,
    bytes: &[u8],
) -> ApiResult<AvatarUpdate> {
    let (encoded, _) = crate::infra::avatars::process_avatar(bytes, &state.cfg)?;
    crate::infra::avatars::ensure_dir(&state.avatars_dir)?;
    let avatar_id = crate::infra::avatars::store_avatar(&state.avatars_dir, &encoded)?;

    let persisted = state.db.with_sync(|conn| -> ApiResult<()> {
        if !crate::infra::repo::compare_and_set_user_avatar(
            conn,
            user_id,
            old_avatar_id,
            &avatar_id,
        )? {
            return Err(ApiError::conflict("头像已被其他请求更新，请重试"));
        }
        crate::app::audit::audited(
            conn,
            Some(user_id),
            "user.avatar_updated",
            Some("user"),
            Some(user_id),
            json!({ "avatar_id": avatar_id }),
        )?;
        Ok(())
    });
    if let Err(e) = persisted {
        // The DB update rolled back; remove the file that was just written.
        crate::infra::avatars::delete_avatar(&state.avatars_dir, &avatar_id);
        return Err(e);
    }

    if let Some(old_id) = old_avatar_id {
        if old_id != avatar_id {
            crate::infra::avatars::delete_avatar(&state.avatars_dir, old_id);
        }
    }
    let rooms = active_room_ids(state, user_id);
    Ok(AvatarUpdate { avatar_id, rooms })
}

pub(crate) fn remove_avatar(
    state: &AppState,
    user_id: &str,
    old_avatar_id: Option<&str>,
) -> ApiResult<Vec<String>> {
    state.db.with_sync(|conn| -> ApiResult<()> {
        crate::infra::repo::set_user_avatar(conn, user_id, None)?;
        Ok(())
    })?;
    if let Some(old_id) = old_avatar_id {
        crate::infra::avatars::delete_avatar(&state.avatars_dir, old_id);
    }
    Ok(active_room_ids(state, user_id))
}

fn active_room_ids(state: &AppState, user_id: &str) -> Vec<String> {
    state
        .db
        .with_sync(|conn| -> ApiResult<Vec<String>> {
            Ok(crate::infra::repo::list_member_rooms(conn, user_id)?
                .into_iter()
                .filter(|r| r.state == RoomState::Active)
                .map(|r| r.room_id)
                .collect())
        })
        .unwrap_or_default()
}
