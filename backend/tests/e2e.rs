//! End-to-end test: boots the real router on an ephemeral port with a temp
//! data dir and exercises bootstrap, auth, room requests, invites, chat,
//! mutes, settings, direct rooms, expiry and WS events over real sockets.

use std::sync::Arc;

use serde_json::{json, Value};
use tempfile::TempDir;

use cloudroom_server::api::AppState;
use cloudroom_server::app::sweeper;
use cloudroom_server::infra::repo;
use cloudroom_server::*;
use cloudroom_server::{api, util};

fn test_state() -> (AppState, TempDir) {
    let dir = TempDir::new().unwrap();
    let mut cfg = Config::default();
    cfg.data.dir = dir.path().join("data");
    cfg.server.public_origin = "http://127.0.0.1".into();
    cfg.server.frontend_origin = "http://127.0.0.1".into();
    cfg.server.insecure_cookies = true;
    cfg.server.trusted_proxy = None;
    std::fs::create_dir_all(cfg.data.dir.join("avatars")).unwrap();
    let db = Db::open(&cfg.data.dir.join("cloudroom.db")).unwrap();
    db.migrate().unwrap();
    let cfg = Arc::new(cfg);
    let auth = Arc::new(Auth {
        cfg: (*cfg).clone(),
    });
    let state = AppState {
        db,
        cfg,
        auth,
        hub: Arc::new(Hub::new()),
        rl: Arc::new(RateLimiter::new()),
        avatars_dir: dir.path().join("data").join("avatars"),
    };
    (state, dir)
}

fn client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .cookie_store(true)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// CSRF token when a session exists (anonymous endpoints skip it).
fn csrf_opt(c: &reqwest::blocking::Client, base: &str) -> Option<String> {
    let r = c.get(format!("{base}/api/v1/me")).send().unwrap();
    if r.status() == 200 {
        r.json::<Value>().unwrap()["csrf_token"]
            .as_str()
            .map(|s| s.to_string())
    } else {
        None
    }
}

fn post_json(c: &reqwest::blocking::Client, base: &str, path: &str, body: Value) -> (u16, Value) {
    let mut req = c.post(format!("{base}/api/v1{path}"));
    if let Some(token) = csrf_opt(c, base) {
        req = req.header("x-csrf-token", token);
    }
    let r = req.json(&body).send().unwrap();
    let status = r.status().as_u16();
    let v: Value = r.json().unwrap_or(Value::Null);
    (status, v)
}

