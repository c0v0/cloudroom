//! Infrastructure layer: SQLite access, sessions/passwords, avatars,
//! LiveKit credentials, in-memory WebSocket hub, and rate limiting.

pub mod auth;
pub mod avatars;
pub mod db;
pub mod hub;
pub mod livekit;
pub mod rate_limit;
pub mod repo;
