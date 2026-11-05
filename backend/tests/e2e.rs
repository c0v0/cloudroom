//! End-to-end test: boots the real router on an ephemeral port with a temp
//! data dir and exercises bootstrap, auth, room requests, invites, chat,
//! mutes, settings, direct rooms, expiry and WS events over real sockets.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;

use cloudroom_server::api::AppState;
use cloudroom_server::app::{revocation, sweeper};
use cloudroom_server::infra::repo;
use cloudroom_server::*;
use cloudroom_server::{api, util};

fn test_state() -> (AppState, TempDir) {
    test_state_with(|_| {})
}

fn test_state_with(configure: impl FnOnce(&mut Config)) -> (AppState, TempDir) {
    let dir = TempDir::new().unwrap();
    let mut cfg = Config::default();
    cfg.data.dir = dir.path().join("data");
    cfg.server.public_origin = "http://127.0.0.1".into();
    cfg.server.frontend_origin = "http://127.0.0.1".into();
    cfg.server.insecure_cookies = true;
    cfg.server.development = true;
    cfg.server.trusted_proxy = None;
    configure(&mut cfg);
    std::fs::create_dir_all(cfg.data.dir.join("avatars")).unwrap();
    let db = Db::open(&cfg.data.dir.join("cloudroom.db")).unwrap();
    db.migrate().unwrap();
    let cfg = Arc::new(cfg);
    let auth = Arc::new(Auth {
        cfg: (*cfg).clone(),
    });
    let hub = Arc::new(Hub::with_limits(
        cfg.security.ws_max_connections_total,
        cfg.security.ws_max_connections_per_user,
    ));
    let state = AppState {
        db,
        cfg,
        auth,
        hub,
        rl: Arc::new(RateLimiter::new()),
        revocation: Arc::new(
            cloudroom_server::app::revocation::LiveKitRevocationMetrics::default(),
        ),
        avatars_dir: dir.path().join("data").join("avatars"),
        attachments_dir: dir.path().join("data").join("attachments"),
    };
    (state, dir)
}

#[derive(Clone, Debug)]
struct TwirpRequest {
    method: String,
    body: Value,
    authorization: String,
}

/// Minimal HTTP/1.1 Twirp stand-in for LiveKit RoomService. It intentionally
/// uses a raw socket so tests do not need another async HTTP dependency.
struct FakeTwirp {
    base: String,
    addr: std::net::SocketAddr,
    status: Arc<AtomicU16>,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<TwirpRequest>>>,
}

impl FakeTwirp {
    fn start(status: u16) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let base = format!("http://{addr}");
        let status = Arc::new(AtomicU16::new(status));
        let stop = Arc::new(AtomicBool::new(false));
        let requests: Arc<Mutex<Vec<TwirpRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let thread_requests = requests.clone();
        let thread_status = status.clone();
        let thread_stop = stop.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if thread_stop.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let Some(req) = read_twirp_request(&mut stream) else {
                    continue;
                };
                thread_requests.lock().unwrap().push(req);
                let code = thread_status.load(Ordering::Relaxed);
                let reason = match code {
                    200 => "OK",
                    404 => "Not Found",
                    _ => "Internal Server Error",
                };
                let body = "{}";
                let response = format!(
                    "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        Self {
            base,
            addr,
            status,
            stop,
            requests,
        }
    }

    fn set_status(&self, status: u16) {
        self.status.store(status, Ordering::Relaxed);
    }

    fn wait_for(&self, method: &str, min: usize, timeout: Duration) -> Vec<TwirpRequest> {
        let deadline = Instant::now() + timeout;
        loop {
            let found: Vec<TwirpRequest> = self
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.method == method)
                .cloned()
                .collect();
            if found.len() >= min || Instant::now() >= deadline {
                return found;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for FakeTwirp {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wake the blocking accept() so the thread can observe `stop`.
        let _ = TcpStream::connect(self.addr);
    }
}

fn read_twirp_request(stream: &mut TcpStream) -> Option<TwirpRequest> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > 64 * 1024 {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]);
    let mut lines = head.lines();
    let mut request_line = lines.next()?.split_whitespace();
    if request_line.next()? != "POST" {
        return None;
    }
    let path = request_line.next()?;
    let mut content_length = 0usize;
    let mut authorization = String::new();
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            match key.trim().to_ascii_lowercase().as_str() {
                "content-length" => content_length = value.trim().parse().unwrap_or(0),
                "authorization" => authorization = value.trim().to_string(),
                _ => {}
            }
        }
    }
    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(content_length);
    Some(TwirpRequest {
        method: path.rsplit('/').next().unwrap_or("").to_string(),
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
        authorization,
    })
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

fn delete_json(c: &reqwest::blocking::Client, base: &str, path: &str) -> (u16, Value) {
    let mut req = c.delete(format!("{base}/api/v1{path}"));
    if let Some(token) = csrf_opt(c, base) {
        req = req.header("x-csrf-token", token);
    }
    let r = req.send().unwrap();
    let status = r.status().as_u16();
    let v: Value = r.json().unwrap_or(Value::Null);
    (status, v)
}

fn audit_entries(c: &reqwest::blocking::Client, base: &str) -> Vec<Value> {
    let r = c
        .get(format!("{base}/api/v1/admin/audit?limit=200"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    r.json::<Value>().unwrap()["entries"]
        .as_array()
        .unwrap()
        .clone()
}

fn start_server(state: &AppState) -> (tokio::runtime::Runtime, String) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let app = api::use_state(state.clone());
    let listener = rt.block_on(async {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                l,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap()
        });
        addr
    });
    (rt, format!("http://{listener}"))
}

fn bootstrap_admin(state: &AppState, base: &str, raw: &str) -> (reqwest::blocking::Client, String) {
    let hash = util::sha256_hex(raw.as_bytes());
    let expires = util::now() + chrono::Duration::hours(24);
    state
        .db
        .with_sync(|conn| repo::insert_bootstrap_token(conn, &hash, &expires))
        .unwrap();
    let admin = client();
    let (status, body) = post_json(
        &admin,
        base,
        "/auth/register",
        json!({"username": "alice", "password": "correct-horse-battery", "token": raw}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["user"]["role"], "super_admin", "{body}");
    let uid = body["user"]["user_id"].as_str().unwrap().to_string();
    (admin, uid)
}

fn register_via_invite(
    admin: &reqwest::blocking::Client,
    base: &str,
    username: &str,
) -> (reqwest::blocking::Client, String) {
    let (status, body) = post_json(admin, base, "/admin/invites", json!({}));
    assert_eq!(status, 200, "{body}");
    let token = body["token"].as_str().unwrap().to_string();
    let c = client();
    let (status, body) = post_json(
        &c,
        base,
        "/auth/register",
        json!({"username": username, "password": "correct-horse-battery", "token": token}),
    );
    assert_eq!(status, 200, "{body}");
    let uid = body["user"]["user_id"].as_str().unwrap().to_string();
    (c, uid)
}

fn create_room(admin: &reqwest::blocking::Client, base: &str, code: &str) {
    let (status, body) = post_json(
        admin,
        base,
        "/room-requests",
        json!({"kind": "create_room", "note": "lifecycle test", "media_mode": "p2p", "desired_code": code}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["auto_approved"], true, "{body}");
}

fn invite_token(owner: &reqwest::blocking::Client, base: &str, code: &str) -> String {
    let (status, body) = post_json(
        owner,
        base,
        &format!("/rooms/{code}/invites"),
        json!({"max_role": "member"}),
    );
    assert_eq!(status, 200, "{body}");
    body["invite"]["token"].as_str().unwrap().to_string()
}

fn join_with_invite(c: &reqwest::blocking::Client, base: &str, code: &str, token: &str) {
    let (status, body) = post_json(
        c,
        base,
        &format!("/rooms/{code}/join"),
        json!({"invite_token": token}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["room"]["my_role"], "member", "{body}");
}

#[test]
fn full_flow_e2e() {
    let (state, _dir) = test_state();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let app = api::use_state(state.clone());

    let listener = rt.block_on(async {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                l,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap()
        });
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
    let _req_id = v["request"]["request_id"].as_str().unwrap().to_string();

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
    let _bob_room_code = v["room"]["code"].as_str().unwrap().to_string();
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
    let root_id = v["message"]["id"].as_str().unwrap().to_string();

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
    assert!(bodies.contains(&"hello from bob"), "{bodies:?}");
    assert!(
        bodies.iter().any(|b| b.contains("加入了房间")),
        "{bodies:?}"
    );
    assert_eq!(v["last_seq"], seq);

    // ---- global inbox: mention + unread counts, then mark read.
    let (s, v) = post_json(&c3, &base, &format!("/rooms/{code}/messages"), json!({"body":"@alice inbox-e2e"}));
    assert_eq!(s, 200, "{v}");
    let mention_seq = v["message"]["seq"].as_i64().unwrap();
    let alice_id = v_uid(&c, &base);
    let r = c.get(format!("{base}/api/v1/inbox")).send().unwrap();
    assert_eq!(r.status(), 200);
    let inbox: Value = r.json().unwrap();
    assert!(inbox["total_mentions"].as_i64().unwrap() >= 1, "{inbox}");
    assert!(inbox["mentions"].as_array().unwrap().iter().any(|m| m["body"] == "@alice inbox-e2e"), "{inbox}");
    let room_id = state.db.with_sync(|conn| crate::infra::repo::get_room_by_code(conn, &code)).unwrap().unwrap().room_id;
    state.db.with_sync(|conn| crate::infra::repo::set_member_read_seq(conn, &room_id, &alice_id, mention_seq)).unwrap();
    let r = c.get(format!("{base}/api/v1/inbox")).send().unwrap();
    let inbox: Value = r.json().unwrap();
    assert_eq!(inbox["total_mentions"], 0, "{inbox}");
    assert!(inbox["mentions"].as_array().unwrap().iter().all(|m| m["unread"] == false), "{inbox}");

    // ---- scheduled messages: due delivery + cancellation.
    let send_at = (chrono::Utc::now() + chrono::Duration::seconds(1)).to_rfc3339();
    let (s, v) = post_json(
        &c3,
        &base,
        &format!("/rooms/{code}/messages/schedule"),
        json!({"body": "scheduled-e2e", "send_at": send_at}),
    );
    assert_eq!(s, 200, "{v}");
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let delivered = rt
        .block_on(cloudroom_server::app::sweeper::sweep_scheduled_once(&state))
        .unwrap();
    assert!(delivered >= 1, "{delivered}");
    let r = c
        .get(format!("{base}/api/v1/rooms/{code}/messages?after=0"))
        .send()
        .unwrap();
    let v: Value = r.json().unwrap();
    assert!(v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["body"] == "scheduled-e2e"), "{v}");

    let later = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let (s, v) = post_json(
        &c3,
        &base,
        &format!("/rooms/{code}/messages/schedule"),
        json!({"body": "cancel-me", "send_at": later}),
    );
    assert_eq!(s, 200, "{v}");
    let schedule_id = v["scheduled"]["id"].as_str().unwrap().to_string();
    let r = c3
        .get(format!("{base}/api/v1/rooms/{code}/scheduled"))
        .send()
        .unwrap();
    assert_eq!(r.json::<Value>().unwrap()["scheduled"].as_array().unwrap().len(), 1);
    let (s, v) = post_json(
        &c3,
        &base,
        &format!("/rooms/{code}/scheduled/{schedule_id}/cancel"),
        json!({}),
    );
    assert_eq!(s, 200, "{v}");
    let r = c3
        .get(format!("{base}/api/v1/rooms/{code}/scheduled"))
        .send()
        .unwrap();
    assert!(r.json::<Value>().unwrap()["scheduled"].as_array().unwrap().is_empty());

    // ---- thread: reply is hidden from the channel list, visible in the thread.
    let (s, v) = post_json(
        &c3,
        &base,
        &format!("/rooms/{code}/messages"),
        json!({"body": "thread reply", "thread_root_id": root_id, "reply_to": root_id}),
    );
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["message"]["thread_root_id"], root_id.as_str(), "{v}");
    let r = c
        .get(format!("{base}/api/v1/rooms/{code}/messages?after=0"))
        .send()
        .unwrap();
    let v: Value = r.json().unwrap();
    assert!(
        !v["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["body"] == "thread reply"),
        "{v}"
    );
    let root = v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"].as_str() == Some(root_id.as_str()))
        .unwrap();
    assert_eq!(root["thread_reply_count"], 1, "{root}");
    assert_eq!(root["thread_unread"], 1, "{root}");
    let (s, v) = post_json(
        &c,
        &base,
        &format!("/rooms/{code}/messages/{root_id}/thread/read"),
        json!({}),
    );
    assert_eq!(s, 200, "{v}");
    let r = c
        .get(format!("{base}/api/v1/rooms/{code}/messages?after=0"))
        .send()
        .unwrap();
    let v: Value = r.json().unwrap();
    let root = v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"].as_str() == Some(root_id.as_str()))
        .unwrap();
    assert_eq!(root["thread_unread"], 0, "{root}");
    let r = c
        .get(format!(
            "{base}/api/v1/rooms/{code}/messages/{root_id}/thread"
        ))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().unwrap();
    assert_eq!(v["root"]["id"].as_str(), Some(root_id.as_str()), "{v}");
    assert_eq!(v["replies"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["replies"][0]["body"], "thread reply", "{v}");

    // ---- pin and forward the root message.
    let (s, v) = post_json(&c, &base, &format!("/rooms/{code}/messages/{root_id}/pin"), json!({}));
    assert_eq!(s, 200, "{v}");
    let r = c.get(format!("{base}/api/v1/rooms/{code}/pins")).send().unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().unwrap();
    assert_eq!(v["messages"][0]["id"].as_str(), Some(root_id.as_str()), "{v}");
    let r = c.get(format!("{base}/api/v1/rooms/{code}/messages?after=0")).send().unwrap();
    let v: Value = r.json().unwrap();
    let root = v["messages"].as_array().unwrap().iter().find(|m| m["id"].as_str() == Some(root_id.as_str())).unwrap();
    assert_eq!(root["pinned"], true, "{root}");

    let (s, v) = post_json(&c, &base, "/room-requests", json!({"kind":"create_room","note":"forward","media_mode":"sfu","desired_code":"fwd-room"}));
    assert_eq!(s, 200, "{v}");
    let (s, v) = post_json(&c, &base, &format!("/rooms/{code}/messages/{root_id}/forward"), json!({"target_room_code":"fwd-room"}));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["message"]["forwarded_from"]["room_code"], code.as_str(), "{v}");
    let r = c.get(format!("{base}/api/v1/rooms/fwd-room/messages?after=0")).send().unwrap();
    let v: Value = r.json().unwrap();
    assert!(v["messages"].as_array().unwrap().iter().any(|m| m["body"] == "hello from bob" && m["forwarded_from"]["room_code"] == code.as_str()), "{v}");
    let (s, v) = post_json(&c, &base, &format!("/rooms/{code}/messages/{root_id}/unpin"), json!({}));
    assert_eq!(s, 200, "{v}");

    // ---- cross-room search: FTS5 for >=3 chars, LIKE fallback for 2.
    let r = c3
        .get(format!("{base}/api/v1/search?q=hello"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().unwrap();
    assert!(
        v["results"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["room_code"].as_str() == Some(code.as_str()) && x["body"] == "hello from bob"),
        "{v}"
    );
    let r = c3
        .get(format!("{base}/api/v1/search?q=he"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().unwrap();
    assert!(!v["results"].as_array().unwrap().is_empty(), "{v}");

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
    // HTTP room-scope mute is retired; the all-room switch lives on the WS.
    let (s, _) = post_json(
        &c,
        &base,
        &format!("/rooms/{code}/mutes"),
        json!({"scope": "room", "duration_seconds": 3600}),
    );
    assert_eq!(s, 422);
    // A member-scope mute never affects the owner.
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
    sweeper::sweep_once(&state.db, &state.cfg).unwrap();
    let r = c
        .get(format!("{base}/api/v1/rooms/{dm_code}"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 410); // GONE: deleted by idle sweep

    // ---- owner deletes the standard room; the code stays occupied during the grace period.
    let (s, _) = post_json(&c, &base, &format!("/rooms/{code}/delete"), json!({}));
    assert_eq!(s, 200);
    let r = c.get(format!("{base}/api/v1/rooms/{code}")).send().unwrap();
    assert_eq!(r.status(), 410);
    // The room row still holds the code until the sweeper purges it.
    let (s, v) = post_json(
        &c,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "note": "again", "desired_code": "666", "media_mode": "p2p"}),
    );
    assert_eq!(s, 409, "{v}");
    assert_eq!(v["error"]["code"], "conflict", "{v}");
    // Push the deletion beyond the grace period, sweep, then the code is free again.
    state
        .db
        .with_sync(|conn| {
            conn.execute(
                "UPDATE rooms SET deleted_at = ?1 WHERE room_code = '666'",
                rusqlite::params![(util::now() - chrono::Duration::days(400)).to_rfc3339()],
            )
            .map_err(cloudroom_server::ApiError::from)?;
            Ok::<(), cloudroom_server::ApiError>(())
        })
        .unwrap();
    sweeper::sweep_once(&state.db, &state.cfg).unwrap();
    let (s, v) = post_json(
        &c,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "note": "reuse", "desired_code": "666", "media_mode": "p2p"}),
    );
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["room"]["code"], "666", "{v}");

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
        tokio::spawn(async move {
            axum::serve(
                l,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap()
        });
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
                repo::create_user(conn, uid, Some("carol"), Some(&hash), UserRole::SuperAdmin, None)?;
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
                    e2ee: false,
                    e2ee_key: None,
                    attachments_enabled: false,
                    attachment_bytes: 0,
                    guest_upgrade_open: false,
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
        tokio::spawn(async move {
            axum::serve(
                l,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap()
        });
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
        tokio::spawn(async move {
            axum::serve(
                l,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap()
        });
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

#[test]
fn join_membership_paths() {
    fn register_user(
        admin: &reqwest::blocking::Client,
        base: &str,
        username: &str,
    ) -> (reqwest::blocking::Client, String) {
        let (status, body) = post_json(admin, base, "/admin/invites", json!({}));
        assert_eq!(status, 200, "{body}");
        let token = body["token"].as_str().unwrap().to_string();
        let c = client();
        let (status, body) = post_json(
            &c,
            base,
            "/auth/register",
            json!({"username": username, "password": "correct-horse-battery", "token": token}),
        );
        assert_eq!(status, 200, "{body}");
        let user_id = body["user"]["user_id"].as_str().unwrap().to_string();
        (c, user_id)
    }

    fn create_room(admin: &reqwest::blocking::Client, base: &str, code: &str) {
        let (status, body) = post_json(
            admin,
            base,
            "/room-requests",
            json!({"kind": "create_room", "note": "join membership test", "media_mode": "p2p", "desired_code": code}),
        );
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["auto_approved"], true, "{body}");
    }

    fn room_invite(admin: &reqwest::blocking::Client, base: &str, code: &str) -> String {
        let (status, body) = post_json(
            admin,
            base,
            &format!("/rooms/{code}/invites"),
            json!({"max_role": "member"}),
        );
        assert_eq!(status, 200, "{body}");
        body["invite"]["token"].as_str().unwrap().to_string()
    }

    fn join(
        c: &reqwest::blocking::Client,
        base: &str,
        code: &str,
        token: Option<&str>,
    ) -> (u16, Value) {
        let body = match token {
            Some(token) => json!({"invite_token": token}),
            None => json!({}),
        };
        post_json(c, base, &format!("/rooms/{code}/join"), body)
    }

    let (state, _dir) = test_state();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let app = api::use_state(state.clone());
    let listener = rt.block_on(async {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                l,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap()
        });
        addr
    });
    let base = format!("http://{listener}");
    let admin = client();

    let raw = "test-join-paths-bootstrap-0123456789";
    let hash = util::sha256_hex(raw.as_bytes());
    let expires = util::now() + chrono::Duration::hours(24);
    state
        .db
        .with_sync(|conn| repo::insert_bootstrap_token(conn, &hash, &expires))
        .unwrap();

    let (status, body) = post_json(
        &admin,
        &base,
        "/auth/register",
        json!({"username": "alice", "password": "correct-horse-battery", "token": raw}),
    );
    assert_eq!(status, 200, "{body}");
    let alice_id = body["user"]["user_id"].as_str().unwrap().to_string();
    let (bob, _) = register_user(&admin, &base, "bob-join");
    let (carol, _) = register_user(&admin, &base, "carol-join");
    let (dave, _) = register_user(&admin, &base, "dave-join");

    // Case 1: owner can join without an invite when the membership row is absent.
    create_room(&admin, &base, "join-owner");
    state
        .db
        .with_sync(|conn| {
            let room = repo::get_room_by_code(conn, "join-owner")?.unwrap();
            conn.execute(
                "DELETE FROM room_members WHERE room_id = ?1 AND user_id = ?2",
                rusqlite::params![room.room_id, alice_id],
            )
            .map_err(cloudroom_server::ApiError::from)?;
            Ok::<(), cloudroom_server::ApiError>(())
        })
        .unwrap();
    let (status, body) = join(&admin, &base, "join-owner", None);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["room"]["my_role"], "owner", "{body}");

    // Case 2: joining an already-active membership is a conflict.
    let (status, body) = join(&admin, &base, "join-owner", None);
    assert_eq!(status, 409, "{body}");

    // Case 3: an existing member can rejoin after leaving, without another invite.
    create_room(&admin, &base, "join-rejoin");
    let rejoin_invite = room_invite(&admin, &base, "join-rejoin");
    let (status, body) = join(&bob, &base, "join-rejoin", Some(&rejoin_invite));
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(&bob, &base, "/rooms/join-rejoin/leave", json!({}));
    assert_eq!(status, 200, "{body}");
    let (status, body) = join(&bob, &base, "join-rejoin", None);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["room"]["my_role"], "member", "{body}");

    // Case 4: max_members is enforced for a new member.
    create_room(&admin, &base, "join-full");
    state
        .db
        .with_sync(|conn| {
            conn.execute(
                "UPDATE rooms SET max_members = 1 WHERE room_code = 'join-full'",
                [],
            )
            .map_err(cloudroom_server::ApiError::from)?;
            Ok::<(), cloudroom_server::ApiError>(())
        })
        .unwrap();
    let full_invite = room_invite(&admin, &base, "join-full");
    let (status, body) = join(&carol, &base, "join-full", Some(&full_invite));
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"]["code"], "room_full", "{body}");

    // Case 5: invalid and already-used invites are rejected.
    create_room(&admin, &base, "join-used");
    let (status, body) = join(&carol, &base, "join-used", Some("invalid-token"));
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invite_invalid", "{body}");

    let used_invite = room_invite(&admin, &base, "join-used");
    let (status, body) = join(&dave, &base, "join-used", Some(&used_invite));
    assert_eq!(status, 200, "{body}");
    let (status, body) = join(&carol, &base, "join-used", Some(&used_invite));
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invite_invalid", "{body}");
}

#[test]
fn message_send_paths() {
    let (state, _dir) = test_state();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let app = api::use_state(state.clone());
    let listener = rt.block_on(async {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                l,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap()
        });
        addr
    });
    let base = format!("http://{listener}");
    let admin = client();

    let raw = "test-message-paths-bootstrap-0123456789";
    let hash = util::sha256_hex(raw.as_bytes());
    let expires = util::now() + chrono::Duration::hours(24);
    state
        .db
        .with_sync(|conn| repo::insert_bootstrap_token(conn, &hash, &expires))
        .unwrap();

    let (status, body) = post_json(
        &admin,
        &base,
        "/auth/register",
        json!({"username": "alice", "password": "correct-horse-battery", "token": raw}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &admin,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "media_mode": "p2p", "desired_code": "msg-paths"}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["auto_approved"], true, "{body}");

    let mut members = Vec::new();
    for username in ["bob-msg", "carol-msg"] {
        let (status, body) = post_json(&admin, &base, "/admin/invites", json!({}));
        assert_eq!(status, 200, "{body}");
        let token = body["token"].as_str().unwrap().to_string();
        let client = client();
        let (status, body) = post_json(
            &client,
            &base,
            "/auth/register",
            json!({"username": username, "password": "correct-horse-battery", "token": token}),
        );
        assert_eq!(status, 200, "{body}");
        members.push(client);
    }
    let (bob, carol) = (&members[0], &members[1]);

    let (status, body) = post_json(
        &admin,
        &base,
        "/rooms/msg-paths/invites",
        json!({"max_role": "member"}),
    );
    assert_eq!(status, 200, "{body}");
    let invite = body["invite"]["token"].as_str().unwrap();
    let (status, body) = post_json(
        bob,
        &base,
        "/rooms/msg-paths/join",
        json!({"invite_token": invite}),
    );
    assert_eq!(status, 200, "{body}");

    let (status, first) = post_json(
        bob,
        &base,
        "/rooms/msg-paths/messages",
        json!({"body": "hello from bob"}),
    );
    assert_eq!(status, 200, "{first}");
    let first_id = first["message"]["id"].as_str().unwrap().to_string();
    let first_seq = first["message"]["seq"].as_i64().unwrap();

    // Duplicate body is allowed; it must get a distinct id and increasing seq.
    let (status, duplicate) = post_json(
        bob,
        &base,
        "/rooms/msg-paths/messages",
        json!({"body": "hello from bob"}),
    );
    assert_eq!(status, 200, "{duplicate}");
    assert_ne!(duplicate["message"]["id"], first_id);
    assert!(
        duplicate["message"]["seq"].as_i64().unwrap() > first_seq,
        "{duplicate}"
    );

    // Reply snapshot keeps the referenced body.
    let (status, reply) = post_json(
        bob,
        &base,
        "/rooms/msg-paths/messages",
        json!({"body": "reply from bob", "reply_to": first_id}),
    );
    assert_eq!(status, 200, "{reply}");
    assert_eq!(reply["message"]["reply"]["body"], "hello from bob");

    // Empty and overlong bodies are validation errors.
    let (status, body) = post_json(
        bob,
        &base,
        "/rooms/msg-paths/messages",
        json!({"body": "   "}),
    );
    assert_eq!(status, 422, "{body}");
    let overlong = "x".repeat(state.cfg.rooms.max_message_chars + 1);
    let (status, body) = post_json(
        bob,
        &base,
        "/rooms/msg-paths/messages",
        json!({"body": overlong}),
    );
    assert_eq!(status, 422, "{body}");

    // A non-member cannot send.
    let (status, body) = post_json(
        carol,
        &base,
        "/rooms/msg-paths/messages",
        json!({"body": "not a member"}),
    );
    assert_eq!(status, 403, "{body}");

    // Muted members are rejected after a owner mute.
    let bob_id = v_uid(bob, &base);
    let (status, body) = post_json(
        &admin,
        &base,
        "/rooms/msg-paths/mutes",
        json!({"scope": "member", "user_id": bob_id, "duration_seconds": 3600}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        bob,
        &base,
        "/rooms/msg-paths/messages",
        json!({"body": "muted send"}),
    );
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["error"]["code"], "muted", "{body}");

    // History remains ordered and contains both duplicate bodies.
    let response = admin
        .get(format!("{base}/api/v1/rooms/msg-paths/messages?after=0"))
        .send()
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().unwrap();
    let messages = body["messages"].as_array().unwrap();
    let seqs: Vec<i64> = messages
        .iter()
        .map(|m| m["seq"].as_i64().unwrap())
        .collect();
    assert!(seqs.windows(2).all(|pair| pair[0] < pair[1]), "{seqs:?}");
    let duplicates = messages
        .iter()
        .filter(|m| m["body"] == "hello from bob")
        .count();
    assert_eq!(duplicates, 2, "{body}");
}

