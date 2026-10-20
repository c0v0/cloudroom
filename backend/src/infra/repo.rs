//! SQLite repository: the only place SQL lives. Handlers and services depend
//! on these functions, never on raw SQL. A future MySQL adapter implements
//! the same operations without duplicating domain logic.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};

use crate::domain::models::*;
use crate::error::{ApiError, ApiResult};
use crate::util;

// ---------------------------------------------------------------- users

pub fn get_user(conn: &mut Connection, user_id: &str) -> ApiResult<Option<User>> {
    conn.query_row(
        "SELECT user_id, username, password_hash, role, avatar_id, disabled, created_at, last_online_at
         FROM users WHERE user_id = ?1",
        [user_id],
        row_to_user,
    )
    .optional()
    .map_err(ApiError::from)
}

pub fn get_user_by_username(conn: &mut Connection, username: &str) -> ApiResult<Option<User>> {
    conn.query_row(
        "SELECT user_id, username, password_hash, role, avatar_id, disabled, created_at, last_online_at
         FROM users WHERE username = ?1",
        [username],
        row_to_user,
    )
    .optional()
    .map_err(ApiError::from)
}

fn row_to_user(row: &rusqlite::Row) -> rusqlite::Result<User> {
    let role_str: String = row.get(3)?;
    Ok(User {
        user_id: row.get(0)?,
        username: row.get(1)?,
        password_hash: row.get(2)?,
        role: UserRole::from_str(&role_str).unwrap_or(UserRole::User),
        avatar_id: row.get(4)?,
        disabled: row.get::<_, i64>(5)? != 0,
        created_at: parse_ts(&row.get::<_, String>(6)?),
        last_online_at: parse_ts(&row.get::<_, String>(7)?),
    })
}

