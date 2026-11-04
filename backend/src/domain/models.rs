//! Domain model structs mirroring the SQLite schema.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub type UserId = String;
pub type RoomId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    SuperAdmin,
    Admin,
    User,
    Guest,
}

impl UserRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            UserRole::SuperAdmin => "super_admin",
            UserRole::Admin => "admin",
            UserRole::User => "user",
            UserRole::Guest => "guest",
        }
    }

    // Option-based parser; implementing FromStr would change the public error contract.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "super_admin" => Some(UserRole::SuperAdmin),
            "admin" => Some(UserRole::Admin),
            "user" => Some(UserRole::User),
            "guest" => Some(UserRole::Guest),
            _ => None,
        }
    }

    pub fn is_admin(&self) -> bool {
        matches!(self, UserRole::SuperAdmin | UserRole::Admin)
    }

    pub fn is_super_admin(&self) -> bool {
        matches!(self, UserRole::SuperAdmin)
    }
}

#[derive(Debug, Clone)]
pub struct User {
    pub user_id: UserId,
    pub username: Option<String>,
    pub password_hash: Option<String>,
    pub role: UserRole,
    pub avatar_id: Option<String>,
    pub disabled: bool,
    pub created_at: DateTime<Utc>,
    pub last_online_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomKind {
    Standard,
    Direct,
}

impl RoomKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            RoomKind::Standard => "standard",
            RoomKind::Direct => "direct",
        }
    }
    // Option-based parser; implementing FromStr would change the public error contract.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "standard" => Some(RoomKind::Standard),
            "direct" => Some(RoomKind::Direct),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomState {
    Active,
    Expired,
    Deleted,
}

impl RoomState {
    pub fn as_str(&self) -> &'static str {
        match self {
            RoomState::Active => "active",
            RoomState::Expired => "expired",
            RoomState::Deleted => "deleted",
        }
    }
    // Option-based parser; implementing FromStr would change the public error contract.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "active" => Some(RoomState::Active),
            "expired" => Some(RoomState::Expired),
            "deleted" => Some(RoomState::Deleted),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaMode {
    P2p,
    Sfu,
}