#[test]
fn room_request_paths() {
    let (state, _dir) = test_state();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let app = api::use_state(state.clone());
    let listener = rt.block_on(async {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                l,
                app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .await
            .unwrap()
        });
        addr
    });
    let base = format!("http://{listener}");
    let admin = client();

    let raw = "test-room-request-paths-bootstrap-0123456789";
    let hash = util::sha256_hex(raw.as_bytes());
    let expires = util::now() + chrono::Duration::hours(24);
    state
        .db
        .with_sync(|conn| repo::insert_bootstrap_token(conn, &hash, &expires))
        .unwrap();
    let (status, body) = post_json(
        &admin,
        &base,
        "/auth/register",
        json!({"username": "alice", "password": "correct-horse-battery", "token": raw}),
    );
    assert_eq!(status, 200, "{body}");

    let (status, body) = post_json(&admin, &base, "/admin/invites", json!({}));
    assert_eq!(status, 200, "{body}");
    let reg_token = body["token"].as_str().unwrap().to_string();
    let bob = client();
    let (status, body) = post_json(
        &bob,
        &base,
        "/auth/register",
        json!({"username": "bob-req", "password": "correct-horse-battery", "token": reg_token}),
    );
    assert_eq!(status, 200, "{body}");

    // Normal user: pending request, no auto-approved room in the response.
    let (status, body) = post_json(
        &bob,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "note": "bob pending", "media_mode": "sfu", "audio_bitrate": 48}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["request"]["status"], "pending", "{body}");
    assert!(body.get("auto_approved").is_none(), "{body}");
    let request_id = body["request"]["request_id"].as_str().unwrap();

    let (status, body) = post_json(
        &admin,
        &base,
        &format!("/admin/room-requests/{request_id}/approve"),
        json!({}),
    );
    assert_eq!(status, 200, "{body}");
    let bob_code = body["room"]["code"].as_str().unwrap();

    // A second pending media-mode request for the same room is rejected.
    let change = json!({
        "kind": "change_media_mode",
        "room_code": bob_code,
        "media_mode": "p2p",
        "audio_bitrate": 0
    });
    let (status, body) = post_json(&bob, &base, "/room-requests", change.clone());
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["request"]["status"], "pending", "{body}");
    let (status, body) = post_json(&bob, &base, "/room-requests", change);
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"]["code"], "conflict", "{body}");

    // Super-admin: create with a custom code auto-approves immediately.
    let (status, body) = post_json(
        &admin,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "note": "auto", "media_mode": "p2p", "desired_code": "req-auto"}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["auto_approved"], true, "{body}");
    assert_eq!(body["room"]["code"], "req-auto", "{body}");

    // Invalid custom code is a validation error.
    let (status, body) = post_json(
        &admin,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "media_mode": "p2p", "desired_code": "a_b"}),
    );
    assert_eq!(status, 422, "{body}");
    assert_eq!(body["error"]["code"], "validation_error", "{body}");

    // An occupied custom code is a conflict.
    let (status, body) = post_json(
        &admin,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "media_mode": "p2p", "desired_code": "req-auto"}),
    );
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"]["code"], "conflict", "{body}");

    // Only super-admins may request a custom code.
    let (status, body) = post_json(
        &bob,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "media_mode": "p2p", "desired_code": "bob-code"}),
    );
    assert_eq!(status, 403, "{body}");

    // ---- cancel semantics: own pending succeeds; approved/foreign/missing fail.
    let (status, body) = post_json(
        &bob,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "note": "cancel me", "media_mode": "p2p"}),
    );
    assert_eq!(status, 200, "{body}");
    let cancel_id = body["request"]["request_id"].as_str().unwrap().to_string();

    // 他人不能取消（不泄露存在性 -> 404）。
    let (status, body) = post_json(
        &admin,
        &base,
        &format!("/room-requests/{cancel_id}/cancel"),
        json!({}),
    );
    assert_eq!(status, 404, "{body}");

    // 本人取消 pending -> 200；重复取消 -> 409。
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/room-requests/{cancel_id}/cancel"),
        json!({}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/room-requests/{cancel_id}/cancel"),
        json!({}),
    );
    assert_eq!(status, 409, "{body}");

    // 已批准的申请不能取消。
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/room-requests/{request_id}/cancel"),
        json!({}),
    );
    assert_eq!(status, 409, "{body}");

    // 不存在的申请 -> 404。
    let (status, body) = post_json(&bob, &base, "/room-requests/nope/cancel", json!({}));
    assert_eq!(status, 404, "{body}");
}

