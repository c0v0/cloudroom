//! Application events broadcast to a room over the in-memory hub.

use serde_json::Value;

use crate::app::state::AppState;

// --------------------------------------------------------------- helpers

pub(crate) fn broadcast_room_event(state: &AppState, room_id: &str, event: &Value) {
    if let Ok(s) = serde_json::to_string(event) {
        state.hub.broadcast(room_id, &s);
    }
}
