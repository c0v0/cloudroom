//! Router construction, public endpoints, and access logging.

use std::time::Duration;

use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderName, HeaderValue, Method};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::json;
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::api::{admin, auth_routes, emojis, export, inbox, link_preview, me, oidc, push, rooms, scheduled, totp, webhooks, ws, AppState};

// ------------------------------------------------------------------ router

pub fn build_router(state: AppState) -> Router {
    let public = Router::new()
        .route("/health", get(health))
        .route("/config/public", get(public_config))
        .route("/auth/register", post(auth_routes::register))
        .route("/auth/login", post(auth_routes::login))
        .route("/auth/logout", post(auth_routes::logout))
        .route("/auth/upgrade-guest", post(auth_routes::upgrade_guest))
        .route("/auth/oidc/login", get(oidc::login))
        .route("/auth/oidc/callback", get(oidc::callback))
        .route("/auth/login/totp", post(auth_routes::login_totp))
        .route("/webhooks/{token}", post(webhooks::ingest))
        .route("/avatars/{avatar_id}", get(me::get_avatar))
        // Auth bodies are tiny; reject oversized JSON before extraction.
        .layer(DefaultBodyLimit::max(64 * 1024));

    let authed = Router::new()
        .route("/me", get(me::me))
        .route("/inbox", get(inbox::inbox))
        .route("/link-preview", get(link_preview::get))
        .route("/emojis", get(emojis::list))
        .route("/emojis/{emoji_id}/image", get(emojis::image))
        .route("/me/password", post(me::change_password))
        .route("/me/totp/setup", post(totp::setup))
        .route("/me/totp/enable", post(totp::enable))
        .route("/me/totp/disable", post(totp::disable))
        .route(
            "/me/avatar",
            post(me::upload_avatar).delete(me::delete_avatar),
        )
        .route("/me/rooms", get(me::my_rooms))
        .route("/me/bookmarks", get(me::bookmarks))
        .route(
            "/me/notification-prefs",
            get(me::get_notification_prefs).put(me::set_notification_prefs),
        )
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
            "/rooms/{room_code}/webhooks",
            post(webhooks::create).get(webhooks::list),
        )
        .route(
            "/rooms/{room_code}/webhooks/{webhook_id}/revoke",
            post(webhooks::revoke),
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
            "/rooms/{room_code}/guest-upgrade-requests",
            post(rooms::request_guest_upgrade),
        )
        .route(
            "/rooms/{room_code}/messages",
            get(rooms::get_messages).post(rooms::send_message_http),
        )
        .route("/rooms/{room_code}/direct", post(rooms::create_direct_room))
        .route("/rooms/{room_code}/ice-servers", get(rooms::ice_servers))
        .route("/voice/options", get(rooms::voice_options))
        .route("/push/public-key", get(push::public_key))
        .route("/push/subscribe", post(push::subscribe))
        .route("/push/unsubscribe", post(push::unsubscribe))
        .route("/rooms/{room_code}/media-token", post(rooms::media_token))
        .route(
            "/rooms/{room_code}/messages/{message_id}/edit",
            post(rooms::edit_message),
        )
        .route(
            "/rooms/{room_code}/messages/{message_id}/delete",
            post(rooms::delete_message),
        )
        .route(
            "/rooms/{room_code}/messages/search",
            get(rooms::search_messages),
        )
        .route(
            "/rooms/{room_code}/bookmarks",
            get(rooms::room_bookmarks),
        )
        .route(
            "/rooms/{room_code}/messages/{message_id}/bookmark",
            post(rooms::bookmark_message).delete(rooms::unbookmark_message),
        )
        .route("/search", get(rooms::global_search))
        .route("/rooms/{room_code}/pins", get(rooms::list_pins))
        .route("/rooms/{room_code}/export", get(export::export_room))
        .route("/rooms/{room_code}/messages/schedule", post(scheduled::create))
        .route("/rooms/{room_code}/scheduled", get(scheduled::list))
        .route(
            "/rooms/{room_code}/scheduled/{schedule_id}/cancel",
            post(scheduled::cancel),
        )
        .route(
            "/rooms/{room_code}/messages/{message_id}/pin",
            post(rooms::pin_message),
        )
        .route(
            "/rooms/{room_code}/messages/{message_id}/unpin",
            post(rooms::unpin_message),
        )
        .route(
            "/rooms/{room_code}/messages/{message_id}/forward",
            post(rooms::forward_message),
        )
        .route(
            "/rooms/{room_code}/messages/{message_id}/thread",
            get(rooms::get_thread),
        )
        .route(
            "/rooms/{room_code}/messages/{message_id}/thread/read",
            post(rooms::mark_thread_read),
        )
        .route(
            "/rooms/{room_code}/attachments",
            post(rooms::upload_attachment)
                .layer(DefaultBodyLimit::max(state.cfg.max_upload_body_bytes())),
        )
        .route(
            "/rooms/{room_code}/attachments/{attachment_id}",
            get(rooms::download_attachment),
        )
        .route(
            "/rooms/{room_code}/attachments/{attachment_id}/thumbnail",
            get(rooms::download_thumbnail),
        )
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
            "/admin/guest-upgrade-requests",
            get(admin::list_guest_upgrade_requests),
        )
        .route(
            "/admin/guest-upgrade-requests/{id}/approve",
            post(admin::approve_guest_upgrade_request),
        )
        .route(
            "/admin/guest-upgrade-requests/{id}/reject",
            post(admin::reject_guest_upgrade_request),
        )
        .route(
            "/admin/invites",
            post(admin::create_registration_invite).get(admin::list_registration_invites),
        )
        .route(
            "/admin/invites/{invite_id}/revoke",
            post(admin::revoke_registration_invite),
        )
        .route("/admin/audit", get(admin::list_audit))
        .route("/admin/audit/export", get(admin::export_audit))
        .route("/admin/emojis", post(emojis::create))
        .route("/admin/emojis/{emoji_id}", delete(emojis::delete))
        .route("/admin/stats", get(admin::stats))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            renew_session_layer,
        ));

    let max_body_bytes = state.cfg.server.max_body_bytes;
    let cors = cors_layer(&state);
    Router::new()
        .nest("/api/v1", public.merge(authed))
        .with_state(state)
        .layer(DefaultBodyLimit::max(max_body_bytes))
        .layer(axum::middleware::from_fn(trace_layer))
        .layer(cors)
}

