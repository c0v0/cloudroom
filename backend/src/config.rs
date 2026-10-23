//! Configuration loading and validation.
//!
//! `config.toml` is the single source of operational settings. Every key has a
//! default; the file overrides. Durations are humantime strings ("30d", "1h").
//! Changing `rooms.generated_code_length` / `rooms.generated_code_alphabet`
//! only affects codes generated *after* the change; existing room codes are
//! stored verbatim and never rewritten.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub server: ServerCfg,
    pub data: DataCfg,
    pub auth: AuthCfg,
    pub rooms: RoomsCfg,
    pub media: MediaCfg,
    pub voice: VoiceCfg,
    pub security: SecurityCfg,
    pub rate_limit: RateLimitCfg,
    pub log: LogCfg,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerCfg {
    /// "127.0.0.1:8080" or "unix:/run/cloudroom/cloudroom.sock"
    pub listen: String,
    /// Public origin as seen by browsers, e.g. "https://chat.example.com".
    pub public_origin: String,
    /// Origin serving the frontend; equal to public_origin when Nginx hosts both.
    pub frontend_origin: String,
    /// Extra origins allowed for CORS / WebSocket / Origin checks.
    pub allowed_origins: Vec<String>,
    /// Peer address allowed to set X-Forwarded-* headers (normally the local Nginx).
    pub trusted_proxy: Option<String>,
    /// Dev only: allow Secure cookies over plain http (curl testing). Never enable in production.
    pub insecure_cookies: bool,
    /// Explicit local/test mode. Production is the default: HTTPS origins,
    /// WSS LiveKit, secure cookies and non-placeholder credentials are required.
    pub development: bool,
    /// Max request body size in bytes (avatar uploads + JSON).
    pub max_body_bytes: usize,
}

