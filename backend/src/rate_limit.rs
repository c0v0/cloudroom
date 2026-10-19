//! In-memory sliding-window rate limiter keyed by (bucket, key).
//! Buckets are per-IP or per-account as configured.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Default)]
struct BucketState {
    hits: Vec<Instant>,
}

#[derive(Default)]
pub struct RateLimiter {
    buckets: Mutex<HashMap<(String, String), BucketState>>,
}

impl RateLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns true when the request is allowed, false when rate-limited.
    pub fn allow(&self, bucket: &str, key: &str, window: Duration, max: u32) -> bool {
        let mut map = self.buckets.lock();
        let now = Instant::now();
        let state = map
            .entry((bucket.to_string(), key.to_string()))
            .or_default();
        state.hits.retain(|t| now.duration_since(*t) < window);
        if state.hits.len() >= max as usize {
            return false;
        }
        state.hits.push(now);
        true
    }

    /// Drop all state for a key (e.g. after successful login).
    pub fn reset(&self, bucket: &str, key: &str) {
        let mut map = self.buckets.lock();
        map.remove(&(bucket.to_string(), key.to_string()));
    }

    /// Periodically shed stale buckets.
    pub fn prune(&self) {
        let mut map = self.buckets.lock();
        let now = Instant::now();
        map.retain(|_, st| {
            st.hits
                .retain(|t| now.duration_since(*t) < Duration::from_secs(3600));
            !st.hits.is_empty()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_and_resets() {
        let rl = RateLimiter::new();
        for _ in 0..3 {
            assert!(rl.allow("t", "k", Duration::from_secs(60), 3));
        }
        assert!(!rl.allow("t", "k", Duration::from_secs(60), 3));
        rl.reset("t", "k");
        assert!(rl.allow("t", "k", Duration::from_secs(60), 3));
    }

    #[test]
    fn separate_keys_are_independent() {
        let rl = RateLimiter::new();
        for _ in 0..5 {
            assert!(rl.allow("t", "a", Duration::from_secs(60), 5));
        }
        assert!(rl.allow("t", "b", Duration::from_secs(60), 5));
    }
}
