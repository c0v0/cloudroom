//! Periodic encrypted bookmark snapshots for users who opt in.
//!
//! One AES-256-GCM key per data directory (`bookmark-backups/.backup-key`,
//! mode 0600), up to eight versioned snapshots per user. The scheduler runs at
//! startup and every six hours; each user is backed up at most once per week.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::json;

use crate::error::{ApiError, ApiResult};
use crate::infra::db::Db;

const BACKUP_INTERVAL_DAYS: i64 = 7;
const SWEEP_SECONDS: u64 = 6 * 3600;
const KEEP_VERSIONS: usize = 8;
const MAGIC: &[u8; 4] = b"CRB1";
const KEY_FILE: &str = ".backup-key";

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

/// Latest encrypted (or legacy plaintext) snapshot for a user.
pub fn latest_backup_path(dir: &Path, user_id: &str) -> Option<PathBuf> {
    let prefix = format!("{user_id}-");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".json.enc"))
        })
        .collect();
    versions.sort();
    versions.pop().or_else(|| {
        let legacy = dir.join(format!("{user_id}.json"));
        legacy.is_file().then_some(legacy)
    })
}

/// Read and decrypt the latest snapshot; legacy plaintext files still work.
pub fn read_latest_backup(dir: &Path, user_id: &str) -> ApiResult<Vec<u8>> {
    let path = latest_backup_path(dir, user_id)
        .ok_or_else(|| ApiError::not_found("还没有备份，请先开启自动备份"))?;
    let bytes = std::fs::read(&path)
        .map_err(|_| ApiError::not_found("备份文件不可读"))?;
    if path.extension().and_then(|e| e.to_str()) == Some("enc") {
        decrypt(&load_or_create_key(dir)?, &bytes)
    } else {
        Ok(bytes)
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

    std::fs::create_dir_all(dir)
        .map_err(|e| ApiError::internal(format!("cannot create bookmark backup dir: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let key = load_or_create_key(dir)?;
    let encrypted = encrypt(&key, &payload)?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ");
    let path = dir.join(format!("{user_id}-{stamp}.json.enc"));
    let tmp = dir.join(format!(".{user_id}-{stamp}.tmp"));
    std::fs::write(&tmp, encrypted)
        .map_err(|e| ApiError::internal(format!("cannot write bookmark backup: {e}")))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| ApiError::internal(format!("cannot finalize bookmark backup: {e}")))?;
    prune_versions(dir, user_id);
    let uid = user_id.to_string();
    let at = crate::util::now_rfc3339();
    db.with(move |conn| crate::infra::repo::mark_bookmark_backup(conn, &uid, &at))
        .await?;
    Ok(path)
}

fn prune_versions(dir: &Path, user_id: &str) {
    let prefix = format!("{user_id}-");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".json.enc"))
        })
        .collect();
    versions.sort();
    let remove = versions.len().saturating_sub(KEEP_VERSIONS);
    for path in versions.into_iter().take(remove) {
        let _ = std::fs::remove_file(path);
    }
}

fn key_path(dir: &Path) -> PathBuf {
    dir.join(KEY_FILE)
}

fn load_or_create_key(dir: &Path) -> ApiResult<[u8; 32]> {
    let path = key_path(dir);
    let mut key = [0u8; 32];
    match std::fs::File::open(&path) {
        Ok(mut file) => {
            file.read_exact(&mut key)
                .map_err(|_| ApiError::internal("backup key is corrupted"))?;
            Ok(key)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            SystemRandom::new()
                .fill(&mut key)
                .map_err(|_| ApiError::internal("cannot generate backup key"))?;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(mut file) => {
                    file.write_all(&key)
                        .map_err(|_| ApiError::internal("cannot write backup key"))?;
                    Ok(key)
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let mut file = std::fs::File::open(&path)
                        .map_err(|_| ApiError::internal("cannot read backup key"))?;
                    file.read_exact(&mut key)
                        .map_err(|_| ApiError::internal("backup key is corrupted"))?;
                    Ok(key)
                }
                Err(e) => Err(ApiError::internal(format!("cannot create backup key: {e}"))),
            }
        }
        Err(e) => Err(ApiError::internal(format!("cannot read backup key: {e}"))),
    }
}

fn cipher(key: &[u8; 32]) -> ApiResult<LessSafeKey> {
    let unbound = UnboundKey::new(&AES_256_GCM, key)
        .map_err(|_| ApiError::internal("cannot initialize backup cipher"))?;
    Ok(LessSafeKey::new(unbound))
}

fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> ApiResult<Vec<u8>> {
    let mut nonce_bytes = [0u8; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce_bytes)
        .map_err(|_| ApiError::internal("cannot generate backup nonce"))?;
    let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + plaintext.len() + 16);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce_bytes);
    let mut body = plaintext.to_vec();
    cipher(key)?
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce_bytes),
            Aad::empty(),
            &mut body,
        )
        .map_err(|_| ApiError::internal("cannot encrypt backup"))?;
    out.extend_from_slice(&body);
    Ok(out)
}

fn decrypt(key: &[u8; 32], data: &[u8]) -> ApiResult<Vec<u8>> {
    if data.len() < MAGIC.len() + NONCE_LEN + 16 || &data[..MAGIC.len()] != MAGIC {
        return Err(ApiError::internal("backup format is invalid"));
    }
    let mut nonce_bytes = [0u8; NONCE_LEN];
    nonce_bytes.copy_from_slice(&data[MAGIC.len()..MAGIC.len() + NONCE_LEN]);
    let mut ciphertext = data[MAGIC.len() + NONCE_LEN..].to_vec();
    let plaintext = cipher(key)?
        .open_in_place(
            Nonce::assume_unique_for_key(nonce_bytes),
            Aad::empty(),
            &mut ciphertext,
        )
        .map_err(|_| ApiError::internal("cannot decrypt backup"))?;
    Ok(plaintext.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::{Message, Room, RoomKind, RoomRole, RoomState, UserRole};
    use crate::infra::repo;
    use crate::util;

    #[tokio::test]
    async fn encrypted_snapshots_are_versioned_and_readable() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open(&dir.path().join("db.sqlite")).unwrap();
        db.migrate().unwrap();
        let uid = "user-1";
        db.with_sync(|conn| {
            repo::create_user(conn, uid, Some("u"), None, UserRole::User, None)?;
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
            repo::add_member(conn, "room-1", uid, RoomRole::Owner, "u")?;
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
        let path = backup_user(&db, &out, uid).await.unwrap();
        assert!(path.extension().and_then(|e| e.to_str()) == Some("enc"));
        assert!(key_path(&out).is_file());
        let raw = std::fs::read(&path).unwrap();
        assert!(raw.starts_with(MAGIC));

        let plain = read_latest_backup(&out, uid).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&plain).unwrap();
        assert_eq!(v["bookmarks"][0]["message_id"], "m1", "{v}");

        backup_user(&db, &out, uid).await.unwrap();
        let count = std::fs::read_dir(&out)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".json.enc"))
            .count();
        assert!(count >= 1 && count <= KEEP_VERSIONS);
        let (_, last) = db
            .with_sync(|conn| repo::bookmark_backup_status(conn, uid))
            .unwrap();
        assert!(last.is_some());
    }
}
