//! Best-effort LiveKit media revocation.
//!
//! Business mutations commit in SQLite first. These tasks only inform LiveKit;
//! a failure never changes the HTTP/WS result and the token TTL remains the
//! final fallback.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::app::state::AppState;
use crate::config::Config;
use crate::infra::livekit::{self, RevocationError};
use crate::infra::repo;

#[derive(Default)]
pub struct LiveKitRevocationMetrics {
    attempts: AtomicU64,
    failures: AtomicU64,
}

impl LiveKitRevocationMetrics {
    pub fn record_attempt(&self) {
        self.attempts.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_failure(&self) {
        self.failures.fetch_add(1, Ordering::Relaxed);
    }

    pub fn attempted(&self) -> u64 {
        self.attempts.load(Ordering::Relaxed)
    }

    pub fn failed(&self) -> u64 {
        self.failures.load(Ordering::Relaxed)
    }
}

async fn revoke_with_retry(
    cfg: &Config,
    metrics: &LiveKitRevocationMetrics,
    room_code: &str,
    user_id: Option<&str>,
) {
    for attempt in 1..=3 {
        let result = match user_id {
            Some(user_id) => livekit::remove_participant(cfg, room_code, user_id).await,
            None => livekit::delete_room(cfg, room_code).await,
        };
        match result {
            Ok(()) => return,
            Err(RevocationError::Permanent(error)) => {
                metrics.record_failure();
                tracing::warn!(
                    room_code = %room_code,
                    user_id = user_id.unwrap_or(""),
                    attempt,
                    error = %error,
                    "livekit revocation failed permanently"
                );
                return;
            }
            Err(RevocationError::Transient(error)) if attempt < 3 => {
                let delay = if attempt == 1 { 100 } else { 300 };
                tracing::debug!(
                    room_code = %room_code,
                    user_id = user_id.unwrap_or(""),
                    attempt,
                    error = %error,
                    "livekit revocation failed transiently; retrying"
                );
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            Err(RevocationError::Transient(error)) => {
                metrics.record_failure();
                tracing::warn!(
                    room_code = %room_code,
                    user_id = user_id.unwrap_or(""),
                    attempts = 3,
                    error = %error,
                    "livekit revocation failed"
                );
                return;
            }
        }
    }
}

fn schedule(
    cfg: Arc<Config>,
    metrics: Arc<LiveKitRevocationMetrics>,
    room_code: String,
    user_id: Option<String>,
) {
    metrics.record_attempt();
    tokio::spawn(async move {
        revoke_with_retry(&cfg, &metrics, &room_code, user_id.as_deref()).await;
    });
}

/// Remove a participant from a single room.
pub fn schedule_participant(
    cfg: Arc<Config>,
    metrics: Arc<LiveKitRevocationMetrics>,
    room_code: String,
    user_id: String,
) {
    schedule(cfg, metrics, room_code, Some(user_id));
}

/// Delete a LiveKit room (used for room deletion/expiry).
pub fn schedule_room(cfg: Arc<Config>, metrics: Arc<LiveKitRevocationMetrics>, room_code: String) {
    schedule(cfg, metrics, room_code, None);
}

/// Account-wide events (disable, password reset): remove the user from every
/// room they still belong to. The DB read happens after the authoritative
/// mutation; a read failure is logged and does not block the caller.
pub async fn schedule_user(state: AppState, user_id: String) {
    let lookup = user_id.clone();
    let rooms = match state
        .db
        .with(move |conn| repo::list_member_rooms(conn, &lookup))
        .await
    {
        Ok(rooms) => rooms,
        Err(e) => {
            tracing::warn!(user_id = %user_id, error = %e.message, "livekit revocation room lookup failed");
            return;
        }
    };
    for room in rooms {
        schedule_participant(
            state.cfg.clone(),
            state.revocation.clone(),
            room.room_code,
            user_id.clone(),
        );
    }
}
