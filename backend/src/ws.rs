//! In-memory WebSocket hub: per-room connection registry, broadcast, voice
//! presence, and signaling relay. Persistence stays in SQLite; this only
//! routes live events.

use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};

use tokio::sync::mpsc;

pub type UserId = String;

#[derive(Default)]
struct RoomConns {
    conns: HashMap<UserId, mpsc::UnboundedSender<String>>,
    voice: HashSet<UserId>,
}

#[derive(Default)]
pub struct Hub {
    rooms: Mutex<HashMap<String, RoomConns>>,
}

impl Hub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or replace) a connection for a user in a room.
    pub fn register(&self, room_id: &str, user_id: &str, tx: mpsc::UnboundedSender<String>) {
        let mut rooms = self.rooms.lock();
        let rc = rooms.entry(room_id.to_string()).or_default();
        // 新连接接管：清除旧连接残留的语音状态（旧 teardown 不会误删新连接，
        // 因此这里必须主动清，否则重连后 voice_join 会被去重拒绝）。
        rc.voice.remove(user_id);
        rc.conns.insert(user_id.to_string(), tx);
    }

    pub fn unregister(&self, room_id: &str, user_id: &str, tx: &mpsc::UnboundedSender<String>) {
        let mut rooms = self.rooms.lock();
        if let Some(rc) = rooms.get_mut(room_id) {
            // 只清理自己的连接：同一用户快速重连时，旧连接的 teardown
            // 不得误删新连接（否则新连接收不到广播且 voice 状态残留）。
            if rc
                .conns
                .get(user_id)
                .map(|t| t.same_channel(tx))
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
            rc.conns.remove(user_id);
            rc.voice.remove(user_id);
            if rc.conns.is_empty() {
                rooms.remove(room_id);
            }
        }
    }

    pub fn broadcast(&self, room_id: &str, payload: &str) {
        let rooms = self.rooms.lock();
        if let Some(rc) = rooms.get(room_id) {
            for tx in rc.conns.values() {
                let _ = tx.send(payload.to_string());
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

    pub fn send_to(&self, room_id: &str, to: &str, payload: &str) -> bool {
        let rooms = self.rooms.lock();
        if let Some(rc) = rooms.get(room_id) {
            if let Some(tx) = rc.conns.get(to) {
                return tx.send(payload.to_string()).is_ok();
            }
        }
        false
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
