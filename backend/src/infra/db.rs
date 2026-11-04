//! SQLite connection management: pragmas, single-writer Mutex, migrations.

use parking_lot::Mutex;
use std::path::Path;
use std::sync::Arc;

use rusqlite::Connection;

use crate::error::{ApiError, ApiResult};

/// Migrations run in order; each entry must be idempotent (guarded by the
/// schema_migrations table, so entries run exactly once).
pub const MIGRATIONS: &[(&str, &str)] = &[
    (
        "2026-08-05-0001-initial",
        r#"
CREATE TABLE users (
  user_id        TEXT PRIMARY KEY,
  username       TEXT UNIQUE,
  password_hash  TEXT,
  role           TEXT NOT NULL CHECK (role IN ('super_admin','admin','user','guest')),
  avatar_id      TEXT,
  disabled       INTEGER NOT NULL DEFAULT 0,
  created_at     TEXT NOT NULL,
  last_online_at TEXT NOT NULL
);

CREATE TABLE sessions (
  session_id   TEXT PRIMARY KEY,
  user_id      TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
  token_hash   TEXT NOT NULL UNIQUE,
  csrf_token   TEXT NOT NULL,
  expires_at   TEXT NOT NULL,
  created_at   TEXT NOT NULL,
  last_seen_at TEXT NOT NULL,
  ip           TEXT,
  user_agent   TEXT
);
CREATE INDEX idx_sessions_user ON sessions(user_id);

CREATE TABLE bootstrap_tokens (
  token_hash TEXT PRIMARY KEY,
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  consumed_at TEXT
);

CREATE TABLE rooms (
  room_id               TEXT PRIMARY KEY,
  room_code             TEXT NOT NULL UNIQUE,
  kind                  TEXT NOT NULL CHECK (kind IN ('standard','direct')),
  owner_user_id         TEXT REFERENCES users(user_id),
  parent_room_id        TEXT REFERENCES rooms(room_id),
  state                 TEXT NOT NULL CHECK (state IN ('active','expired','deleted')),
  created_at            TEXT NOT NULL,
  expires_at            TEXT,
  deleted_at            TEXT,
  allow_direct_messages INTEGER NOT NULL DEFAULT 1,
  text_enabled          INTEGER NOT NULL DEFAULT 1,
  media_mode            TEXT NOT NULL DEFAULT 'auto' CHECK (media_mode IN ('auto','p2p','sfu')),
  last_seq              INTEGER NOT NULL DEFAULT 0,
  version               INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_rooms_state ON rooms(state);
CREATE INDEX idx_rooms_parent ON rooms(parent_room_id);
CREATE INDEX idx_rooms_owner ON rooms(owner_user_id);

CREATE TABLE room_code_tombstones (
  code       TEXT PRIMARY KEY,
  created_at TEXT NOT NULL
);

CREATE TABLE direct_pairs (
  room_id        TEXT PRIMARY KEY REFERENCES rooms(room_id) ON DELETE CASCADE,
  parent_room_id TEXT NOT NULL REFERENCES rooms(room_id),
  user_a         TEXT NOT NULL REFERENCES users(user_id),
  user_b         TEXT NOT NULL REFERENCES users(user_id),
  UNIQUE (parent_room_id, user_a, user_b)
);
CREATE INDEX idx_direct_pairs_a ON direct_pairs(user_a);
CREATE INDEX idx_direct_pairs_b ON direct_pairs(user_b);

CREATE TABLE room_members (
  room_id        TEXT NOT NULL REFERENCES rooms(room_id) ON DELETE CASCADE,
  user_id        TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
  room_role      TEXT NOT NULL CHECK (room_role IN ('owner','manager','member','guest')),
  nickname       TEXT NOT NULL,
  joined_at      TEXT NOT NULL,
  left_at        TEXT,
  last_online_at TEXT,
  PRIMARY KEY (room_id, user_id)
);
CREATE INDEX idx_members_user ON room_members(user_id);

CREATE TABLE mutes (
  mute_id        TEXT PRIMARY KEY,
  room_id        TEXT NOT NULL REFERENCES rooms(room_id) ON DELETE CASCADE,
  scope          TEXT NOT NULL CHECK (scope IN ('member','room')),
  target_user_id TEXT REFERENCES users(user_id),
  muted_by       TEXT NOT NULL REFERENCES users(user_id),
  created_at     TEXT NOT NULL,
  expires_at     TEXT
);
CREATE INDEX idx_mutes_room ON mutes(room_id);

CREATE TABLE room_requests (
  request_id      TEXT PRIMARY KEY,
  requester_id    TEXT NOT NULL REFERENCES users(user_id),
  note            TEXT NOT NULL DEFAULT '',
  desired_code    TEXT,
  status          TEXT NOT NULL CHECK (status IN ('pending','approved','rejected','cancelled')),
  created_at      TEXT NOT NULL,
  decided_at      TEXT,
  decided_by      TEXT REFERENCES users(user_id),
  decision_reason TEXT,
  created_room_id TEXT REFERENCES rooms(room_id)
);
CREATE INDEX idx_requests_status ON room_requests(status);
CREATE INDEX idx_requests_requester ON room_requests(requester_id);

CREATE TABLE invites (
  invite_id   TEXT PRIMARY KEY,
  kind        TEXT NOT NULL CHECK (kind IN ('room','registration')),
  token_hash  TEXT NOT NULL UNIQUE,
  room_id     TEXT REFERENCES rooms(room_id) ON DELETE CASCADE,
  created_by  TEXT NOT NULL REFERENCES users(user_id),
  max_role    TEXT NOT NULL DEFAULT 'guest',
  created_at  TEXT NOT NULL,
  expires_at  TEXT NOT NULL,
  consumed_at TEXT,
  consumed_by TEXT REFERENCES users(user_id),
  revoked_at  TEXT
);
CREATE INDEX idx_invites_room ON invites(room_id);
CREATE INDEX idx_invites_kind ON invites(kind);

CREATE TABLE messages (
  message_id     TEXT PRIMARY KEY,
  room_id        TEXT NOT NULL REFERENCES rooms(room_id) ON DELETE CASCADE,
  seq            INTEGER NOT NULL,
  sender_user_id TEXT NOT NULL REFERENCES users(user_id),
  body           TEXT NOT NULL,
  created_at     TEXT NOT NULL,
  UNIQUE (room_id, seq)
);
CREATE INDEX idx_messages_room_seq ON messages(room_id, seq);

CREATE TABLE audit_log (
  entry_id     INTEGER PRIMARY KEY AUTOINCREMENT,
  actor_user_id TEXT REFERENCES users(user_id),
  action       TEXT NOT NULL,
  target_type  TEXT,
  target_id    TEXT,
  detail       TEXT NOT NULL DEFAULT '{}',
  created_at   TEXT NOT NULL
);
CREATE INDEX idx_audit_created ON audit_log(created_at);
"#,
    ),
    (
        "2026-08-07-0002-simplify-room-roles",
        r#"
-- 房间角色精简：只保留 owner / member（manager、guest 均转为 member）。
-- SQLite 无法直接修改 CHECK 约束，重建表并迁移数据。
CREATE TABLE room_members_new (
  room_id        TEXT NOT NULL REFERENCES rooms(room_id) ON DELETE CASCADE,
  user_id        TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
  room_role      TEXT NOT NULL CHECK (room_role IN ('owner','member')),
  nickname       TEXT NOT NULL,
  joined_at      TEXT NOT NULL,
  left_at        TEXT,
  last_online_at TEXT,
  PRIMARY KEY (room_id, user_id)
);
INSERT INTO room_members_new (room_id, user_id, room_role, nickname, joined_at, left_at, last_online_at)
  SELECT room_id, user_id,
         CASE WHEN room_role = 'owner' THEN 'owner' ELSE 'member' END,
         nickname, joined_at, left_at, last_online_at
  FROM room_members;
DROP TABLE room_members;
ALTER TABLE room_members_new RENAME TO room_members;
CREATE INDEX idx_members_user ON room_members(user_id);
"#,
    ),
    (
        "2026-08-07-0003-nullable-message-sender",
        r#"
-- 系统播报消息（成员进出）不需要真实发送者，sender_user_id 改为可空。
CREATE TABLE messages_new (
  message_id     TEXT PRIMARY KEY,
  room_id        TEXT NOT NULL REFERENCES rooms(room_id) ON DELETE CASCADE,
  seq            INTEGER NOT NULL,
  sender_user_id TEXT REFERENCES users(user_id),
  body           TEXT NOT NULL,
  created_at     TEXT NOT NULL,
  UNIQUE (room_id, seq)
);
INSERT INTO messages_new (message_id, room_id, seq, sender_user_id, body, created_at)
  SELECT message_id, room_id, seq, sender_user_id, body, created_at FROM messages;
DROP TABLE messages;
ALTER TABLE messages_new RENAME TO messages;
CREATE INDEX idx_messages_room_seq ON messages(room_id, seq);
"#,
    ),
    (
        "2026-08-08-0004-explicit-room-media-mode",
        r#"
-- 房间媒体模式显式化：创建房间必须选择 p2p/sfu，模式变更走申请审批通道。
-- 旧 auto 房间保守迁移为 sfu（避免已有多人房间突然触发 P2P 上限失败）。
UPDATE rooms SET media_mode = 'sfu' WHERE media_mode = 'auto';
-- room_requests 支持 create_room 与 change_media_mode 两种申请。
ALTER TABLE room_requests ADD COLUMN request_kind TEXT NOT NULL DEFAULT 'create_room'
  CHECK (request_kind IN ('create_room','change_media_mode'));
ALTER TABLE room_requests ADD COLUMN target_room_id TEXT REFERENCES rooms(room_id) ON DELETE SET NULL;
ALTER TABLE room_requests ADD COLUMN media_mode TEXT NOT NULL DEFAULT 'p2p'
  CHECK (media_mode IN ('p2p','sfu'));
-- 同一房间同时只允许一个 pending 的模式变更申请。
CREATE UNIQUE INDEX idx_pending_media_change
  ON room_requests(target_room_id)
  WHERE request_kind = 'change_media_mode' AND status = 'pending';
"#,
    ),
    (
        "2026-08-09-0005-audio-bitrate-and-room-mute",
        r#"
-- 音频码率（kbps，0=自动）；房间级禁言开关（仅房主可操作，用户无法自行解除）。
ALTER TABLE rooms ADD COLUMN audio_bitrate INTEGER NOT NULL DEFAULT 0;
ALTER TABLE rooms ADD COLUMN room_muted INTEGER NOT NULL DEFAULT 0;
-- 建房申请携带码率（change_media_mode 申请不带）。
ALTER TABLE room_requests ADD COLUMN audio_bitrate INTEGER NOT NULL DEFAULT 0;
-- 禁言状态下获准发言的用户（房主批准后加入，可单独取消）。
CREATE TABLE speak_grants (
  room_id    TEXT NOT NULL REFERENCES rooms(room_id) ON DELETE CASCADE,
  user_id    TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
  granted_at TEXT NOT NULL,
  granted_by TEXT NOT NULL REFERENCES users(user_id),
  PRIMARY KEY (room_id, user_id)
);
-- 待房主审批的发言申请（批准/拒绝后删除；房主离线时保留）。
CREATE TABLE speak_requests (
  request_id TEXT PRIMARY KEY,
  room_id    TEXT NOT NULL REFERENCES rooms(room_id) ON DELETE CASCADE,
  user_id    TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
  created_at TEXT NOT NULL,
  UNIQUE (room_id, user_id)
);
CREATE INDEX idx_speak_requests_room ON speak_requests(room_id);
-- 房间公告（房主设置，顶部横幅展示）。
ALTER TABLE rooms ADD COLUMN announcement TEXT NOT NULL DEFAULT '';
-- 房间成员上限（0=不限）。
ALTER TABLE rooms ADD COLUMN max_members INTEGER NOT NULL DEFAULT 0;
-- 消息回复引用（reply_snapshot 为原文快照，原消息删除后引用仍可显示）。
ALTER TABLE messages ADD COLUMN reply_to_id TEXT;
ALTER TABLE messages ADD COLUMN reply_snapshot TEXT;
-- 消息 emoji 回应（同一用户对同一消息同一表情只能一个）。
CREATE TABLE message_reactions (
  message_id TEXT NOT NULL REFERENCES messages(message_id) ON DELETE CASCADE,
  user_id    TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
  emoji      TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (message_id, user_id, emoji)
);
CREATE INDEX idx_reactions_message ON message_reactions(message_id);
"#,
    ),
    (
        "2026-10-21-0006-room-member-left-reason",
        r#"
-- 区分主动离开与房主移除：NULL = 活跃或迁移前的历史离开（按主动离开兼容），
-- 'left' = 主动离开，'removed' = 被房主移除。
ALTER TABLE room_members ADD COLUMN left_reason TEXT;
"#,
    ),
    (
        "2026-11-03-0007-release-room-codes",
        r#"
DROP TABLE IF EXISTS room_code_tombstones;
"#,
    ),
    (
        "2026-11-03-0008-message-edit-delete-attachments",
        r#"
ALTER TABLE messages ADD COLUMN edited_at TEXT;
ALTER TABLE messages ADD COLUMN deleted_at TEXT;
ALTER TABLE messages ADD COLUMN attachment_id TEXT;
ALTER TABLE rooms ADD COLUMN e2ee INTEGER NOT NULL DEFAULT 0;
ALTER TABLE rooms ADD COLUMN e2ee_key TEXT;
CREATE TABLE attachments (
  attachment_id TEXT PRIMARY KEY,
  room_id       TEXT NOT NULL REFERENCES rooms(room_id) ON DELETE CASCADE,
  uploader_id   TEXT NOT NULL REFERENCES users(user_id),
  filename      TEXT NOT NULL,
  mime          TEXT NOT NULL,
  size          INTEGER NOT NULL,
  created_at    TEXT NOT NULL
);
CREATE INDEX idx_attachments_room ON attachments(room_id);
"#,
    ),
    (
        "2026-11-03-0009-room-attachment-policy",
        r#"
ALTER TABLE rooms ADD COLUMN attachments_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE rooms ADD COLUMN attachment_bytes INTEGER NOT NULL DEFAULT 0;
ALTER TABLE room_requests ADD COLUMN attachments INTEGER NOT NULL DEFAULT 0;
"#,
    ),
    (
        "2026-11-03-0010-read-receipts",
        r#"
ALTER TABLE room_members ADD COLUMN last_read_seq INTEGER NOT NULL DEFAULT 0;
CREATE INDEX idx_members_room_read ON room_members(room_id, last_read_seq);
"#,
    ),
    (
        "2026-11-03-0011-push-subscriptions",
        r#"
CREATE TABLE push_subscriptions (
  subscription_id TEXT PRIMARY KEY,
  user_id         TEXT NOT NULL,
  endpoint        TEXT NOT NULL UNIQUE,
  p256dh          TEXT NOT NULL,
  auth            TEXT NOT NULL,
  created_at      TEXT NOT NULL
);
CREATE INDEX idx_push_user ON push_subscriptions(user_id);
"#,
    ),
    (
        "2026-11-03-0012-room-bans",
        r#"
CREATE TABLE room_bans (
  room_id   TEXT NOT NULL,
  user_id   TEXT NOT NULL,
  banned_at TEXT NOT NULL,
  banned_by TEXT,
  PRIMARY KEY (room_id, user_id)
);
"#,
    ),
    (
        "2026-11-04-0013-session-expiry-index",
        r#"
CREATE INDEX IF NOT EXISTS idx_sessions_expires ON sessions(expires_at);
"#,
    ),
];

#[derive(Clone)]
pub struct Db {
    inner: Arc<Mutex<Connection>>,
    /// 只读连接池：WAL 下读不阻塞写。池内连接不做写操作（写仍走 `inner`）。
    readers: Arc<Vec<Mutex<Connection>>>,
    next_reader: Arc<std::sync::atomic::AtomicUsize>,
    pub path: std::path::PathBuf,
}

/// 读连接数；读写混合负载下 4 个足够，且远低于 SQLite 文件句柄压力。
const READER_CONNECTIONS: usize = 4;

impl Db {
    pub fn open(path: &Path) -> ApiResult<Self> {
        let conn = Connection::open(path).map_err(|e| {
            ApiError::internal(format!("cannot open sqlite {}: {e}", path.display()))
        })?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| ApiError::internal(format!("cannot enable WAL: {e}")))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| ApiError::internal(format!("无法启用外键: {e}")))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| ApiError::internal(format!("cannot set busy timeout: {e}")))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| ApiError::internal(format!("无法设置同步模式: {e}")))?;
        // `:memory:` 库的每个连接都是独立数据库，不能建池；此时读走写连接。
        let is_memory = path.to_string_lossy().contains(":memory:");
        let mut readers = Vec::with_capacity(READER_CONNECTIONS);
        if !is_memory {
            for _ in 0..READER_CONNECTIONS {
                let r = Connection::open(path).map_err(|e| {
                    ApiError::internal(format!("cannot open sqlite reader {}: {e}", path.display()))
                })?;
                r.pragma_update(None, "foreign_keys", "ON")
                    .map_err(|e| ApiError::internal(format!("无法启用外键: {e}")))?;
                r.busy_timeout(std::time::Duration::from_secs(5))
                    .map_err(|e| ApiError::internal(format!("cannot set busy timeout: {e}")))?;
                readers.push(Mutex::new(r));
            }
        }
        Ok(Self {
            inner: Arc::new(Mutex::new(conn)),
            readers: Arc::new(readers),
            next_reader: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            path: path.to_path_buf(),
        })
    }

    pub fn migrate(&self) -> ApiResult<()> {
        let mut conn = self.inner.lock();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
               version TEXT PRIMARY KEY, applied_at TEXT NOT NULL);",
        )
        .map_err(|e| ApiError::internal(format!("cannot create schema_migrations: {e}")))?;
        for (version, sql) in MIGRATIONS {
            let applied: bool = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
                    [version],
                    |r| r.get(0),
                )
                .map_err(|e| ApiError::internal(format!("数据库迁移检查失败: {e}")))?;
            if !applied {
                let tx = conn
                    .transaction()
                    .map_err(|e| ApiError::internal(format!("数据库迁移事务失败: {e}")))?;
                tx.execute_batch(sql)
                    .map_err(|e| ApiError::internal(format!("migration {version} failed: {e}")))?;
                tx.execute(
                    "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                    rusqlite::params![version, crate::util::now_rfc3339()],
                )
                .map_err(|e| ApiError::internal(format!("数据库迁移记录失败: {e}")))?;
                tx.commit()
                    .map_err(|e| ApiError::internal(format!("migration commit failed: {e}")))?;
                tracing::info!(version, "applied migration");
            }
        }
        Ok(())
    }

    /// Run a blocking DB closure on the worker pool. Serializes all writers
    /// through the single Mutex (SQLite single-writer contract).
    /// The closure runs inside a SAVEPOINT: any ApiError rolls back every
    /// statement executed so far (multi-statement handlers stay atomic).
    pub async fn with<T, F>(&self, f: F) -> ApiResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> ApiResult<T> + Send + 'static,
    {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = inner.lock();
            run_in_tx(&mut conn, f)
        })
        .await
        .map_err(|e| ApiError::internal(format!("数据库任务异常: {e}")))?
    }

    /// Synchronous variant used by the sweeper task.
    pub fn with_sync<T, F>(&self, f: F) -> ApiResult<T>
    where
        F: FnOnce(&mut Connection) -> ApiResult<T>,
    {
        let mut conn = self.inner.lock();
        run_in_tx(&mut conn, f)
    }

    /// 只读查询：轮询取一个池连接，不占用写连接。
    /// 闭包内不得执行写语句（写路径一律走 `with` / `with_sync`）。
    pub async fn read<T, F>(&self, f: F) -> ApiResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> ApiResult<T> + Send + 'static,
    {
        if self.readers.is_empty() {
            return self.with(f).await;
        }
        let idx = self.pick_reader();
        let readers = self.readers.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = readers[idx].lock();
            run_in_tx(&mut conn, f)
        })
        .await
        .map_err(|e| ApiError::internal(format!("数据库任务异常: {e}")))?
    }

    /// 同步版只读查询（后台任务用）。
    pub fn read_sync<T, F>(&self, f: F) -> ApiResult<T>
    where
        F: FnOnce(&mut Connection) -> ApiResult<T>,
    {
        if self.readers.is_empty() {
            return self.with_sync(f);
        }
        let idx = self.pick_reader();
        let mut conn = self.readers[idx].lock();
        run_in_tx(&mut conn, f)
    }

    fn pick_reader(&self) -> usize {
        self.next_reader
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            % self.readers.len().max(1)
    }
}

/// Execute a closure inside a SAVEPOINT so a failing statement rolls back
/// earlier ones. Uses SAVEPOINT (not BEGIN) so nested calls are safe.
fn run_in_tx<T, F>(conn: &mut Connection, f: F) -> ApiResult<T>
where
    F: FnOnce(&mut Connection) -> ApiResult<T>,
{
    conn.execute_batch("SAVEPOINT api_tx")
        .map_err(|e| ApiError::internal(format!("db tx begin failed: {e}")))?;
    match f(conn) {
        Ok(v) => {
            conn.execute_batch("RELEASE api_tx")
                .map_err(|e| ApiError::internal(format!("数据库事务提交失败: {e}")))?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO api_tx; RELEASE api_tx");
            Err(e)
        }
    }
}
