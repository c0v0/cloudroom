//! Shared application state handed to every handler.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::FromRef;

use crate::config::Config;
use crate::infra::auth::Auth;
use crate::infra::db::Db;
use crate::infra::hub::Hub;
use crate::infra::rate_limit::RateLimiter;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub cfg: Arc<Config>,
    pub auth: Arc<Auth>,
    pub hub: Arc<Hub>,
    pub rl: Arc<RateLimiter>,
    pub avatars_dir: PathBuf,
}

impl FromRef<AppState> for Arc<Config> {
    fn from_ref(s: &AppState) -> Self {
        s.cfg.clone()
    }
}

impl FromRef<AppState> for Arc<Auth> {
    fn from_ref(s: &AppState) -> Self {
        s.auth.clone()
    }
}

impl FromRef<AppState> for Db {
    fn from_ref(s: &AppState) -> Self {
        s.db.clone()
    }
}

impl FromRef<AppState> for Arc<Hub> {
    fn from_ref(s: &AppState) -> Self {
        s.hub.clone()
    }
}

impl FromRef<AppState> for Arc<RateLimiter> {
    fn from_ref(s: &AppState) -> Self {
        s.rl.clone()
    }
}
