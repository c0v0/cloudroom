//! CloudRoom server library crate (binary `cloudroom-server` uses this).
//!
//! Layering: `api` (HTTP/WS transport) → `app` (background jobs) →
//! `domain` (models, room-code rules) → `infra` (SQLite, auth, LiveKit,
//! hub, rate limiting). `config`, `error`, `util` are shared by all layers.

pub mod api;
pub mod app;
pub mod config;
pub mod domain;
pub mod error;
pub mod infra;
pub mod util;

pub use config::Config;
pub use domain::models::*;
pub use error::{ApiError, ApiResult};
pub use infra::auth::Auth;
pub use infra::db::Db;
pub use infra::hub::Hub;
pub use infra::rate_limit::RateLimiter;