#[test]
fn full_flow_e2e() {
    let (state, _dir) = test_state();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let app = api::use_state(state.clone());

    let listener = rt.block_on(async {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        addr
    });
    let base = format!("http://{listener}");
    let c = client();

    // ---- health
    let r = c.get(format!("{base}/api/v1/health")).send().unwrap();
    assert_eq!(r.status(), 200);

    // ---- bootstrap: no super_admin -> public config says pending
    let r = c
        .get(format!("{base}/api/v1/config/public"))
        .send()
        .unwrap();
    let v: Value = r.json().unwrap();
    assert_eq!(v["bootstrap_pending"], true);

    // Insert a bootstrap token directly (the binary prints it on first start).
    let raw = "test-bootstrap-token-0123456789abcdef";
    let hash = util::sha256_hex(raw.as_bytes());
    let expires = util::now() + chrono::Duration::hours(24);
    state
        .db
        .with_sync(|conn| repo::insert_bootstrap_token(conn, &hash, &expires))
        .unwrap();

    // Register without token -> fail.
    let (s, v) = post_json(
        &c,
        &base,
        "/auth/register",
        json!({"username": "alice", "password": "correct-horse-battery"}),
    );
    assert_eq!(s, 400, "{v}");
    assert_eq!(v["error"]["code"], "bootstrap_required");

    // Register with token -> super_admin.
    let (s, v) = post_json(
        &c,
        &base,
        "/auth/register",
        json!({"username": "alice", "password": "correct-horse-battery", "token": raw}),
    );
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["user"]["role"], "super_admin");

    // Same token again -> fail.
    let c2 = client();
    let (s, _) = post_json(
        &c2,
        &base,
        "/auth/register",
        json!({"username": "mallory", "password": "correct-horse-battery", "token": raw}),
    );
    assert_eq!(s, 400);

    // ---- alice (super_admin) creates a room request -> auto-approved, room created.
    let (s, v) = post_json(
        &c,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "note": "team room", "desired_code": "666", "media_mode": "p2p"}),
    );
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["auto_approved"], true, "{v}");
    assert_eq!(v["room"]["code"], "666", "{v}");
    assert_eq!(v["room"]["media_mode"], "p2p", "{v}");
    let req_id = v["request"]["request_id"].as_str().unwrap().to_string();

    // Creating without a media_mode is rejected (no default guessing).
    let (s, v) = post_json(
        &c,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "note": "no mode"}),
    );
    assert_eq!(s, 422, "{v}");

    // ---- bob registers via a registration invite.
    let (s, v) = post_json(&c, &base, "/admin/invites", json!({}));
    assert_eq!(s, 200, "{v}");
    let reg_token = v["token"].as_str().unwrap().to_string();
    let c3 = client();
    let (s, v) = post_json(
        &c3,
        &base,
        "/auth/register",
        json!({"username": "bob", "password": "correct-horse-battery", "token": reg_token}),
    );
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["user"]["role"], "user");

    // bob submits a request (user role) -> pending; alice approves.
    let (s, v) = post_json(
        &c3,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "note": "bob room", "media_mode": "sfu"}),
    );
    assert_eq!(s, 200, "{v}");
    let req2 = v["request"]["request_id"].as_str().unwrap().to_string();

    // bob (user) cannot approve.
    let (s, v) = post_json(
        &c3,
        &base,
        &format!("/admin/room-requests/{req2}/approve"),
        json!({}),
    );
    assert_eq!(s, 403, "{v}");

    let (s, v) = post_json(
        &c,
        &base,
        &format!("/admin/room-requests/{req2}/approve"),
        json!({}),
    );
    assert_eq!(s, 200, "{v}");
    let bob_room_code = v["room"]["code"].as_str().unwrap().to_string();
    // alice's room 666 remains the invite/join target for the rest of the flow.
    let code = "666".to_string();

    // ---- admin room list shows both rooms.
    let r = c.get(format!("{base}/api/v1/admin/rooms")).send().unwrap();
    let v: Value = r.json().unwrap();
    assert_eq!(v["total"], 2);
    let codes: Vec<String> = v["rooms"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["room"]["code"].as_str().unwrap().to_string())
        .collect();
    assert!(codes.contains(&"666".to_string()), "666 in list: {codes:?}");

    // ---- bob (not a member of 666) cannot GET alice's room.
    let r = c3.get(format!("{base}/api/v1/rooms/666")).send().unwrap();
    assert_eq!(r.status(), 403);

    // ---- alice creates a room invite for bob (guest/member).
    let (s, v) = post_json(
        &c,
        &base,
        &format!("/rooms/{code}/invites"),
        json!({"max_role": "member"}),
    );
    assert_eq!(s, 200, "{v}");
    let invite_token = v["invite"]["token"].as_str().unwrap().to_string();
    let invite_url = v["invite_url"].as_str().unwrap().to_string();
    assert!(invite_url.contains(&format!("{code}?invite=")));

    // ---- bob joins with the invite.
    let (s, v) = post_json(
        &c3,
        &base,
        &format!("/rooms/{code}/join"),
        json!({"invite_token": invite_token}),
    );
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["room"]["my_role"], "member");

    // Same invite cannot be reused (by an authenticated non-member).
    let (s, v) = post_json(&c, &base, "/admin/invites", json!({}));
    assert_eq!(s, 200, "{v}");
    let reg2 = v["token"].as_str().unwrap().to_string();
    let c4 = client();
    let (s, v) = post_json(
        &c4,
        &base,
        "/auth/register",
        json!({"username": "carol", "password": "correct-horse-battery", "token": reg2}),
    );
    assert_eq!(s, 200, "{v}");
    let (s, v) = post_json(
        &c4,
        &base,
        &format!("/rooms/{code}/join"),
        json!({"invite_token": invite_token}),
    );
    assert_eq!(s, 400, "{v}");
    assert_eq!(v["error"]["code"], "invite_invalid");

    // ---- bob sends a message over HTTP.
    let (s, v) = post_json(
        &c3,
        &base,
        &format!("/rooms/{code}/messages"),
        json!({"body": "hello from bob"}),
    );
    assert_eq!(s, 200, "{v}");
    let seq = v["message"]["seq"].as_i64().unwrap();

    // ---- alice fetches history (含成员进房的系统播报).
    let r = c
        .get(format!("{base}/api/v1/rooms/{code}/messages?after=0"))
        .send()
        .unwrap();
    let v: Value = r.json().unwrap();
    let bodies: Vec<&str> = v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["body"].as_str().unwrap())
        .collect();
    assert!(bodies.iter().any(|b| *b == "hello from bob"), "{bodies:?}");
    assert!(
        bodies.iter().any(|b| b.contains("加入了房间")),
        "{bodies:?}"
    );
    assert_eq!(v["last_seq"], seq);

    // ---- alice mutes bob; bob's message is rejected server-side.
    let (s, v) = post_json(
        &c,
        &base,
        &format!("/rooms/{code}/mutes"),
        json!({"scope": "member", "user_id": v_uid(&c3, &base), "duration_seconds": 3600}),
    );
    assert_eq!(s, 200, "{v}");
    let (s, v) = post_json(
        &c3,
        &base,
        &format!("/rooms/{code}/messages"),
        json!({"body": "nope"}),
    );
    assert_eq!(s, 403, "{v}");
    assert_eq!(v["error"]["code"], "muted");
    // Owner is exempt from room-scope mute:
    let (s, _) = post_json(
        &c,
        &base,
        &format!("/rooms/{code}/mutes"),
        json!({"scope": "room", "duration_seconds": 3600}),
    );
    assert_eq!(s, 200);
    let (s, _) = post_json(
        &c,
        &base,
        &format!("/rooms/{code}/messages"),
        json!({"body": "owner still speaks"}),
    );
    assert_eq!(s, 200);
    let (s, v) = post_json(
        &c3,
        &base,
        &format!("/rooms/{code}/messages"),
        json!({"body": "still muted"}),
    );
    assert_eq!(s, 403, "{v}");

    // ---- text channel closed: owner toggles, member rejected.
    let (s, _) = post_json(
        &c,
        &base,
        &format!("/rooms/{code}/settings"),
        json!({"text_enabled": false}),
    );
    assert_eq!(s, 200);
    let (s, v) = post_json(
        &c3,
        &base,
        &format!("/rooms/{code}/messages"),
        json!({"body": "channel closed"}),
    );
    assert_eq!(s, 409, "{v}");
    assert_eq!(v["error"]["code"], "text_disabled");
    let (s, _) = post_json(
        &c,
        &base,
        &format!("/rooms/{code}/settings"),
        json!({"text_enabled": true}),
    );
    assert_eq!(s, 200);

    // ---- direct room between alice and bob.
    let (s, v) = post_json(
        &c,
        &base,
        &format!("/rooms/{code}/direct"),
        json!({"user_id": v_uid(&c3, &base)}),
    );
    assert_eq!(s, 200, "{v}");
    let dm_code = v["room"]["code"].as_str().unwrap().to_string();
    assert_eq!(v["room"]["kind"], "direct");
    assert_eq!(v["room"]["my_role"], "owner");
    // Duplicate creation returns the same room.
    let (s, v2) = post_json(
        &c,
        &base,
        &format!("/rooms/{code}/direct"),
        json!({"user_id": v_uid(&c3, &base)}),
    );
    assert_eq!(s, 200);
    assert_eq!(v2["room"]["code"], dm_code);

    // ---- DM idle expiry: force last_online_at into the past, sweep.
    let now = util::now() - chrono::Duration::hours(2);
    state
        .db
        .with_sync(|conn| {
            conn.execute(
                "UPDATE room_members SET last_online_at = ?1",
                rusqlite::params![now.to_rfc3339()],
            )
            .unwrap();
            Ok::<(), cloudroom_server::ApiError>(())
        })
        .unwrap();
    // Need an actual room row for the direct pair: fetch id via members.
    let (s, _) = post_json(&c, &base, &format!("/rooms/{dm_code}/ws"), json!({}));
    let _ = s; // ws endpoint over HTTP returns 400/405 — fine, not part of this flow
    let _ = sweeper::sweep_once(&state.db, &state.cfg).unwrap();
    let r = c
        .get(format!("{base}/api/v1/rooms/{dm_code}"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 410); // GONE: deleted by idle sweep

    // ---- owner deletes the standard room; code tombstoned forever.
    let (s, _) = post_json(&c, &base, &format!("/rooms/{code}/delete"), json!({}));
    assert_eq!(s, 200);
    let r = c.get(format!("{base}/api/v1/rooms/{code}")).send().unwrap();
    assert_eq!(r.status(), 410);
    // Custom code 666 is tombstoned: a new request with 666 must fail (super_admin auto-approve rejects at submit).
    let (s, v) = post_json(
        &c,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "note": "again", "desired_code": "666", "media_mode": "p2p"}),
    );
    assert_eq!(s, 409, "{v}");
    assert_eq!(v["error"]["code"], "conflict", "{v}");

    // ---- reserved/invalid custom codes rejected at request time.
    let (s, v) = post_json(
        &c,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "note": "", "desired_code": "api", "media_mode": "p2p"}),
    );
    assert_eq!(s, 422, "{v}");
    let (s, v) = post_json(
        &c,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "note": "", "desired_code": "-bad", "media_mode": "p2p"}),
    );
    assert_eq!(s, 422, "{v}");

    // ---- CSRF enforcement: mutation without token fails.
    let r = c
        .post(format!("{base}/api/v1/auth/logout"))
        .json(&json!({}))
        .send()
        .unwrap();
    assert_eq!(r.status(), 403);

    // ---- avatar upload: real PNG accepted, SVG rejected.
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_pixel(64, 64, image::Rgb([9, 8, 7]))
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let token = csrf_opt(&c, &base).unwrap();
    let part = reqwest::blocking::multipart::Part::bytes(png.into_inner())
        .file_name("avatar.png")
        .mime_str("image/png")
        .unwrap();
    let form = reqwest::blocking::multipart::Form::new().part("avatar", part);
    let r = c
        .post(format!("{base}/api/v1/me/avatar"))
        .header("x-csrf-token", &token)
        .multipart(form)
        .send()
        .unwrap();
    assert_eq!(r.status(), 200, "{:?}", r.text());
    let v: Value = r.json().unwrap();
    let avatar_url = v["avatar_url"].as_str().unwrap().to_string();
    let r = c.get(format!("{base}{avatar_url}")).send().unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "image/png");

    let bad_form = reqwest::blocking::multipart::Form::new().part(
        "avatar",
        reqwest::blocking::multipart::Part::bytes(b"<svg onload=alert(1)>".to_vec())
            .file_name("x.svg")
            .mime_str("image/svg+xml")
            .unwrap(),
    );
    let r = c
        .post(format!("{base}/api/v1/me/avatar"))
        .header("x-csrf-token", &token)
        .multipart(bad_form)
        .send()
        .unwrap();
    assert_eq!(r.status(), 400); // avatar_unsupported

    // ---- unauthenticated room access: 403 (frontend shows join flow),
    // never room data.
    let c_anon = client();
    let r = c_anon
        .get(format!("{base}/api/v1/rooms/zzzzzzz"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 403);
}

