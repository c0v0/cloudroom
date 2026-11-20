//! Short-window Web Push coalescing.
//!
//! A burst of messages in the same room/thread would otherwise send one push
//! per message. Pending notifications are kept per `(user, tag)` and the last
//! enqueue in the window wins; the payload carries the latest body plus a
//! `N 条新消息` suffix. Tasks that lost the race exit without sending.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::Value;

use crate::config::PushCfg;
use crate::infra::db::Db;

pub struct PushDigest {
    pending: Mutex<HashMap<String, Pending>>,
    window: Duration,
}

struct Pending {
    user_id: String,
    payload: Value,
    count: u32,
    generation: u64,
}

impl PushDigest {
    pub fn new(window_seconds: u64) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            window: Duration::from_secs(window_seconds),
        }
    }

    /// Queue the same payload for each user. Coalescing key includes the tag
    /// so room and thread notifications remain separate.
    pub fn enqueue(self: &Arc<Self>, db: Db, cfg: PushCfg, user_ids: Vec<String>, payload: Value) {
        if !cfg.enabled() || user_ids.is_empty() {
            return;
        }
        let tag = payload
            .get("tag")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        for uid in user_ids {
            let key = format!("{uid}\u{0}{tag}");
            let generation = {
                let mut map = self.pending.lock();
                let entry = map.entry(key.clone()).or_insert_with(|| Pending {
                    user_id: uid.clone(),
                    payload: payload.clone(),
                    count: 0,
                    generation: 0,
                });
                entry.payload = payload.clone();
                entry.count = entry.count.saturating_add(1);
                entry.generation = entry.generation.wrapping_add(1);
                entry.generation
            };
            let digest = self.clone();
            let db = db.clone();
            let cfg = cfg.clone();
            tokio::spawn(async move {
                tokio::time::sleep(digest.window).await;
                let pending = {
                    let mut map = digest.pending.lock();
                    match map.get(&key) {
                        Some(p) if p.generation == generation => map.remove(&key),
                        _ => None,
                    }
                };
                let Some(pending) = pending else { return };
                let mut payload = pending.payload;
                if pending.count > 1 {
                    let base = payload
                        .get("body")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    payload["body"] = Value::String(digest_body(&base, pending.count));
                }
                crate::infra::push::notify_users(&db, &cfg, vec![pending.user_id], payload).await;
            });
        }
    }
}

/// Format used by the push payload; kept separate for deterministic tests.
pub(crate) fn digest_body(base: &str, count: u32) -> String {
    if count <= 1 {
        return base.to_string();
    }
    if base.is_empty() {
        format!("{count} 条新消息")
    } else {
        format!("{base}（{count} 条新消息）")
    }
}

#[cfg(test)]
mod tests {
    use super::{digest_body, PushDigest};
    use crate::config::PushCfg;
    use crate::infra::db::Db;
    use serde_json::json;
    use std::sync::Arc;

    #[tokio::test]
    async fn coalesces_same_user_and_tag() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Db::open(&dir.path().join("db.sqlite")).unwrap();
        let digest = Arc::new(PushDigest::new(60));
        let cfg = PushCfg {
            vapid_public_key: "key".into(),
            vapid_private_key: "private".into(),
            subject: "mailto:test@example.com".into(),
            digest_seconds: 60,
        };
        digest.enqueue(
            db.clone(),
            cfg.clone(),
            vec!["u1".into()],
            json!({"tag": "room-a", "body": "first"}),
        );
        digest.enqueue(
            db,
            cfg,
            vec!["u1".into()],
            json!({"tag": "room-a", "body": "second"}),
        );
        let map = digest.pending.lock();
        assert_eq!(map.len(), 1);
        let pending = map.values().next().unwrap();
        assert_eq!(pending.count, 2);
        assert_eq!(pending.payload["body"], "second");
    }

    #[test]
    fn digest_body_only_adds_count_for_multiple_messages() {
        assert_eq!(digest_body("hi", 1), "hi");
        assert_eq!(digest_body("hi", 2), "hi（2 条新消息）");
        assert_eq!(digest_body("", 3), "3 条新消息");
    }
}
