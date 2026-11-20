//! Infrastructure layer: SQLite access, sessions/passwords, avatars,
//! LiveKit credentials, in-memory WebSocket hub, and rate limiting.

pub mod auth;
pub mod avatars;
pub mod db;
pub mod hub;
pub mod link_preview;
pub mod livekit;
pub mod oidc;
pub mod push;
pub mod rate_limit;
pub mod repo;
pub mod thumbnails;
pub mod totp;
