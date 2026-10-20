//! Audit-log helper shared by application workflows and handlers.

use rusqlite::Connection;
use serde_json::Value;

use crate::error::ApiResult;

pub fn audited(
    conn: &mut Connection,
    actor: Option<&str>,
    action: &str,
    target_type: Option<&str>,
    target_id: Option<&str>,
    detail: Value,
) -> ApiResult<()> {
    crate::infra::repo::insert_audit(conn, actor, action, target_type, target_id, &detail)?;
    Ok(())
}