pub fn create_user(
    conn: &mut Connection,
    user_id: &str,
    username: Option<&str>,
    password_hash: Option<&str>,
    role: UserRole,
) -> ApiResult<()> {
    let now = util::now_rfc3339();
    conn.execute(
        "INSERT INTO users (user_id, username, password_hash, role, created_at, last_online_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![user_id, username, password_hash, role.as_str(), now],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

pub fn set_user_role(conn: &mut Connection, user_id: &str, role: UserRole) -> ApiResult<()> {
    conn.execute(
        "UPDATE users SET role = ?1 WHERE user_id = ?2",
        params![role.as_str(), user_id],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

pub fn set_user_disabled(conn: &mut Connection, user_id: &str, disabled: bool) -> ApiResult<()> {
    conn.execute(
        "UPDATE users SET disabled = ?1 WHERE user_id = ?2",
        params![disabled as i64, user_id],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

pub fn set_user_password(
    conn: &mut Connection,
    user_id: &str,
    password_hash: &str,
) -> ApiResult<()> {
    conn.execute(
        "UPDATE users SET password_hash = ?1 WHERE user_id = ?2",
        params![password_hash, user_id],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

pub fn set_user_avatar(
    conn: &mut Connection,
    user_id: &str,
    avatar_id: Option<&str>,
) -> ApiResult<()> {
    conn.execute(
        "UPDATE users SET avatar_id = ?1 WHERE user_id = ?2",
        params![avatar_id, user_id],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

pub fn touch_user_online(
    conn: &mut Connection,
    user_id: &str,
    at: &DateTime<Utc>,
) -> ApiResult<()> {
    conn.execute(
        "UPDATE users SET last_online_at = ?1 WHERE user_id = ?2",
        params![at.to_rfc3339(), user_id],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

pub fn super_admin_exists(conn: &mut Connection) -> ApiResult<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM users WHERE role = 'super_admin')",
        [],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .map_err(ApiError::from)
}

pub fn count_users(conn: &mut Connection) -> ApiResult<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM users WHERE role != 'guest'",
        [],
        |r| r.get(0),
    )
    .map_err(ApiError::from)
}

pub fn list_users(
    conn: &mut Connection,
    search: Option<&str>,
    offset: i64,
    limit: i64,
) -> ApiResult<(Vec<User>, i64)> {
    let like = format!("%{}%", search.unwrap_or(""));
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM users WHERE username IS NOT NULL AND (username LIKE ?1 OR user_id LIKE ?1)",
            [&like],
            |r| r.get(0),
        )
        .map_err(ApiError::from)?;
    let mut stmt = conn
        .prepare(
            "SELECT user_id, username, password_hash, role, avatar_id, disabled, created_at, last_online_at
             FROM users WHERE username IS NOT NULL AND (username LIKE ?1 OR user_id LIKE ?1)
             ORDER BY created_at DESC LIMIT ?2 OFFSET ?3",
        )
        .map_err(ApiError::from)?;
    let rows = stmt
        .query_map(params![like, limit, offset], row_to_user)
        .map_err(ApiError::from)?;
    let mut users = Vec::new();
    for r in rows {
        users.push(r.map_err(ApiError::from)?);
    }
    Ok((users, total))
}

// -------------------------------------------------------------- sessions

pub fn create_session(conn: &mut Connection, session: &Session) -> ApiResult<()> {
    conn.execute(
        "INSERT INTO sessions (session_id, user_id, token_hash, csrf_token, expires_at, created_at, last_seen_at, ip, user_agent)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?5, ?6, ?7)",
        params![
            session.session_id,
            session.user_id,
            session.token_hash,
            session.csrf_token,
            session.expires_at.to_rfc3339(),
            session.ip,
            session.user_agent,
        ],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

pub fn get_session_by_token_hash(
    conn: &mut Connection,
    token_hash: &str,
) -> ApiResult<Option<Session>> {
    conn.query_row(
        "SELECT session_id, user_id, token_hash, csrf_token, expires_at, created_at, last_seen_at, ip, user_agent
         FROM sessions WHERE token_hash = ?1",
        [token_hash],
        |row| {
            Ok(Session {
                session_id: row.get(0)?,
                user_id: row.get(1)?,
                token_hash: row.get(2)?,
                csrf_token: row.get(3)?,
                expires_at: parse_ts(&row.get::<_, String>(4)?),
                created_at: parse_ts(&row.get::<_, String>(5)?),
                last_seen_at: parse_ts(&row.get::<_, String>(6)?),
                ip: row.get(7)?,
                user_agent: row.get(8)?,
            })
        },
    )
    .optional()
    .map_err(ApiError::from)
}

pub fn delete_session(conn: &mut Connection, session_id: &str) -> ApiResult<()> {
    conn.execute("DELETE FROM sessions WHERE session_id = ?1", [session_id])
        .map_err(ApiError::from)?;
    Ok(())
}

pub fn delete_sessions_for_user(conn: &mut Connection, user_id: &str) -> ApiResult<()> {
    conn.execute("DELETE FROM sessions WHERE user_id = ?1", [user_id])
        .map_err(ApiError::from)?;
    Ok(())
}

pub fn delete_expired_sessions(conn: &mut Connection, now: &DateTime<Utc>) -> ApiResult<usize> {
    conn.execute(
        "DELETE FROM sessions WHERE expires_at < ?1",
        params![now.to_rfc3339()],
    )
    .map_err(ApiError::from)
}

// ------------------------------------------------------- bootstrap tokens

pub fn insert_bootstrap_token(
    conn: &mut Connection,
    token_hash: &str,
    expires_at: &DateTime<Utc>,
) -> ApiResult<()> {
    conn.execute(
        "INSERT INTO bootstrap_tokens (token_hash, created_at, expires_at) VALUES (?1, ?2, ?3)",
        params![token_hash, util::now_rfc3339(), expires_at.to_rfc3339()],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

pub fn bootstrap_token_available(
    conn: &mut Connection,
    token_hash: &str,
    now: &DateTime<Utc>,
) -> ApiResult<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM bootstrap_tokens WHERE token_hash = ?1 AND consumed_at IS NULL AND expires_at > ?2)",
        params![token_hash, now.to_rfc3339()],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .map_err(ApiError::from)
}

/// Atomically consume a bootstrap token; returns true when this call won it.
pub fn consume_bootstrap_token(conn: &mut Connection, token_hash: &str) -> ApiResult<bool> {
    let n = conn
        .execute(
            "UPDATE bootstrap_tokens SET consumed_at = ?1 WHERE token_hash = ?2 AND consumed_at IS NULL",
            params![util::now_rfc3339(), token_hash],
        )
        .map_err(ApiError::from)?;
    Ok(n == 1)
}

// ---------------------------------------------------------------- rooms

pub fn insert_room(conn: &mut Connection, room: &Room) -> ApiResult<()> {
    conn.execute(
        "INSERT INTO rooms (room_id, room_code, kind, owner_user_id, parent_room_id, state, created_at,
                            expires_at, deleted_at, allow_direct_messages, text_enabled, media_mode,
                            audio_bitrate, room_muted, announcement, max_members, last_seq, version)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            room.room_id,
            room.room_code,
            room.kind.as_str(),
            room.owner_user_id,
            room.parent_room_id,
            room.state.as_str(),
            room.created_at.to_rfc3339(),
            room.expires_at.map(|d| d.to_rfc3339()),
            room.deleted_at.map(|d| d.to_rfc3339()),
            room.allow_direct_messages as i64,
            room.text_enabled as i64,
            room.media_mode.as_str(),
            room.audio_bitrate as i64,
            room.room_muted as i64,
            room.announcement,
            room.max_members,
            room.last_seq,
            room.version,
        ],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

/// Single source of truth for room SELECT columns; `row_to_room` reads them by name.
pub const ROOM_COLUMNS: [&str; 18] = [
    "room_id",
    "room_code",
    "kind",
    "owner_user_id",
    "parent_room_id",
    "state",
    "created_at",
    "expires_at",
    "deleted_at",
    "allow_direct_messages",
    "text_enabled",
    "media_mode",
    "audio_bitrate",
    "room_muted",
    "announcement",
    "max_members",
    "last_seq",
    "version",
];

/// Render the shared room columns for a SELECT, optionally table-qualified.
pub fn room_cols(prefix: &str) -> String {
    ROOM_COLUMNS
        .iter()
        .map(|column| format!("{prefix}{column}"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn row_to_room(row: &rusqlite::Row) -> rusqlite::Result<Room> {
    let [room_id, room_code, kind, owner_user_id, parent_room_id, state, created_at, expires_at, deleted_at, allow_direct_messages, text_enabled, media_mode, audio_bitrate, room_muted, announcement, max_members, last_seq, version] =
        ROOM_COLUMNS;
    let kind_raw: String = row.get(kind)?;
    let state_raw: String = row.get(state)?;
    let media_mode_raw: String = row.get(media_mode)?;
    Ok(Room {
        room_id: row.get(room_id)?,
        room_code: row.get(room_code)?,
        kind: RoomKind::from_str(&kind_raw).unwrap_or(RoomKind::Standard),
        owner_user_id: row.get(owner_user_id)?,
        parent_room_id: row.get(parent_room_id)?,
        state: RoomState::from_str(&state_raw).unwrap_or(RoomState::Deleted),
        created_at: parse_ts(&row.get::<_, String>(created_at)?),
        expires_at: row
            .get::<_, Option<String>>(expires_at)?
            .map(|s| parse_ts(&s)),
        deleted_at: row
            .get::<_, Option<String>>(deleted_at)?
            .map(|s| parse_ts(&s)),
        allow_direct_messages: row.get::<_, i64>(allow_direct_messages)? != 0,
        text_enabled: row.get::<_, i64>(text_enabled)? != 0,
        media_mode: MediaMode::from_str(&media_mode_raw).unwrap_or(MediaMode::Sfu),
        audio_bitrate: row.get::<_, i64>(audio_bitrate)? as u32,
        room_muted: row.get::<_, i64>(room_muted)? != 0,
        announcement: row.get(announcement)?,
        max_members: row.get(max_members)?,
        last_seq: row.get(last_seq)?,
        version: row.get(version)?,
    })
}

pub fn get_room_by_code(conn: &mut Connection, code: &str) -> ApiResult<Option<Room>> {
    conn.query_row(
        &format!("SELECT {} FROM rooms WHERE room_code = ?1", room_cols("")),
        [code],
        row_to_room,
    )
    .optional()
    .map_err(ApiError::from)
}

/// 用户当前所在（未离开）的全部房间。
pub fn list_member_rooms(conn: &mut Connection, user_id: &str) -> ApiResult<Vec<Room>> {
    let sql = format!(
        "SELECT {} FROM room_members m JOIN rooms r ON r.room_id = m.room_id
         WHERE m.user_id = ?1 AND m.left_at IS NULL",
        room_cols("r.")
    );
    let mut stmt = conn.prepare(&sql).map_err(ApiError::from)?;
    let rows = stmt
        .query_map([user_id], row_to_room)
        .map_err(ApiError::from)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(ApiError::from)?);
    }
    Ok(out)
}

pub fn get_room_by_id(conn: &mut Connection, room_id: &str) -> ApiResult<Option<Room>> {
    conn.query_row(
        &format!("SELECT {} FROM rooms WHERE room_id = ?1", room_cols("")),
        [room_id],
        row_to_room,
    )
    .optional()
    .map_err(ApiError::from)
}

pub fn code_tombstoned(conn: &mut Connection, code: &str) -> ApiResult<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM room_code_tombstones WHERE code = ?1)",
        [code],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .map_err(ApiError::from)
}

pub fn insert_tombstone(conn: &mut Connection, code: &str) -> ApiResult<()> {
    conn.execute(
        "INSERT OR IGNORE INTO room_code_tombstones (code, created_at) VALUES (?1, ?2)",
        params![code, util::now_rfc3339()],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

/// Atomically claim a free code: insert tombstone if not present, else conflict.
pub fn claim_code(conn: &mut Connection, code: &str) -> ApiResult<bool> {
    let n = conn
        .execute(
            "INSERT OR IGNORE INTO room_code_tombstones (code, created_at) VALUES (?1, ?2)",
            params![code, util::now_rfc3339()],
        )
        .map_err(ApiError::from)?;
    Ok(n == 1)
}

pub fn update_room_state(
    conn: &mut Connection,
    room_id: &str,
    state: RoomState,
    deleted_at: Option<&DateTime<Utc>>,
) -> ApiResult<()> {
    conn.execute(
        "UPDATE rooms SET state = ?1, deleted_at = ?2, version = version + 1 WHERE room_id = ?3",
        params![state.as_str(), deleted_at.map(|d| d.to_rfc3339()), room_id],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

pub fn update_room_settings(
    conn: &mut Connection,
    room_id: &str,
    text_enabled: bool,
    allow_direct_messages: bool,
    expires_at: Option<&DateTime<Utc>>,
    audio_bitrate: u32,
    announcement: &str,
    max_members: i64,
) -> ApiResult<()> {
    conn.execute(
        "UPDATE rooms SET text_enabled = ?1, allow_direct_messages = ?2,
                          expires_at = ?3, audio_bitrate = ?4, announcement = ?5, max_members = ?6,
                          version = version + 1 WHERE room_id = ?7",
        params![
            text_enabled as i64,
            allow_direct_messages as i64,
            expires_at.map(|d| d.to_rfc3339()),
            audio_bitrate as i64,
            announcement,
            max_members,
            room_id,
        ],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

/// 房间当前活跃成员数。
pub fn count_active_members(conn: &mut Connection, room_id: &str) -> ApiResult<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM room_members WHERE room_id = ?1 AND left_at IS NULL",
        [room_id],
        |r| r.get(0),
    )
    .map_err(ApiError::from)
}

pub fn update_room_audio_bitrate(conn: &mut Connection, room_id: &str, kbps: u32) -> ApiResult<()> {
    conn.execute(
        "UPDATE rooms SET audio_bitrate = ?1, version = version + 1 WHERE room_id = ?2",
        params![kbps as i64, room_id],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

pub fn set_room_muted(conn: &mut Connection, room_id: &str, muted: bool) -> ApiResult<()> {
    conn.execute(
        "UPDATE rooms SET room_muted = ?1, version = version + 1 WHERE room_id = ?2",
        params![muted as i64, room_id],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

// ---------------------------------------------------------- speak rights

/// 授予发言权。返回是否产生新授权（已有授权时返回 false，调用方不再广播/插消息）。
pub fn grant_speak(
    conn: &mut Connection,
    room_id: &str,
    user_id: &str,
    by: &str,
) -> ApiResult<bool> {
    let exists = has_speak_grant(conn, room_id, user_id)?;
    if !exists {
        conn.execute(
            "INSERT OR REPLACE INTO speak_grants (room_id, user_id, granted_at, granted_by)
             VALUES (?1, ?2, ?3, ?4)",
            params![room_id, user_id, util::now_rfc3339(), by],
        )
        .map_err(ApiError::from)?;
    }
    conn.execute(
        "DELETE FROM speak_requests WHERE room_id = ?1 AND user_id = ?2",
        params![room_id, user_id],
    )
    .map_err(ApiError::from)?;
    Ok(!exists)
}

/// 取消发言权。返回是否确有删除（不存在时返回 false，调用方不再广播）。
pub fn revoke_speak(conn: &mut Connection, room_id: &str, user_id: &str) -> ApiResult<bool> {
    let n = conn
        .execute(
            "DELETE FROM speak_grants WHERE room_id = ?1 AND user_id = ?2",
            params![room_id, user_id],
        )
        .map_err(ApiError::from)?;
    Ok(n == 1)
}

pub fn has_speak_grant(conn: &mut Connection, room_id: &str, user_id: &str) -> ApiResult<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM speak_grants WHERE room_id = ?1 AND user_id = ?2)",
        [room_id, user_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .map_err(ApiError::from)
}

pub fn list_speak_grants(conn: &mut Connection, room_id: &str) -> ApiResult<Vec<(String, String)>> {
    let mut stmt = conn
        .prepare(
            "SELECT user_id, granted_at FROM speak_grants WHERE room_id = ?1 ORDER BY granted_at",
        )
        .map_err(ApiError::from)?;
    let rows = stmt
        .query_map([room_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(ApiError::from)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(ApiError::from)?;
    Ok(rows)
}

/// 提交发言申请。返回是否新增（重复申请返回 false，调用方不再通知房主）。
pub fn insert_speak_request(
    conn: &mut Connection,
    room_id: &str,
    user_id: &str,
) -> ApiResult<bool> {
    let n = conn
        .execute(
            "INSERT OR IGNORE INTO speak_requests (request_id, room_id, user_id, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![util::random_id(), room_id, user_id, util::now_rfc3339()],
        )
        .map_err(ApiError::from)?;
    Ok(n == 1)
}

pub fn list_speak_requests(
    conn: &mut Connection,
    room_id: &str,
) -> ApiResult<Vec<(String, String)>> {
    let mut stmt = conn
        .prepare(
            "SELECT user_id, created_at FROM speak_requests WHERE room_id = ?1 ORDER BY created_at",
        )
        .map_err(ApiError::from)?;
    let rows = stmt
        .query_map([room_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(ApiError::from)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(ApiError::from)?;
    Ok(rows)
}

/// 消息是否存在且属于该房间。
pub fn message_belongs_to_room(
    conn: &mut Connection,
    message_id: &str,
    room_id: &str,
) -> ApiResult<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM messages WHERE message_id = ?1 AND room_id = ?2)",
        [message_id, room_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .map_err(ApiError::from)
}

/// 切换 emoji 回应：返回是否新增（true=添加，false=移除）。
pub fn toggle_reaction(
    conn: &mut Connection,
    message_id: &str,
    user_id: &str,
    emoji: &str,
) -> ApiResult<bool> {
    let exists = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM message_reactions WHERE message_id = ?1 AND user_id = ?2 AND emoji = ?3)",
            [message_id, user_id, emoji],
            |r| r.get::<_, i64>(0),
        )
        .map_err(ApiError::from)?;
    if exists != 0 {
        conn.execute(
            "DELETE FROM message_reactions WHERE message_id = ?1 AND user_id = ?2 AND emoji = ?3",
            params![message_id, user_id, emoji],
        )
        .map_err(ApiError::from)?;
        Ok(false)
    } else {
        conn.execute(
            "INSERT INTO message_reactions (message_id, user_id, emoji, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![message_id, user_id, emoji, util::now_rfc3339()],
        )
        .map_err(ApiError::from)?;
        Ok(true)
    }
}

/// 批量查询消息的回应聚合（含当前用户是否已点）。
pub fn message_reactions(
    conn: &mut Connection,
    room_id: &str,
    message_ids: &[String],
    uid: &str,
) -> ApiResult<HashMap<String, Vec<(String, i64, bool)>>> {
    let mut out: HashMap<String, Vec<(String, i64, bool)>> = HashMap::new();
    if message_ids.is_empty() {
        return Ok(out);
    }
    let placeholders = (0..message_ids.len())
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT r.message_id, r.emoji, COUNT(*) AS cnt,
                MAX(CASE WHEN r.user_id = ?1 THEN 1 ELSE 0 END) AS me
         FROM message_reactions r
         JOIN messages m ON m.message_id = r.message_id AND m.room_id = ?2
         WHERE r.message_id IN ({placeholders})
         GROUP BY r.message_id, r.emoji"
    );
    let mut params: Vec<Box<dyn rusqlite::ToSql>> =
        vec![Box::new(uid.to_string()), Box::new(room_id.to_string())];
    for id in message_ids {
        params.push(Box::new(id.clone()));
    }
    let mut stmt = conn.prepare(&sql).map_err(ApiError::from)?;
    let rows = stmt
        .query_map(
            rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)? != 0,
                ))
            },
        )
        .map_err(ApiError::from)?;
    for r in rows {
        let (mid, emoji, cnt, me) = r.map_err(ApiError::from)?;
        out.entry(mid).or_default().push((emoji, cnt, me));
    }
    Ok(out)
}

/// 禁言开启/关闭时清空全部发言权与待审批申请（权限生命周期跟随禁言开关）。
pub fn clear_speak_grants(conn: &mut Connection, room_id: &str) -> ApiResult<()> {
    conn.execute("DELETE FROM speak_grants WHERE room_id = ?1", [room_id])
        .map_err(ApiError::from)?;
    conn.execute("DELETE FROM speak_requests WHERE room_id = ?1", [room_id])
        .map_err(ApiError::from)?;
    Ok(())
}

pub fn delete_speak_request(conn: &mut Connection, room_id: &str, user_id: &str) -> ApiResult<()> {
    conn.execute(
        "DELETE FROM speak_requests WHERE room_id = ?1 AND user_id = ?2",
        params![room_id, user_id],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

/// Apply an approved media-mode change: update the room and bump its version.
pub fn update_room_media_mode(
    conn: &mut Connection,
    room_id: &str,
    media_mode: MediaMode,
) -> ApiResult<()> {
    conn.execute(
        "UPDATE rooms SET media_mode = ?1, version = version + 1 WHERE room_id = ?2",
        params![media_mode.as_str(), room_id],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

pub fn bump_room_seq(conn: &mut Connection, room_id: &str) -> ApiResult<i64> {
    conn.execute(
        "UPDATE rooms SET last_seq = last_seq + 1 WHERE room_id = ?1",
        [room_id],
    )
    .map_err(ApiError::from)?;
    conn.query_row(
        "SELECT last_seq FROM rooms WHERE room_id = ?1",
        [room_id],
        |r| r.get(0),
    )
    .map_err(ApiError::from)
}

pub fn list_rooms(
    conn: &mut Connection,
    state: Option<&str>,
    kind: Option<&str>,
    search: Option<&str>,
    offset: i64,
    limit: i64,
) -> ApiResult<(Vec<Room>, i64)> {
    let mut where_clauses: Vec<String> = vec![];
    let mut vals: Vec<Box<dyn rusqlite::ToSql>> = vec![];
    if let Some(s) = state {
        where_clauses.push("state = ?".into());
        vals.push(Box::new(s.to_string()));
    }
    if let Some(k) = kind {
        where_clauses.push("kind = ?".into());
        vals.push(Box::new(k.to_string()));
    }
    if let Some(q) = search {
        if !q.is_empty() {
            where_clauses.push("room_code LIKE ?".into());
            vals.push(Box::new(format!("%{q}%")));
        }
    }
    let where_sql = if where_clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_clauses.join(" AND "))
    };
    let total: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM rooms{where_sql}"),
            rusqlite::params_from_iter(vals.iter().map(|v| v.as_ref())),
            |r| r.get(0),
        )
        .map_err(ApiError::from)?;
    vals.push(Box::new(limit));
    vals.push(Box::new(offset));
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {} FROM rooms{where_sql} ORDER BY created_at DESC LIMIT ? OFFSET ?",
            room_cols("")
        ))
        .map_err(ApiError::from)?;
    let rows = stmt
        .query_map(
            rusqlite::params_from_iter(vals.iter().map(|v| v.as_ref())),
            row_to_room,
        )
        .map_err(ApiError::from)?;
    let mut rooms = Vec::new();
    for r in rows {
        rooms.push(r.map_err(ApiError::from)?);
    }
    Ok((rooms, total))
}

pub fn count_rooms(conn: &mut Connection) -> ApiResult<i64> {
    conn.query_row("SELECT COUNT(*) FROM rooms", [], |r| r.get(0))
        .map_err(ApiError::from)
}

/// Active rooms whose expires_at has passed.
pub fn rooms_to_expire(
    conn: &mut Connection,
    now: &DateTime<Utc>,
) -> ApiResult<Vec<(String, String)>> {
    let mut stmt = conn
        .prepare(
            "SELECT room_id, room_code FROM rooms WHERE state = 'active' AND expires_at IS NOT NULL AND expires_at <= ?1",
        )
        .map_err(ApiError::from)?;
    let rows = stmt
        .query_map(params![now.to_rfc3339()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(ApiError::from)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(ApiError::from)?);
    }
    Ok(out)
}

/// Active direct rooms where either participant has been offline longer than `cutoff`.
pub fn direct_rooms_idle(
    conn: &mut Connection,
    cutoff: &DateTime<Utc>,
) -> ApiResult<Vec<(String, String)>> {
    let mut stmt = conn
        .prepare(
            "SELECT r.room_id, r.room_code FROM rooms r
             JOIN direct_pairs dp ON dp.room_id = r.room_id
             JOIN room_members m1 ON m1.room_id = r.room_id AND m1.user_id = dp.user_a
             JOIN room_members m2 ON m2.room_id = r.room_id AND m2.user_id = dp.user_b
             WHERE r.state = 'active' AND r.kind = 'direct'
               AND (m1.last_online_at IS NULL OR m1.last_online_at < ?1
                    OR m2.last_online_at IS NULL OR m2.last_online_at < ?1)",
        )
        .map_err(ApiError::from)?;
    let rows = stmt
        .query_map(params![cutoff.to_rfc3339()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(ApiError::from)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(ApiError::from)?);
    }
    Ok(out)
}

/// Deleted/expired rooms whose grace period has elapsed (deleted_at <= cutoff).
pub fn rooms_to_purge(conn: &mut Connection, cutoff: &DateTime<Utc>) -> ApiResult<Vec<String>> {
    let mut stmt = conn
        .prepare("SELECT room_id FROM rooms WHERE state IN ('deleted','expired') AND deleted_at IS NOT NULL AND deleted_at <= ?1")
        .map_err(ApiError::from)?;
    let rows = stmt
        .query_map(params![cutoff.to_rfc3339()], |r| r.get::<_, String>(0))
        .map_err(ApiError::from)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(ApiError::from)?);
    }
    Ok(out)
}

/// Physically remove all data of a terminated room (after the grace period).
/// The code tombstone stays forever: codes are never reused.
pub fn purge_room(conn: &mut Connection, room_id: &str) -> ApiResult<()> {
    let tx = conn.transaction().map_err(ApiError::from)?;
    tx.execute("DELETE FROM messages WHERE room_id = ?1", [room_id])
        .map_err(ApiError::from)?;
    tx.execute("DELETE FROM mutes WHERE room_id = ?1", [room_id])
        .map_err(ApiError::from)?;
    tx.execute("DELETE FROM room_members WHERE room_id = ?1", [room_id])
        .map_err(ApiError::from)?;
    tx.execute("DELETE FROM direct_pairs WHERE room_id = ?1", [room_id])
        .map_err(ApiError::from)?;
    tx.execute("DELETE FROM invites WHERE room_id = ?1", [room_id])
        .map_err(ApiError::from)?;
    tx.execute("DELETE FROM rooms WHERE room_id = ?1", [room_id])
        .map_err(ApiError::from)?;
    tx.commit().map_err(ApiError::from)?;
    Ok(())
}

// -------------------------------------------------------------- members

pub fn add_member(
    conn: &mut Connection,
    room_id: &str,
    user_id: &str,
    role: RoomRole,
    nickname: &str,
) -> ApiResult<()> {
    conn.execute(
        "INSERT INTO room_members (room_id, user_id, room_role, nickname, joined_at, last_online_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![room_id, user_id, role.as_str(), nickname, util::now_rfc3339()],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

pub fn get_member(
    conn: &mut Connection,
    room_id: &str,
    user_id: &str,
) -> ApiResult<Option<RoomMember>> {
    conn.query_row(
        "SELECT room_id, user_id, room_role, nickname, joined_at, left_at, last_online_at
         FROM room_members WHERE room_id = ?1 AND user_id = ?2",
        params![room_id, user_id],
        row_to_member,
    )
    .optional()
    .map_err(ApiError::from)
}

fn row_to_member(row: &rusqlite::Row) -> rusqlite::Result<RoomMember> {
    let role: String = row.get(2)?;
    Ok(RoomMember {
        room_id: row.get(0)?,
        user_id: row.get(1)?,
        room_role: RoomRole::from_str(&role).unwrap_or(RoomRole::Member),
        nickname: row.get(3)?,
        joined_at: parse_ts(&row.get::<_, String>(4)?),
        left_at: row.get::<_, Option<String>>(5)?.map(|s| parse_ts(&s)),
        last_online_at: row.get::<_, Option<String>>(6)?.map(|s| parse_ts(&s)),
    })
}

/// Current (not left) members of a room.
pub fn list_members(conn: &mut Connection, room_id: &str) -> ApiResult<Vec<RoomMember>> {
    let mut stmt = conn
        .prepare(
            "SELECT room_id, user_id, room_role, nickname, joined_at, left_at, last_online_at
             FROM room_members WHERE room_id = ?1 AND left_at IS NULL ORDER BY joined_at",
        )
        .map_err(ApiError::from)?;
    let rows = stmt
        .query_map([room_id], row_to_member)
        .map_err(ApiError::from)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(ApiError::from)?);
    }
    Ok(out)
}

pub fn count_members(conn: &mut Connection, room_id: &str) -> ApiResult<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM room_members WHERE room_id = ?1 AND left_at IS NULL",
        [room_id],
        |r| r.get(0),
    )
    .map_err(ApiError::from)
}

pub fn set_member_role(
    conn: &mut Connection,
    room_id: &str,
    user_id: &str,
    role: RoomRole,
) -> ApiResult<()> {
    conn.execute(
        "UPDATE room_members SET room_role = ?1 WHERE room_id = ?2 AND user_id = ?3",
        params![role.as_str(), room_id, user_id],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

pub fn set_member_nickname(
    conn: &mut Connection,
    room_id: &str,
    user_id: &str,
    nickname: &str,
) -> ApiResult<()> {
    conn.execute(
        "UPDATE room_members SET nickname = ?1 WHERE room_id = ?2 AND user_id = ?3",
        params![nickname, room_id, user_id],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

pub fn touch_member_online(
    conn: &mut Connection,
    room_id: &str,
    user_id: &str,
    at: &DateTime<Utc>,
) -> ApiResult<()> {
    conn.execute(
        "UPDATE room_members SET last_online_at = ?1 WHERE room_id = ?2 AND user_id = ?3",
        params![at.to_rfc3339(), room_id, user_id],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

pub fn leave_room(conn: &mut Connection, room_id: &str, user_id: &str) -> ApiResult<()> {
    conn.execute(
        "UPDATE room_members SET left_at = ?1 WHERE room_id = ?2 AND user_id = ?3 AND left_at IS NULL",
        params![util::now_rfc3339(), room_id, user_id],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

// --------------------------------------------------------- direct pairs

pub fn create_direct_pair(
    conn: &mut Connection,
    room_id: &str,
    parent_room_id: &str,
    user_a: &str,
    user_b: &str,
) -> ApiResult<()> {
    let (a, b) = if user_a < user_b {
        (user_a, user_b)
    } else {
        (user_b, user_a)
    };
    conn.execute(
        "INSERT INTO direct_pairs (room_id, parent_room_id, user_a, user_b) VALUES (?1, ?2, ?3, ?4)",
        params![room_id, parent_room_id, a, b],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

pub fn direct_room_by_pair(
    conn: &mut Connection,
    parent_room_id: &str,
    user_a: &str,
    user_b: &str,
) -> ApiResult<Option<String>> {
    let (a, b) = if user_a < user_b {
        (user_a, user_b)
    } else {
        (user_b, user_a)
    };
    conn.query_row(
        "SELECT room_id FROM direct_pairs WHERE parent_room_id = ?1 AND user_a = ?2 AND user_b = ?3",
        params![parent_room_id, a, b],
        |r| r.get(0),
    )
    .optional()
    .map_err(ApiError::from)
}

// ---------------------------------------------------------------- mutes

pub fn insert_mute(conn: &mut Connection, m: &Mute) -> ApiResult<()> {
    conn.execute(
        "INSERT INTO mutes (mute_id, room_id, scope, target_user_id, muted_by, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            m.mute_id,
            m.room_id,
            m.scope,
            m.target_user_id,
            m.muted_by,
            m.created_at.to_rfc3339(),
            m.expires_at.map(|d| d.to_rfc3339()),
        ],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

pub fn list_active_mutes(
    conn: &mut Connection,
    room_id: &str,
    now: &DateTime<Utc>,
) -> ApiResult<Vec<Mute>> {
    let mut stmt = conn
        .prepare(
            "SELECT mute_id, room_id, scope, target_user_id, muted_by, created_at, expires_at
             FROM mutes WHERE room_id = ?1 AND (expires_at IS NULL OR expires_at > ?2) ORDER BY created_at",
        )
        .map_err(ApiError::from)?;
    let rows = stmt
        .query_map(params![room_id, now.to_rfc3339()], |row| {
            Ok(Mute {
                mute_id: row.get(0)?,
                room_id: row.get(1)?,
                scope: row.get(2)?,
                target_user_id: row.get(3)?,
                muted_by: row.get(4)?,
                created_at: parse_ts(&row.get::<_, String>(5)?),
                expires_at: row.get::<_, Option<String>>(6)?.map(|s| parse_ts(&s)),
            })
        })
        .map_err(ApiError::from)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(ApiError::from)?);
    }
    Ok(out)
}

pub fn get_mute(conn: &mut Connection, mute_id: &str) -> ApiResult<Option<Mute>> {
    conn.query_row(
        "SELECT mute_id, room_id, scope, target_user_id, muted_by, created_at, expires_at
         FROM mutes WHERE mute_id = ?1",
        [mute_id],
        |row| {
            Ok(Mute {
                mute_id: row.get(0)?,
                room_id: row.get(1)?,
                scope: row.get(2)?,
                target_user_id: row.get(3)?,
                muted_by: row.get(4)?,
                created_at: parse_ts(&row.get::<_, String>(5)?),
                expires_at: row.get::<_, Option<String>>(6)?.map(|s| parse_ts(&s)),
            })
        },
    )
    .optional()
    .map_err(ApiError::from)
}

pub fn delete_mute(conn: &mut Connection, mute_id: &str) -> ApiResult<()> {
    conn.execute("DELETE FROM mutes WHERE mute_id = ?1", [mute_id])
        .map_err(ApiError::from)?;
    Ok(())
}

/// Is `user_id` (a non-owner) muted for text in this room right now?
pub fn member_is_muted(
    conn: &mut Connection,
    room_id: &str,
    user_id: &str,
    now: &DateTime<Utc>,
) -> ApiResult<bool> {
    conn.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM mutes WHERE room_id = ?1 AND (expires_at IS NULL OR expires_at > ?2)
             AND (scope = 'member' AND target_user_id = ?3
                  OR scope = 'room' AND target_user_id IS NULL))",
        params![room_id, now.to_rfc3339(), user_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .map_err(ApiError::from)
}

// --------------------------------------------------------- room requests

const REQUEST_COLS: &str = "request_id, requester_id, request_kind, target_room_id, media_mode,
                            audio_bitrate, note, desired_code, status, created_at, decided_at,
                            decided_by, decision_reason, created_room_id";

pub fn insert_room_request(conn: &mut Connection, req: &RoomRequest) -> ApiResult<()> {
    conn.execute(
        "INSERT INTO room_requests (request_id, requester_id, request_kind, target_room_id, media_mode,
                                    audio_bitrate, note, desired_code, status, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9)",
        params![
            req.request_id,
            req.requester_id,
            req.request_kind,
            req.target_room_id,
            req.media_mode.as_str(),
            req.audio_bitrate as i64,
            req.note,
            req.desired_code,
            req.created_at.to_rfc3339(),
        ],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

pub fn get_room_request(conn: &mut Connection, request_id: &str) -> ApiResult<Option<RoomRequest>> {
    conn.query_row(
        &format!("SELECT {REQUEST_COLS} FROM room_requests WHERE request_id = ?1"),
        [request_id],
        row_to_request,
    )
    .optional()
    .map_err(ApiError::from)
}

fn row_to_request(row: &rusqlite::Row) -> rusqlite::Result<RoomRequest> {
    let mm: String = row.get(4)?;
    Ok(RoomRequest {
        request_id: row.get(0)?,
        requester_id: row.get(1)?,
        request_kind: row.get(2)?,
        target_room_id: row.get(3)?,
        media_mode: MediaMode::from_str(&mm).unwrap_or(MediaMode::Sfu),
        audio_bitrate: row.get::<_, i64>(5)? as u32,
        note: row.get(6)?,
        desired_code: row.get(7)?,
        status: row.get(8)?,
        created_at: parse_ts(&row.get::<_, String>(9)?),
        decided_at: row.get::<_, Option<String>>(10)?.map(|s| parse_ts(&s)),
        decided_by: row.get(11)?,
        decision_reason: row.get(12)?,
        created_room_id: row.get(13)?,
    })
}

pub fn list_room_requests(
    conn: &mut Connection,
    requester_id: Option<&str>,
    status: Option<&str>,
    offset: i64,
    limit: i64,
) -> ApiResult<(Vec<RoomRequest>, i64)> {
    let mut clauses: Vec<String> = vec![];
    let mut vals: Vec<Box<dyn rusqlite::ToSql>> = vec![];
    if let Some(u) = requester_id {
        clauses.push("requester_id = ?".into());
        vals.push(Box::new(u.to_string()));
    }
    if let Some(s) = status {
        clauses.push("status = ?".into());
        vals.push(Box::new(s.to_string()));
    }
    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    let total: i64 = conn
        .query_row(
            &format!("SELECT COUNT(*) FROM room_requests{where_sql}"),
            rusqlite::params_from_iter(vals.iter().map(|v| v.as_ref())),
            |r| r.get(0),
        )
        .map_err(ApiError::from)?;
    vals.push(Box::new(limit));
    vals.push(Box::new(offset));
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {REQUEST_COLS} FROM room_requests{where_sql} ORDER BY created_at DESC LIMIT ? OFFSET ?"
        ))
        .map_err(ApiError::from)?;
    let rows = stmt
        .query_map(
            rusqlite::params_from_iter(vals.iter().map(|v| v.as_ref())),
            row_to_request,
        )
        .map_err(ApiError::from)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(ApiError::from)?);
    }
    Ok((out, total))
}

pub fn update_request_decision(
    conn: &mut Connection,
    request_id: &str,
    status: &str,
    decided_by: &str,
    reason: Option<&str>,
    created_room_id: Option<&str>,
) -> ApiResult<()> {
    conn.execute(
        "UPDATE room_requests SET status = ?1, decided_at = ?2, decided_by = ?3, decision_reason = ?4, created_room_id = ?5
         WHERE request_id = ?6 AND status = 'pending'",
        params![status, util::now_rfc3339(), decided_by, reason, created_room_id, request_id],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

/// Pending media-mode change request for a room, if any.
pub fn pending_media_change(
    conn: &mut Connection,
    room_id: &str,
) -> ApiResult<Option<RoomRequest>> {
    conn.query_row(
        &format!(
            "SELECT {REQUEST_COLS} FROM room_requests
             WHERE request_kind = 'change_media_mode' AND status = 'pending' AND target_room_id = ?1"
        ),
        [room_id],
        row_to_request,
    )
    .optional()
    .map_err(ApiError::from)
}

pub fn cancel_room_request(
    conn: &mut Connection,
    request_id: &str,
    user_id: &str,
) -> ApiResult<()> {
    conn.execute(
        "UPDATE room_requests SET status = 'cancelled', decided_at = ?1, decided_by = ?2
         WHERE request_id = ?3 AND requester_id = ?4 AND status = 'pending'",
        params![util::now_rfc3339(), user_id, request_id, user_id],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

// -------------------------------------------------------------- invites

pub fn insert_invite(conn: &mut Connection, inv: &Invite) -> ApiResult<()> {
    conn.execute(
        "INSERT INTO invites (invite_id, kind, token_hash, room_id, created_by, max_role, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            inv.invite_id,
            inv.kind,
            inv.token_hash,
            inv.room_id,
            inv.created_by,
            inv.max_role,
            inv.created_at.to_rfc3339(),
            inv.expires_at.to_rfc3339(),
        ],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

pub fn get_invite_by_hash(conn: &mut Connection, token_hash: &str) -> ApiResult<Option<Invite>> {
    conn.query_row(
        "SELECT invite_id, kind, token_hash, room_id, created_by, max_role, created_at, expires_at, consumed_at, consumed_by, revoked_at
         FROM invites WHERE token_hash = ?1",
        [token_hash],
        row_to_invite,
    )
    .optional()
    .map_err(ApiError::from)
}

pub fn get_invite_by_id(conn: &mut Connection, invite_id: &str) -> ApiResult<Option<Invite>> {
    conn.query_row(
        "SELECT invite_id, kind, token_hash, room_id, created_by, max_role, created_at, expires_at, consumed_at, consumed_by, revoked_at
         FROM invites WHERE invite_id = ?1",
        [invite_id],
        row_to_invite,
    )
    .optional()
    .map_err(ApiError::from)
}

fn row_to_invite(row: &rusqlite::Row) -> rusqlite::Result<Invite> {
    Ok(Invite {
        invite_id: row.get(0)?,
        kind: row.get(1)?,
        token_hash: row.get(2)?,
        room_id: row.get(3)?,
        created_by: row.get(4)?,
        max_role: row.get(5)?,
        created_at: parse_ts(&row.get::<_, String>(6)?),
        expires_at: parse_ts(&row.get::<_, String>(7)?),
        consumed_at: row.get::<_, Option<String>>(8)?.map(|s| parse_ts(&s)),
        consumed_by: row.get(9)?,
        revoked_at: row.get::<_, Option<String>>(10)?.map(|s| parse_ts(&s)),
    })
}

pub fn list_room_invites(conn: &mut Connection, room_id: &str) -> ApiResult<Vec<Invite>> {
    let mut stmt = conn
        .prepare(
            "SELECT invite_id, kind, token_hash, room_id, created_by, max_role, created_at, expires_at, consumed_at, consumed_by, revoked_at
             FROM invites WHERE room_id = ?1 AND kind = 'room' ORDER BY created_at DESC",
        )
        .map_err(ApiError::from)?;
    let rows = stmt
        .query_map([room_id], row_to_invite)
        .map_err(ApiError::from)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(ApiError::from)?);
    }
    Ok(out)
}

pub fn list_registration_invites(conn: &mut Connection) -> ApiResult<Vec<Invite>> {
    let mut stmt = conn
        .prepare(
            "SELECT invite_id, kind, token_hash, room_id, created_by, max_role, created_at, expires_at, consumed_at, consumed_by, revoked_at
             FROM invites WHERE kind = 'registration' ORDER BY created_at DESC",
        )
        .map_err(ApiError::from)?;
    let rows = stmt.query_map([], row_to_invite).map_err(ApiError::from)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(ApiError::from)?);
    }
    Ok(out)
}

pub fn revoke_invite(conn: &mut Connection, invite_id: &str) -> ApiResult<()> {
    conn.execute(
        "UPDATE invites SET revoked_at = ?1 WHERE invite_id = ?2 AND consumed_at IS NULL",
        params![util::now_rfc3339(), invite_id],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

/// Atomically consume an invite; true when this call won.
pub fn consume_invite(
    conn: &mut Connection,
    invite_id: &str,
    consumed_by: &str,
) -> ApiResult<bool> {
    let n = conn
        .execute(
            "UPDATE invites SET consumed_at = ?1, consumed_by = ?2 WHERE invite_id = ?3 AND consumed_at IS NULL AND revoked_at IS NULL AND expires_at > ?1",
            params![util::now_rfc3339(), consumed_by, invite_id],
        )
        .map_err(ApiError::from)?;
    Ok(n == 1)
}

// ------------------------------------------------------------- messages

pub fn insert_message(conn: &mut Connection, msg: &Message) -> ApiResult<()> {
    conn.execute(
        "INSERT INTO messages (message_id, room_id, seq, sender_user_id, body, created_at, reply_snapshot)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            msg.message_id,
            msg.room_id,
            msg.seq,
            msg.sender_user_id,
            msg.body,
            msg.created_at.to_rfc3339(),
            msg.reply_snapshot,
        ],
    )
    .map_err(ApiError::from)?;
    Ok(())
}

/// 插入一条系统播报消息（sender 为 NULL），返回消息体。
pub fn insert_system_message(
    conn: &mut Connection,
    room_id: &str,
    body: &str,
) -> ApiResult<Message> {
    let seq = bump_room_seq(conn, room_id)?;
    let m = Message {
        message_id: util::random_id(),
        room_id: room_id.to_string(),
        seq,
        sender_user_id: None,
        body: body.to_string(),
        reply_snapshot: None,
        created_at: util::now(),
    };
    insert_message(conn, &m)?;
    Ok(m)
}

fn row_to_message(row: &rusqlite::Row) -> rusqlite::Result<Message> {
    Ok(Message {
        message_id: row.get(0)?,
        room_id: row.get(1)?,
        seq: row.get(2)?,
        sender_user_id: row.get(3)?,
        body: row.get(4)?,
        created_at: parse_ts(&row.get::<_, String>(5)?),
        reply_snapshot: row.get(6)?,
    })
}

pub fn get_message(conn: &mut Connection, message_id: &str) -> ApiResult<Option<Message>> {
    conn.query_row(
        "SELECT message_id, room_id, seq, sender_user_id, body, created_at, reply_snapshot
         FROM messages WHERE message_id = ?1",
        [message_id],
        row_to_message,
    )
    .optional()
    .map_err(ApiError::from)
}

pub fn list_messages(
    conn: &mut Connection,
    room_id: &str,
    after_seq: i64,
    limit: i64,
) -> ApiResult<Vec<Message>> {
    let mut stmt = conn
        .prepare(
            "SELECT message_id, room_id, seq, sender_user_id, body, created_at, reply_snapshot
             FROM messages WHERE room_id = ?1 AND seq > ?2 ORDER BY seq ASC LIMIT ?3",
        )
        .map_err(ApiError::from)?;
    let rows = stmt
        .query_map(params![room_id, after_seq, limit], row_to_message)
        .map_err(ApiError::from)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(ApiError::from)?);
    }
    Ok(out)
}

// --------------------------------------------------------------- audit

pub fn insert_audit(
    conn: &mut Connection,
    actor: Option<&str>,
    action: &str,
    target_type: Option<&str>,
    target_id: Option<&str>,
    detail: &serde_json::Value,
) -> ApiResult<i64> {
    conn.execute(
        "INSERT INTO audit_log (actor_user_id, action, target_type, target_id, detail, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            actor,
            action,
            target_type,
            target_id,
            detail.to_string(),
            util::now_rfc3339()
        ],
    )
    .map_err(ApiError::from)?;
    Ok(conn.last_insert_rowid())
}

pub fn list_audit(
    conn: &mut Connection,
    offset: i64,
    limit: i64,
) -> ApiResult<(Vec<AuditEntry>, i64)> {
    let total: i64 = conn
        .query_row("SELECT COUNT(*) FROM audit_log", [], |r| r.get(0))
        .map_err(ApiError::from)?;
    let mut stmt = conn
        .prepare(
            "SELECT entry_id, actor_user_id, action, target_type, target_id, detail, created_at
             FROM audit_log ORDER BY entry_id DESC LIMIT ?1 OFFSET ?2",
        )
        .map_err(ApiError::from)?;
    let rows = stmt
        .query_map(params![limit, offset], |row| {
            Ok(AuditEntry {
                entry_id: row.get(0)?,
                actor_user_id: row.get(1)?,
                action: row.get(2)?,
                target_type: row.get(3)?,
                target_id: row.get(4)?,
                detail: row.get(5)?,
                created_at: parse_ts(&row.get::<_, String>(6)?),
            })
        })
        .map_err(ApiError::from)?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(ApiError::from)?);
    }
    Ok((out, total))
}

// -------------------------------------------------------------- helpers

pub fn parse_ts(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}