#[test]
fn room_settings_paths() {
    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);
    let (alice, _alice_id) = bootstrap_admin(&state, &base, "test-settings-bootstrap-0123456789");
    let (bob, _bob_id) = register_via_invite(&alice, &base, "bob-settings");
    let (carol, _carol_id) = register_via_invite(&alice, &base, "carol-settings");
    let (dave, dave_id) = register_via_invite(&alice, &base, "dave-settings");

    // Account-level admin does not become a room admin.
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/admin/users/{dave_id}/role"),
        json!({"role": "admin"}),
    );
    assert_eq!(status, 200, "{body}");

    create_room(&alice, &base, "settings-room");
    let token = invite_token(&alice, &base, "settings-room");
    join_with_invite(&bob, &base, "settings-room", &token);
    let token = invite_token(&alice, &base, "settings-room");
    join_with_invite(&dave, &base, "settings-room", &token);

    // Owner succeeds and the response is the persisted room.
    let announcement = "公".repeat(500);
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/settings-room/settings",
        json!({
            "text_enabled": false,
            "allow_direct_messages": false,
            "max_members": 3,
            "announcement": announcement,
        }),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["room"]["text_enabled"], false, "{body}");
    assert_eq!(body["room"]["allow_direct_messages"], false, "{body}");
    assert_eq!(body["room"]["max_members"], 3, "{body}");
    assert_eq!(
        body["room"]["announcement"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        500,
        "{body}"
    );

    // Members and the account-level admin see the new settings.
    let r = bob
        .get(format!("{base}/api/v1/rooms/settings-room"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().unwrap();
    assert_eq!(body["room"]["text_enabled"], false, "{body}");
    assert_eq!(body["room"]["max_members"], 3, "{body}");

    // Room settings require the room owner role, not the account admin role.
    let (status, body) = post_json(
        &bob,
        &base,
        "/rooms/settings-room/settings",
        json!({"text_enabled": true}),
    );
    assert_eq!(status, 403, "{body}");
    let (status, body) = post_json(
        &dave,
        &base,
        "/rooms/settings-room/settings",
        json!({"text_enabled": true}),
    );
    assert_eq!(status, 403, "{body}");
    // Non-members and missing rooms are rejected.
    let (status, body) = post_json(
        &carol,
        &base,
        "/rooms/settings-room/settings",
        json!({"text_enabled": true}),
    );
    assert_eq!(status, 403, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/no-such-room/settings",
        json!({"text_enabled": true}),
    );
    assert_eq!(status, 404, "{body}");
    assert_eq!(body["error"]["code"], "not_found", "{body}");

    // Existing boundary behavior: announcement is truncated to 500 chars.
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/settings-room/settings",
        json!({"announcement": "界".repeat(501)}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body["room"]["announcement"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        500,
        "{body}"
    );

    // max_members is clamped to 0..=500 (0 means unlimited).
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/settings-room/settings",
        json!({"max_members": -1}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["room"]["max_members"], 0, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/settings-room/settings",
        json!({"max_members": 501}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["room"]["max_members"], 500, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/settings-room/settings",
        json!({"max_members": 0}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["room"]["max_members"], 0, "{body}");

    // The final settings are visible to an active member.
    let r = bob
        .get(format!("{base}/api/v1/rooms/settings-room"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().unwrap();
    assert_eq!(body["room"]["announcement"].as_str().unwrap().len(), 1500);
    assert_eq!(body["room"]["max_members"], 0, "{body}");
    assert_eq!(body["room"]["text_enabled"], false, "{body}");
}

#[test]
fn room_member_lifecycle_paths() {
    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);
    let (alice, alice_id) = bootstrap_admin(&state, &base, "test-lifecycle-bootstrap-0123456789");
    let (bob, bob_id) = register_via_invite(&alice, &base, "bob-lifecycle");
    let (carol, carol_id) = register_via_invite(&alice, &base, "carol-lifecycle");
    let (dave, dave_id) = register_via_invite(&alice, &base, "dave-lifecycle");

    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/admin/users/{dave_id}/role"),
        json!({"role": "admin"}),
    );
    assert_eq!(status, 200, "{body}");

    create_room(&alice, &base, "life-room");
    let token = invite_token(&alice, &base, "life-room");
    join_with_invite(&bob, &base, "life-room", &token);
    let token = invite_token(&alice, &base, "life-room");
    join_with_invite(&carol, &base, "life-room", &token);
    let token = invite_token(&alice, &base, "life-room");
    join_with_invite(&dave, &base, "life-room", &token);

    // Room role changes: owner-only, target must be a non-owner, no self edits.
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/rooms/life-room/members/{bob_id}/role"),
        json!({"role": "member"}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/rooms/life-room/members/{bob_id}/role"),
        json!({"role": "owner"}),
    );
    assert_eq!(status, 422, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/rooms/life-room/members/{alice_id}/role"),
        json!({"role": "member"}),
    );
    assert_eq!(status, 422, "{body}");
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/rooms/life-room/members/{carol_id}/role"),
        json!({"role": "member"}),
    );
    assert_eq!(status, 403, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/life-room/members/missing-user/role",
        json!({"role": "member"}),
    );
    assert_eq!(status, 404, "{body}");

    // Nicknames: self-service, 32-char limit, owner can edit others.
    let long_nick = "n".repeat(32);
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/rooms/life-room/members/{bob_id}/nickname"),
        json!({"nickname": long_nick}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/rooms/life-room/members/{bob_id}/nickname"),
        json!({"nickname": "n".repeat(33)}),
    );
    assert_eq!(status, 422, "{body}");
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/rooms/life-room/members/{carol_id}/nickname"),
        json!({"nickname": "not-allowed"}),
    );
    assert_eq!(status, 403, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/rooms/life-room/members/{bob_id}/nickname"),
        json!({"nickname": "bob-renamed"}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/life-room/members/missing-user/nickname",
        json!({"nickname": "nobody"}),
    );
    assert_eq!(status, 404, "{body}");
    let r = bob
        .get(format!("{base}/api/v1/rooms/life-room"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().unwrap();
    let bob_member = body["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["user_id"] == bob_id)
        .unwrap();
    assert_eq!(bob_member["nickname"], "bob-renamed", "{body}");

    // A normal member can leave; the existing membership can rejoin without a new invite.
    let (status, body) = post_json(&bob, &base, "/rooms/life-room/leave", json!({}));
    assert_eq!(status, 200, "{body}");
    let r = bob
        .get(format!("{base}/api/v1/rooms/life-room"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 403);
    let (status, body) = post_json(&bob, &base, "/rooms/life-room/join", json!({}));
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["room"]["my_role"], "member", "{body}");

    // Owner and account-admin members cannot remove through the member route.
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/rooms/life-room/members/{carol_id}/remove"),
        json!({}),
    );
    assert_eq!(status, 403, "{body}");
    let (status, body) = post_json(
        &dave,
        &base,
        &format!("/rooms/life-room/members/{carol_id}/remove"),
        json!({}),
    );
    assert_eq!(status, 403, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/rooms/life-room/members/{alice_id}/remove"),
        json!({}),
    );
    assert_eq!(status, 422, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/life-room/members/missing-user/remove",
        json!({}),
    );
    assert_eq!(status, 404, "{body}");

    // Owner removes a member; the active member list excludes them.
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/rooms/life-room/members/{carol_id}/remove"),
        json!({}),
    );
    assert_eq!(status, 200, "{body}");
    let r = bob
        .get(format!("{base}/api/v1/rooms/life-room"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().unwrap();
    assert!(
        !body["members"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["user_id"] == carol_id),
        "{body}"
    );
    // The removed member can no longer read over HTTP.
    let r = carol
        .get(format!("{base}/api/v1/rooms/life-room"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 403);

    // Owner leave is currently allowed (no transfer/forbid path).
    let (status, body) = post_json(&alice, &base, "/rooms/life-room/leave", json!({}));
    assert_eq!(status, 200, "{body}");
    let r = alice
        .get(format!("{base}/api/v1/rooms/life-room"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 403);
    let room = state
        .db
        .with_sync(|conn| repo::get_room_by_code(conn, "life-room"))
        .unwrap()
        .unwrap();
    assert_eq!(room.owner_user_id.as_deref(), Some(alice_id.as_str()));
    let r = bob
        .get(format!("{base}/api/v1/rooms/life-room"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().unwrap();
    assert_eq!(body["room"]["owner_user_id"], alice_id, "{body}");
    assert_eq!(body["room"]["my_role"], "member", "{body}");
}

#[test]
fn room_mutes_and_invite_revoke_paths() {
    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);
    let (alice, alice_id) = bootstrap_admin(&state, &base, "test-batch5-bootstrap-0123456789");
    let (bob, bob_id) = register_via_invite(&alice, &base, "bob-batch5");
    let (carol, _carol_id) = register_via_invite(&alice, &base, "carol-batch5");
    let (dave, _dave_id) = register_via_invite(&alice, &base, "dave-batch5");

    create_room(&alice, &base, "batch5-room");
    let token = invite_token(&alice, &base, "batch5-room");
    join_with_invite(&bob, &base, "batch5-room", &token);

    // Only the owner can create mutes; owner cannot mute themselves.
    let (status, body) = post_json(
        &bob,
        &base,
        "/rooms/batch5-room/mutes",
        json!({"scope": "member", "user_id": alice_id, "duration_seconds": 3600}),
    );
    assert_eq!(status, 403, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/batch5-room/mutes",
        json!({"scope": "member", "user_id": alice_id, "duration_seconds": 3600}),
    );
    assert_eq!(status, 422, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/batch5-room/mutes",
        json!({"scope": "member", "duration_seconds": 3600}),
    );
    assert_eq!(status, 422, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/batch5-room/mutes",
        json!({"scope": "member", "user_id": "missing-user", "duration_seconds": 3600}),
    );
    assert_eq!(status, 404, "{body}");

    // A member mute rejects sends until it is removed.
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/batch5-room/mutes",
        json!({"scope": "member", "user_id": bob_id, "duration_seconds": 3600}),
    );
    assert_eq!(status, 200, "{body}");
    let member_mute_id = body["mute"]["mute_id"].as_str().unwrap().to_string();
    assert_eq!(body["mute"]["scope"], "member", "{body}");
    let (status, body) = post_json(
        &bob,
        &base,
        "/rooms/batch5-room/messages",
        json!({"body": "muted by batch5"}),
    );
    assert_eq!(status, 403, "{body}");
    assert_eq!(body["error"]["code"], "muted", "{body}");
    let r = alice
        .get(format!("{base}/api/v1/rooms/batch5-room/mutes"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().unwrap();
    assert!(
        body["mutes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["mute_id"] == member_mute_id),
        "{body}"
    );
    // Non-owners cannot remove mutes.
    let (status, body) = delete_json(
        &bob,
        &base,
        &format!("/rooms/batch5-room/mutes/{member_mute_id}"),
    );
    assert_eq!(status, 403, "{body}");
    let (status, body) = delete_json(
        &alice,
        &base,
        &format!("/rooms/batch5-room/mutes/{member_mute_id}"),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &bob,
        &base,
        "/rooms/batch5-room/messages",
        json!({"body": "after member unmute"}),
    );
    assert_eq!(status, 200, "{body}");

    // HTTP room-scope mute is retired; the canonical all-room switch is the
    // WebSocket room_mute_toggle (covered by the browser suite).
    let (status, body) = post_json(
        &bob,
        &base,
        "/rooms/batch5-room/mutes",
        json!({"scope": "room", "duration_seconds": 3600}),
    );
    assert_eq!(status, 422, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/batch5-room/mutes",
        json!({"scope": "room", "duration_seconds": 3600}),
    );
    assert_eq!(status, 422, "{body}");

    // Creating and listing invites; non-owner revoke is forbidden.
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/batch5-room/invites",
        json!({"max_role": "member"}),
    );
    assert_eq!(status, 200, "{body}");
    let revoked_invite_id = body["invite"]["invite_id"].as_str().unwrap().to_string();
    let revoked_token = body["invite"]["token"].as_str().unwrap().to_string();
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/rooms/batch5-room/invites/{revoked_invite_id}/revoke"),
        json!({}),
    );
    assert_eq!(status, 403, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/batch5-room/invites/no-such-invite/revoke",
        json!({}),
    );
    assert_eq!(status, 404, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/rooms/batch5-room/invites/{revoked_invite_id}/revoke"),
        json!({}),
    );
    assert_eq!(status, 200, "{body}");
    // Revoking an already-revoked, still-unused invite is idempotent.
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/rooms/batch5-room/invites/{revoked_invite_id}/revoke"),
        json!({}),
    );
    assert_eq!(status, 200, "{body}");
    let r = alice
        .get(format!("{base}/api/v1/rooms/batch5-room/invites"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().unwrap();
    let revoked = body["invites"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["invite_id"] == revoked_invite_id)
        .unwrap();
    assert!(!revoked["revoked_at"].is_null(), "{body}");
    assert_eq!(revoked["active"], false, "{body}");
    let (status, body) = post_json(
        &carol,
        &base,
        "/rooms/batch5-room/join",
        json!({"invite_token": revoked_token}),
    );
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invite_invalid", "{body}");

    // Already-used invites cannot be redeemed or revoked.
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/batch5-room/invites",
        json!({"max_role": "member"}),
    );
    assert_eq!(status, 200, "{body}");
    let used_invite_id = body["invite"]["invite_id"].as_str().unwrap().to_string();
    let used_token = body["invite"]["token"].as_str().unwrap().to_string();
    let (status, body) = post_json(
        &dave,
        &base,
        "/rooms/batch5-room/join",
        json!({"invite_token": used_token}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/rooms/batch5-room/invites/{used_invite_id}/revoke"),
        json!({}),
    );
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"]["code"], "conflict", "{body}");
    let (status, body) = post_json(
        &carol,
        &base,
        "/rooms/batch5-room/join",
        json!({"invite_token": used_token}),
    );
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invite_invalid", "{body}");

    // Expired invites are rejected by the same redeem path.
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/batch5-room/invites",
        json!({"max_role": "member"}),
    );
    assert_eq!(status, 200, "{body}");
    let expired_invite_id = body["invite"]["invite_id"].as_str().unwrap().to_string();
    let expired_token = body["invite"]["token"].as_str().unwrap().to_string();
    let past = (util::now() - chrono::Duration::hours(1)).to_rfc3339();
    state
        .db
        .with_sync(|conn| {
            conn.execute(
                "UPDATE invites SET expires_at = ?1 WHERE invite_id = ?2",
                rusqlite::params![past, expired_invite_id],
            )
            .map_err(cloudroom_server::ApiError::from)?;
            Ok::<(), cloudroom_server::ApiError>(())
        })
        .unwrap();
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/rooms/batch5-room/invites/{expired_invite_id}/revoke"),
        json!({}),
    );
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"]["code"], "conflict", "{body}");
    let (status, body) = post_json(
        &carol,
        &base,
        "/rooms/batch5-room/join",
        json!({"invite_token": expired_token}),
    );
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invite_invalid", "{body}");
}

#[test]
fn direct_room_paths() {
    fn counts(state: &AppState) -> (i64, i64) {
        state
            .db
            .with_sync(|conn| {
                let rooms = conn
                    .query_row(
                        "SELECT COUNT(*) FROM rooms WHERE kind = 'direct'",
                        [],
                        |r| r.get(0),
                    )
                    .map_err(cloudroom_server::ApiError::from)?;
                let pairs = conn
                    .query_row("SELECT COUNT(*) FROM direct_pairs", [], |r| r.get(0))
                    .map_err(cloudroom_server::ApiError::from)?;
                Ok((rooms, pairs))
            })
            .unwrap()
    }

    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);
    let (alice, _alice_id) = bootstrap_admin(&state, &base, "test-batch6-bootstrap-0123456789");
    let (bob, bob_id) = register_via_invite(&alice, &base, "bob-direct");
    let (carol, carol_id) = register_via_invite(&alice, &base, "carol-direct");
    let (dave, dave_id) = register_via_invite(&alice, &base, "dave-direct");

    create_room(&alice, &base, "direct-parent");
    let token = invite_token(&alice, &base, "direct-parent");
    join_with_invite(&bob, &base, "direct-parent", &token);

    // Non-members cannot create a direct room from the parent.
    let (status, body) = post_json(
        &carol,
        &base,
        "/rooms/direct-parent/direct",
        json!({"user_id": bob_id}),
    );
    assert_eq!(status, 403, "{body}");
    // The peer must be an active member of the parent.
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/direct-parent/direct",
        json!({"user_id": "missing-peer"}),
    );
    assert_eq!(status, 404, "{body}");
    // Anonymous requests are rejected by the auth extractor (401, existing contract).
    let anon = client();
    let (status, body) = post_json(
        &anon,
        &base,
        "/rooms/direct-parent/direct",
        json!({"user_id": bob_id}),
    );
    assert_eq!(status, 401, "{body}");

    let token = invite_token(&alice, &base, "direct-parent");
    join_with_invite(&carol, &base, "direct-parent", &token);
    let token = invite_token(&alice, &base, "direct-parent");
    join_with_invite(&dave, &base, "direct-parent", &token);

    // Normal creation, then the same request returns the same room.
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/direct-parent/direct",
        json!({"user_id": bob_id}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["room"]["kind"], "direct", "{body}");
    assert_eq!(body["room"]["my_role"], "owner", "{body}");
    let bob_dm = body["room"]["code"].as_str().unwrap().to_string();
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/direct-parent/direct",
        json!({"user_id": bob_id}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["room"]["code"], bob_dm, "{body}");
    let r = bob
        .get(format!("{base}/api/v1/rooms/{bob_dm}"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().unwrap();
    assert_eq!(body["room"]["my_role"], "owner", "{body}");
    assert_eq!(body["members"].as_array().unwrap().len(), 2, "{body}");
    assert!(
        body["members"]
            .as_array()
            .unwrap()
            .iter()
            .all(|m| m["room_role"] == "owner"),
        "{body}"
    );

    // Concurrent identical requests serialize through the DB mutex and converge
    // on one room. There is no request field to collide a code directly.
    let token = csrf_opt(&alice, &base).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for _ in 0..2 {
        let c = alice.clone();
        let base = base.clone();
        let token = token.clone();
        let peer = dave_id.clone();
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            let r = c
                .post(format!("{base}/api/v1/rooms/direct-parent/direct"))
                .header("x-csrf-token", token)
                .json(&json!({"user_id": peer}))
                .send()
                .unwrap();
            let status = r.status().as_u16();
            let body: Value = r.json().unwrap_or(Value::Null);
            (status, body)
        }));
    }
    let results: Vec<(u16, Value)> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results[0].0, 200, "{}", results[0].1);
    assert_eq!(results[1].0, 200, "{}", results[1].1);
    let dave_dm = results[0].1["room"]["code"].as_str().unwrap();
    assert_eq!(results[1].1["room"]["code"], dave_dm);
    assert_eq!(counts(&state).0, 2, "one DM per peer pair");

    // Force a failure after room insert + direct_pair + members;
    // the surrounding SAVEPOINT must roll the whole creation back.
    state
        .db
        .with_sync(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER fail_direct_insert BEFORE INSERT ON rooms
                 WHEN NEW.kind = 'direct'
                 BEGIN
                   SELECT RAISE(ABORT, 'forced direct insert failure');
                 END;",
            )
            .map_err(cloudroom_server::ApiError::from)?;
            Ok::<(), cloudroom_server::ApiError>(())
        })
        .unwrap();
    let before = counts(&state);
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/direct-parent/direct",
        json!({"user_id": carol_id}),
    );
    assert_ne!(status, 200, "{body}");
    assert_eq!(counts(&state), before, "failed creation left partial rows");
    state
        .db
        .with_sync(|conn| {
            conn.execute_batch("DROP TRIGGER fail_direct_insert;")
                .map_err(cloudroom_server::ApiError::from)?;
            Ok::<(), cloudroom_server::ApiError>(())
        })
        .unwrap();
    // After removing the trigger the same request succeeds with no residue from
    // the failed attempt.
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/direct-parent/direct",
        json!({"user_id": carol_id}),
    );
    assert_eq!(status, 200, "{body}");
    let carol_dm = body["room"]["code"].as_str().unwrap().to_string();
    let after = counts(&state);
    assert_eq!(after.0, before.0 + 1);
    assert_eq!(after.1, before.1 + 1);
    let r = carol
        .get(format!("{base}/api/v1/rooms/{carol_dm}"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
}

#[test]
fn avatar_upload_and_delete_paths() {
    fn png(color: [u8; 3]) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(64, 64, image::Rgb(color))
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    fn upload(
        c: &reqwest::blocking::Client,
        base: &str,
        token: &str,
        name: &str,
        mime: &str,
        bytes: Vec<u8>,
    ) -> (u16, Value) {
        let part = reqwest::blocking::multipart::Part::bytes(bytes)
            .file_name(name.to_string())
            .mime_str(mime)
            .unwrap();
        let form = reqwest::blocking::multipart::Form::new().part("avatar", part);
        let r = c
            .post(format!("{base}/api/v1/me/avatar"))
            .header("x-csrf-token", token)
            .multipart(form)
            .send()
            .unwrap();
        let status = r.status().as_u16();
        let body: Value = r.json().unwrap_or(Value::Null);
        (status, body)
    }

    fn avatar_files(state: &AppState) -> usize {
        std::fs::read_dir(&state.avatars_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "png"))
            .count()
    }

    let (state, _dir) = test_state_with(|cfg| cfg.security.max_avatar_bytes = 4096);
    let (_rt, base) = start_server(&state);
    let (alice, alice_id) = bootstrap_admin(&state, &base, "test-batch7-bootstrap-0123456789");
    let (bob, _bob_id) = register_via_invite(&alice, &base, "bob-avatar");
    let token = csrf_opt(&alice, &base).unwrap();

    let (status, body) = upload(
        &alice,
        &base,
        &token,
        "first.png",
        "image/png",
        png([1, 2, 3]),
    );
    assert_eq!(status, 200, "{body}");
    let first_url = body["avatar_url"].as_str().unwrap().to_string();
    let first_id = first_url.rsplit('/').next().unwrap().to_string();
    let first_path = state.avatars_dir.join(format!("{first_id}.png"));
    assert!(first_path.exists(), "{first_url}");
    assert_eq!(avatar_files(&state), 1);
    let r = alice.get(format!("{base}{first_url}")).send().unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "image/png");

    // Invalid, oversized and empty uploads leave no files behind.
    let before = avatar_files(&state);
    let (status, body) = upload(
        &alice,
        &base,
        &token,
        "bad.svg",
        "image/svg+xml",
        b"<svg onload=alert(1)>".to_vec(),
    );
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "avatar_unsupported", "{body}");
    assert_eq!(avatar_files(&state), before);
    let oversized = vec![0u8; state.cfg.security.max_avatar_bytes + 1];
    let (status, body) = upload(&alice, &base, &token, "big.png", "image/png", oversized);
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "avatar_too_large", "{body}");
    assert_eq!(avatar_files(&state), before);
    let (status, body) = upload(&alice, &base, &token, "empty.png", "image/png", Vec::new());
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "avatar_unsupported", "{body}");
    assert_eq!(avatar_files(&state), before);

    // Replacing an avatar writes the new file and removes the old one.
    let (status, body) = upload(
        &alice,
        &base,
        &token,
        "second.png",
        "image/png",
        png([4, 5, 6]),
    );
    assert_eq!(status, 200, "{body}");
    let second_url = body["avatar_url"].as_str().unwrap().to_string();
    assert_ne!(second_url, first_url, "{body}");
    assert!(!first_path.exists(), "old avatar file was not removed");
    assert_eq!(avatar_files(&state), 1);
    let r = alice.get(format!("{base}{first_url}")).send().unwrap();
    assert_eq!(r.status(), 404);
    let r = alice.get(format!("{base}{second_url}")).send().unwrap();
    assert_eq!(r.status(), 200);
    let r = alice.get(format!("{base}/api/v1/me")).send().unwrap();
    assert_eq!(r.json::<Value>().unwrap()["user"]["avatar_url"], second_url);

    // DELETE /me/avatar has no target parameter: another user cannot delete
    // someone else's avatar, even if a user_id is supplied in the body.
    let bob_token = csrf_opt(&bob, &base).unwrap();
    let r = bob
        .delete(format!("{base}/api/v1/me/avatar"))
        .header("x-csrf-token", bob_token)
        .json(&json!({"user_id": alice_id}))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200, "{:?}", r.text());
    let second_id = second_url.rsplit('/').next().unwrap().to_string();
    assert!(state.avatars_dir.join(format!("{second_id}.png")).exists());
    let r = alice.get(format!("{base}{second_url}")).send().unwrap();
    assert_eq!(r.status(), 200);
    let r = alice.get(format!("{base}/api/v1/me")).send().unwrap();
    assert_eq!(r.json::<Value>().unwrap()["user"]["avatar_url"], second_url);

    // Deleting one's own avatar removes the file and clears the field.
    let (status, body) = delete_json(&alice, &base, "/me/avatar");
    assert_eq!(status, 200, "{body}");
    assert_eq!(avatar_files(&state), 0);
    let r = alice.get(format!("{base}{second_url}")).send().unwrap();
    assert_eq!(r.status(), 404);
    let r = alice.get(format!("{base}/api/v1/me")).send().unwrap();
    assert!(r.json::<Value>().unwrap()["user"]["avatar_url"].is_null());
    // Deleting again is idempotent (there is no avatar to delete).
    let (status, body) = delete_json(&alice, &base, "/me/avatar");
    assert_eq!(status, 200, "{body}");
    assert_eq!(avatar_files(&state), 0);
    let r = alice
        .get(format!("{base}/api/v1/avatars/{}", "0".repeat(32)))
        .send()
        .unwrap();
    assert_eq!(r.status(), 404);
}

