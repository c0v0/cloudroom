//! CloudRoom server library crate (binary `cloudroom-server` uses this).

pub mod api;
pub mod auth;
pub mod avatars;
pub mod codes;
pub mod config;
pub mod db;
pub mod error;
pub mod livekit;
pub mod models;
pub mod rate_limit;
pub mod repo;
pub mod sweeper;
pub mod util;
pub mod ws;

pub use auth::Auth;
pub use config::Config;
pub use db::Db;
pub use error::{ApiError, ApiResult};
pub use models::*;
pub use rate_limit::RateLimiter;
pub use ws::Hub;
