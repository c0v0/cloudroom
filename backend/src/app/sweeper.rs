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

pub async fn run(
    db: Db,
    cfg: Config,
    revocation_metrics: Arc<LiveKitRevocationMetrics>,
    hub: Arc<crate::infra::hub::Hub>,
) {
    let cfg = Arc::new(cfg);
    let mut ticker = tokio::time::interval(Duration::from_secs(60));
    ticker.tick().await; // first tick immediately
    loop {
        ticker.tick().await;
        // sweeper 是同步 SQLite 工作，放到 blocking 池，避免占用 async worker。
        let db2 = db.clone();
        let cfg2 = cfg.clone();
        let hub2 = hub.clone();
        match tokio::task::spawn_blocking(move || sweep_once(&db2, &cfg2)).await {
            Ok(Ok((codes, closed_rooms))) => {
                // 过期/删除的房间立即断开在线 WS，避免继续产生 typing/signal 等事件。
                for room_id in closed_rooms {
                    hub2.close_room(&room_id);
                }
                for code in codes {
                    revocation::schedule_room(cfg.clone(), revocation_metrics.clone(), code);
                }
            }
            Ok(Err(e)) => tracing::error!(error = %e.message, "sweep failed"),
            Err(e) => tracing::error!(error = %e, "sweep task failed"),
        }
    }
}

pub fn sweep_once(db: &Db, cfg: &Config) -> ApiResult<(Vec<String>, Vec<String>)> {
    let now = Utc::now();
    let (revoked, files, closed_rooms) = db.with_sync(|conn| {
        let mut revoked = Vec::new();
        let mut closed_rooms: Vec<String> = Vec::new();
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
            closed_rooms.push(room_id);
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
            closed_rooms.push(room_id);
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
            closed_rooms.push(room_id);
        }
        // 4. Expired sessions.
        let n = crate::infra::repo::delete_expired_sessions(conn, &now)?;
        if n > 0 {
            tracing::debug!(sessions = n, "expired sessions removed");
        }
        Ok((revoked, purged_files, closed_rooms))
    })?;
    // 文件删除放在数据库锁之外。
    if !files.is_empty() {
        let dir = cfg.data.dir.join("attachments");
        for id in &files {
            let _ = std::fs::remove_file(dir.join(id));
        }
        tracing::info!(files = files.len(), "attachment files removed");
    }
    // 5. 孤儿附件文件：磁盘上存在但数据库无记录（只处理 24h 前的文件，
    //    避免把正在写入的上传误删）。数据库不可读时跳过，宁可少删。
    let dir = cfg.data.dir.join("attachments");
    // 一次取全部附件 ID：避免每个文件一次查询（文件多时每分钟一次长阻塞）。
    // 读库失败时跳过本轮清理（宁可少删，不能误删）。
    let known: Option<std::collections::HashSet<String>> = db
        .with_sync(crate::infra::repo::all_attachment_ids)
        .ok()
        .map(|ids| ids.into_iter().collect());
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut orphans = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let old = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .map(|d| d.as_secs() > 24 * 3600)
                .unwrap_or(false);
            if !old {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(known) = &known else {
                continue;
            };
            if !known.contains(name) {
                orphans.push(path);
            }
        }
        for path in &orphans {
            let _ = std::fs::remove_file(path);
        }
        if !orphans.is_empty() {
            tracing::info!(files = orphans.len(), "orphan attachment files removed");
        }
    }
    Ok((revoked, closed_rooms))
}