#[test]
fn avatar_db_failure_cleans_new_file() {
    fn png() -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::RgbImage::from_pixel(64, 64, image::Rgb([7, 8, 9]))
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    fn upload(c: &reqwest::blocking::Client, base: &str, token: &str) -> (u16, Value) {
        let part = reqwest::blocking::multipart::Part::bytes(png())
            .file_name("avatar.png")
            .mime_str("image/png")
            .unwrap();
        let form = reqwest::blocking::multipart::Form::new().part("avatar", part);
        let r = c
            .post(format!("{base}/api/v1/me/avatar"))
            .header("x-csrf-token", token)
            .multipart(form)
            .send()
            .unwrap();
        let status = r.status().as_u16();
        let body: Value = r.json().unwrap_or(Value::Null);
        (status, body)
    }

    fn avatar_files(state: &AppState) -> usize {
        std::fs::read_dir(&state.avatars_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "png"))
            .count()
    }

    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);
    let (alice, _alice_id) = bootstrap_admin(&state, &base, "test-batch7-failure-0123456789");
    let token = csrf_opt(&alice, &base).unwrap();

    state
        .db
        .with_sync(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER fail_avatar_update BEFORE UPDATE OF avatar_id ON users
                 WHEN NEW.avatar_id IS NOT NULL
                 BEGIN
                   SELECT RAISE(ABORT, 'forced avatar update failure');
                 END;",
            )
            .map_err(cloudroom_server::ApiError::from)?;
            Ok::<(), cloudroom_server::ApiError>(())
        })
        .unwrap();
    let (status, body) = upload(&alice, &base, &token);
    assert_ne!(status, 200, "{body}");
    assert_eq!(avatar_files(&state), 0, "failed upload left an orphan file");
    state
        .db
        .with_sync(|conn| {
            conn.execute_batch("DROP TRIGGER fail_avatar_update;")
                .map_err(cloudroom_server::ApiError::from)?;
            Ok::<(), cloudroom_server::ApiError>(())
        })
        .unwrap();
    let (status, body) = upload(&alice, &base, &token);
    assert_eq!(status, 200, "{body}");
    assert_eq!(avatar_files(&state), 1);
    let (status, body) = delete_json(&alice, &base, "/me/avatar");
    assert_eq!(status, 200, "{body}");
    assert_eq!(avatar_files(&state), 0);
}

