//! HTTP API transport layer: router, shared state, auth extractors,
//! request guards, and JSON views.

pub mod admin;
pub mod auth_routes;
pub mod extract;
pub mod guards;
pub mod json;
pub mod me;
pub mod rooms;
pub mod routes;
pub mod security;
pub mod state;
pub mod ws;

pub use extract::{AuthUser, MaybeAuth};
pub use guards::{active_member_room, audited, can, require_cap};
pub use json::{
    audit_json, invite_json, member_json, message_json, requests_json, room_json,
    room_request_json, user_json,
};
pub use routes::{build_router, use_state};
pub use security::{check_ws_origin, client_ip, require_csrf};
pub use state::AppState;