fn v_uid(c: &reqwest::blocking::Client, base: &str) -> String {
    let r = c.get(format!("{base}/api/v1/me")).send().unwrap();
    r.json::<Value>().unwrap()["user"]["user_id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn ws_chat_and_voice_events() {
    let (state, _dir) = test_state();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let app = api::use_state(state.clone());
    let listener = rt.block_on(async {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        addr
    });
    let base = format!("http://{listener}");

    {
        // Bootstrap a user and a room directly in the DB.
        let uid = "11111111111111111111111111111111";
        let hash = state.auth.hash_password("pw").unwrap();
        state
            .db
            .with_sync(|conn| {
                repo::create_user(conn, uid, Some("carol"), Some(&hash), UserRole::SuperAdmin)?;
                let now = util::now();
                let room = Room {
                    room_id: "22222222222222222222222222222222".into(),
                    room_code: "abc1234".into(),
                    kind: RoomKind::Standard,
                    owner_user_id: Some(uid.into()),
                    parent_room_id: None,
                    state: RoomState::Active,
                    created_at: now,
                    expires_at: None,
                    deleted_at: None,
                    allow_direct_messages: true,
                    text_enabled: true,
                    media_mode: MediaMode::P2p,
                    audio_bitrate: 0,
                    room_muted: false,
                    announcement: String::new(),
                    max_members: 0,
                    last_seq: 0,
                    version: 1,
                };
                repo::insert_room(conn, &room)?;
                repo::add_member(conn, &room.room_id, uid, RoomRole::Owner, "carol")?;
                Ok::<(), cloudroom_server::ApiError>(())
            })
            .unwrap();

        // Login to get a session cookie.
        let c = client();
        let r = c
            .post(format!("{base}/api/v1/auth/login"))
            .json(&json!({"username": "carol", "password": "pw"}))
            .send()
            .unwrap();
        assert_eq!(r.status(), 200);

        let url = format!("ws://{}/api/v1/rooms/abc1234/ws", listener);
        let mut req =
            tungstenite::client::IntoClientRequest::into_client_request(url.clone()).unwrap();
        // Re-login and grab cookie properly.
        let login = c
            .post(format!("{base}/api/v1/auth/login"))
            .json(&json!({"username": "carol", "password": "pw"}))
            .send()
            .unwrap();
        let set = login
            .headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let cookie_val = set.split(';').next().unwrap().to_string();
        req.headers_mut()
            .insert("cookie", cookie_val.parse().unwrap());
        let (mut ws, _) = tungstenite::connect(req).expect("ws connect");

        // hello
        let hello: Value = serde_json::from_str(&ws.read().unwrap().into_text().unwrap()).unwrap();
        assert_eq!(hello["type"], "hello");
        assert_eq!(hello["room"]["code"], "abc1234");

        // send_message over WS
        ws.send(tungstenite::Message::Text(
            r#"{"type":"send_message","body":"ws hello"}"#.into(),
        ))
        .unwrap();
        let ev: Value = serde_json::from_str(&ws.read().unwrap().into_text().unwrap()).unwrap();
        assert_eq!(ev["type"], "message");
        assert_eq!(ev["message"]["body"], "ws hello");
        assert_eq!(ev["message"]["seq"], 1);

        // voice_join -> voice_state
        ws.send(tungstenite::Message::Text(
            r#"{"type":"voice_join"}"#.into(),
        ))
        .unwrap();
        let ev: Value = serde_json::from_str(&ws.read().unwrap().into_text().unwrap()).unwrap();
        assert_eq!(ev["type"], "voice_state");
        assert!(ev["members"].as_array().unwrap().contains(&json!(uid)));

        // ping -> pong
        ws.send(tungstenite::Message::Text(r#"{"type":"ping"}"#.into()))
            .unwrap();
        let ev: Value = serde_json::from_str(&ws.read().unwrap().into_text().unwrap()).unwrap();
        assert_eq!(ev["type"], "pong");

        // Signaling relay: second user must exist; skip — single-connection
        // relay is covered implicitly by send_to paths.
        ws.send(tungstenite::Message::Text(
            r#"{"type":"signal","to":"nobody","data":{"type":"offer","sdp":"x"}}"#.into(),
        ))
        .unwrap();
        // no event should arrive for an unknown target; nothing to assert beyond liveness.

        ws.close(None).unwrap();
        drop(ws);
    }
}

#[test]
fn media_mode_change_and_gating() {
    let (state, _dir) = test_state();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let app = api::use_state(state.clone());
    let listener = rt.block_on(async {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        addr
    });
    let base = format!("http://{listener}");
    let c = client();

    // Bootstrap token + alice = super_admin.
    let raw = "media-mode-bootstrap-token-0123456789";
    let hash = util::sha256_hex(raw.as_bytes());
    let expires = util::now() + chrono::Duration::hours(24);
    state
        .db
        .with_sync(|conn| repo::insert_bootstrap_token(conn, &hash, &expires))
        .unwrap();
    let (s, v) = post_json(
        &c,
        &base,
        "/auth/register",
        json!({"username": "alice", "password": "correct-horse-battery", "token": raw}),
    );
    assert_eq!(s, 200, "{v}");
    let alice_uid = v["user"]["user_id"].as_str().unwrap().to_string();

    // alice auto-creates room 666 in p2p mode.
    let (s, v) = post_json(
        &c,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "note": "p2p room", "desired_code": "666", "media_mode": "p2p"}),
    );
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["room"]["media_mode"], "p2p", "{v}");

    // p2p room: ice-servers works (LiveKit embedded TURN), media-token rejected.
    let r = c
        .get(format!("{base}/api/v1/rooms/666/ice-servers"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    let ice: Value = r.json().unwrap();
    assert!(
        ice["iceServers"][0]["urls"][0]
            .as_str()
            .unwrap()
            .contains("turn:"),
        "{ice}"
    );
    let (s, v) = post_json(&c, &base, "/rooms/666/media-token", json!({}));
    assert_eq!(s, 409, "{v}");
    assert_eq!(v["error"]["code"], "media_mode_mismatch", "{v}");

    // bob registers via invite (user role).
    let (s, v) = post_json(&c, &base, "/admin/invites", json!({}));
    assert_eq!(s, 200, "{v}");
    let reg_token = v["token"].as_str().unwrap().to_string();
    let cb = client();
    let (s, v) = post_json(
        &cb,
        &base,
        "/auth/register",
        json!({"username": "bob", "password": "correct-horse-battery", "token": reg_token}),
    );
    assert_eq!(s, 200, "{v}");
    let bob_uid = v["user"]["user_id"].as_str().unwrap().to_string();

    // bob (user) creates an sfu room -> pending; alice approves.
    let (s, v) = post_json(
        &cb,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "note": "sfu room", "media_mode": "sfu"}),
    );
    assert_eq!(s, 200, "{v}");
    let req2 = v["request"]["request_id"].as_str().unwrap().to_string();
    let (s, v) = post_json(
        &c,
        &base,
        &format!("/admin/room-requests/{req2}/approve"),
        json!({}),
    );
    assert_eq!(s, 200, "{v}");
    let bob_code = v["room"]["code"].as_str().unwrap().to_string();
    assert_eq!(v["room"]["media_mode"], "sfu", "{v}");

    // sfu room: media-token works, ice-servers rejected.
    let (s, v) = post_json(
        &cb,
        &base,
        &format!("/rooms/{bob_code}/media-token"),
        json!({}),
    );
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["room"], bob_code, "{v}");
    assert!(v["token"].as_str().unwrap().split('.').count() == 3, "{v}");
    assert!(v["url"].is_string(), "{v}");
    let r = cb
        .get(format!("{base}/api/v1/rooms/{bob_code}/ice-servers"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 409);

    // bob (owner) requests a change to p2p -> pending; request JSON exposes kind/target/mode.
    let (s, v) = post_json(
        &cb,
        &base,
        "/room-requests",
        json!({"kind": "change_media_mode", "room_code": bob_code, "media_mode": "p2p", "note": "switch me"}),
    );
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["request"]["kind"], "change_media_mode", "{v}");
    assert_eq!(v["request"]["target_room_code"], bob_code, "{v}");
    assert_eq!(v["request"]["media_mode"], "p2p", "{v}");
    assert_eq!(v["request"]["status"], "pending", "{v}");
    let change_req = v["request"]["request_id"].as_str().unwrap().to_string();

    // Owner cannot approve their own change request.
    let (s, v) = post_json(
        &cb,
        &base,
        &format!("/admin/room-requests/{change_req}/approve"),
        json!({}),
    );
    assert_eq!(s, 403, "{v}");

    // A second pending change for the same room is rejected.
    let (s, v) = post_json(
        &cb,
        &base,
        "/room-requests",
        json!({"kind": "change_media_mode", "room_code": bob_code, "media_mode": "p2p"}),
    );
    assert_eq!(s, 409, "{v}");
    assert_eq!(v["error"]["code"], "conflict", "{v}");

    // alice approves -> room becomes p2p.
    let (s, v) = post_json(
        &c,
        &base,
        &format!("/admin/room-requests/{change_req}/approve"),
        json!({}),
    );
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["room"]["media_mode"], "p2p", "{v}");
    let r = cb
        .get(format!("{base}/api/v1/rooms/{bob_code}"))
        .send()
        .unwrap();
    let v: Value = r.json().unwrap();
    assert_eq!(v["room"]["media_mode"], "p2p", "{v}");

    // Same-mode change is rejected.
    let (s, v) = post_json(
        &cb,
        &base,
        "/room-requests",
        json!({"kind": "change_media_mode", "room_code": bob_code, "media_mode": "p2p"}),
    );
    assert_eq!(s, 422, "{v}");

    // alice switches 666 to sfu (super_admin auto-approve).
    let (s, v) = post_json(
        &c,
        &base,
        "/room-requests",
        json!({"kind": "change_media_mode", "room_code": "666", "media_mode": "sfu"}),
    );
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["auto_approved"], true, "{v}");
    assert_eq!(v["room"]["media_mode"], "sfu", "{v}");
    let (s, v) = post_json(&c, &base, "/rooms/666/media-token", json!({}));
    assert_eq!(s, 200, "{v}");
    let r = c
        .get(format!("{base}/api/v1/rooms/666/ice-servers"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 409);

    // Direct rooms inherit the parent mode and cannot be changed separately.
    let (s, v) = post_json(
        &c,
        &base,
        "/rooms/666/invites",
        json!({"max_role": "member"}),
    );
    assert_eq!(s, 200, "{v}");
    let inv = v["invite"]["token"].as_str().unwrap().to_string();
    let (s, v) = post_json(&cb, &base, "/rooms/666/join", json!({"invite_token": inv}));
    assert_eq!(s, 200, "{v}");
    let (s, v) = post_json(
        &cb,
        &base,
        "/rooms/666/direct",
        json!({"user_id": alice_uid}),
    );
    assert_eq!(s, 200, "{v}");
    let dm_code = v["room"]["code"].as_str().unwrap().to_string();
    assert_eq!(v["room"]["media_mode"], "sfu", "{v}");
    let (s, v) = post_json(
        &cb,
        &base,
        "/room-requests",
        json!({"kind": "change_media_mode", "room_code": dm_code, "media_mode": "p2p"}),
    );
    assert_eq!(s, 422, "{v}");

    // Non-owner members cannot request a mode change.
    let (s, v) = post_json(
        &cb,
        &base,
        "/room-requests",
        json!({"kind": "change_media_mode", "room_code": "666", "media_mode": "p2p"}),
    );
    assert_eq!(s, 403, "{v}");

    // Unknown/malformed kinds and fields are 422.
    let (s, v) = post_json(
        &c,
        &base,
        "/room-requests",
        json!({"kind": "explode", "media_mode": "p2p"}),
    );
    assert_eq!(s, 422, "{v}");
    let (s, v) = post_json(
        &c,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "media_mode": "auto"}),
    );
    assert_eq!(s, 422, "{v}");
    let _ = bob_uid;
}