#[test]
fn admin_reject_force_delete_and_transfer() {
    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);
    let (alice, alice_id) = bootstrap_admin(&state, &base, "test-batch8-admin-0123456789");
    let (bob, bob_id) = register_via_invite(&alice, &base, "bob-admin");
    let (carol, carol_id) = register_via_invite(&alice, &base, "carol-admin");
    let (dave, dave_id) = register_via_invite(&alice, &base, "dave-admin-transfer");

    // Promote bob to admin, but keep carol as a regular user.
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/admin/users/{bob_id}/role"),
        json!({"role": "admin"}),
    );
    assert_eq!(status, 200, "{body}");

    // Anonymous and non-admin writes are rejected.
    let anon = client();
    let (status, _) = post_json(
        &anon,
        &base,
        "/admin/room-requests/missing/reject",
        json!({"reason": "x"}),
    );
    assert_eq!(status, 401);
    let (status, body) = post_json(
        &carol,
        &base,
        "/admin/room-requests/missing/reject",
        json!({"reason": "x"}),
    );
    assert_eq!(status, 403, "{body}");

    // Reject a pending room request, then lock the current duplicate behavior.
    let (status, body) = post_json(
        &carol,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "media_mode": "p2p"}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["request"]["status"], "pending", "{body}");
    let request_id = body["request"]["request_id"].as_str().unwrap().to_string();
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/admin/room-requests/{request_id}/reject"),
        json!({"reason": "batch8 reject"}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/admin/room-requests/{request_id}/reject"),
        json!({"reason": "again"}),
    );
    assert_eq!(status, 409, "{body}");

    // Force-delete a standard room, verify gone, and duplicate behavior.
    create_room(&alice, &base, "admin-close");
    let closed_id = state
        .db
        .with_sync(|conn| repo::get_room_by_code(conn, "admin-close"))
        .unwrap()
        .unwrap()
        .room_id;
    let (status, body) = post_json(&carol, &base, "/admin/rooms/admin-close/delete", json!({}));
    assert_eq!(status, 403, "{body}");
    let (status, body) = post_json(&bob, &base, "/admin/rooms/admin-close/delete", json!({}));
    assert_eq!(status, 200, "{body}");
    let r = alice
        .get(format!("{base}/api/v1/rooms/admin-close"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 410);
    let (status, body) = post_json(&bob, &base, "/admin/rooms/admin-close/delete", json!({}));
    assert_eq!(status, 409, "{body}");
    let (status, body) = post_json(&bob, &base, "/admin/rooms/no-such-room/delete", json!({}));
    assert_eq!(status, 404, "{body}");

    // Transfer ownership (super-admin only) and lock repeat behavior.
    create_room(&alice, &base, "transfer-room");
    let transfer_id = state
        .db
        .with_sync(|conn| repo::get_room_by_code(conn, "transfer-room"))
        .unwrap()
        .unwrap()
        .room_id;
    let token = invite_token(&alice, &base, "transfer-room");
    join_with_invite(&carol, &base, "transfer-room", &token);
    let (status, body) = post_json(
        &bob,
        &base,
        "/admin/rooms/transfer-room/transfer-owner",
        json!({"user_id": carol_id}),
    );
    assert_eq!(status, 403, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        "/admin/rooms/transfer-room/transfer-owner",
        json!({"user_id": carol_id}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        "/admin/rooms/transfer-room/transfer-owner",
        json!({"user_id": carol_id}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        "/admin/rooms/transfer-room/transfer-owner",
        json!({"user_id": "missing-user"}),
    );
    assert_eq!(status, 404, "{body}");
    let r = carol
        .get(format!("{base}/api/v1/rooms/transfer-room"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().unwrap();
    assert_eq!(body["room"]["my_role"], "owner", "{body}");
    assert_eq!(body["room"]["owner_user_id"], carol_id, "{body}");
    assert_eq!(
        body["members"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["room_role"] == "owner")
            .count(),
        1,
        "{body}"
    );
    let r = alice
        .get(format!("{base}/api/v1/rooms/transfer-room"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<Value>().unwrap()["room"]["my_role"], "member");
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/transfer-room/settings",
        json!({"text_enabled": false}),
    );
    assert_eq!(status, 403, "{body}");
    let (status, body) = post_json(
        &carol,
        &base,
        "/rooms/transfer-room/settings",
        json!({"text_enabled": false}),
    );
    assert_eq!(status, 200, "{body}");

    // Transfer to a user who is not yet a room member; the previous owner is demoted.
    let (status, body) = post_json(
        &alice,
        &base,
        "/admin/rooms/transfer-room/transfer-owner",
        json!({"user_id": dave_id}),
    );
    assert_eq!(status, 200, "{body}");
    let r = dave
        .get(format!("{base}/api/v1/rooms/transfer-room"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().unwrap();
    assert_eq!(body["room"]["my_role"], "owner", "{body}");
    assert_eq!(body["room"]["owner_user_id"], dave_id, "{body}");
    assert_eq!(
        body["members"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["room_role"] == "owner")
            .count(),
        1,
        "{body}"
    );
    let r = carol
        .get(format!("{base}/api/v1/rooms/transfer-room"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<Value>().unwrap()["room"]["my_role"], "member");

    // Audit records carry actor/action/target/detail fields.
    let entries = audit_entries(&bob, &base);
    let rejected = entries
        .iter()
        .find(|e| e["action"] == "room_request.rejected" && e["target_id"] == request_id)
        .unwrap();
    assert_eq!(rejected["actor_user_id"], bob_id, "{rejected}");
    assert_eq!(rejected["target_type"], "room_request", "{rejected}");
    assert_eq!(rejected["detail"]["reason"], "batch8 reject", "{rejected}");
    let deleted = entries
        .iter()
        .find(|e| {
            e["action"] == "room.deleted"
                && e["target_id"] == closed_id
                && e["detail"]["reason"] == "admin_delete"
        })
        .unwrap();
    assert_eq!(deleted["actor_user_id"], bob_id, "{deleted}");
    let transferred: Vec<&Value> = entries
        .iter()
        .filter(|e| e["action"] == "room.owner_transferred" && e["target_id"] == transfer_id)
        .collect();
    assert_eq!(transferred.len(), 3, "{entries:?}");
    // CSV export is admin-only, bounded, and includes a header.
    let r = bob
        .get(format!("{base}/api/v1/admin/audit/export?limit=10"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("text/csv"));
    let csv = r.text().unwrap();
    assert!(csv.starts_with("entry_id,created_at,actor_user_id,action"), "{csv}");
    assert!(csv.lines().count() <= 11, "{csv}");
    assert_eq!(transferred[0]["detail"]["new_owner"], dave_id);
    assert_eq!(transferred[1]["detail"]["new_owner"], carol_id);
    assert_eq!(transferred[0]["actor_user_id"], alice_id);
    assert_eq!(transferred[0]["detail"]["room_code"], "transfer-room");
}

#[test]
fn admin_user_management() {
    fn login(base: &str, username: &str, password: &str) -> (u16, Value) {
        let c = client();
        post_json(
            &c,
            base,
            "/auth/login",
            json!({"username": username, "password": password}),
        )
    }

    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);
    let (alice, alice_id) = bootstrap_admin(&state, &base, "test-batch8-users-0123456789");
    let (bob, bob_id) = register_via_invite(&alice, &base, "bob-admin-users");
    let (carol, carol_id) = register_via_invite(&alice, &base, "carol-admin-users");
    let (_dave, dave_id) = register_via_invite(&alice, &base, "dave-admin-users");

    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/admin/users/{bob_id}/role"),
        json!({"role": "admin"}),
    );
    assert_eq!(status, 200, "{body}");

    // Unauthenticated/non-admin guards.
    let anon = client();
    let (status, _) = post_json(
        &anon,
        &base,
        &format!("/admin/users/{carol_id}/disable"),
        json!({"disabled": true}),
    );
    assert_eq!(status, 401);
    let (status, body) = post_json(
        &carol,
        &base,
        &format!("/admin/users/{bob_id}/disable"),
        json!({"disabled": true}),
    );
    assert_eq!(status, 403, "{body}");
    let r = carol
        .get(format!("{base}/api/v1/admin/audit"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 403);

    // Disable/enable a normal user; disable revokes sessions, repeat is idempotent.
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/admin/users/{carol_id}/disable"),
        json!({"disabled": true}),
    );
    assert_eq!(status, 200, "{body}");
    let r = carol.get(format!("{base}/api/v1/me")).send().unwrap();
    assert_eq!(r.status(), 401);
    let (status, body) = login(&base, "carol-admin-users", "correct-horse-battery");
    assert_eq!(status, 403, "{body}");
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/admin/users/{carol_id}/disable"),
        json!({"disabled": true}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/admin/users/{carol_id}/disable"),
        json!({"disabled": false}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = login(&base, "carol-admin-users", "correct-horse-battery");
    assert_eq!(status, 200, "{body}");

    // Target restrictions.
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/admin/users/{bob_id}/disable"),
        json!({"disabled": true}),
    );
    assert_eq!(status, 422, "{body}");
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/admin/users/{alice_id}/disable"),
        json!({"disabled": true}),
    );
    assert_eq!(status, 403, "{body}");
    let (status, body) = post_json(
        &bob,
        &base,
        "/admin/users/missing-user/disable",
        json!({"disabled": true}),
    );
    assert_eq!(status, 404, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/admin/users/{dave_id}/role"),
        json!({"role": "admin"}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/admin/users/{dave_id}/disable"),
        json!({"disabled": true}),
    );
    assert_eq!(status, 403, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/admin/users/{dave_id}/disable"),
        json!({"disabled": true}),
    );
    assert_eq!(status, 200, "{body}");

    // Role changes are super-admin only; invalid/self/missing targets are rejected.
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/admin/users/{carol_id}/role"),
        json!({"role": "admin"}),
    );
    assert_eq!(status, 403, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/admin/users/{carol_id}/role"),
        json!({"role": "admin"}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/admin/users/{carol_id}/role"),
        json!({"role": "admin"}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/admin/users/{alice_id}/role"),
        json!({"role": "user"}),
    );
    assert_eq!(status, 422, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/admin/users/{carol_id}/role"),
        json!({"role": "owner"}),
    );
    assert_eq!(status, 422, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        "/admin/users/missing-user/role",
        json!({"role": "user"}),
    );
    assert_eq!(status, 404, "{body}");

    // A plain admin cannot reset another admin; super_admin can.
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/admin/users/{carol_id}/reset-password"),
        json!({}),
    );
    assert_eq!(status, 403, "{body}");
    // Password reset invalidates old sessions/passwords; repeat issues a new temp.
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/admin/users/{carol_id}/reset-password"),
        json!({}),
    );
    assert_eq!(status, 200, "{body}");
    let temp1 = body["temporary_password"].as_str().unwrap().to_string();
    let (status, _) = login(&base, "carol-admin-users", "correct-horse-battery");
    assert_eq!(status, 401);
    let (status, body) = login(&base, "carol-admin-users", &temp1);
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/admin/users/{carol_id}/reset-password"),
        json!({}),
    );
    assert_eq!(status, 200, "{body}");
    let temp2 = body["temporary_password"].as_str().unwrap().to_string();
    let (status, _) = login(&base, "carol-admin-users", &temp1);
    assert_eq!(status, 401);
    let (status, body) = login(&base, "carol-admin-users", &temp2);
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/admin/users/{alice_id}/reset-password"),
        json!({}),
    );
    assert_eq!(status, 403, "{body}");

    // Audit fields and actions.
    let entries = audit_entries(&alice, &base);
    let disabled = entries
        .iter()
        .find(|e| e["action"] == "user.disabled" && e["target_id"] == carol_id)
        .unwrap();
    assert_eq!(disabled["actor_user_id"], bob_id, "{disabled}");
    assert_eq!(disabled["target_type"], "user", "{disabled}");
    assert!(disabled["created_at"].is_string(), "{disabled}");
    assert!(
        entries
            .iter()
            .any(|e| e["action"] == "user.enabled" && e["target_id"] == carol_id),
        "{entries:?}"
    );
    let role_change = entries
        .iter()
        .find(|e| e["action"] == "user.role_changed" && e["target_id"] == carol_id)
        .unwrap();
    assert_eq!(role_change["detail"]["role"], "admin", "{role_change}");
    let reset = entries
        .iter()
        .find(|e| {
            e["action"] == "user.password_reset"
                && e["target_id"] == carol_id
                && e["actor_user_id"] == alice_id
        })
        .unwrap();
    assert_eq!(reset["actor_user_id"], alice_id, "{reset}");
    assert_eq!(reset["target_type"], "user", "{reset}");
}

#[test]
fn admin_invite_revoke_and_audit() {
    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);
    let (alice, _alice_id) = bootstrap_admin(&state, &base, "test-batch8-invite-0123456789");
    let (bob, bob_id) = register_via_invite(&alice, &base, "bob-admin-invite");
    let (carol, _carol_id) = register_via_invite(&alice, &base, "carol-admin-invite");

    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/admin/users/{bob_id}/role"),
        json!({"role": "admin"}),
    );
    assert_eq!(status, 200, "{body}");

    // Only admins may create/revoke registration invites.
    let anon = client();
    let (status, _) = post_json(&anon, &base, "/admin/invites", json!({}));
    assert_eq!(status, 401);
    let (status, body) = post_json(&carol, &base, "/admin/invites", json!({}));
    assert_eq!(status, 403, "{body}");

    let (status, body) = post_json(&bob, &base, "/admin/invites", json!({}));
    assert_eq!(status, 200, "{body}");
    let invite_id = body["invite"]["invite_id"].as_str().unwrap().to_string();
    let token = body["token"].as_str().unwrap().to_string();
    let r = bob
        .get(format!("{base}/api/v1/admin/invites"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().unwrap();
    let listed = body["invites"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["invite_id"] == invite_id)
        .unwrap();
    assert_eq!(listed["active"], true, "{body}");

    let (status, _) = post_json(
        &anon,
        &base,
        &format!("/admin/invites/{invite_id}/revoke"),
        json!({}),
    );
    assert_eq!(status, 401);
    let (status, body) = post_json(
        &carol,
        &base,
        &format!("/admin/invites/{invite_id}/revoke"),
        json!({}),
    );
    assert_eq!(status, 403, "{body}");
    let (status, body) = post_json(
        &bob,
        &base,
        "/admin/invites/no-such-invite/revoke",
        json!({}),
    );
    assert_eq!(status, 404, "{body}");

    // A room invite is not a registration invite.
    create_room(&alice, &base, "invite-admin-room");
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/invite-admin-room/invites",
        json!({"max_role": "member"}),
    );
    assert_eq!(status, 200, "{body}");
    let room_invite_id = body["invite"]["invite_id"].as_str().unwrap().to_string();
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/admin/invites/{room_invite_id}/revoke"),
        json!({}),
    );
    assert_eq!(status, 404, "{body}");

    // Revoke, repeat (current behavior is 200), and verify the token is dead.
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/admin/invites/{invite_id}/revoke"),
        json!({}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/admin/invites/{invite_id}/revoke"),
        json!({}),
    );
    assert_eq!(status, 200, "{body}");
    let r = bob
        .get(format!("{base}/api/v1/admin/invites"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().unwrap();
    let revoked = body["invites"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["invite_id"] == invite_id)
        .unwrap();
    assert!(!revoked["revoked_at"].is_null(), "{body}");
    assert_eq!(revoked["active"], false, "{body}");
    let fresh = client();
    let (status, body) = post_json(
        &fresh,
        &base,
        "/auth/register",
        json!({"username": "revoked-user", "password": "correct-horse-battery", "token": token}),
    );
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invite_invalid", "{body}");

    // A consumed registration invite cannot be revoked.
    let (status, body) = post_json(&bob, &base, "/admin/invites", json!({}));
    assert_eq!(status, 200, "{body}");
    let used_invite_id = body["invite"]["invite_id"].as_str().unwrap().to_string();
    let used_token = body["token"].as_str().unwrap().to_string();
    let used_client = client();
    let (status, body) = post_json(
        &used_client,
        &base,
        "/auth/register",
        json!({"username": "used-invite-user", "password": "correct-horse-battery", "token": used_token}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/admin/invites/{used_invite_id}/revoke"),
        json!({}),
    );
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"]["code"], "conflict", "{body}");

    // An expired registration invite cannot be revoked.
    let (status, body) = post_json(&bob, &base, "/admin/invites", json!({}));
    assert_eq!(status, 200, "{body}");
    let expired_invite_id = body["invite"]["invite_id"].as_str().unwrap().to_string();
    let past = (util::now() - chrono::Duration::hours(1)).to_rfc3339();
    state
        .db
        .with_sync(|conn| {
            conn.execute(
                "UPDATE invites SET expires_at = ?1 WHERE invite_id = ?2",
                rusqlite::params![past, expired_invite_id],
            )
            .map_err(cloudroom_server::ApiError::from)?;
            Ok::<(), cloudroom_server::ApiError>(())
        })
        .unwrap();
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/admin/invites/{expired_invite_id}/revoke"),
        json!({}),
    );
    assert_eq!(status, 409, "{body}");
    assert_eq!(body["error"]["code"], "conflict", "{body}");

    // Audit record fields and duplicate writes.
    let entries = audit_entries(&bob, &base);
    let revokes: Vec<&Value> = entries
        .iter()
        .filter(|e| {
            e["action"] == "invite.registration_revoked"
                && e["target_id"] == invite_id
                && e["actor_user_id"] == bob_id
        })
        .collect();
    assert_eq!(revokes.len(), 2, "{entries:?}");
    assert_eq!(revokes[0]["target_type"], "invite", "{revokes:?}");
    assert!(revokes[0]["created_at"].is_string(), "{revokes:?}");
    assert!(revokes[0]["detail"].is_object(), "{revokes:?}");
}

#[test]
fn left_member_access_paths() {
    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);
    let (alice, _alice_id) =
        bootstrap_admin(&state, &base, "test-left-access-bootstrap-0123456789");
    let (bob, bob_id) = register_via_invite(&alice, &base, "bob-left-access");
    let (carol, _carol_id) = register_via_invite(&alice, &base, "carol-left-access");

    create_room(&alice, &base, "left-access");
    let token = invite_token(&alice, &base, "left-access");
    join_with_invite(&bob, &base, "left-access", &token);

    let r = bob
        .get(format!("{base}/api/v1/rooms/left-access"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = bob
        .get(format!("{base}/api/v1/rooms/left-access/messages?after=0"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);

    let (status, body) = post_json(&bob, &base, "/rooms/left-access/leave", json!({}));
    assert_eq!(status, 200, "{body}");

    let r = bob
        .get(format!("{base}/api/v1/rooms/left-access"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 403);
    let r = bob
        .get(format!("{base}/api/v1/rooms/left-access/messages?after=0"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 403);
    let (status, body) = post_json(
        &bob,
        &base,
        "/rooms/left-access/messages",
        json!({"body": "after leave"}),
    );
    assert_eq!(status, 403, "{body}");
    let (status, body) = post_json(
        &bob,
        &base,
        "/rooms/left-access/settings",
        json!({"text_enabled": false}),
    );
    assert_eq!(status, 403, "{body}");
    let (status, body) = post_json(
        &bob,
        &base,
        "/rooms/left-access/direct",
        json!({"user_id": _carol_id}),
    );
    assert_eq!(status, 403, "{body}");
    let r = bob
        .get(format!("{base}/api/v1/rooms/left-access/ice-servers"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 403);
    let (status, body) = post_json(&bob, &base, "/rooms/left-access/media-token", json!({}));
    assert_eq!(status, 403, "{body}");
    let (status, body) = post_json(&bob, &base, "/rooms/left-access/leave", json!({}));
    assert_eq!(status, 403, "{body}");

    // A former member can still rejoin; the access gate applies only to active members.
    let (status, body) = post_json(&bob, &base, "/rooms/left-access/join", json!({}));
    assert_eq!(status, 200, "{body}");
    let r = bob
        .get(format!("{base}/api/v1/rooms/left-access"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = bob
        .get(format!("{base}/api/v1/rooms/left-access/messages?after=0"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);

    // Removal also closes HTTP access.
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/rooms/left-access/members/{bob_id}/remove"),
        json!({}),
    );
    assert_eq!(status, 200, "{body}");
    let r = bob
        .get(format!("{base}/api/v1/rooms/left-access"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 403);
    let r = bob
        .get(format!("{base}/api/v1/rooms/left-access/messages?after=0"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 403);

    // Existing non-member/unknown/anonymous contracts are unchanged.
    let r = carol
        .get(format!("{base}/api/v1/rooms/left-access"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 403);
    let r = carol
        .get(format!("{base}/api/v1/rooms/no-such-room"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 404);
    let anon = client();
    let r = anon
        .get(format!("{base}/api/v1/rooms/left-access"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 403);
}

#[test]
fn left_member_ws_closes() {
    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);

    let uid = "33333333333333333333333333333333";
    let hash = state.auth.hash_password("pw").unwrap();
    state
        .db
        .with_sync(|conn| {
            repo::create_user(
                conn,
                uid,
                Some("left-ws"),
                Some(&hash),
                UserRole::SuperAdmin,
                None,
            )?;
            let now = util::now();
            let room = Room {
                room_id: "44444444444444444444444444444444".into(),
                room_code: "left-ws".into(),
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
                e2ee: false,
                e2ee_key: None,
                attachments_enabled: false,
                attachment_bytes: 0,
                guest_upgrade_open: false,
            };
            repo::insert_room(conn, &room)?;
            repo::add_member(conn, &room.room_id, uid, RoomRole::Owner, "left-ws")?;
            Ok::<(), cloudroom_server::ApiError>(())
        })
        .unwrap();

    let c = client();
    let login = c
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"username": "left-ws", "password": "pw"}))
        .send()
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookie = login
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let ws_url = format!(
        "ws://{}/api/v1/rooms/left-ws/ws",
        base.trim_start_matches("http://")
    );

    let mut req =
        tungstenite::client::IntoClientRequest::into_client_request(ws_url.clone()).unwrap();
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    let (mut ws, _) = tungstenite::connect(req).expect("ws connect");
    if let tungstenite::stream::MaybeTlsStream::Plain(tcp) = ws.get_mut() {
        tcp.set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
    }
    let hello: Value = serde_json::from_str(&ws.read().unwrap().into_text().unwrap()).unwrap();
    assert_eq!(hello["type"], "hello");

    let (status, body) = post_json(&c, &base, "/rooms/left-ws/leave", json!({}));
    assert_eq!(status, 200, "{body}");
    ws.send(tungstenite::Message::Text(r#"{"type":"ping"}"#.into()))
        .unwrap();
    let started = std::time::Instant::now();
    if let Ok(tungstenite::Message::Text(text)) = ws.read() {
        assert!(
            !text.contains("pong"),
            "unexpected pong after leave: {text}"
        );
    }
    assert!(
        started.elapsed() < std::time::Duration::from_millis(1500),
        "stale WS did not close promptly"
    );

    let mut req2 = tungstenite::client::IntoClientRequest::into_client_request(ws_url).unwrap();
    req2.headers_mut().insert("cookie", cookie.parse().unwrap());
    let (mut ws2, _) = tungstenite::connect(req2).expect("ws reconnect");
    if let tungstenite::stream::MaybeTlsStream::Plain(tcp) = ws2.get_mut() {
        tcp.set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
    }
    match ws2.read() {
        Ok(tungstenite::Message::Text(text)) => {
            panic!("left member received a WS message: {text}")
        }
        Ok(tungstenite::Message::Close(_)) | Err(_) => {}
        Ok(_) => {}
    }
}

#[test]
fn kicked_rejoin_requires_new_invite() {
    fn left_reason(state: &AppState, room_id: &str, user_id: &str) -> Option<String> {
        state
            .db
            .with_sync(|conn| {
                conn.query_row(
                    "SELECT left_reason FROM room_members WHERE room_id = ?1 AND user_id = ?2",
                    rusqlite::params![room_id, user_id],
                    |r| r.get(0),
                )
                .map_err(cloudroom_server::ApiError::from)
            })
            .unwrap()
    }

    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);
    let (alice, _alice_id) =
        bootstrap_admin(&state, &base, "test-kick-rejoin-bootstrap-0123456789");
    let (bob, bob_id) = register_via_invite(&alice, &base, "bob-kick-rejoin");

    create_room(&alice, &base, "kick-rejoin");
    let room_id = state
        .db
        .with_sync(|conn| repo::get_room_by_code(conn, "kick-rejoin"))
        .unwrap()
        .unwrap()
        .room_id;
    let token = invite_token(&alice, &base, "kick-rejoin");
    join_with_invite(&bob, &base, "kick-rejoin", &token);

    // Deliberately leave an unused invite from before the kick.
    let old_invite = invite_token(&alice, &base, "kick-rejoin");
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/rooms/kick-rejoin/members/{bob_id}/remove"),
        json!({}),
    );
    assert_eq!(status, 200, "{body}");
    let r = bob
        .get(format!("{base}/api/v1/rooms/kick-rejoin"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 403);
    let member = state
        .db
        .with_sync(|conn| repo::get_member(conn, &room_id, &bob_id))
        .unwrap()
        .unwrap();
    assert!(member.left_at.is_some());
    assert_eq!(
        left_reason(&state, &room_id, &bob_id).as_deref(),
        Some("removed")
    );

    // A kicked user cannot rejoin with an empty request.
    let (status, body) = post_json(&bob, &base, "/rooms/kick-rejoin/join", json!({}));
    assert_eq!(status, 403, "{body}");

    // A pre-kick invite is not a new grant of entry.
    let (status, body) = post_json(
        &bob,
        &base,
        "/rooms/kick-rejoin/join",
        json!({"invite_token": old_invite}),
    );
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invite_invalid", "{body}");

    // A fresh invite created after the kick succeeds.
    std::thread::sleep(std::time::Duration::from_millis(5));
    let fresh_invite = invite_token(&alice, &base, "kick-rejoin");
    let (status, body) = post_json(
        &bob,
        &base,
        "/rooms/kick-rejoin/join",
        json!({"invite_token": fresh_invite}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["room"]["my_role"], "member", "{body}");
    let r = bob
        .get(format!("{base}/api/v1/rooms/kick-rejoin"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 200);
    let member = state
        .db
        .with_sync(|conn| repo::get_member(conn, &room_id, &bob_id))
        .unwrap()
        .unwrap();
    assert!(member.left_at.is_none());
    assert!(left_reason(&state, &room_id, &bob_id).is_none());

    // Voluntary leave/rejoin remains invite-free.
    let (status, body) = post_json(&bob, &base, "/rooms/kick-rejoin/leave", json!({}));
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        left_reason(&state, &room_id, &bob_id).as_deref(),
        Some("left")
    );
    let (status, body) = post_json(&bob, &base, "/rooms/kick-rejoin/join", json!({}));
    assert_eq!(status, 200, "{body}");
    assert!(left_reason(&state, &room_id, &bob_id).is_none());
}

#[test]
fn ws_mute_speak_and_reaction_sequences() {
    type Ws = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>;

    fn set_timeout(ws: &mut Ws, timeout: std::time::Duration) {
        if let tungstenite::stream::MaybeTlsStream::Plain(tcp) = ws.get_mut() {
            tcp.set_read_timeout(Some(timeout)).unwrap();
        }
    }

    fn connect_ws(url: &str, cookie: &str) -> Ws {
        let mut req = tungstenite::client::IntoClientRequest::into_client_request(url).unwrap();
        req.headers_mut().insert("cookie", cookie.parse().unwrap());
        let (mut ws, _) = tungstenite::connect(req).expect("ws connect");
        set_timeout(&mut ws, std::time::Duration::from_secs(2));
        ws
    }

    fn read_event(ws: &mut Ws) -> Value {
        let text = ws.read().unwrap().into_text().unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn expect_no_event(ws: &mut Ws) {
        set_timeout(ws, std::time::Duration::from_millis(250));
        let result = ws.read();
        set_timeout(ws, std::time::Duration::from_secs(2));
        match result {
            Err(tungstenite::Error::Io(err))
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Ok(tungstenite::Message::Text(text)) => panic!("unexpected WS event: {text}"),
            Ok(tungstenite::Message::Close(_)) => panic!("unexpected WS close"),
            Err(err) => panic!("unexpected WS error: {err}"),
            other => panic!("unexpected WS frame: {other:?}"),
        }
    }

    fn login_cookie(base: &str, username: &str, password: &str) -> String {
        let c = client();
        let r = c
            .post(format!("{base}/api/v1/auth/login"))
            .json(&json!({"username": username, "password": password}))
            .send()
            .unwrap();
        assert_eq!(r.status(), 200);
        r.headers()
            .get("set-cookie")
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    }

    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);
    let owner_id = "55555555555555555555555555555555";
    let member_id = "66666666666666666666666666666666";
    let hash = state.auth.hash_password("pw").unwrap();
    state
        .db
        .with_sync(|conn| {
            repo::create_user(
                conn,
                owner_id,
                Some("ws-owner"),
                Some(&hash),
                UserRole::SuperAdmin,
                None,
            )?;
            repo::create_user(
                conn,
                member_id,
                Some("ws-member"),
                Some(&hash),
                UserRole::User,
                None,
            )?;
            let now = util::now();
            let room = Room {
                room_id: "77777777777777777777777777777777".into(),
                room_code: "ws-events".into(),
                kind: RoomKind::Standard,
                owner_user_id: Some(owner_id.into()),
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
                e2ee: false,
                e2ee_key: None,
                attachments_enabled: false,
                attachment_bytes: 0,
                guest_upgrade_open: false,
            };
            repo::insert_room(conn, &room)?;
            repo::add_member(conn, &room.room_id, owner_id, RoomRole::Owner, "ws-owner")?;
            repo::add_member(
                conn,
                &room.room_id,
                member_id,
                RoomRole::Member,
                "ws-member",
            )?;
            Ok::<(), cloudroom_server::ApiError>(())
        })
        .unwrap();

    let owner_cookie = login_cookie(&base, "ws-owner", "pw");
    let member_cookie = login_cookie(&base, "ws-member", "pw");
    let url = format!(
        "ws://{}/api/v1/rooms/ws-events/ws",
        base.trim_start_matches("http://")
    );
    let mut owner = connect_ws(&url, &owner_cookie);
    let mut member = connect_ws(&url, &member_cookie);
    assert_eq!(read_event(&mut owner)["type"], "hello");
    assert_eq!(read_event(&mut member)["type"], "hello");

    // Room mute: state broadcast precedes the system message.
    owner
        .send(tungstenite::Message::Text(
            r#"{"type":"room_mute_toggle","muted":true}"#.into(),
        ))
        .unwrap();
    for ws in [&mut owner, &mut member] {
        let first = read_event(ws);
        let second = read_event(ws);
        assert_eq!(first["type"], "room_muted", "{first}");
        assert_eq!(first["muted"], true, "{first}");
        assert_eq!(second["type"], "message", "{second}");
        assert_eq!(second["message"]["body"], "房主开启了全员禁言", "{second}");
    }
    owner
        .send(tungstenite::Message::Text(
            r#"{"type":"room_mute_toggle","muted":true}"#.into(),
        ))
        .unwrap();
    expect_no_event(&mut owner);
    expect_no_event(&mut member);
    member
        .send(tungstenite::Message::Text(
            r#"{"type":"room_mute_toggle","muted":false}"#.into(),
        ))
        .unwrap();
    let err = read_event(&mut member);
    assert_eq!(err["type"], "error", "{err}");
    assert_eq!(err["code"], "forbidden", "{err}");
    expect_no_event(&mut owner);

    // Speak request -> owner notification; approval grant precedes system message.
    member
        .send(tungstenite::Message::Text(
            r#"{"type":"speak_request"}"#.into(),
        ))
        .unwrap();
    let ack = read_event(&mut member);
    assert_eq!(ack["type"], "speak_requested", "{ack}");
    let notify = read_event(&mut owner);
    assert_eq!(notify["type"], "speak_request_notify", "{notify}");
    assert_eq!(notify["user_id"], member_id, "{notify}");
    owner
        .send(tungstenite::Message::Text(format!(
            r#"{{"type":"speak_reply","user_id":"{member_id}","approve":true}}"#
        )))
        .unwrap();
    for ws in [&mut owner, &mut member] {
        let first = read_event(ws);
        let second = read_event(ws);
        assert_eq!(first["type"], "speak_grant", "{first}");
        assert_eq!(first["user_id"], member_id, "{first}");
        assert_eq!(second["type"], "message", "{second}");
        assert_eq!(
            second["message"]["body"], "ws-member 获得了发言权限",
            "{second}"
        );
    }
    owner
        .send(tungstenite::Message::Text(format!(
            r#"{{"type":"speak_reply","user_id":"{member_id}","approve":true}}"#
        )))
        .unwrap();
    expect_no_event(&mut owner);
    expect_no_event(&mut member);

    // Revoke then re-grant, then self-relinquish.
    owner
        .send(tungstenite::Message::Text(format!(
            r#"{{"type":"speak_revoke","user_id":"{member_id}"}}"#
        )))
        .unwrap();
    for ws in [&mut owner, &mut member] {
        assert_eq!(read_event(ws)["type"], "speak_revoked");
        let msg = read_event(ws);
        assert_eq!(msg["type"], "message", "{msg}");
        assert_eq!(msg["message"]["body"], "ws-member 的发言权限被取消");
    }
    owner
        .send(tungstenite::Message::Text(format!(
            r#"{{"type":"speak_reply","user_id":"{member_id}","approve":true}}"#
        )))
        .unwrap();
    for ws in [&mut owner, &mut member] {
        assert_eq!(read_event(ws)["type"], "speak_grant");
        assert_eq!(read_event(ws)["type"], "message");
    }
    member
        .send(tungstenite::Message::Text(
            r#"{"type":"speak_relinquish"}"#.into(),
        ))
        .unwrap();
    for ws in [&mut owner, &mut member] {
        let revoked = read_event(ws);
        assert_eq!(revoked["type"], "speak_revoked", "{revoked}");
        assert_eq!(revoked["relinquished"], true, "{revoked}");
        let msg = read_event(ws);
        assert_eq!(msg["type"], "message", "{msg}");
        assert_eq!(msg["message"]["body"], "ws-member 放弃了发言权限");
    }

    // Unmute the room so ordinary text/reaction events are allowed.
    owner
        .send(tungstenite::Message::Text(
            r#"{"type":"room_mute_toggle","muted":false}"#.into(),
        ))
        .unwrap();
    for ws in [&mut owner, &mut member] {
        let first = read_event(ws);
        let second = read_event(ws);
        assert_eq!(first["type"], "room_muted", "{first}");
        assert_eq!(first["muted"], false, "{first}");
        assert_eq!(second["type"], "message", "{second}");
        assert_eq!(second["message"]["body"], "房主关闭了全员禁言", "{second}");
    }

    // Reaction snapshot and idempotent toggle; invalid emoji is rejected.
    member
        .send(tungstenite::Message::Text(
            r#"{"type":"send_message","body":"reaction target"}"#.into(),
        ))
        .unwrap();
    let msg_owner = read_event(&mut owner);
    let msg_member = read_event(&mut member);
    assert_eq!(msg_owner["type"], "message", "{msg_owner}");
    assert_eq!(msg_member["type"], "message", "{msg_member}");
    let message_id = msg_member["message"]["id"].as_str().unwrap().to_string();
    member
        .send(tungstenite::Message::Text(format!(
            r#"{{"type":"reaction","message_id":"{message_id}","emoji":"👍"}}"#
        )))
        .unwrap();
    for ws in [&mut owner, &mut member] {
        let reaction = read_event(ws);
        assert_eq!(reaction["type"], "reaction", "{reaction}");
        assert_eq!(reaction["added"], true, "{reaction}");
        assert_eq!(reaction["reactions"][0]["count"], 1, "{reaction}");
    }
    member
        .send(tungstenite::Message::Text(format!(
            r#"{{"type":"reaction","message_id":"{message_id}","emoji":"👍"}}"#
        )))
        .unwrap();
    for ws in [&mut owner, &mut member] {
        let reaction = read_event(ws);
        assert_eq!(reaction["type"], "reaction", "{reaction}");
        assert_eq!(reaction["added"], false, "{reaction}");
        assert_eq!(
            reaction["reactions"].as_array().unwrap().len(),
            0,
            "{reaction}"
        );
    }
    member
        .send(tungstenite::Message::Text(format!(
            r#"{{"type":"reaction","message_id":"{message_id}","emoji":"💥"}}"#
        )))
        .unwrap();
    let err = read_event(&mut member);
    assert_eq!(err["type"], "error", "{err}");
    assert_eq!(err["code"], "validation", "{err}");
    expect_no_event(&mut owner);

    owner.close(None).unwrap();
    member.close(None).unwrap();
}

#[test]
fn forged_xff_from_untrusted_peer_cannot_bypass_login_limit() {
    // Claim a trusted proxy address that does not match the actual loopback
    // peer. The forged header must be ignored, and the per-IP bucket must trip.
    let (state, _dir) = test_state_with(|cfg| {
        cfg.server.trusted_proxy = Some("10.0.0.1".into());
    });
    let (_rt, base) = start_server(&state);
    let c = client();

    let mut statuses = Vec::new();
    for i in 0..12 {
        let r = c
            .post(format!("{base}/api/v1/auth/login"))
            .header("x-forwarded-for", format!("198.51.100.{i}"))
            .json(&json!({"username": format!("probe-{i}"), "password": "wrong"}))
            .send()
            .unwrap();
        statuses.push(r.status().as_u16());
    }
    assert!(
        statuses.contains(&429),
        "forged XFF bypassed the login IP limiter: {statuses:?}"
    );
}

#[test]
fn ws_rejects_oversized_messages_and_event_bursts() {
    type Ws = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>;

    fn set_timeout(ws: &mut Ws) {
        if let tungstenite::stream::MaybeTlsStream::Plain(tcp) = ws.get_mut() {
            tcp.set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
        }
    }

    fn connect(url: &str, cookie: &str) -> Ws {
        let mut req = tungstenite::client::IntoClientRequest::into_client_request(url).unwrap();
        req.headers_mut().insert("cookie", cookie.parse().unwrap());
        let (mut ws, _) = tungstenite::connect(req).expect("ws connect");
        set_timeout(&mut ws);
        ws
    }

    let (state, _dir) = test_state_with(|cfg| {
        cfg.security.ws_max_message_bytes = 1024;
        cfg.security.ws_events_per_second = 2;
    });
    let (_rt, base) = start_server(&state);
    let (admin, _uid) = bootstrap_admin(&state, &base, "test-ws-bounds-bootstrap-0123456789");
    create_room(&admin, &base, "ws-bounds");

    let login = admin
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"username": "alice", "password": "correct-horse-battery"}))
        .send()
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookie = login
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let url = format!(
        "ws://{}/api/v1/rooms/ws-bounds/ws",
        base.trim_start_matches("http://")
    );

    let mut ws = connect(&url, &cookie);
    let hello: Value = serde_json::from_str(&ws.read().unwrap().into_text().unwrap()).unwrap();
    assert_eq!(hello["type"], "hello");
    let _ = ws.send(tungstenite::Message::Text("x".repeat(4096)));
    let oversized_closed = matches!(
        ws.read(),
        Ok(tungstenite::Message::Close(_)) | Err(tungstenite::Error::Protocol(_)) | Err(_)
    );
    assert!(
        oversized_closed,
        "oversized message did not close the socket"
    );

    let mut ws = connect(&url, &cookie);
    let hello: Value = serde_json::from_str(&ws.read().unwrap().into_text().unwrap()).unwrap();
    assert_eq!(hello["type"], "hello");
    for _ in 0..4 {
        let _ = ws.send(tungstenite::Message::Text(r#"{"type":"ping"}"#.into()));
    }
    let mut burst_closed = false;
    for _ in 0..6 {
        match ws.read() {
            Ok(tungstenite::Message::Close(_)) | Err(_) => {
                burst_closed = true;
                break;
            }
            Ok(tungstenite::Message::Text(_)) => {}
            Ok(_) => {}
        }
    }
    assert!(burst_closed, "event burst did not close the socket");
}

#[test]
fn invalid_anonymous_join_does_not_create_guest_rows() {
    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);
    let counts = |state: &AppState| {
        state
            .db
            .with_sync(|conn| -> cloudroom_server::ApiResult<(i64, i64)> {
                let users = conn
                    .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
                    .map_err(cloudroom_server::ApiError::from)?;
                let sessions = conn
                    .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
                    .map_err(cloudroom_server::ApiError::from)?;
                Ok((users, sessions))
            })
            .unwrap()
    };
    let before = counts(&state);
    for _ in 0..3 {
        let r = client()
            .post(format!("{base}/api/v1/rooms/no-such-room/join"))
            .json(&json!({}))
            .send()
            .unwrap();
        assert_eq!(r.status(), 404);
    }
    assert_eq!(counts(&state), before, "failed anonymous join wrote rows");
}

#[test]
fn disabling_an_account_closes_its_open_websocket() {
    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);
    let (admin, _admin_id) = bootstrap_admin(&state, &base, "test-ws-revoke-bootstrap-0123456789");
    create_room(&admin, &base, "ws-revoke");
    let (user, user_id) = register_via_invite(&admin, &base, "ws-revoke-user");
    join_with_invite(
        &user,
        &base,
        "ws-revoke",
        &invite_token(&admin, &base, "ws-revoke"),
    );
    // Obtain a raw login cookie for the WebSocket handshake.
    let login = client()
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"username": "ws-revoke-user", "password": "correct-horse-battery"}))
        .send()
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookie = login
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let mut req = tungstenite::client::IntoClientRequest::into_client_request(format!(
        "ws://{}/api/v1/rooms/ws-revoke/ws",
        base.trim_start_matches("http://")
    ))
    .unwrap();
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    let (mut ws, _) = tungstenite::connect(req).expect("ws connect");
    if let tungstenite::stream::MaybeTlsStream::Plain(tcp) = ws.get_mut() {
        tcp.set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
    }
    let hello: Value = serde_json::from_str(&ws.read().unwrap().into_text().unwrap()).unwrap();
    assert_eq!(hello["type"], "hello");

    let (status, body) = post_json(
        &admin,
        &base,
        &format!("/admin/users/{user_id}/disable"),
        json!({"disabled": true}),
    );
    assert_eq!(status, 200, "{body}");
    let closed = matches!(ws.read(), Ok(tungstenite::Message::Close(_)) | Err(_));
    assert!(closed, "disabled account kept its WebSocket open");
}

#[test]
fn oversized_json_body_is_rejected_before_extraction() {
    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);
    let body = format!(
        r#"{{"username":"alice","password":"{}"}}"#,
        "x".repeat(128 * 1024)
    );
    let result = client()
        .post(format!("{base}/api/v1/auth/login"))
        .header("content-type", "application/json")
        .body(body)
        .send();
    match result {
        Ok(r) => assert_eq!(r.status(), 413),
        // Hyper may reset a connection whose oversized body is rejected
        // before the response is fully written; either outcome proves the
        // request was not extracted and processed.
        Err(e) => assert!(e.is_request(), "{e}"),
    }
}

#[test]
fn grace_purge_handles_room_request_fk_and_continues_sweeping() {
    let (state, _dir) = test_state_with(|cfg| cfg.rooms.expired_cleanup_grace = "0s".into());
    let (_rt, base) = start_server(&state);
    let (admin, _admin_id) = bootstrap_admin(&state, &base, "test-purge-bootstrap-0123456789");
    create_room(&admin, &base, "purge-room");
    let (status, body) = post_json(&admin, &base, "/rooms/purge-room/delete", json!({}));
    assert_eq!(status, 200, "{body}");
    sweeper::sweep_once(&state.db, &state.cfg).unwrap();
    let room_id = state
        .db
        .with_sync(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM rooms WHERE room_code = 'purge-room'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map_err(cloudroom_server::ApiError::from)
        })
        .unwrap();
    assert_eq!(room_id, 0, "purged room row remained");
    let requests = state
        .db
        .with_sync(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM room_requests WHERE created_room_id IS NOT NULL",
                [],
                |r| r.get::<_, i64>(0),
            )
            .map_err(cloudroom_server::ApiError::from)
        })
        .unwrap();
    assert_eq!(requests, 0, "room request still references purged room");
}

#[test]
fn livekit_revokes_kick_delete_and_treats_404_as_success() {
    let fake = FakeTwirp::start(200);
    let (state, _dir) = test_state_with(|cfg| {
        cfg.media.livekit_api_url = fake.base.clone();
        cfg.media.livekit_api_key = "devkey".into();
        cfg.media.livekit_api_secret = "devsecret-devsecret-devsecret-1234".into();
    });
    let (_rt, base) = start_server(&state);
    let (alice, _alice_id) =
        bootstrap_admin(&state, &base, "test-livekit-kick-bootstrap-0123456789");
    let (bob, bob_id) = register_via_invite(&alice, &base, "bob-livekit");
    create_room(&alice, &base, "livekit-kick");
    let token = invite_token(&alice, &base, "livekit-kick");
    join_with_invite(&bob, &base, "livekit-kick", &token);

    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/rooms/livekit-kick/members/{bob_id}/remove"),
        json!({}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        bob.get(format!("{base}/api/v1/rooms/livekit-kick"))
            .send()
            .unwrap()
            .status(),
        403
    );
    let requests = fake.wait_for("RemoveParticipant", 1, Duration::from_secs(3));
    assert_eq!(
        requests.len(),
        1,
        "expected one RemoveParticipant: {requests:?}"
    );
    assert_eq!(requests[0].body["room"], "livekit-kick");
    assert_eq!(requests[0].body["identity"], bob_id);
    assert!(requests[0].authorization.starts_with("Bearer "));
    assert!(!requests[0].authorization.contains("devsecret"));

    let (status, body) = post_json(&alice, &base, "/rooms/livekit-kick/delete", json!({}));
    assert_eq!(status, 200, "{body}");
    let requests = fake.wait_for("DeleteRoom", 1, Duration::from_secs(3));
    assert_eq!(requests.len(), 1, "expected one DeleteRoom: {requests:?}");
    assert_eq!(requests[0].body["room"], "livekit-kick");
    assert_eq!(state.revocation.failed(), 0);

    // A 404 means LiveKit already removed it; this is success, not a failure.
    fake.set_status(404);
    create_room(&alice, &base, "livekit-gone");
    let (status, body) = post_json(&alice, &base, "/rooms/livekit-gone/delete", json!({}));
    assert_eq!(status, 200, "{body}");
    let requests = fake.wait_for("DeleteRoom", 2, Duration::from_secs(3));
    assert_eq!(requests.len(), 2, "expected 404 DeleteRoom: {requests:?}");
    assert_eq!(state.revocation.attempted(), 3);
    assert_eq!(state.revocation.failed(), 0);
}

#[test]
fn livekit_revokes_disable_and_password_reset_for_all_joined_rooms() {
    let fake = FakeTwirp::start(200);
    let (state, _dir) = test_state_with(|cfg| {
        cfg.media.livekit_api_url = fake.base.clone();
        cfg.media.livekit_api_key = "devkey".into();
        cfg.media.livekit_api_secret = "devsecret-devsecret-devsecret-1234".into();
    });
    let (_rt, base) = start_server(&state);
    let (alice, _alice_id) =
        bootstrap_admin(&state, &base, "test-livekit-account-bootstrap-0123456789");
    let (bob, bob_id) = register_via_invite(&alice, &base, "bob-account");
    for code in ["livekit-account-a", "livekit-account-b"] {
        create_room(&alice, &base, code);
        join_with_invite(&bob, &base, code, &invite_token(&alice, &base, code));
    }

    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/admin/users/{bob_id}/disable"),
        json!({"disabled": true}),
    );
    assert_eq!(status, 200, "{body}");
    let requests = fake.wait_for("RemoveParticipant", 2, Duration::from_secs(3));
    assert_eq!(
        requests.len(),
        2,
        "disable must cover both rooms: {requests:?}"
    );
    let mut rooms: Vec<&str> = requests
        .iter()
        .map(|r| r.body["room"].as_str().unwrap())
        .collect();
    rooms.sort_unstable();
    assert_eq!(rooms, ["livekit-account-a", "livekit-account-b"]);
    assert!(requests
        .iter()
        .all(|r| r.body["identity"] == bob_id && !r.authorization.contains("devsecret")));

    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/admin/users/{bob_id}/reset-password"),
        json!({}),
    );
    assert_eq!(status, 200, "{body}");
    let requests = fake.wait_for("RemoveParticipant", 4, Duration::from_secs(3));
    assert_eq!(
        requests.len(),
        4,
        "reset must cover both rooms: {requests:?}"
    );
    assert_eq!(state.revocation.attempted(), 4);
    assert_eq!(state.revocation.failed(), 0);
}

#[test]
fn livekit_revokes_expired_room_through_sweeper_codes() {
    let fake = FakeTwirp::start(200);
    let (state, _dir) = test_state_with(|cfg| {
        cfg.media.livekit_api_url = fake.base.clone();
        cfg.media.livekit_api_key = "devkey".into();
        cfg.media.livekit_api_secret = "devsecret-devsecret-devsecret-1234".into();
    });
    let (rt, base) = start_server(&state);
    let (alice, _alice_id) =
        bootstrap_admin(&state, &base, "test-livekit-expiry-bootstrap-0123456789");
    create_room(&alice, &base, "livekit-expire");
    state
        .db
        .with_sync(|conn| {
            conn.execute(
                "UPDATE rooms SET expires_at = ?1 WHERE room_code = 'livekit-expire'",
                [util::now().to_rfc3339()],
            )
            .map_err(cloudroom_server::ApiError::from)?;
            Ok(())
        })
        .unwrap();
    let (codes, _closed) = sweeper::sweep_once(&state.db, &state.cfg).unwrap();
    assert_eq!(codes, vec!["livekit-expire".to_string()]);
    rt.block_on(async {
        for code in codes {
            revocation::schedule_room(state.cfg.clone(), state.revocation.clone(), code);
        }
    });
    let requests = fake.wait_for("DeleteRoom", 1, Duration::from_secs(3));
    assert_eq!(
        requests.len(),
        1,
        "expected expiry DeleteRoom: {requests:?}"
    );
    assert_eq!(requests[0].body["room"], "livekit-expire");
    assert_eq!(state.revocation.failed(), 0);
}

#[test]
fn livekit_revocation_failure_is_best_effort() {
    let fake = FakeTwirp::start(500);
    let (state, _dir) = test_state_with(|cfg| {
        cfg.media.livekit_api_url = fake.base.clone();
        cfg.media.livekit_api_key = "devkey".into();
        cfg.media.livekit_api_secret = "devsecret-devsecret-devsecret-1234".into();
    });
    let (_rt, base) = start_server(&state);
    let (alice, _alice_id) =
        bootstrap_admin(&state, &base, "test-livekit-failure-bootstrap-0123456789");
    let (bob, bob_id) = register_via_invite(&alice, &base, "bob-failure");
    create_room(&alice, &base, "livekit-failure");
    join_with_invite(
        &bob,
        &base,
        "livekit-failure",
        &invite_token(&alice, &base, "livekit-failure"),
    );

    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/rooms/livekit-failure/members/{bob_id}/remove"),
        json!({}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        bob.get(format!("{base}/api/v1/rooms/livekit-failure"))
            .send()
            .unwrap()
            .status(),
        403
    );
    let requests = fake.wait_for("RemoveParticipant", 3, Duration::from_secs(4));
    assert_eq!(
        requests.len(),
        3,
        "expected three retry attempts: {requests:?}"
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    while state.revocation.failed() == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(state.revocation.attempted(), 1);
    assert_eq!(state.revocation.failed(), 1);
    let stats = alice
        .get(format!("{base}/api/v1/admin/stats"))
        .send()
        .unwrap()
        .json::<Value>()
        .unwrap();
    assert_eq!(stats["livekit_revocations_attempted"], 1);
    assert_eq!(stats["livekit_revocations_failed"], 1);
}

#[test]
fn livekit_revokes_leave_and_admin_force_delete() {
    let fake = FakeTwirp::start(200);
    let (state, _dir) = test_state_with(|cfg| {
        cfg.media.livekit_api_url = fake.base.clone();
        cfg.media.livekit_api_key = "devkey".into();
        cfg.media.livekit_api_secret = "devsecret-devsecret-devsecret-1234".into();
    });
    let (_rt, base) = start_server(&state);
    let (alice, _alice_id) =
        bootstrap_admin(&state, &base, "test-livekit-extra-bootstrap-0123456789");
    let (bob, bob_id) = register_via_invite(&alice, &base, "bob-extra");
    create_room(&alice, &base, "livekit-leave");
    join_with_invite(
        &bob,
        &base,
        "livekit-leave",
        &invite_token(&alice, &base, "livekit-leave"),
    );
    let (status, body) = post_json(&bob, &base, "/rooms/livekit-leave/leave", json!({}));
    assert_eq!(status, 200, "{body}");
    let requests = fake.wait_for("RemoveParticipant", 1, Duration::from_secs(3));
    assert_eq!(requests.len(), 1, "leave must revoke: {requests:?}");
    assert_eq!(requests[0].body["identity"], bob_id);

    create_room(&alice, &base, "livekit-admin-delete");
    let (status, body) = post_json(
        &alice,
        &base,
        "/admin/rooms/livekit-admin-delete/delete",
        json!({}),
    );
    assert_eq!(status, 200, "{body}");
    let requests = fake.wait_for("DeleteRoom", 1, Duration::from_secs(3));
    assert_eq!(requests.len(), 1, "admin delete must revoke: {requests:?}");
    assert_eq!(requests[0].body["room"], "livekit-admin-delete");
    assert_eq!(state.revocation.attempted(), 2);
    assert_eq!(state.revocation.failed(), 0);
}

#[test]
fn livekit_revokes_self_password_change_for_all_joined_rooms() {
    let fake = FakeTwirp::start(200);
    let (state, _dir) = test_state_with(|cfg| {
        cfg.media.livekit_api_url = fake.base.clone();
        cfg.media.livekit_api_key = "devkey".into();
        cfg.media.livekit_api_secret = "devsecret-devsecret-devsecret-1234".into();
    });
    let (_rt, base) = start_server(&state);
    let (alice, _alice_id) = bootstrap_admin(
        &state,
        &base,
        "test-livekit-self-password-bootstrap-0123456789",
    );
    let (bob, bob_id) = register_via_invite(&alice, &base, "bob-self-password");
    for code in ["self-password-a", "self-password-b"] {
        create_room(&alice, &base, code);
        join_with_invite(&bob, &base, code, &invite_token(&alice, &base, code));
    }

    let (status, body) = post_json(
        &bob,
        &base,
        "/me/password",
        json!({
            "current_password": "correct-horse-battery",
            "new_password": "new-correct-horse-battery",
        }),
    );
    assert_eq!(status, 200, "{body}");
    let requests = fake.wait_for("RemoveParticipant", 2, Duration::from_secs(3));
    assert_eq!(
        requests.len(),
        2,
        "self password change must revoke both rooms: {requests:?}"
    );
    assert!(requests
        .iter()
        .all(|r| r.body["identity"] == bob_id && !r.authorization.contains("devsecret")));
    let mut rooms: Vec<&str> = requests
        .iter()
        .map(|r| r.body["room"].as_str().unwrap())
        .collect();
    rooms.sort_unstable();
    assert_eq!(rooms, ["self-password-a", "self-password-b"]);
    assert_eq!(state.revocation.attempted(), 2);
    assert_eq!(state.revocation.failed(), 0);
}

#[test]
fn cors_preflight_allows_configured_frontend_with_credentials() {
    let (state, _dir) = test_state_with(|cfg| {
        cfg.server.frontend_origin = "https://v.qxq.li".into();
    });
    let (_rt, base) = start_server(&state);
    let http = reqwest::blocking::Client::new();

    let preflight = http
        .request(reqwest::Method::OPTIONS, format!("{base}/api/v1/health"))
        .header("Origin", "https://v.qxq.li")
        .header("Access-Control-Request-Method", "POST")
        .header(
            "Access-Control-Request-Headers",
            "content-type,x-csrf-token",
        )
        .send()
        .unwrap();
    assert_eq!(preflight.status(), 200);
    assert_eq!(
        preflight
            .headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("https://v.qxq.li")
    );
    assert_eq!(
        preflight
            .headers()
            .get("access-control-allow-credentials")
            .and_then(|v| v.to_str().ok()),
        Some("true")
    );
    let methods = preflight
        .headers()
        .get("access-control-allow-methods")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(methods.contains("POST"));
    let allowed_headers = preflight
        .headers()
        .get("access-control-allow-headers")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(allowed_headers.contains("x-csrf-token"));

    let actual = http
        .get(format!("{base}/api/v1/health"))
        .header("Origin", "https://v.qxq.li")
        .send()
        .unwrap();
    assert_eq!(actual.status(), 200);
    assert_eq!(
        actual
            .headers()
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok()),
        Some("https://v.qxq.li")
    );

    let denied = http
        .get(format!("{base}/api/v1/health"))
        .header("Origin", "https://evil.example")
        .send()
        .unwrap();
    assert_eq!(denied.status(), 200);
    assert!(denied
        .headers()
        .get("access-control-allow-origin")
        .is_none());
}

#[test]
fn cross_site_cookie_is_none_and_secure() {
    let (state, _dir) = test_state_with(|cfg| {
        cfg.server.insecure_cookies = false;
        cfg.server.cross_site_cookies = true;
    });
    let cookie = state.auth.session_cookie("raw-token");
    assert!(cookie.contains("SameSite=None"), "{cookie}");
    assert!(cookie.contains("Secure"), "{cookie}");
    let clear = state.auth.clear_cookie();
    assert!(clear.contains("SameSite=None"), "{clear}");
    assert!(clear.contains("Secure"), "{clear}");
}

#[test]
fn message_edit_delete_search_contract() {
    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);
    let (alice, _) = bootstrap_admin(&state, &base, "test-msg-ops-bootstrap-0123456789");
    create_room(&alice, &base, "msg-ops");

    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/msg-ops/messages",
        json!({"body": "hello-world"}),
    );
    assert_eq!(status, 200, "{body}");
    let mid = body["message"]["id"].as_str().unwrap().to_string();

    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/rooms/msg-ops/messages/{mid}/edit"),
        json!({"body": "edited-body"}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["message"]["body"], "edited-body", "{body}");
    assert!(body["message"]["edited_at"].is_string(), "{body}");

    let res = alice
        .get(format!(
            "{base}/api/v1/rooms/msg-ops/messages/search?q=edited"
        ))
        .send()
        .unwrap();
    let st = res.status();
    let txt = res.text().unwrap();
    assert_eq!(st, 200, "{txt}");
    let body: Value = serde_json::from_str(&txt).unwrap();
    assert_eq!(body["messages"][0]["body"], "edited-body", "{body}");

    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/rooms/msg-ops/messages/{mid}/delete"),
        json!({}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["message"]["deleted"], true, "{body}");

    // 已删除消息不再出现在搜索结果中。
    let res = alice
        .get(format!(
            "{base}/api/v1/rooms/msg-ops/messages/search?q=edited"
        ))
        .send()
        .unwrap();
    let body: Value = res.json().unwrap();
    assert_eq!(body["messages"].as_array().unwrap().len(), 0, "{body}");
}