fn cors_layer(state: &AppState) -> CorsLayer {
    let origins: Vec<HeaderValue> = state
        .cfg
        .allowed_origins()
        .iter()
        .filter_map(|origin| origin.parse::<HeaderValue>().ok())
        .collect();
    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_credentials(true)
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::OPTIONS])
        .allow_headers([
            header::CONTENT_TYPE,
            HeaderName::from_static("x-csrf-token"),
        ])
        .max_age(Duration::from_secs(600))
}

// ------------------------------------------------------------- health etc.

async fn health(State(state): axum::extract::State<AppState>) -> impl IntoResponse {
    let sqlite_ok = state
        .db
        .read(|conn| {
            conn.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))
                .map(|_| ())
                .map_err(crate::error::ApiError::from)
        })
        .await
        .is_ok();
    let mut checks = json!({
        "sqlite": if sqlite_ok { "ok" } else { "error" },
    });
    // LiveKit 配置已通过启动校验；这里只报告配置状态，不做网络探测。
    checks["livekit"] = json!("configured");
    let status = if sqlite_ok {
        axum::http::StatusCode::OK
    } else {
        // 数据库不可用时必须让负载均衡/监控看到失败，而不是 200 + degraded。
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(json!({
            "status": if sqlite_ok { "ok" } else { "degraded" },
            "checks": checks,
            "time": chrono::Utc::now().to_rfc3339(),
        })),
    )
}

async fn public_config(State(state): axum::extract::State<AppState>) -> impl IntoResponse {
    let bootstrap_pending = state
        .db
        .read(|conn| crate::infra::repo::super_admin_exists(conn).map(|b| !b))
        .await
        .unwrap_or(false);
    Json(json!({
        "registration_mode": state.cfg.auth.registration_mode,
        "bootstrap_pending": bootstrap_pending,
        "oidc": {
            "enabled": state.cfg.auth.oidc.enabled,
            "label": state.cfg.auth.oidc.button_label,
        },
        "features": {
            "voice": true,
            "screen_share": true,
            "media_playback": false,
            "voice_modes": ["p2p", "sfu"],
            "p2p_voice_max_participants": state.cfg.media.p2p_voice_max_participants,
        },
        "limits": {
            "max_avatar_bytes": state.cfg.security.max_avatar_bytes,
            "max_avatar_dimension": state.cfg.security.max_avatar_dimension,
            "max_nickname_graphemes": state.cfg.security.max_nickname_graphemes,
            "max_message_chars": state.cfg.rooms.max_message_chars,
            "max_attachment_bytes": state.cfg.security.max_attachment_bytes,
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
    let raw_path = req.uri().path().to_string();
    let path = if raw_path.starts_with("/api/v1/webhooks/") {
        "/api/v1/webhooks/***".to_string()
    } else {
        raw_path
    };
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

/// 滑动续期：每 7 天最多续一次；同时刷新浏览器 Cookie 的 Max-Age。
/// 仅对携带会话 Cookie 的请求做一次轻量判断，未到期时零数据库开销。
async fn renew_session_layer(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use crate::infra::auth::{cookie_name, cookie_value, SEEN_COOKIE};
    let raw = cookie_value(req.headers(), cookie_name(&state.cfg));
    let seen = cookie_value(req.headers(), SEEN_COOKIE).and_then(|v| v.parse::<i64>().ok());
    let now_ts = chrono::Utc::now().timestamp();
    let due = seen
        .map(|s| now_ts.saturating_sub(s) > 7 * 24 * 3600)
        .unwrap_or(true);
    let mut res = next.run(req).await;
    if !due {
        return res;
    }
    let Some(raw) = raw else {
        return res;
    };
    let hash = crate::util::sha256_hex(raw.as_bytes());
    let now = chrono::Utc::now();
    let ttl = state.cfg.session_ttl();
    let expires = now + chrono::Duration::from_std(ttl).unwrap_or_default();
    let renewed = state
        .db
        .with(move |conn| crate::infra::repo::renew_session(conn, &hash, &now, &expires))
        .await
        .unwrap_or(false);
    if renewed {
        if let Ok(v) = HeaderValue::from_str(&state.auth.session_cookie(&raw)) {
            res.headers_mut().append(header::SET_COOKIE, v);
        }
        if let Ok(v) = HeaderValue::from_str(&state.auth.seen_cookie(now_ts)) {
            res.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    res
}

pub fn use_state(state: AppState) -> Router {
    build_router(state)
}
