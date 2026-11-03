//! Background sweeper: room expiry, direct-room idle timeout, grace-period
//! physical cleanup, session expiry, rate-limiter pruning.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;

use crate::app::revocation::{self, LiveKitRevocationMetrics};
use crate::config::Config;
use crate::domain::models::RoomState;
use crate::error::ApiResult;
use crate::infra::db::Db;

pub async fn run(db: Db, cfg: Config, revocation_metrics: Arc<LiveKitRevocationMetrics>) {
    let cfg = Arc::new(cfg);
    let mut ticker = tokio::time::interval(Duration::from_secs(60));
    ticker.tick().await; // first tick immediately
    loop {
        ticker.tick().await;
        match sweep_once(&db, &cfg) {
            Ok(codes) => {
                for code in codes {
                    revocation::schedule_room(cfg.clone(), revocation_metrics.clone(), code);
                }
            }
            Err(e) => tracing::error!(error = %e.message, "sweep failed"),
        }
    }
}

pub fn sweep_once(db: &Db, cfg: &Config) -> ApiResult<Vec<String>> {
    let now = Utc::now();
    let (revoked, files) = db.with_sync(|conn| {
        let mut revoked = Vec::new();
        // 1. Standard/any rooms whose expires_at passed -> expired.
        for (room_id, code) in crate::infra::repo::rooms_to_expire(conn, &now)? {
            crate::infra::repo::update_room_state(conn, &room_id, RoomState::Expired, Some(&now))?;
            crate::infra::repo::insert_audit(
                conn,
                None,
                "room.expired",
                Some("room"),
                Some(&room_id),
                &serde_json::json!({}),
            )?;
            tracing::info!(room_id = %room_id, code = %code, "room expired");
            revoked.push(code);
        }
        // 2. Direct rooms: either participant offline beyond dm_idle_timeout.
        let idle_cutoff = now - chrono::Duration::from_std(cfg.dm_idle_timeout()).unwrap_or_default();
        for (room_id, code) in crate::infra::repo::direct_rooms_idle(conn, &idle_cutoff)? {
            crate::infra::repo::update_room_state(conn, &room_id, RoomState::Deleted, Some(&now))?;
            crate::infra::repo::insert_audit(
                conn,
                None,
                "room.direct_idle_deleted",
                Some("room"),
                Some(&room_id),
                &serde_json::json!({ "reason": "participant offline beyond dm_idle_timeout" }),
            )?;
            tracing::info!(room_id = %room_id, code = %code, "direct room deleted for idle timeout");
            revoked.push(code);
        }
        // 3. Grace period elapsed -> physical cleanup. Codes become allocatable again.
        let cleanup_cutoff = now - chrono::Duration::from_std(cfg.cleanup_grace()).unwrap_or_default();
        let mut purged_files: Vec<String> = Vec::new();
        for room_id in crate::infra::repo::rooms_to_purge(conn, &cleanup_cutoff)? {
            // 附件行随房间级联删除，文件本身在这里回收。
            purged_files.extend(crate::infra::repo::list_attachment_ids(conn, &room_id)?);
            crate::infra::repo::purge_room(conn, &room_id)?;
            crate::infra::repo::insert_audit(
                conn,
                None,
                "room.purged",
                Some("room"),
                Some(&room_id),
                &serde_json::json!({}),
            )?;
            tracing::info!(room_id = %room_id, "room data purged after grace period");
        }
        // 4. Expired sessions.
        let n = crate::infra::repo::delete_expired_sessions(conn, &now)?;
        if n > 0 {
            tracing::debug!(sessions = n, "expired sessions removed");
        }
        Ok((revoked, purged_files))
    })?;
    // 文件删除放在数据库锁之外。
    if !files.is_empty() {
        let dir = cfg.data.dir.join("attachments");
        for id in &files {
            let _ = std::fs::remove_file(dir.join(id));
        }
        tracing::info!(files = files.len(), "attachment files removed");
    }
    Ok(revoked)
}