#[test]
fn webhook_ingest_and_revoke() {
    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);
    let (alice, _) = bootstrap_admin(&state, &base, "test-webhook-bootstrap-0123456789");
    create_room(&alice, &base, "hook-room");

    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/hook-room/webhooks",
        json!({"name": "ci"}),
    );
    assert_eq!(status, 200, "{body}");
    let token = body["token"].as_str().unwrap().to_string();
    let hook_id = body["webhook"]["id"].as_str().unwrap().to_string();

    // Public ingest does not use a session cookie.
    let res = client()
        .post(format!("{base}/api/v1/webhooks/{token}"))
        .json(&json!({"body": "webhook hello"}))
        .send()
        .unwrap();
    assert_eq!(res.status(), 200, "{}", res.text().unwrap_or_default());
    let r = alice
        .get(format!("{base}/api/v1/rooms/hook-room/messages?after=0"))
        .send()
        .unwrap();
    let v: Value = r.json().unwrap();
    assert!(
        v["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["body"] == "webhook hello"),
        "{v}"
    );

    let r = alice
        .get(format!("{base}/api/v1/rooms/hook-room/webhooks"))
        .send()
        .unwrap();
    let v: Value = r.json().unwrap();
    assert_eq!(v["webhooks"].as_array().unwrap().len(), 1, "{v}");
    assert_eq!(v["webhooks"][0]["id"], hook_id);

    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/rooms/hook-room/webhooks/{hook_id}/revoke"),
        json!({}),
    );
    assert_eq!(status, 200, "{body}");
    let res = client()
        .post(format!("{base}/api/v1/webhooks/{token}"))
        .json(&json!({"body": "after revoke"}))
        .send()
        .unwrap();
    assert_eq!(res.status(), 404);
}

