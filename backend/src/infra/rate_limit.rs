//! In-memory sliding-window rate limiter keyed by (bucket, key).
//! Buckets are per-IP or per-account as configured.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::time::{Duration, Instant};

struct BucketState {
    hits: Vec<Instant>,
    last_seen: Instant,
    window: Duration,
}

impl Default for BucketState {
    fn default() -> Self {
        Self {
            hits: Vec::new(),
            last_seen: Instant::now(),
            window: Duration::from_secs(60),
        }
    }
}

const MAX_KEY_CHARS: usize = 128;
const MAX_BUCKETS: usize = 100_000;

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
        let key: String = key.chars().take(MAX_KEY_CHARS).collect();
        if !map.contains_key(&(bucket.to_string(), key.clone())) && map.len() >= MAX_BUCKETS {
            // Never evict a live throttle just because an attacker filled the
            // table: that would reset brute-force limits on demand. Scan a
            // small bounded sample for an expired bucket; if none exists,
            // fail closed for the new key until prune() runs.
            let mut expired = None;
            for (candidate, st) in map.iter().take(64) {
                let live = st.hits.iter().any(|t| now.duration_since(*t) < st.window);
                if !live {
                    expired = Some(candidate.clone());
                    break;
                }
            }
            match expired {
                Some(candidate) => {
                    map.remove(&candidate);
                }
                None => return false,
            }
        }
        let state = map.entry((bucket.to_string(), key)).or_default();
        state.window = window;
        state.last_seen = now;
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
        let key: String = key.chars().take(MAX_KEY_CHARS).collect();
        map.remove(&(bucket.to_string(), key));
    }

    /// Periodically shed stale buckets.
    pub fn prune(&self) {
        let mut map = self.buckets.lock();
        let now = Instant::now();
        map.retain(|_, st| {
            st.hits.retain(|t| now.duration_since(*t) < st.window);
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

    #[test]
    fn long_keys_share_the_truncated_bucket() {
        let rl = RateLimiter::new();
        let prefix = "x".repeat(MAX_KEY_CHARS);
        assert!(rl.allow("t", &format!("{prefix}a"), Duration::from_secs(60), 1));
        assert!(!rl.allow("t", &format!("{prefix}b"), Duration::from_secs(60), 1));
    }
}