/// 旧库（0001-0003 时代）中 media_mode='auto' 的房间必须迁移为 'sfu'，
/// 且 room_requests 获得 request_kind/target_room_id/media_mode 新列。
#[test]
fn legacy_auto_room_migrates_to_sfu() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("legacy.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (version TEXT PRIMARY KEY, applied_at TEXT NOT NULL);",
        )
        .unwrap();
        for (version, sql) in cloudroom_server::infra::db::MIGRATIONS.iter().take(3) {
            conn.execute_batch(sql).unwrap();
            conn.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                rusqlite::params![*version, "2026-08-07T00:00:00Z"],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO users (user_id, username, password_hash, role, avatar_id, disabled, created_at, last_online_at)
             VALUES ('u1', 'legacy', NULL, 'super_admin', NULL, 0, '2026-08-07T00:00:00Z', '2026-08-07T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO rooms (room_id, room_code, kind, owner_user_id, parent_room_id, state, created_at,
                                expires_at, deleted_at, allow_direct_messages, text_enabled, media_mode, last_seq, version)
             VALUES ('r1', 'legacy1', 'standard', 'u1', NULL, 'active', '2026-08-07T00:00:00Z',
                     NULL, NULL, 1, 1, 'auto', 0, 1)",
            [],
        )
        .unwrap();
        // 旧申请表结构（无 request_kind/target_room_id/media_mode 列）。
        conn.execute(
            "INSERT INTO room_requests (request_id, requester_id, note, desired_code, status, created_at)
             VALUES ('req1', 'u1', 'old', NULL, 'pending', '2026-08-07T00:00:00Z')",
            [],
        )
        .unwrap();
    }

    let db = Db::open(&path).unwrap();
    db.migrate().unwrap();
    db.with_sync(|conn| {
        let room = repo::get_room_by_code(conn, "legacy1").unwrap().unwrap();
        assert_eq!(
            room.media_mode,
            MediaMode::Sfu,
            "legacy auto room must become sfu"
        );
        // 旧申请默认视为 create_room + p2p，且新列可读。
        let req = repo::get_room_request(conn, "req1").unwrap().unwrap();
        assert_eq!(req.request_kind, "create_room");
        assert_eq!(req.media_mode, MediaMode::P2p);
        assert!(req.target_room_id.is_none());
        Ok::<(), cloudroom_server::ApiError>(())
    })
    .unwrap();
}