#[test]
fn attachment_gate_and_e2ee_key_contract() {
    use reqwest::blocking::multipart::{Form, Part};

    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);
    let (alice, _) = bootstrap_admin(&state, &base, "test-attach-e2ee-bootstrap-0123456789");

    // 1) 未开启附件的房间：上传 403。
    create_room(&alice, &base, "att-off");
    let img = image::RgbaImage::from_pixel(64, 64, image::Rgba([200, 10, 10, 255]));
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let form = Form::new().part(
        "file",
        Part::bytes(png.clone())
            .file_name("x.png")
            .mime_str("image/png")
            .unwrap(),
    );
    let mut req = alice.post(format!("{base}/api/v1/rooms/att-off/attachments"));
    if let Some(token) = csrf_opt(&alice, &base) {
        req = req.header("x-csrf-token", token);
    }
    let res = req.multipart(form).send().unwrap();
    assert_eq!(res.status(), 403, "{}", res.text().unwrap_or_default());

    // 2) 开启附件：上传 200，下载 200，响应含 url。
    let (status, body) = post_json(
        &alice,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "note": "attach", "media_mode": "sfu",
               "desired_code": "att-on", "attachments": true}),
    );
    assert_eq!(status, 200, "{body}");
    let form = Form::new().part(
        "file",
        Part::bytes(png)
            .file_name("x.png")
            .mime_str("image/png")
            .unwrap(),
    );
    let mut req = alice.post(format!("{base}/api/v1/rooms/att-on/attachments"));
    if let Some(token) = csrf_opt(&alice, &base) {
        req = req.header("x-csrf-token", token);
    }
    let res = req.multipart(form).send().unwrap();
    let status = res.status();
    let body: Value = res.json().unwrap();
    assert_eq!(status, 200, "{body}");
    let url = body["attachment"]["url"].as_str().unwrap().to_string();
    let res = alice.get(format!("{base}{url}")).send().unwrap();
    assert_eq!(res.status(), 200);
    assert!(res
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .starts_with("image/png"));
    let thumb = body["attachment"]["thumbnail_url"]
        .as_str()
        .expect("image attachment must expose thumbnail_url");
    let res = alice.get(format!("{base}{thumb}")).send().unwrap();
    assert_eq!(res.status(), 200);
    assert!(res
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .starts_with("image/jpeg"));
    assert!(!res.bytes().unwrap().is_empty());

    // 3) audio upload is sniffed and served inline.
    let mp3 = b"ID3\x04\x00\x00\x00\x00\x00\x00".to_vec();
    let form = Form::new().part(
        "file",
        Part::bytes(mp3)
            .file_name("a.mp3")
            .mime_str("audio/mpeg")
            .unwrap(),
    );
    let mut req = alice.post(format!("{base}/api/v1/rooms/att-on/attachments"));
    if let Some(token) = csrf_opt(&alice, &base) {
        req = req.header("x-csrf-token", token);
    }
    let res = req.multipart(form).send().unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().unwrap();
    assert_eq!(body["attachment"]["mime"], "audio/mpeg", "{body}");
    let url = body["attachment"]["url"].as_str().unwrap();
    let res = alice.get(format!("{base}{url}")).send().unwrap();
    assert_eq!(res.status(), 200);
    assert!(res
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("audio/mpeg"));
    let res = alice.get(format!("{base}{url}")).header("range", "bytes=0-2").send().unwrap();
    assert_eq!(res.status(), 206);
    assert_eq!(res.headers().get("content-range").unwrap().to_str().unwrap(), "bytes 0-2/10");
    assert_eq!(res.bytes().unwrap().as_ref(), b"ID3");
    let res = alice.get(format!("{base}{url}")).header("range", "bytes=99-100").send().unwrap();
    assert_eq!(res.status(), 416);

    // Larger than the global 4 MiB JSON limit; the upload route must allow it.
    let mut big = vec![0u8; 5 * 1024 * 1024];
    big[0..3].copy_from_slice(b"ID3");
    let form = Form::new().part(
        "file",
        Part::bytes(big)
            .file_name("big.mp3")
            .mime_str("audio/mpeg")
            .unwrap(),
    );
    let mut req = alice.post(format!("{base}/api/v1/rooms/att-on/attachments"));
    if let Some(token) = csrf_opt(&alice, &base) {
        req = req.header("x-csrf-token", token);
    }
    let res = req.multipart(form).send().unwrap();
    assert_eq!(res.status(), 200, "{}", res.text().unwrap_or_default());

    // 4) SFU 房间开启 E2EE：settings 200，media-token 下发密钥。
    let (status, body) = post_json(
        &alice,
        &base,
        "/rooms/att-on/settings",
        json!({"e2ee": true}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(&alice, &base, "/rooms/att-on/media-token", json!({}));
    assert_eq!(status, 200, "{body}");
    assert!(
        body["e2ee_key"]
            .as_str()
            .map(|k| !k.is_empty())
            .unwrap_or(false),
        "{body}"
    );
}

#[test]
fn session_sliding_renewal_refreshes_expiry_and_cookie() {
    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);
    let raw = "test-bootstrap-token-sliding-renewal-0123456789";
    let (_admin, _uid) = bootstrap_admin(&state, &base, raw);

    // 重新登录拿原始会话 cookie（新客户端，避免复用已有 jar）。
    let login = client()
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"username": "alice", "password": "correct-horse-battery"}))
        .send()
        .unwrap();
    assert_eq!(login.status(), 200);
    let session_cookie = login
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.starts_with("cloudroom_session="))
        .expect("session cookie")
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let raw_token = session_cookie.split_once('=').unwrap().1.to_string();

    // 无 cookie 存储的客户端：手动带 cookie，才能看到响应里的 Set-Cookie。
    let raw_client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let get_with = |cookie: String| {
        raw_client
            .get(format!("{base}/api/v1/me"))
            .header("cookie", cookie)
            .send()
            .unwrap()
    };

    // 1) seen 缺失/过期 -> 续期：返回新的会话 cookie 与 seen cookie。
    let r = get_with(format!("{session_cookie}; cloudroom_seen=0"));
    assert_eq!(r.status(), 200);
    let cookies: Vec<String> = r
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.to_string())
        .collect();
    assert!(
        cookies.iter().any(|c| c.starts_with("cloudroom_session=")),
        "{cookies:?}"
    );
    assert!(
        cookies.iter().any(|c| c.starts_with("cloudroom_seen=")),
        "{cookies:?}"
    );

    // 2) 服务端过期时间被延长到 now + ttl 附近。
    let hash = util::sha256_hex(raw_token.as_bytes());
    let expires: String = state
        .db
        .with_sync(|conn| -> cloudroom_server::ApiResult<String> {
            let v = conn
                .query_row(
                    "SELECT expires_at FROM sessions WHERE token_hash = ?1",
                    [&hash],
                    |r| r.get::<_, String>(0),
                )
                .map_err(cloudroom_server::ApiError::from)?;
            Ok(v)
        })
        .unwrap();
    let expires = chrono::DateTime::parse_from_rfc3339(&expires).unwrap();
    let ttl = state.cfg.session_ttl();
    assert!(
        expires
            > util::now() + chrono::Duration::from_std(ttl).unwrap() - chrono::Duration::hours(1),
        "expires_at not extended: {expires}"
    );

    // 3) seen 新鲜 -> 不再续期（响应无 Set-Cookie）。
    let now_ts = util::now().timestamp();
    let r = get_with(format!("{session_cookie}; cloudroom_seen={now_ts}"));
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers().get_all("set-cookie").iter().count(), 0);
}

