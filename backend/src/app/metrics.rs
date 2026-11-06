//! Lightweight Prometheus counters/gauges. The endpoint lives at `/metrics`
//! (outside `/api/v1`), so the standard Nginx API proxy does not expose it.

use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub struct Metrics {
    ws_connections_total: AtomicU64,
    messages_total: AtomicU64,
}

impl Metrics {
    pub fn ws_connected(&self) {
        self.ws_connections_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn message_sent(&self) {
        self.messages_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn ws_connections_total(&self) -> u64 {
        self.ws_connections_total.load(Ordering::Relaxed)
    }

    pub fn messages_total(&self) -> u64 {
        self.messages_total.load(Ordering::Relaxed)
    }
}
