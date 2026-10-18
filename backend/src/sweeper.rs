//! Background sweeper: room expiry, direct-room idle timeout, grace-period
//! physical cleanup, session expiry, rate-limiter pruning.

use std::time::Duration;

use chrono::Utc;

use crate::config::Config;
use crate::db::Db;
use crate::error::ApiResult;
use crate::models::RoomState;
use crate::util;

pub async fn run(db: Db, cfg: Config) {
    let mut ticker = tokio::time::interval(Duration::from_secs(60));
    ticker.tick().await; // first tick immediately
    loop {
        ticker.tick().await;
        if let Err(e) = sweep_once(&db, &cfg) {
            tracing::error!(error = %e.message, "sweep failed");
        }
    }
}

pub fn sweep_once(db: &Db, cfg: &Config) -> ApiResult<()> {
    let now = Utc::now();
    db.with_sync(|conn| {
        // 1. Standard/any rooms whose expires_at passed -> expired.
        for (room_id, code) in crate::repo::rooms_to_expire(conn, &now)? {
            crate::repo::update_room_state(conn, &room_id, RoomState::Expired, Some(&now))?;
            crate::repo::insert_tombstone(conn, &code)?;
            crate::repo::insert_audit(
                conn,
                None,
                "room.expired",
                Some("room"),
                Some(&room_id),
                &serde_json::json!({}),
            )?;
            tracing::info!(room_id = %room_id, code = %code, "room expired");
        }
        // 2. Direct rooms: either participant offline beyond dm_idle_timeout.
        let idle_cutoff = now - chrono::Duration::from_std(cfg.dm_idle_timeout()).unwrap_or_default();
        for (room_id, code) in crate::repo::direct_rooms_idle(conn, &idle_cutoff)? {
            crate::repo::update_room_state(conn, &room_id, RoomState::Deleted, Some(&now))?;
            crate::repo::insert_tombstone(conn, &code)?;
            crate::repo::insert_audit(
                conn,
                None,
                "room.direct_idle_deleted",
                Some("room"),
                Some(&room_id),
                &serde_json::json!({ "reason": "participant offline beyond dm_idle_timeout" }),
            )?;
            tracing::info!(room_id = %room_id, code = %code, "direct room deleted for idle timeout");
        }
        // 3. Grace period elapsed -> physical cleanup. Codes stay tombstoned.
        let cleanup_cutoff = now - chrono::Duration::from_std(cfg.cleanup_grace()).unwrap_or_default();
        for room_id in crate::repo::rooms_to_purge(conn, &cleanup_cutoff)? {
            crate::repo::purge_room(conn, &room_id)?;
            crate::repo::insert_audit(
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
        let n = crate::repo::delete_expired_sessions(conn, &now)?;
        if n > 0 {
            tracing::debug!(sessions = n, "expired sessions removed");
        }
        Ok(())
    })
}

pub fn _unused() {
    let _ = util::now_rfc3339();
}