#[test]
fn guest_upgrade_approval_room_switch_and_demotion() {
    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);
    let raw = "test-guest-upgrade-bootstrap-0123456789";
    let (alice, _alice_id) = bootstrap_admin(&state, &base, raw);

    // bob: registered user -> promoted to admin (room owner must be admin for the switch).
    let (bob, bob_id) = register_via_invite(&alice, &base, "bob");
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/admin/users/{bob_id}/role"),
        json!({"role": "admin"}),
    );
    assert_eq!(status, 200, "{body}");

    // bob creates a room (pending -> alice approves).
    let (status, body) = post_json(
        &bob,
        &base,
        "/room-requests",
        json!({"kind": "create_room", "note": "guest upgrade room", "media_mode": "p2p"}),
    );
    assert_eq!(status, 200, "{body}");
    let room_req = body["request"]["request_id"].as_str().unwrap().to_string();
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/admin/room-requests/{room_req}/approve"),
        json!({}),
    );
    assert_eq!(status, 200, "{body}");
    let code = body["room"]["code"].as_str().unwrap().to_string();

    // Guest joins via room invite.
    let token = invite_token(&bob, &base, &code);
    let guest = client();
    let (status, body) = post_json(
        &guest,
        &base,
        &format!("/rooms/{code}/join"),
        json!({"invite_token": token, "nickname": "访客甲"}),
    );
    assert_eq!(status, 200, "{body}");
    let guest_id = body["member"]["user_id"].as_str().unwrap().to_string();

    // Room invite cannot be used for upgrade (default D).
    let fresh_room_invite = invite_token(&bob, &base, &code);
    let (status, body) = post_json(
        &guest,
        &base,
        "/auth/upgrade-guest",
        json!({"username": "carol", "password": "correct-horse-battery", "invite_token": fresh_room_invite}),
    );
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invite_invalid", "{body}");

    // bob requests, alice approves, guest upgrades without a token.
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/rooms/{code}/guest-upgrade-requests"),
        json!({"user_id": guest_id}),
    );
    assert_eq!(status, 200, "{body}");
    let req_id = body["request"]["request_id"].as_str().unwrap().to_string();
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/admin/guest-upgrade-requests/{req_id}/approve"),
        json!({}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &guest,
        &base,
        "/auth/upgrade-guest",
        json!({"username": "carol", "password": "correct-horse-battery"}),
    );
    assert_eq!(status, 200, "{body}");

    // Inviter is the requesting owner (bob), visible only in the admin list.
    let r = alice
        .get(format!("{base}/api/v1/admin/users?search=carol"))
        .send()
        .unwrap();
    let v: Value = r.json().unwrap();
    assert_eq!(v["users"][0]["invited_by"], "bob", "{v}");

    // bob (admin owner) enables the room switch; a second guest upgrades directly.
    let (status, body) = post_json(
        &bob,
        &base,
        &format!("/rooms/{code}/settings"),
        json!({"guest_upgrade_open": true}),
    );
    assert_eq!(status, 200, "{body}");
    let token2 = invite_token(&bob, &base, &code);
    let guest2 = client();
    let (status, body) = post_json(
        &guest2,
        &base,
        &format!("/rooms/{code}/join"),
        json!({"invite_token": token2, "nickname": "访客乙"}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &guest2,
        &base,
        "/auth/upgrade-guest",
        json!({"username": "dave", "password": "correct-horse-battery"}),
    );
    assert_eq!(status, 200, "{body}");

    // Demotion resets the switch for all rooms owned by bob.
    let (status, body) = post_json(
        &alice,
        &base,
        &format!("/admin/users/{bob_id}/role"),
        json!({"role": "user"}),
    );
    assert_eq!(status, 200, "{body}");
    let token3 = invite_token(&bob, &base, &code);
    let guest3 = client();
    let (status, body) = post_json(
        &guest3,
        &base,
        &format!("/rooms/{code}/join"),
        json!({"invite_token": token3, "nickname": "访客丙"}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &guest3,
        &base,
        "/auth/upgrade-guest",
        json!({"username": "erin", "password": "correct-horse-battery"}),
    );
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"]["code"], "invite_required", "{body}");
}

#[test]
fn oidc_login_redirect_callback_and_auto_create() {
    use axum::extract::Form;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    const PRIVATE_KEY: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvwIBADANBgkqhkiG9w0BAQEFAASCBKkwggSlAgEAAoIBAQDyosTHER4QoVg8
y5qJfTAIOPjoTIXaQ57dFFlNqBbh60OAJw3RDUUt3C9Y4lJsTer24JD/a8KbeRV4
CqK1iWH9DindY5Mp1rgspKU4BJsyHqpfIwwhAZAkONokLSyEd4M/3/QSKLB8JRCZ
fqpPcJZvjTIBP0gj8nrm/xP9E0aN3BNbAHOjHbNhq2yRlkyxLpyQUMtaTZTpUcxD
x/HUisi+fQYkk1GlL5jkm/v6BhFoZOwGGYsrmDjre+koG8OHyE7LKf+WcjL74H3T
jQk4Np2GPCP2T9eDzN7K1VSkToebeKsLDMrIDcFychc2/Jd2MyIBgje5AjS7NhaZ
gOHYMJNHAgMBAAECggEAc/V4Z871eGszuXJw+855myRnSNXxjpuMnAnymBuCPBxs
2R1RDBz0d/NoKWmYjKw8ZBp1Ea0Vh6o5qXIjyvDKhN81ANpq5a75vFQ/peMFrNFd
CTS9wjR2B+mnz2xLf0+CyY8lnVwW5G3uXf1cNbUQIGXxhQkSWCO1uxojGmsQ6BBc
YeL7yHrfr1hOSOYSZCHYGn9FlBVyybXZ7Aqqdpdbafyn0LBDJy5Yzi4qIKZVCToM
G7dV3g8U44yLLQ6UqQGrLAPbcERUf35gpsMFXn+ZTm1nfcPWf5nlzQC0Qmwhuh8m
QVkI6jNz8hh38r8v3RiwEsesxQQVm2BACniHE6awiQKBgQD/J5/p+ARJAivQEv17
Koyw3f2KANMHa+7omx+HvmbUe8R4NC3XA5ouPIGcEc2syQiZcwILCghh2Sk9A0/2
4zE7a0Il7HMuJJS1QTKcd4ccj5GkKGZeGexW6JaHjTsXDafGScERYfUVNh/VVl7u
pTxjsT7a10x4kcvFc7uLOEOHVQKBgQDzcIcYM3WcRzk1WjKfqpPLih32wYKKB/hA
6Q0wTFkdNOpZPfxzpb9+Wa3gxVtad27ImdCdoks3aBl8NbeSjLA3N7unHljKJA/Y
tow/vMM4PHP/3UA3e6zx6yJGEnzJKPklBt+zt2Lk/uDVk+QJLb5+EyDAhLB1Qv26
xFKUNcV4KwKBgQC8Z33lPYWsgzXmxp71O033q3NzW3h3mnMmvqRhj25Op7VDLQRM
KBTI31S08VB1rVv0Mr/ndtkJp1iT/MYm/0tzHJgTeKzz/X7w99tvPbM+m6RnGo28
HlgtNEEyhk6Dj0m7WOp+814M85I97TDb9pbI5jALV77XqJBWlEZ/XmQ+oQKBgQC2
ofJIHrsaXni15RBbpu4wYlMuf2i+w1Hk6LKjTHQCrGMwRoex/2RVl1QScB2kFjv/
tR3EVTCWxbtw6x/oizHciyx2KRc1t65HyFQZsTTIaNoh18xIk7lg2SWialZB2EJq
G+kR+//uiqUEtmn61N232gPVSvbTUknmp0jItjqKdwKBgQDz4znuSm9gKK8oYRe3
w1T7wW4mFDX93kz6numhxrxAd5/0SW6RxZXHU5vaBi78H7qV9AqTzdka5++pmAxF
a/SdiNkF3biBITySgLgDFt2K4XJ61VqL1rG1HjCwErnnbmYO8JZ+jK6J/iOIZCb8
WcGgt66Vm9Bn2YMV6AJn++afsA==
-----END PRIVATE KEY-----"#;
    const JWK_N: &str = "8qLExxEeEKFYPMuaiX0wCDj46EyF2kOe3RRZTagW4etDgCcN0Q1FLdwvWOJSbE3q9uCQ_2vCm3kVeAqitYlh_Q4p3WOTKda4LKSlOASbMh6qXyMMIQGQJDjaJC0shHeDP9_0EiiwfCUQmX6qT3CWb40yAT9II_J65v8T_RNGjdwTWwBzox2zYatskZZMsS6ckFDLWk2U6VHMQ8fx1IrIvn0GJJNRpS-Y5Jv7-gYRaGTsBhmLK5g463vpKBvDh8hOyyn_lnIy--B9040JODadhjwj9k_Xg8zeytVUpE6Hm3irCwzKyA3BcnIXNvyXdjMiAYI3uQI0uzYWmYDh2DCTRw";

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let issuer = format!("http://{addr}");
    let nonce_slot = Arc::new(Mutex::new(String::new()));
    let nonce_for_token = nonce_slot.clone();
    let issuer_for_discovery = issuer.clone();
    let issuer_for_token = issuer.clone();
    let app = Router::new()
        .route(
            "/.well-known/openid-configuration",
            get(move || {
                let issuer = issuer_for_discovery.clone();
                async move {
                    Json(json!({
                        "issuer": issuer,
                        "authorization_endpoint": format!("{issuer}/authorize"),
                        "token_endpoint": format!("{issuer}/token"),
                        "jwks_uri": format!("{issuer}/jwks"),
                    }))
                }
            }),
        )
        .route(
            "/jwks",
            get(|| async {
                Json(json!({"keys": [{
                    "kty": "RSA", "kid": "test", "use": "sig", "alg": "RS256",
                    "n": JWK_N, "e": "AQAB"
                }]}))
            }),
        )
        .route(
            "/token",
            post(move |Form(_form): Form<HashMap<String, String>>| {
                let nonce = nonce_for_token.lock().unwrap().clone();
                let issuer = issuer_for_token.clone();
                async move {
                    let claims = json!({
                        "iss": issuer,
                        "aud": "cloudroom-test",
                        "sub": "oidc-user-1",
                        "email": "sso@example.com",
                        "email_verified": true,
                        "preferred_username": "sso-user",
                        "nonce": nonce,
                        "exp": chrono::Utc::now().timestamp() + 300
                    });
                    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
                    header.kid = Some("test".into());
                    let token = jsonwebtoken::encode(
                        &header,
                        &claims,
                        &jsonwebtoken::EncodingKey::from_rsa_pem(PRIVATE_KEY.as_bytes()).unwrap(),
                    )
                    .unwrap();
                    Json(json!({"id_token": token}))
                }
            }),
        )
        .route("/authorize", get(|| async { "authorize" }));
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let l = tokio::net::TcpListener::from_std(listener).unwrap();
            axum::serve(l, app).await.unwrap();
        });
    });

    let issuer_for_cfg = issuer.clone();
    let (state, _dir) = test_state_with(|cfg| {
        cfg.auth.oidc.enabled = true;
        cfg.auth.oidc.issuer = issuer_for_cfg;
        cfg.auth.oidc.client_id = "cloudroom-test".into();
        cfg.auth.oidc.client_secret = "secret".into();
        cfg.auth.oidc.redirect_url = "http://127.0.0.1/callback".into();
        cfg.auth.oidc.auto_create = true;
        cfg.auth.oidc.require_verified_email = true;
    });
    let (_rt, base) = start_server(&state);
    let c = client();
    let r = c
        .get(format!("{base}/api/v1/auth/oidc/login?next=/"))
        .send()
        .unwrap();
    assert_eq!(r.status(), 303);
    let location = r.headers().get("location").unwrap().to_str().unwrap();
    let url = url::Url::parse(location).unwrap();
    let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
    let state_param = q.get("state").unwrap().clone();
    let nonce = q.get("nonce").unwrap().clone();
    assert_eq!(q.get("code_challenge_method").map(String::as_str), Some("S256"));
    *nonce_slot.lock().unwrap() = nonce;

    let r = c
        .get(format!(
            "{base}/api/v1/auth/oidc/callback?code=test-code&state={state_param}"
        ))
        .send()
        .unwrap();
    assert_eq!(r.status(), 303);
    assert_eq!(
        r.headers().get("location").unwrap(),
        "http://127.0.0.1/"
    );
    assert!(r
        .headers()
        .get_all("set-cookie")
        .iter()
        .any(|v| v.to_str().unwrap_or("").contains("cloudroom_session")));
    let me = c.get(format!("{base}/api/v1/me")).send().unwrap();
    assert_eq!(me.status(), 200);
    let v: Value = me.json().unwrap();
    assert_eq!(v["user"]["username"], "sso-user", "{v}");
}

#[test]
fn totp_setup_login_and_disable() {
    use cloudroom_server::infra::totp;
    let (state, _dir) = test_state();
    let (_rt, base) = start_server(&state);
    let (alice, _) = bootstrap_admin(&state, &base, "test-totp-bootstrap-0123456789");

    let (status, body) = post_json(&alice, &base, "/me/totp/setup", json!({}));
    assert_eq!(status, 200, "{body}");
    let secret = body["secret"].as_str().unwrap().to_string();
    assert!(body["otpauth_url"]
        .as_str()
        .unwrap()
        .starts_with("otpauth://totp/"));
    let code = totp::code_now(&secret).unwrap();
    let (status, body) = post_json(
        &alice,
        &base,
        "/me/totp/enable",
        json!({"code": code}),
    );
    assert_eq!(status, 200, "{body}");
    let recovery = body["recovery_codes"].as_array().unwrap().clone();
    assert_eq!(recovery.len(), 10, "{body}");

    let fresh = client();
    let (status, body) = post_json(
        &fresh,
        &base,
        "/auth/login",
        json!({"username": "alice", "password": "correct-horse-battery"}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["totp_required"], true, "{body}");
    let challenge = body["challenge_token"].as_str().unwrap().to_string();
    let code = totp::code_now(&secret).unwrap();
    let (status, body) = post_json(
        &fresh,
        &base,
        "/auth/login/totp",
        json!({"challenge_token": challenge, "code": code}),
    );
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["user"]["username"], "alice", "{body}");
    let r = fresh.get(format!("{base}/api/v1/me")).send().unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<Value>().unwrap()["totp_enabled"], true);

    // A wrong second factor consumes the challenge and is rejected.
    let (status, body) = post_json(
        &fresh,
        &base,
        "/auth/login",
        json!({"username": "alice", "password": "correct-horse-battery"}),
    );
    assert_eq!(status, 200, "{body}");
    let challenge = body["challenge_token"].as_str().unwrap().to_string();
    let wrong = if totp::code_now(&secret).unwrap() == "000000" {
        "111111"
    } else {
        "000000"
    };
    let (status, _) = post_json(
        &fresh,
        &base,
        "/auth/login/totp",
        json!({"challenge_token": challenge, "code": wrong}),
    );
    assert_eq!(status, 401);

    // Disable requires password + a valid code.
    let code = totp::code_now(&secret).unwrap();
    let (status, body) = post_json(
        &fresh,
        &base,
        "/me/totp/disable",
        json!({"password": "correct-horse-battery", "code": code}),
    );
    assert_eq!(status, 200, "{body}");
    let (status, body) = post_json(
        &client(),
        &base,
        "/auth/login",
        json!({"username": "alice", "password": "correct-horse-battery"}),
    );
    assert_eq!(status, 200, "{body}");
    assert!(body.get("totp_required").is_none(), "{body}");
}
