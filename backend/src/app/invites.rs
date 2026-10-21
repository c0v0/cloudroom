//! Invite use-cases shared by room and admin endpoints.

use crate::domain::models::Invite;
use crate::error::{ApiError, ApiResult};
use crate::util;

/// Revocation is allowed only before the invite is consumed or expired.
/// Already-revoked invites remain idempotent.
pub(crate) fn ensure_revocable(invite: &Invite) -> ApiResult<()> {
    if invite.consumed_at.is_some() {
        return Err(ApiError::conflict("邀请已被使用，无法撤销"));
    }
    if invite.expires_at <= util::now() {
        return Err(ApiError::conflict("邀请已过期，无法撤销"));
    }
    Ok(())
}
