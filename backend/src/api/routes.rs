//! Router construction, public endpoints, and access logging.

use axum::extract::{DefaultBodyLimit, State};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::json;

use crate::api::{admin, auth_routes, me, rooms, ws, AppState};

// ------------------------------------------------------------------ router

pub fn build_router(state: AppState) -> Router {
    let public = Router::new()
        .route("/health", get(health))
        .route("/config/public", get(public_config))
        .route("/auth/register", post(auth_routes::register))
        .route("/auth/login", post(auth_routes::login))
        .route("/auth/logout", post(auth_routes::logout))
        .route("/auth/upgrade-guest", post(auth_routes::upgrade_guest))
        .route("/avatars/{avatar_id}", get(me::get_avatar))
        // Auth bodies are tiny; reject oversized JSON before extraction.
        .layer(DefaultBodyLimit::max(64 * 1024));

    let authed = Router::new()
        .route("/me", get(me::me))
        .route("/me/password", post(me::change_password))
        .route(
            "/me/avatar",
            post(me::upload_avatar).delete(me::delete_avatar),
        )
        .route("/me/rooms", get(me::my_rooms))
        .route(
            "/room-requests",
            post(rooms::create_room_request).get(rooms::my_room_requests),
        )
        .route(
            "/room-requests/{id}/cancel",
            post(rooms::cancel_room_request),
        )
        .route("/rooms/{room_code}", get(rooms::get_room))
        .route("/rooms/{room_code}/join", post(rooms::join_room))
        .route("/rooms/{room_code}/leave", post(rooms::leave_room))
        .route("/rooms/{room_code}/settings", post(rooms::update_settings))
        .route("/rooms/{room_code}/delete", post(rooms::delete_room))
        .route(
            "/rooms/{room_code}/invites",
            post(rooms::create_invite).get(rooms::list_invites),
        )
        .route(
            "/rooms/{room_code}/invites/{invite_id}/revoke",
            post(rooms::revoke_invite),
        )
        .route(
            "/rooms/{room_code}/members/{user_id}/role",
            post(rooms::set_member_role),
        )
        .route(
            "/rooms/{room_code}/members/{user_id}/nickname",
            post(rooms::set_nickname),
        )
        .route(
            "/rooms/{room_code}/members/{user_id}/remove",
            post(rooms::remove_member),
        )
        .route(
            "/rooms/{room_code}/mutes",
            post(rooms::create_mute).get(rooms::list_mutes),
        )
        .route(
            "/rooms/{room_code}/mutes/{mute_id}",
            delete(rooms::remove_mute),
        )
        .route(
            "/rooms/{room_code}/messages",
            get(rooms::get_messages).post(rooms::send_message_http),
        )
        .route("/rooms/{room_code}/direct", post(rooms::create_direct_room))
        .route("/rooms/{room_code}/ice-servers", get(rooms::ice_servers))
        .route("/voice/options", get(rooms::voice_options))
        .route("/rooms/{room_code}/media-token", post(rooms::media_token))
        .route("/rooms/{room_code}/ws", get(ws::ws_handler))
        .route("/admin/room-requests", get(admin::list_room_requests))
        .route(
            "/admin/room-requests/{id}/approve",
            post(admin::approve_room_request),
        )
        .route(
            "/admin/room-requests/{id}/reject",
            post(admin::reject_room_request),
        )
        .route("/admin/rooms", get(admin::list_rooms))
        .route("/admin/rooms/{room_code}", get(admin::room_detail))
        .route(
            "/admin/rooms/{room_code}/delete",
            post(admin::force_delete_room),
        )
        .route(
            "/admin/rooms/{room_code}/transfer-owner",
            post(admin::transfer_owner),
        )
        .route("/admin/users", get(admin::list_users))
        .route(
            "/admin/users/{user_id}/disable",
            post(admin::set_user_disabled),
        )
        .route(
            "/admin/users/{user_id}/reset-password",
            post(admin::reset_password),
        )
        .route("/admin/users/{user_id}/role", post(admin::set_user_role))
        .route(
            "/admin/invites",
            post(admin::create_registration_invite).get(admin::list_registration_invites),
        )
        .route(
            "/admin/invites/{invite_id}/revoke",
            post(admin::revoke_registration_invite),
        )
        .route("/admin/audit", get(admin::list_audit))
        .route("/admin/stats", get(admin::stats));

    let max_body_bytes = state.cfg.server.max_body_bytes;
    Router::new()
        .nest("/api/v1", public.merge(authed))
        .with_state(state)
        .layer(DefaultBodyLimit::max(max_body_bytes))
        .layer(axum::middleware::from_fn(trace_layer))
}

// ------------------------------------------------------------- health etc.

async fn health(State(state): axum::extract::State<AppState>) -> impl IntoResponse {
    let sqlite_ok = state
        .db
        .with(|conn| crate::infra::repo::count_users(conn).map(|_| true))
        .await
        .is_ok();
    let mut checks = json!({
        "sqlite": if sqlite_ok { "ok" } else { "error" },
    });
    // LiveKit 配置已通过启动校验；这里只报告配置状态，不做网络探测。
    checks["livekit"] = json!("configured");
    Json(json!({
        "status": if sqlite_ok { "ok" } else { "degraded" },
        "checks": checks,
        "time": chrono::Utc::now().to_rfc3339(),
    }))
}

async fn public_config(State(state): axum::extract::State<AppState>) -> impl IntoResponse {
    let bootstrap_pending = state
        .db
        .with(|conn| crate::infra::repo::super_admin_exists(conn).map(|b| !b))
        .await
        .unwrap_or(false);
    Json(json!({
        "registration_mode": state.cfg.auth.registration_mode,
        "bootstrap_pending": bootstrap_pending,
        "features": {
            "voice": true,
            "screen_share": false,
            "media_playback": false,
            "voice_modes": ["p2p", "sfu"],
            "p2p_voice_max_participants": state.cfg.media.p2p_voice_max_participants,
        },
        "limits": {
            "max_avatar_bytes": state.cfg.security.max_avatar_bytes,
            "max_avatar_dimension": state.cfg.security.max_avatar_dimension,
            "max_nickname_graphemes": state.cfg.security.max_nickname_graphemes,
            "max_message_chars": state.cfg.rooms.max_message_chars,
        },
        "rooms": {
            "generated_code_length": state.cfg.rooms.generated_code_length,
        },
    }))
}

/// Access log with redaction: never log cookies, tokens, passwords, bodies.
async fn trace_layer(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let start = std::time::Instant::now();
    let resp = next.run(req).await;
    tracing::info!(
        method = %method,
        path = %path,
        status = resp.status().as_u16(),
        ms = start.elapsed().as_millis(),
        "http"
    );
    resp
}

pub fn use_state(state: AppState) -> Router {
    build_router(state)
}