#[test]
fn my_rooms_returns_complete_room_fields() {
    let (state, _dir) = test_state();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let app = api::use_state(state.clone());

    let listener = rt.block_on(async {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        addr
    });
    let base = format!("http://{listener}");
    let c = client();

    let raw = "test-me-rooms-bootstrap-0123456789";
    let hash = util::sha256_hex(raw.as_bytes());
    let expires = util::now() + chrono::Duration::hours(24);
    state
        .db
        .with_sync(|conn| repo::insert_bootstrap_token(conn, &hash, &expires))
        .unwrap();

    let (status, body) = post_json(
        &c,
        &base,
        "/auth/register",
        json!({"username": "alice", "password": "correct-horse-battery", "token": raw}),
    );
    assert_eq!(status, 200, "{body}");

    let (status, body) = post_json(
        &c,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "media_mode": "sfu", "audio_bitrate": 48, "desired_code": "fields"}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["auto_approved"], true, "{body}");

    let response = c.get(format!("{base}/api/v1/me/rooms")).send().unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().unwrap();
    let rooms = body["rooms"].as_array().expect("rooms must be an array");
    assert_eq!(rooms.len(), 1, "{body}");
    let room = &rooms[0];
    assert_eq!(room["code"], "fields");
    assert_eq!(room["media_mode"], "sfu");
    assert_eq!(room["my_role"], "owner");
    assert_eq!(room["announcement"], "");
    assert_eq!(room["max_members"], 0);
    assert!(room["last_seq"].is_number(), "{room}");
}