impl MediaMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            MediaMode::P2p => "p2p",
            MediaMode::Sfu => "sfu",
        }
    }
    // Option-based parser; implementing FromStr would change the public error contract.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "p2p" => Some(MediaMode::P2p),
            "sfu" => Some(MediaMode::Sfu),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Room {
    pub room_id: RoomId,
    pub room_code: String,
    pub kind: RoomKind,
    pub owner_user_id: Option<UserId>,
    pub parent_room_id: Option<RoomId>,
    pub state: RoomState,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub allow_direct_messages: bool,
    pub text_enabled: bool,
    pub media_mode: MediaMode,
    /// 音频码率（kbps，0=自动）。仅 SFU 模式生效。
    pub audio_bitrate: u32,
    /// 房间级禁言（仅房主可开关；禁言中仅获发言权用户可加入语音）。
    pub room_muted: bool,
    /// 房间公告（房主设置，顶部横幅展示）。
    pub announcement: String,
    /// 成员上限（0=不限）。
    pub max_members: i64,
    pub last_seq: i64,
    pub version: i64,
    /// 媒体加密（服务器持钥）（仅 SFU 生效；密钥由服务端下发给成员）。
    pub e2ee: bool,
    /// LiveKit E2EE 共享密钥（32 字节随机，base64）；仅成员可见。
    pub e2ee_key: Option<String>,
    /// 房间是否允许消息附件（建房申请时确定）。
    pub attachments_enabled: bool,
    /// 已占用附件字节数（配额用）。
    pub attachment_bytes: i64,
    /// 房主为管理员时可开启：房间内访客可直接注册为正式用户。
    pub guest_upgrade_open: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomRole {
    Owner,
    Member,
}

impl RoomRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            RoomRole::Owner => "owner",
            RoomRole::Member => "member",
        }
    }
    // Option-based parser; implementing FromStr would change the public error contract.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "owner" => Some(RoomRole::Owner),
            // 历史值（manager/guest 等）一律视为普通成员；
            // 账号权限（user/admin/super_admin/guest）与房间权限分离。
            _ => Some(RoomRole::Member),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RoomMember {
    pub room_id: RoomId,
    pub user_id: UserId,
    pub room_role: RoomRole,
    pub nickname: String,
    pub joined_at: DateTime<Utc>,
    pub left_at: Option<DateTime<Utc>>,
    pub left_reason: Option<String>,
    pub last_online_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub message_id: String,
    pub room_id: RoomId,
    pub seq: i64,
    /// NULL = 系统播报（成员进出等），前端渲染为系统消息
    pub sender_user_id: Option<UserId>,
    pub body: String,
    /// 回复引用的原消息快照 JSON（{"id","sender","body"}，防原消息删除后丢失）。
    pub reply_snapshot: Option<String>,
    pub created_at: DateTime<Utc>,
    /// 编辑时间；None = 未编辑过。
    pub edited_at: Option<DateTime<Utc>>,
    /// 软删除时间；None = 正常消息。删除后正文不下发。
    pub deleted_at: Option<DateTime<Utc>>,
    /// 关联附件（图片/文件）。
    pub attachment_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Attachment {
    pub attachment_id: String,
    pub room_id: RoomId,
    pub uploader_id: UserId,
    pub filename: String,
    pub mime: String,
    pub size: i64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct Mute {
    pub mute_id: String,
    pub room_id: RoomId,
    pub scope: String, // "member" | "room"
    pub target_user_id: Option<UserId>,
    pub muted_by: UserId,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl Mute {
    pub fn active(&self, now: DateTime<Utc>) -> bool {
        match self.expires_at {
            Some(exp) => now < exp,
            None => true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RoomRequest {
    pub request_id: String,
    pub requester_id: UserId,
    /// "create_room" | "change_media_mode"
    pub request_kind: String,
    /// Room this change_media_mode request targets (NULL for create_room).
    pub target_room_id: Option<RoomId>,
    /// Media mode requested for the new room / target room.
    pub media_mode: MediaMode,
    /// 建房申请携带的目标码率（kbps，0=自动；模式变更申请恒为 0）。
    pub audio_bitrate: u32,
    pub note: String,
    pub desired_code: Option<String>,
    pub status: String, // pending | approved | rejected | cancelled
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub decided_by: Option<UserId>,
    pub decision_reason: Option<String>,
    pub created_room_id: Option<RoomId>,
    /// 建房申请是否申请附件能力。
    pub attachments: bool,
}

#[derive(Debug, Clone)]
pub struct Invite {
    pub invite_id: String,
    pub kind: String, // room | registration
    pub token_hash: String,
    pub room_id: Option<RoomId>,
    pub created_by: UserId,
    pub max_role: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
    pub consumed_by: Option<UserId>,
    pub revoked_at: Option<DateTime<Utc>>,
    /// 仅逐客转正邀请使用：绑定到该访客，其他人不可兑换。
    pub guest_user_id: Option<UserId>,
}

#[derive(Debug, Clone)]
pub struct GuestUpgradeRequest {
    pub request_id: String,
    pub room_id: RoomId,
    pub guest_user_id: UserId,
    pub requested_by: UserId,
    pub status: String,
    pub note: String,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub decided_by: Option<UserId>,
    pub invite_id: Option<String>,
}

impl Invite {
    pub fn active(&self, now: DateTime<Utc>) -> bool {
        self.consumed_at.is_none() && self.revoked_at.is_none() && now < self.expires_at
    }
}

#[derive(Debug, Clone)]
pub struct Session {
    pub session_id: String,
    pub user_id: UserId,
    pub token_hash: String,
    pub csrf_token: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditEntry {
    pub entry_id: i64,
    pub actor_user_id: Option<UserId>,
    pub action: String,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub detail: String,
    pub created_at: DateTime<Utc>,
}
