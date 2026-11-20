//! Durable LiveKit media revocation.
//!
//! Business mutations commit in SQLite first. A revocation row is persisted in
//! the same database, then attempted immediately. Transient failures remain
//! queued and are retried by the sweeper until LiveKit accepts the request or
//! the token TTL makes it irrelevant.
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::app::state::AppState;
use crate::config::Config;
use crate::infra::db::Db;
use crate::infra::livekit::{self, RevocationError};
use crate::infra::repo::{self, LivekitRevocation};

const MAX_PERSISTED_ATTEMPTS: i64 = 20;
const DUE_BATCH: i64 = 50;

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

enum RevokeOutcome {
    Done,
    Permanent(String),
    Transient(String),
}

async fn revoke_once(cfg: &Config, room_code: &str, user_id: Option<&str>) -> RevokeOutcome {
    for attempt in 1..=3 {
        let result = match user_id {
            Some(user_id) => livekit::remove_participant(cfg, room_code, user_id).await,
            None => livekit::delete_room(cfg, room_code).await,
        };
        match result {
            Ok(()) => return RevokeOutcome::Done,
            Err(RevocationError::Permanent(error)) => {
                tracing::warn!(
                    room_code = %room_code,
                    user_id = user_id.unwrap_or(""),
                    attempt,
                    error = %error,
                    "livekit revocation failed permanently"
                );
                return RevokeOutcome::Permanent(error);
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
                tracing::warn!(
                    room_code = %room_code,
                    user_id = user_id.unwrap_or(""),
                    attempts = 3,
                    error = %error,
                    "livekit revocation failed; queued for retry"
                );
                return RevokeOutcome::Transient(error);
            }
        }
    }
    RevokeOutcome::Transient("unknown revocation failure".into())
}

fn backoff_seconds(attempts: i64) -> i64 {
    let exponent = attempts.clamp(1, 8) as u32;
    (60i64 * (1i64 << exponent)).min(3600)
}

async fn process_job(
    db: Db,
    cfg: Arc<Config>,
    metrics: Arc<LiveKitRevocationMetrics>,
    job: LivekitRevocation,
) {
    metrics.record_attempt();
    let user_id = (job.action == "participant").then_some(job.target.as_str());
    let outcome = revoke_once(&cfg, &job.room_code, user_id).await;
    match outcome {
        RevokeOutcome::Done => {
            let _ = db.with_sync(|conn| repo::delete_livekit_revocation(conn, job.id));
        }
        RevokeOutcome::Permanent(error) => {
            metrics.record_failure();
            tracing::warn!(
                room_code = %job.room_code,
                target = %job.target,
                error = %error,
                "livekit revocation dropped after permanent failure"
            );
            let _ = db.with_sync(|conn| repo::delete_livekit_revocation(conn, job.id));
        }
        RevokeOutcome::Transient(error) => {
            metrics.record_failure();
            let next_attempts = job.attempts + 1;
            if next_attempts >= MAX_PERSISTED_ATTEMPTS {
                tracing::warn!(
                    room_code = %job.room_code,
                    target = %job.target,
                    attempts = next_attempts,
                    error = %error,
                    "livekit revocation exhausted retry budget"
                );
                let _ = db.with_sync(|conn| repo::delete_livekit_revocation(conn, job.id));
                return;
            }
            let next =
                crate::util::now() + chrono::Duration::seconds(backoff_seconds(next_attempts));
            let next = next.to_rfc3339();
            let error = error.chars().take(500).collect::<String>();
            let _ = db
                .with_sync(|conn| repo::reschedule_livekit_revocation(conn, job.id, &next, &error));
        }
    }
}

fn schedule(
    db: Db,
    cfg: Arc<Config>,
    metrics: Arc<LiveKitRevocationMetrics>,
    room_code: String,
    target: String,
    action: &'static str,
) {
    let now = crate::util::now_rfc3339();
    let job = match db
        .with_sync(|conn| repo::enqueue_livekit_revocation(conn, &room_code, &target, action, &now))
    {
        Ok(job) => job,
        Err(e) => {
            tracing::warn!(
                room_code = %room_code,
                target = %target,
                error = %e.message,
                "livekit revocation could not be persisted; attempting once"
            );
            LivekitRevocation {
                id: 0,
                room_code,
                target,
                action: action.to_string(),
                attempts: 0,
            }
        }
    };
    tokio::spawn(async move {
        process_job(db, cfg, metrics, job).await;
    });
}

/// Remove a participant from a single room.
pub fn schedule_participant(
    db: Db,
    cfg: Arc<Config>,
    metrics: Arc<LiveKitRevocationMetrics>,
    room_code: String,
    user_id: String,
) {
    schedule(db, cfg, metrics, room_code, user_id, "participant");
}

/// Delete a LiveKit room (used for room deletion/expiry).
pub fn schedule_room(
    db: Db,
    cfg: Arc<Config>,
    metrics: Arc<LiveKitRevocationMetrics>,
    room_code: String,
) {
    schedule(db, cfg, metrics, room_code, String::new(), "room");
}

/// Process due durable revocation rows. Called by the background sweeper.
pub async fn process_due(db: &Db, cfg: Arc<Config>, metrics: Arc<LiveKitRevocationMetrics>) {
    let now = crate::util::now_rfc3339();
    let jobs = match db
        .read(move |conn| repo::due_livekit_revocations(conn, &now, DUE_BATCH))
        .await
    {
        Ok(jobs) => jobs,
        Err(e) => {
            tracing::warn!(error = %e.message, "livekit revocation queue read failed");
            return;
        }
    };
    for job in jobs {
        process_job(db.clone(), cfg.clone(), metrics.clone(), job).await;
    }
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
            state.db.clone(),
            state.cfg.clone(),
            state.revocation.clone(),
            room.room_code,
            user_id.clone(),
        );
    }
}