impl Default for ServerCfg {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8080".into(),
            public_origin: "http://127.0.0.1:8080".into(),
            frontend_origin: "http://127.0.0.1:5173".into(),
            allowed_origins: vec![],
            trusted_proxy: Some("127.0.0.1".into()),
            insecure_cookies: false,
            development: false,
            max_body_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DataCfg {
    /// Directory holding cloudroom.db, avatars/, backups/.
    pub dir: PathBuf,
}

impl Default for DataCfg {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("data"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AuthCfg {
    pub session_ttl: String,
    pub bootstrap_ttl: String,
    /// "invite_only" (default) or "open".
    pub registration_mode: String,
    pub argon2_memory_kib: u32,
    pub argon2_iterations: u32,
    pub argon2_parallelism: u32,
    pub password_min_bytes: usize,
    pub password_max_bytes: usize,
}

impl Default for AuthCfg {
    fn default() -> Self {
        Self {
            session_ttl: "30d".into(),
            bootstrap_ttl: "24h".into(),
            registration_mode: "invite_only".into(),
            argon2_memory_kib: 19_456,
            argon2_iterations: 2,
            argon2_parallelism: 1,
            password_min_bytes: 8,
            password_max_bytes: 1024,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RoomsCfg {
    /// Direct-room idle timeout: either participant offline longer than this
    /// causes the direct room to be deleted (owner-equivalent action).
    pub dm_idle_timeout: String,
    /// How long deleted/expired room data is retained before physical cleanup.
    pub expired_cleanup_grace: String,
    /// Always "keep": standard rooms are never destroyed for being empty.
    pub empty_room_policy: String,
    /// Length of automatically generated room codes. Only affects new rooms.
    pub generated_code_length: usize,
    /// Literal alphabet for generated codes (lowercase Crockford Base32
    /// default: no i/l/o/u). Only affects new rooms.
    pub generated_code_alphabet: String,
    /// Regex for custom codes chosen by super_admin.
    pub custom_code_pattern: String,
    /// Reserved words that custom codes may never use (frontend routes etc).
    pub reserved_codes: Vec<String>,
    /// Default lifetime of room/registration invites.
    pub invite_ttl: String,
    /// Max chars for a room-request note.
    pub max_note_chars: usize,
    /// Max chars for a text message.
    pub max_message_chars: usize,
}

impl Default for RoomsCfg {
    fn default() -> Self {
        Self {
            dm_idle_timeout: "1h".into(),
            expired_cleanup_grace: "30d".into(),
            empty_room_policy: "keep".into(),
            generated_code_length: 7,
            generated_code_alphabet: "0123456789abcdefghjkmnpqrstvwxyz".into(),
            custom_code_pattern: "^[a-z0-9][a-z0-9-]{2,31}$".into(),
            reserved_codes: vec![
                "api".into(),
                "admin".into(),
                "register".into(),
                "health".into(),
                "ws".into(),
                "login".into(),
                "logout".into(),
                "me".into(),
                "rooms".into(),
                "room-requests".into(),
                "invites".into(),
                "assets".into(),
                "static".into(),
                "settings".into(),
                "favicon.ico".into(),
            ],
            invite_ttl: "24h".into(),
            max_note_chars: 500,
            max_message_chars: 4000,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MediaCfg {
    /// Max participants in a P2P voice mesh room. Above this, joining voice
    /// in a p2p room fails with `sfu_required`; the room must request an sfu
    /// mode change through the approval channel.
    pub p2p_voice_max_participants: usize,
    /// LiveKit WSS URL (browser-reachable; also the host used for the
    /// LiveKit embedded TURN relay credentials).
    pub livekit_url: String,
    pub livekit_api_key: String,
    pub livekit_api_secret: String,
}

impl Default for MediaCfg {
    fn default() -> Self {
        Self {
            p2p_voice_max_participants: 4,
            livekit_url: String::new(),
            livekit_api_key: String::new(),
            livekit_api_secret: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct VoiceCfg {
    /// 音频码率档位（kbps）及房主可设置该档所需的最低账号角色
    /// （"user" / "admin" / "super_admin"）。0（自动）恒可用。
    /// 默认：≤128 任何房主、160/192 管理员房主、256 及以上（Opus 上限 510）
    /// 超管房主。档位参考 RFC 7587（语音 ≤40、48 起为音乐档）。
    /// 部署方可放宽。
    pub bitrate_tiers: BTreeMap<u32, String>,
}

impl Default for VoiceCfg {
    fn default() -> Self {
        Self {
            bitrate_tiers: BTreeMap::from([
                (32, "user".into()),
                (48, "user".into()),
                (64, "user".into()),
                (96, "user".into()),
                (128, "user".into()),
                (160, "admin".into()),
                (192, "admin".into()),
                (256, "super_admin".into()),
                (320, "super_admin".into()),
                (384, "super_admin".into()),
                (448, "super_admin".into()),
                (510, "super_admin".into()),
            ]),
        }
    }
}

impl VoiceCfg {
    /// 账号角色层级：guest < user < admin < super_admin。
    pub fn role_rank(role: &str) -> u8 {
        match role {
            "super_admin" => 3,
            "admin" => 2,
            "user" => 1,
            _ => 0,
        }
    }

    /// 房主账号能否为房间设置该码率（0 表示自动，恒可用）。
    pub fn can_set_bitrate(&self, owner_role: &str, kbps: u32) -> bool {
        if kbps == 0 {
            return true;
        }
        match self.bitrate_tiers.get(&kbps) {
            Some(need) => Self::role_rank(owner_role) >= Self::role_rank(need),
            None => false,
        }
    }

    /// 档位列表（升序），含角色门槛，供前端渲染可选项。
    pub fn bitrate_tiers_sorted(&self) -> Vec<(u32, &str)> {
        self.bitrate_tiers
            .iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SecurityCfg {
    pub max_avatar_bytes: usize,
    pub max_avatar_dimension: u32,
    pub max_nickname_graphemes: usize,
    /// LiveKit token lifetime.
    pub media_token_ttl: String,
    /// Session idle expiry for sweepers (WS heartbeat refresh).
    pub presence_heartbeat_interval: String,
}

impl Default for SecurityCfg {
    fn default() -> Self {
        Self {
            max_avatar_bytes: 2 * 1024 * 1024,
            max_avatar_dimension: 1024,
            max_nickname_graphemes: 32,
            media_token_ttl: "2h".into(),
            presence_heartbeat_interval: "60s".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RateLimitCfg {
    pub login_per_minute_per_ip: u32,
    pub login_per_hour_per_account: u32,
    pub register_per_hour_per_ip: u32,
    pub invite_redeem_per_hour_per_ip: u32,
    pub room_request_per_hour_per_user: u32,
    pub message_per_minute_per_user: u32,
    /// 加入房间的每分钟频次（防高频进出抖动）。
    pub room_join_per_minute_per_user: u32,
    /// 离开房间的每分钟频次。
    pub room_leave_per_minute_per_user: u32,
}

impl Default for RateLimitCfg {
    fn default() -> Self {
        Self {
            login_per_minute_per_ip: 10,
            login_per_hour_per_account: 30,
            register_per_hour_per_ip: 5,
            invite_redeem_per_hour_per_ip: 20,
            room_request_per_hour_per_user: 10,
            message_per_minute_per_user: 60,
            room_join_per_minute_per_user: 20,
            room_leave_per_minute_per_user: 20,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LogCfg {
    pub level: String,
}

impl Default for LogCfg {
    fn default() -> Self {
        Self {
            level: "info".into(),
        }
    }
}

fn is_placeholder_livekit_value(value: &str) -> bool {
    let value = value.trim();
    value.is_empty()
        || value.eq_ignore_ascii_case("CHANGE_ME")
        || value.starts_with("CHANGE_ME_")
        || value.eq_ignore_ascii_case("devkey")
        || value.to_ascii_lowercase().starts_with("devsecret")
}

impl Config {
    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read config {}: {e}", path.display()))?;
        let cfg: Config =
            toml::from_str(&text).map_err(|e| format!("invalid config {}: {e}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.rooms.generated_code_length < 4 || self.rooms.generated_code_length > 64 {
            return Err("rooms.generated_code_length must be 4..=64".into());
        }
        if self.rooms.generated_code_alphabet.chars().count() < 16 {
            return Err("rooms.generated_code_alphabet must have >= 16 distinct chars".into());
        }
        let unique: std::collections::HashSet<char> =
            self.rooms.generated_code_alphabet.chars().collect();
        if unique.len() != self.rooms.generated_code_alphabet.chars().count() {
            return Err("rooms.generated_code_alphabet must not repeat characters".into());
        }
        if self.rooms.empty_room_policy != "keep" {
            return Err("rooms.empty_room_policy must be \"keep\"".into());
        }
        if self.auth.registration_mode != "invite_only" && self.auth.registration_mode != "open" {
            return Err("auth.registration_mode must be \"invite_only\" or \"open\"".into());
        }
        // LiveKit is the single media infrastructure for both p2p (embedded
        // TURN relay credentials) and sfu (JWTs); it is always required.
        if self.media.livekit_url.is_empty()
            || self.media.livekit_api_key.is_empty()
            || self.media.livekit_api_secret.is_empty()
        {
            return Err(
                "media.livekit_url, media.livekit_api_key, and media.livekit_api_secret are required"
                    .into(),
            );
        }
        if !self.media.livekit_url.starts_with("ws://")
            && !self.media.livekit_url.starts_with("wss://")
        {
            return Err("media.livekit_url must start with ws:// or wss://".into());
        }
        if !self.server.insecure_cookies && !self.media.livekit_url.starts_with("wss://") {
            return Err(
                "media.livekit_url must use wss:// when server.insecure_cookies=false".into(),
            );
        }
        if self.server.public_origin.is_empty() {
            return Err("server.public_origin must be set".into());
        }
        if !self.server.development {
            if self.server.insecure_cookies {
                return Err(
                    "server.insecure_cookies=true is dev-only; set server.development=true for local testing"
                        .into(),
                );
            }
            if !self.server.public_origin.starts_with("https://")
                || !self.server.frontend_origin.starts_with("https://")
            {
                return Err(
                    "production requires https:// server.public_origin and server.frontend_origin"
                        .into(),
                );
            }
            if !self.media.livekit_url.starts_with("wss://") {
                return Err("production requires media.livekit_url to use wss://".into());
            }
            if is_placeholder_livekit_value(&self.media.livekit_api_key)
                || is_placeholder_livekit_value(&self.media.livekit_api_secret)
            {
                return Err(
                    "production rejects placeholder/default LiveKit credentials; set random media.livekit_api_key and media.livekit_api_secret"
                        .into(),
                );
            }
        }
        for (kbps, need) in &self.voice.bitrate_tiers {
            if VoiceCfg::role_rank(need) == 0 {
                return Err(format!(
                    "voice.bitrate_tiers[{kbps}] must be one of user/admin/super_admin"
                ));
            }
        }
        Ok(())
    }

    pub fn duration(s: &str) -> Result<Duration, String> {
        humantime::parse_duration(s).map_err(|e| format!("invalid duration {s:?}: {e}"))
    }

    pub fn session_ttl(&self) -> Duration {
        Self::duration(&self.auth.session_ttl).unwrap_or(Duration::from_secs(30 * 86400))
    }

    pub fn bootstrap_ttl(&self) -> Duration {
        Self::duration(&self.auth.bootstrap_ttl).unwrap_or(Duration::from_secs(86400))
    }

    pub fn invite_ttl(&self) -> Duration {
        Self::duration(&self.rooms.invite_ttl).unwrap_or(Duration::from_secs(86400))
    }

    pub fn dm_idle_timeout(&self) -> Duration {
        Self::duration(&self.rooms.dm_idle_timeout).unwrap_or(Duration::from_secs(3600))
    }

    pub fn cleanup_grace(&self) -> Duration {
        Self::duration(&self.rooms.expired_cleanup_grace).unwrap_or(Duration::from_secs(30 * 86400))
    }

    pub fn media_token_ttl(&self) -> Duration {
        Self::duration(&self.security.media_token_ttl).unwrap_or(Duration::from_secs(7200))
    }

    pub fn heartbeat_interval(&self) -> Duration {
        Self::duration(&self.security.presence_heartbeat_interval)
            .unwrap_or(Duration::from_secs(60))
    }

    pub fn listen_addr(&self) -> Result<ListenAddr, String> {
        if let Some(path) = self.server.listen.strip_prefix("unix:") {
            Ok(ListenAddr::Unix(PathBuf::from(path)))
        } else {
            let addr: SocketAddr = self
                .server
                .listen
                .parse()
                .map_err(|e| format!("invalid server.listen {:?}: {e}", self.server.listen))?;
            Ok(ListenAddr::Tcp(addr))
        }
    }

    /// Origins allowed for Origin checks / CORS / WS: public + frontend + extras.
    pub fn allowed_origins(&self) -> Vec<String> {
        let mut v = vec![
            self.server.public_origin.clone(),
            self.server.frontend_origin.clone(),
        ];
        v.extend(self.server.allowed_origins.iter().cloned());
        v.sort();
        v.dedup();
        v
    }
}

#[derive(Debug, Clone)]
pub enum ListenAddr {
    Tcp(SocketAddr),
    Unix(PathBuf),
}

pub const DEFAULT_CONFIG: &str = r#"# CloudRoom server configuration. See docs/configuration.md for every key.
[server]
listen = "127.0.0.1:8080"
public_origin = "https://chat.example.com"
frontend_origin = "https://chat.example.com"
trusted_proxy = "127.0.0.1"
insecure_cookies = false
development = false

[data]
dir = "/var/lib/cloudroom"

[auth]
registration_mode = "invite_only"

[rooms]
dm_idle_timeout = "1h"
expired_cleanup_grace = "30d"
generated_code_length = 7
generated_code_alphabet = "0123456789abcdefghjkmnpqrstvwxyz"

[media]
livekit_url = "wss://lk.example.com"
livekit_api_key = "CHANGE_ME"
livekit_api_secret = "CHANGE_ME"

[voice]
# 音频码率档位（kbps）与房主可设置该档所需的最低账号角色。
# 0（自动）恒可用。部署方可放宽（例如全部改为 "user"）。
# Opus 编码器上限 510 kbps；256 以上听感收益递减，仅超管房主可选。
bitrate_tiers = { 32 = "user", 48 = "user", 64 = "user", 96 = "user", 128 = "user", 160 = "admin", 192 = "admin", 256 = "super_admin", 320 = "super_admin", 384 = "super_admin", 448 = "super_admin", 510 = "super_admin" }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn dev_config() -> Config {
        let mut cfg = Config::default();
        cfg.server.development = true;
        cfg.server.insecure_cookies = true;
        cfg.server.public_origin = "http://127.0.0.1:5173".into();
        cfg.server.frontend_origin = "http://127.0.0.1:5173".into();
        cfg.media.livekit_url = "ws://127.0.0.1:7880".into();
        cfg.media.livekit_api_key = "dev-key".into();
        cfg.media.livekit_api_secret = "dev-secret".into();
        cfg
    }

    fn production_config() -> Config {
        let mut cfg = Config::default();
        cfg.server.public_origin = "https://chat.example.com".into();
        cfg.server.frontend_origin = "https://chat.example.com".into();
        cfg.media.livekit_url = "wss://lk.example.com".into();
        cfg.media.livekit_api_key = "livekit-key".into();
        cfg.media.livekit_api_secret = "unit-test".into();
        cfg
    }

    #[test]
    fn default_config_is_rejected_as_production() {
        let err = Config::default().validate().unwrap_err();
        assert!(err.contains("livekit"), "unexpected error: {err}");
    }

    #[test]
    fn explicit_dev_config_starts() {
        dev_config().validate().unwrap();
    }

    #[test]
    fn production_rejects_insecure_cookies() {
        let mut cfg = production_config();
        cfg.server.insecure_cookies = true;
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("insecure_cookies"), "unexpected error: {err}");
    }

    #[test]
    fn production_rejects_http_origins() {
        let mut cfg = production_config();
        cfg.server.public_origin = "http://chat.example.com".into();
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("https"), "unexpected error: {err}");
    }

    #[test]
    fn production_rejects_ws_livekit() {
        let mut cfg = production_config();
        cfg.media.livekit_url = "ws://lk.example.com".into();
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("wss"), "unexpected error: {err}");
    }

    #[test]
    fn production_rejects_placeholder_credentials() {
        let mut cfg = production_config();
        cfg.media.livekit_api_key = "CHANGE_ME".into();
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("placeholder"), "unexpected error: {err}");
    }

    #[test]
    fn production_rejects_empty_credentials() {
        let mut cfg = production_config();
        cfg.media.livekit_api_secret = String::new();
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("required"), "unexpected error: {err}");
    }

    #[test]
    fn complete_production_config_starts() {
        production_config().validate().unwrap();
    }
}
