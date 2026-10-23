//! In-memory WebSocket hub: per-room connection registry, broadcast, voice
//! presence, and signaling relay. Persistence stays in SQLite; this only
//! routes live events.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::{mpsc, Notify};

pub type UserId = String;

/// Outbound control message. `Close` is used by replacement/queue overflow and
/// lets the writer send a WebSocket close frame before exiting.
#[derive(Debug)]
pub enum ServerMessage {
    Text(String),
    Close,
}

struct Conn {
    tx: mpsc::Sender<ServerMessage>,
    close: Arc<Notify>,
}

#[derive(Default)]
struct RoomConns {
    conns: HashMap<UserId, Conn>,
    voice: HashSet<UserId>,
}

pub struct Hub {
    rooms: Mutex<HashMap<String, RoomConns>>,
    user_counts: Mutex<HashMap<UserId, usize>>,
    total: AtomicUsize,
    max_total: usize,
    max_per_user: usize,
}

impl Default for Hub {
    fn default() -> Self {
        Self::with_limits(1024, 4)
    }
}

impl Hub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_limits(max_total: usize, max_per_user: usize) -> Self {
        Self {
            rooms: Mutex::new(HashMap::new()),
            user_counts: Mutex::new(HashMap::new()),
            total: AtomicUsize::new(0),
            max_total,
            max_per_user,
        }
    }

    /// Register a connection. Returns false when the process/user cap is hit;
    /// the caller must close the socket and must not call `connection_closed`.
    /// A newer connection for the same user replaces the sender and asks the
    /// old writer to close, but the old task remains counted until teardown.
    pub fn register(
        &self,
        room_id: &str,
        user_id: &str,
        tx: mpsc::Sender<ServerMessage>,
        close: Arc<Notify>,
    ) -> bool {
        let mut counts = self.user_counts.lock();
        let count = counts.get(user_id).copied().unwrap_or(0);
        if self.total.load(Ordering::Relaxed) >= self.max_total || count >= self.max_per_user {
            return false;
        }
        *counts.entry(user_id.to_string()).or_insert(0) += 1;
        self.total.fetch_add(1, Ordering::Relaxed);
        drop(counts);

        let mut rooms = self.rooms.lock();
        let rc = rooms.entry(room_id.to_string()).or_default();
        // A new connection takes over the room slot; clear stale voice state
        // so the new connection can join again.
        rc.voice.remove(user_id);
        if let Some(old) = rc.conns.insert(user_id.to_string(), Conn { tx, close }) {
            old.close.notify_one();
        }
        true
    }

    /// Release the count for a connection after its read loop teardown.
    pub fn connection_closed(&self, user_id: &str) {
        let mut counts = self.user_counts.lock();
        if let Some(count) = counts.get_mut(user_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                counts.remove(user_id);
            }
        }
        self.total.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn unregister(&self, room_id: &str, user_id: &str, tx: &mpsc::Sender<ServerMessage>) {
        let mut rooms = self.rooms.lock();
        if let Some(rc) = rooms.get_mut(room_id) {
            // Only clean up this connection: a fast reconnect may have replaced
            // the sender, and the old teardown must not remove the new one.
            if rc
                .conns
                .get(user_id)
                .map(|conn| conn.tx.same_channel(tx))
                .unwrap_or(false)
            {
                rc.conns.remove(user_id);
                rc.voice.remove(user_id);
            }
            if rc.conns.is_empty() {
                rooms.remove(room_id);
            }
        }
    }

    /// Unconditional removal (member left / was removed from the room):
    /// every connection of that user is torn down.
    pub fn unregister_all(&self, room_id: &str, user_id: &str) {
        let mut rooms = self.rooms.lock();
        if let Some(rc) = rooms.get_mut(room_id) {
            if let Some(conn) = rc.conns.remove(user_id) {
                conn.close.notify_one();
            }
            rc.voice.remove(user_id);
            if rc.conns.is_empty() {
                rooms.remove(room_id);
            }
        }
    }

    /// Close every connection for a user across all rooms. Used when the
    /// account is disabled, its password is reset/changed, or it logs out.
    pub fn unregister_user(&self, user_id: &str) {
        let mut rooms = self.rooms.lock();
        for rc in rooms.values_mut() {
            if let Some(conn) = rc.conns.remove(user_id) {
                conn.close.notify_one();
            }
            rc.voice.remove(user_id);
        }
        rooms.retain(|_, rc| !rc.conns.is_empty());
    }

    pub fn broadcast(&self, room_id: &str, payload: &str) {
        let rooms = self.rooms.lock();
        if let Some(rc) = rooms.get(room_id) {
            for conn in rc.conns.values() {
                match conn.tx.try_send(ServerMessage::Text(payload.to_string())) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        // The queue is bounded; close the slow consumer rather
                        // than letting memory grow without limit.
                        conn.close.notify_one();
                        tracing::warn!(room_id, "ws outbound queue full; closing slow connection");
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {}
                }
            }
        }
    }

    /// Whether the user currently has a live connection in this room.
    pub fn is_connected(&self, room_id: &str, user_id: &str) -> bool {
        let rooms = self.rooms.lock();
        rooms
            .get(room_id)
            .map(|rc| rc.conns.contains_key(user_id))
            .unwrap_or(false)
    }

    /// Whether this exact sender is still the current connection for the user.
    /// A displaced socket uses this to stop processing events.
    pub fn is_current(
        &self,
        room_id: &str,
        user_id: &str,
        tx: &mpsc::Sender<ServerMessage>,
    ) -> bool {
        let rooms = self.rooms.lock();
        rooms
            .get(room_id)
            .and_then(|rc| rc.conns.get(user_id))
            .map(|conn| conn.tx.same_channel(tx))
            .unwrap_or(false)
    }

    pub fn send_to(&self, room_id: &str, to: &str, payload: &str) -> bool {
        let rooms = self.rooms.lock();
        let Some(conn) = rooms.get(room_id).and_then(|rc| rc.conns.get(to)) else {
            return false;
        };
        match conn.tx.try_send(ServerMessage::Text(payload.to_string())) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                conn.close.notify_one();
                tracing::warn!(room_id, user_id = %to, "ws outbound queue full; closing slow connection");
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    /// Add a user to the voice presence set. Returns true if newly added.
    pub fn voice_join(&self, room_id: &str, user_id: &str) -> bool {
        let mut rooms = self.rooms.lock();
        rooms
            .entry(room_id.to_string())
            .or_default()
            .voice
            .insert(user_id.to_string())
    }

    /// Remove a user from the voice set. Returns true if they were present.
    pub fn voice_leave(&self, room_id: &str, user_id: &str) -> bool {
        let mut rooms = self.rooms.lock();
        if let Some(rc) = rooms.get_mut(room_id) {
            rc.voice.remove(user_id)
        } else {
            false
        }
    }

    pub fn voice_members(&self, room_id: &str) -> Vec<String> {
        let rooms = self.rooms.lock();
        rooms
            .get(room_id)
            .map(|rc| rc.voice.iter().cloned().collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn channel() -> (
        mpsc::Sender<ServerMessage>,
        mpsc::Receiver<ServerMessage>,
        Arc<Notify>,
    ) {
        let (tx, rx) = mpsc::channel(1);
        (tx, rx, Arc::new(Notify::new()))
    }

    #[test]
    fn connection_caps_are_enforced_and_released() {
        let hub = Hub::with_limits(2, 1);
        let (tx1, _rx1, close1) = channel();
        let (tx2, _rx2, close2) = channel();
        let (tx3, _rx3, close3) = channel();

        assert!(hub.register("room-a", "u1", tx1, close1));
        let (tx1b, _rx1b, close1b) = channel();
        assert!(!hub.register("room-b", "u1", tx1b, close1b));
        assert!(hub.register("room-a", "u2", tx2, close2));
        assert!(!hub.register("room-a", "u3", tx3, close3));

        hub.connection_closed("u1");
        let (tx3b, _rx3b, close3b) = channel();
        assert!(hub.register("room-a", "u3", tx3b, close3b));
    }

    #[tokio::test]
    async fn full_queue_closes_slow_consumer() {
        let hub = Hub::with_limits(1, 1);
        let (tx, _rx, close) = channel();
        assert!(hub.register("room-a", "u1", tx, close.clone()));
        assert!(hub.send_to("room-a", "u1", "first"));
        assert!(!hub.send_to("room-a", "u1", "second"));
        tokio::time::timeout(Duration::from_millis(50), close.notified())
            .await
            .expect("slow consumer should be notified");
    }

    #[test]
    fn displaced_sender_is_not_current() {
        let hub = Hub::with_limits(2, 2);
        let (tx1, _rx1, close1) = channel();
        let (tx2, _rx2, close2) = channel();
        assert!(hub.register("room-a", "u1", tx1.clone(), close1));
        assert!(hub.is_current("room-a", "u1", &tx1));
        assert!(hub.register("room-a", "u1", tx2.clone(), close2));
        assert!(!hub.is_current("room-a", "u1", &tx1));
        assert!(hub.is_current("room-a", "u1", &tx2));
    }
}
