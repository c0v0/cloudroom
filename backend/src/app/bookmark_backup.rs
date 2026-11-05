//! Periodic bookmark snapshots (JSON) for users who opt in.
//!
//! One file per user, atomically replaced. The scheduler runs at startup and
//! every six hours; each user is backed up at most once per seven days.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::error::{ApiError, ApiResult};
use crate::infra::db::Db;

const BACKUP_INTERVAL_DAYS: i64 = 7;
const SWEEP_SECONDS: u64 = 6 * 3600;

pub async fn run(db: Db, dir: PathBuf) {
    loop {
        sweep_once(&db, &dir).await;
        tokio::time::sleep(std::time::Duration::from_secs(SWEEP_SECONDS)).await;
    }
}

async fn sweep_once(db: &Db, dir: &Path) {
    let cutoff = (chrono::Utc::now() - chrono::Duration::days(BACKUP_INTERVAL_DAYS)).to_rfc3339();
    let users = match db
        .read(move |conn| crate::infra::repo::due_bookmark_backup_users(conn, &cutoff))
        .await
    {
        Ok(users) => users,
        Err(e) => {
            tracing::warn!(error = %e.message, "bookmark backup: cannot list due users");
            return;
        }
    };
    for uid in users {
        if let Err(e) = backup_user(db, dir, &uid).await {
            tracing::warn!(user_id = %uid, error = %e.message, "bookmark backup failed");
        }
    }
}

/// Write a snapshot now (used by the scheduler and by enabling the feature).
pub async fn backup_user(db: &Db, dir: &Path, user_id: &str) -> ApiResult<PathBuf> {
    let uid = user_id.to_string();
    let rows = db
        .read(move |conn| crate::infra::repo::list_bookmarks(conn, &uid, 10_000, None))
        .await?;
    let bookmarks: Vec<serde_json::Value> = rows
        .iter()
        .map(|b| {
            json!({
                "message_id": b.message_id,
                "room_code": b.room_code,
                "seq": b.seq,
                "body": b.body,
                "created_at": b.created_at,
                "sender_username": b.sender_username,
                "sender_nickname": b.sender_nickname,
            })
        })
        .collect();
    let payload = serde_json::to_vec(&json!({
        "exported_at": crate::util::now_rfc3339(),
        "bookmarks": bookmarks,
    }))
    .map_err(|e| ApiError::internal(format!("cannot serialize bookmark backup: {e}")))?;

    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| ApiError::internal(format!("cannot create bookmark backup dir: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let path = dir.join(format!("{user_id}.json"));
    let tmp = dir.join(format!(".{user_id}.tmp"));
    tokio::fs::write(&tmp, payload)
        .await
        .map_err(|e| ApiError::internal(format!("cannot write bookmark backup: {e}")))?;
    tokio::fs::rename(&tmp, &path)
        .await
        .map_err(|e| ApiError::internal(format!("cannot finalize bookmark backup: {e}")))?;
    let uid = user_id.to_string();
    let at = crate::util::now_rfc3339();
    db.with(move |conn| crate::infra::repo::mark_bookmark_backup(conn, &uid, &at))
        .await?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::{Message, Room, RoomKind, RoomState};
    use crate::infra::repo;
    use crate::util;

    #[tokio::test]
    async fn snapshot_contains_bookmarks_and_updates_last_backup() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open(&dir.path().join("db.sqlite")).unwrap();
        db.migrate().unwrap();
        let uid = "user-1";
        db.with_sync(|conn| {
            repo::create_user(conn, uid, Some("u"), None, crate::domain::models::UserRole::User, None)?;
            let now = util::now();
            repo::insert_room(
                conn,
                &Room {
                    room_id: "room-1".into(),
                    room_code: "room".into(),
                    kind: RoomKind::Standard,
                    owner_user_id: Some(uid.into()),
                    parent_room_id: None,
                    state: RoomState::Active,
                    created_at: now,
                    expires_at: None,
                    deleted_at: None,
                    allow_direct_messages: true,
                    text_enabled: true,
                    media_mode: crate::domain::models::MediaMode::P2p,
                    audio_bitrate: 0,
                    room_muted: false,
                    announcement: String::new(),
                    max_members: 0,
                    last_seq: 1,
                    version: 1,
                    e2ee: false,
                    e2ee_key: None,
                    attachments_enabled: false,
                    attachment_bytes: 0,
                    guest_upgrade_open: false,
                },
            )?;
            repo::add_member(conn, "room-1", uid, crate::domain::models::RoomRole::Owner, "u")?;
            repo::insert_message(
                conn,
                &Message {
                    message_id: "m1".into(),
                    room_id: "room-1".into(),
                    seq: 1,
                    sender_user_id: Some(uid.into()),
                    body: "hello".into(),
                    reply_snapshot: None,
                    created_at: now,
                    edited_at: None,
                    deleted_at: None,
                    attachment_id: None,
                    thread_root_id: None,
                    forwarded_from: None,
                },
            )?;
            repo::add_bookmark(conn, uid, "m1")
        })
        .unwrap();

        let out = dir.path().join("backups");
        backup_user(&db, &out, uid).await.unwrap();
        let text = std::fs::read_to_string(out.join(format!("{uid}.json"))).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["bookmarks"][0]["message_id"], "m1", "{v}");
        let (_, last) = db
            .with_sync(|conn| repo::bookmark_backup_status(conn, uid))
            .unwrap();
        assert!(last.is_some());
    }
}
