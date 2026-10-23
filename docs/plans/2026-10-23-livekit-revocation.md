# LiveKit server-side media revocation

## Context

CloudRoom currently issues LiveKit SFU JWTs with only a TTL (default 10m, production cap 15m). Kicking, disabling, resetting a password, deleting a room, or leaving only mutates SQLite / closes the control WebSocket; the LiveKit participant and previously issued token remain usable until expiry. Add a server-to-server LiveKit RoomService client and schedule best-effort revocation after the authoritative DB change. Keep the existing TTL as a fallback; do not block the business response on LiveKit availability.

**Decision:** implement **B: direct Twirp HTTP** instead of an official/third-party Rust SDK (A). The bundled LiveKit binary contains `livekit.RoomService` / `RemoveParticipant` / `DeleteRoom` and Twirp handlers, and the backend already hand-signs LiveKit HS256 tokens. B reuses the existing `reqwest` dependency family, avoids adding an unverified SDK plus protobuf/tonic generated clients, and keeps the upgrade surface small. A would require a new dependency audit and static-musl verification; do not introduce it in this task.

**Execution setup:** create branch `fix/livekit-revocation` from `main`; first copy this plan to `docs/plans/2026-10-23-livekit-revocation.md`. Do not push or merge.

## Approach

1. **Add the internal LiveKit API address.**
   - `backend/src/config.rs`, `MediaCfg`: add `pub livekit_api_url: String` after `livekit_url`.
   - Default it to `http://127.0.0.1:7880`.
   - In `Config::validate`, require `livekit_api_url` to start with `http://` or `https://`; do not require public TLS because the API is intended to be internal/loopback.
   - Add the key to `backend/src/config.rs` `DEFAULT_CONFIG`, `release/config.toml.example` (`http://127.0.0.1:7880`), and Compose `cloudroom/config.template.toml` (`http://livekit:7880`). Add `livekit_api_url = "http://127.0.0.1:$LIVEKIT_PORT"` to `test-server/run-tests.sh`, `test-server/start-test-env.sh`, and `scripts/smoke.sh`.
   - `docs/configuration.md`: document it as the internal Twirp base URL, never exposed through the public reverse proxy.
   - Do not add `/twirp` to Nginx. In `release/nginx/cloudroom.conf.example` and Compose `nginx/cloudroom.conf`, add `location /twirp { return 404; }` so an accidental public route fails closed. In `frontend-server/src/main.rs::handle`, return 404 for `req.uri().path().starts_with("/twirp")` before static/SPA handling.

2. **Promote the HTTP client dependency.**
   - `backend/Cargo.toml`: add `reqwest = { version = "0.13.4", default-features = false, features = ["rustls", "json"] }` to `[dependencies]`.
   - Keep the existing `[dev-dependencies] reqwest` entry with its test-only `blocking`, `cookies`, `multipart` features; Cargo feature unification is test-only.
   - Run `cargo check`/`cargo build --locked` and `scripts/build-musl.sh all`; if the static musl link fails, stop and report the exact linker error. Do not silently replace this with a TTL-only fallback.

3. **Add the LiveKit admin JWT and Twirp calls.**
   - `backend/src/infra/livekit.rs`:
     - Add `fn livekit_api_base(cfg: &Config) -> &str` (trim trailing `/`).
     - Add `fn livekit_admin_token(cfg: &Config, room_name: &str) -> ApiResult<String>` with header `{"alg":"HS256","typ":"JWT"}` and payload:
       `iss`/`sub` = `cfg.media.livekit_api_key`, `nbf = now - 10`, `exp = now + 60`, `video = {"room": room_name, "roomAdmin": true, "roomCreate": true}`; sign with the existing `sign()`. `roomCreate` is required by LiveKit `DeleteRoom`; verified 2026-10-23 against the bundled server (same token without it returned 401).
     - Add `pub enum RevocationError { Transient(String), Permanent(String) }` and `pub async fn remove_participant(cfg: &Config, room_name: &str, identity: &str) -> Result<(), RevocationError>`:
       `POST {base}/twirp/livekit.RoomService/RemoveParticipant`, headers `Authorization: Bearer {admin_jwt}`, `Content-Type: application/json`, body `{"room": room_name, "identity": identity}`, client timeout 2s.
       Return `Ok(())` for HTTP 2xx and 404 (already gone). Map network/timeout/5xx to `Transient`; map other 4xx to `Permanent`; error text contains only the HTTP status/class, never the token/secret/body.
     - Add `pub async fn delete_room(cfg: &Config, room_name: &str) -> Result<(), RevocationError>` with the same pattern and body `{"room": room_name}` to `/twirp/livekit.RoomService/DeleteRoom`; 404 is success.
     - Never log the JWT, secret, request headers, or full response bodies.
   - Add unit test `livekit_admin_token_has_room_admin_claim`: decode the payload and assert `video.room`, `video.roomAdmin=true`, `video.roomCreate=true`, `exp > nbf`; assert no secret substring appears in the token.
   - Do not change the existing `livekit_token` user grant or `livekit_turn_credentials`.

4. **Add metrics and a best-effort scheduler.**
   - New `backend/src/app/revocation.rs`:
     - `#[derive(Default)] pub struct LiveKitRevocationMetrics { attempts: AtomicU64, failures: AtomicU64 }` with `record_attempt`, `record_failure`, `attempted`, `failed`.
     - `pub fn schedule_participant(cfg: Arc<Config>, metrics: Arc<LiveKitRevocationMetrics>, room_code: String, user_id: String)`.
     - `pub fn schedule_room(cfg: Arc<Config>, metrics: Arc<LiveKitRevocationMetrics>, room_code: String)`.
     - Each schedule increments `attempts`, spawns a Tokio task, calls the LiveKit function, and retries **only `RevocationError::Transient`**, 3 attempts total with 100ms then 300ms sleeps. A `Permanent` error is logged once and increments `failures` immediately. On exhausted transient failure increment `failures` and emit `tracing::warn!(room_code, user_id, attempts=3, "livekit revocation failed")` (no token/secret). On 404 success no failure.
     - `pub async fn schedule_user(state: AppState, user_id: String)` queries `repo::list_member_rooms` through `state.db.with(...).await`; on a DB error log a warning and return without blocking the caller. On success schedule a participant removal for every returned room code. This is only for account-wide events.
   - `backend/src/app/state.rs`: add `pub revocation: Arc<LiveKitRevocationMetrics>` to `AppState`; import the type. Construct `Arc::new(LiveKitRevocationMetrics::default())` in `main.rs` and the e2e `test_state_with`.
   - `backend/src/app/mod.rs`: add `pub mod revocation;` (do not add it to `backend/src/api/mod.rs`). API handlers call it as `crate::app::revocation`.
   - `backend/src/app/sweeper.rs`: change `run(db: Db, cfg: Config)` to `run(db: Db, cfg: Config, revocation: Arc<LiveKitRevocationMetrics>)`; for each code returned by `sweep_once`, call `crate::app::revocation::schedule_room(Arc::new(cfg.clone()), revocation.clone(), code)`.
   - `backend/src/main.rs`: pass `state.revocation.clone()` to `sweeper::run`; the `AppState` constructor gets the new field before the spawn.
   - `backend/src/api/admin.rs::stats`: read `state.revocation.attempted()`/`failed()` and add exact JSON fields `livekit_revocations_attempted` and `livekit_revocations_failed`; no other stats field changes.

5. **Schedule revocation at each authoritative mutation point.**
   - `backend/src/api/rooms.rs::remove_member`: after `remove_member_workflow`, `state.hub.unregister_all`, and before returning, call `revocation::schedule_participant(state.cfg.clone(), state.revocation.clone(), room.room_code.clone(), user_id.clone())`. The HTTP response remains 200 regardless of LiveKit.
   - `backend/src/api/rooms.rs::leave_room`: after `leave_member` and `unregister_all`, schedule the leaving `uid` for `room.room_code`.
   - `backend/src/api/rooms.rs::delete_room`: after `terminate_room`, schedule `delete_room` for `room.room_code`.
   - `backend/src/api/admin.rs::force_delete_room`: after `terminate_room`, schedule `delete_room` for `room.room_code`.
   - `backend/src/api/admin.rs::set_user_disabled`: only when `req.disabled == true`, after `app::admin::set_user_disabled` and `state.hub.unregister_user`, `await revocation::schedule_user(state.clone(), user_id.clone())`.
   - `backend/src/api/admin.rs::reset_password`: after `apply_password_reset` and `unregister_user`, `await revocation::schedule_user(state.clone(), user_id.clone())`.
   - `backend/src/app/sweeper.rs`: change `sweep_once` to return `ApiResult<Vec<String>>` containing room codes transitioned to expired or deleted by this sweep (including direct-idle). Existing e2e calls that discard the value remain source-compatible. In `run(db, cfg, revocation_metrics)`, after a successful sweep, call `schedule_room` for each returned code; update `main.rs` call with `state.revocation.clone()`.
   - Do not call revocation from inside a `db.with_sync` closure or while holding the DB mutex.

6. **Failure policy (fixed).**
   - DB mutation commits first; revocation is scheduled after and never changes the HTTP/WS business result. A LiveKit 500/timeout/401 still leaves the room/member/session state revoked locally and returns success.
   - 404 is success. Retry only transient errors (network, timeout, 5xx); 3 attempts total, 2s per attempt, 100ms/300ms backoff. No persistent queue across restarts; TTL (10m default/15m cap) remains the final fallback.
   - Logs use room code/user ID/attempt/status only. Metrics count logical revocations: one attempt increment at schedule time, one failure increment after all retries. `/admin/stats` is the operational surface; no new public endpoint or UI.
   - If `livekit_api_url` is unreachable, the operation still succeeds; operators see failure count and warnings and can restart/retry by re-invoking the action or waiting for TTL.

7. **Deploy/security wiring.**
   - Keep `livekit_url` as the browser WSS URL. `livekit_api_url` is server-only: release `http://127.0.0.1:7880`; Compose `http://livekit:7880`. Do not publish 7880 to the host (already removed from Compose); firewall keeps it internal.
   - Add `/twirp` 404 blocking to both Nginx templates and `frontend-server` as in step 1. Access/error logs remain off for token-bearing paths; the API secret is never serialized.
   - `docs/deployment.md` / `release/deployment-zh.md`: document that `/twirp` must stay internal and that `livekit_api_url` points at the internal LiveKit API, not the public `/rtc` URL.

## Critical files & anchors

- `backend/src/infra/livekit.rs` — existing `sign`/`livekit_token`; add admin token and Twirp calls beside them.
- `backend/src/app/revocation.rs` (new) — metrics, retry loop, scheduler, `schedule_user`.
- `backend/src/api/rooms.rs` — `remove_member` ~715, `leave_room` ~368, `delete_room` ~507.
- `backend/src/api/admin.rs` — `set_user_disabled` ~291, `reset_password` ~314, `force_delete_room` ~208, `stats` ~458.
- `backend/src/config.rs` — `MediaCfg`/`DEFAULT_CONFIG`; `AppState`/`main.rs`/`tests/e2e.rs` constructors for metrics.

## Verification

1. **Compile/unit**
   - `cd backend && cargo test --locked --all-targets`
   - `cargo test --locked livekit_admin_token_has_room_admin_claim`
   - `scripts/check.sh` (expect existing 18 PASS / 0 FAIL / 0 SKIP).
2. **Fake LiveKit integration (no network)**
   - Add a test-only fake Twirp HTTP server in `backend/tests/e2e.rs` that listens on `127.0.0.1:0`, routes `POST /twirp/livekit.RoomService/{method}`, records `(method, JSON body, Authorization header)`, and returns 200 by default (configurable 500).
   - Via `test_state_with`, set `cfg.media.livekit_api_url` to the fake URL. Create a room/member, kick via owner API, wait ≤2s, assert exactly one `RemoveParticipant` request with `room`/`identity`; kick response is 200 and member access is 403.
   - Disable the member, assert `RemoveParticipant`; reset password, assert the same for each joined room; delete a room, assert `DeleteRoom` with the room code; expire a room via `sweep_once`, assert `DeleteRoom`.
   - Fake returns 500: kick still returns 200, member is locally removed, `livekit_revocations_failed` increments, and the warning contains no token/secret.
   - Fake returns 404: treated as success, no failure metric.
3. **Exposure checks**
   - `curl -sk -o /dev/null -w '%{http_code}' https://localhost:8081/twirp/livekit.RoomService/RemoveParticipant` → 404 (frontend-server), and Nginx `/twirp` returns 404.
   - `grep -R "twirp" release/nginx ../cloudroom-compose/nginx` must only show the explicit deny.
4. **Static release build**
   - `scripts/build-musl.sh all`; `file release/cloudroom-server` and `readelf -d release/cloudroom-server` show static-pie/no NEEDED or INTERP.
   - `scripts/check.sh`; existing `./test-server/run-tests.sh`, `./test-server/sfu-verify.sh`, `./scripts/ui-smoke.sh` still pass (no media behavior regression).
5. **Real isolated LiveKit (manual, required before claiming High closed)**
   - Start `./test-server/start-test-env.sh`; open two browsers or two profiles in the same SFU room (owner + member), join voice.
   - Owner kicks the member; expect the member’s LiveKit connection to disconnect/participant to disappear within ~2s, not just the app WS.
   - Repeat for admin disable (`/admin`), password reset, and room delete; expect removal/disconnection for each.
   - Point `livekit_api_url` at a closed port, kick a different member: HTTP kick still 200 and DB removal succeeds; `/admin/stats` failure count increments; logs show a revocation warning without JWT/secret; TTL remains the fallback.
   - If `livekit-cli` is available, corroborate with `livekit-cli room participants list`; browser observation is the required minimum if it is not.

## Assumptions & contingencies

- LiveKit exposes Twirp at the same host/port as its API (`7880`) and accepts JSON Twirp. The bundled binary contains `livekit.RoomService` and both methods; if a lab `400/404` shows a path/body mismatch, fix the Twirp path/field names before changing the design.
- The API is reachable from the backend only: release localhost, Compose service DNS; it is never exposed through public Nginx. If a deployment must use an external LiveKit API, set `livekit_api_url` to a private/VPN address with TLS validation; do not add a public `/twirp` proxy.
- Revocation is best-effort with no durable queue. Process crash after DB commit but before HTTP call, or repeated LiveKit outage, leaves the TTL fallback; operators see failures in `/admin/stats` and warning logs. Do not change the business response/transaction semantics.
- If the static musl build cannot link `reqwest`/rustls after two attempts, stop and report the linker error; do not merge a TTL-only substitute. The approved fallback, only if the user explicitly accepts it, is TTL 3m default / 5m production cap plus a `docs/debt.md` accepted-risk entry.

## Implementation verification (2026-10-23)

- `cargo test --locked --all-targets`: 41 unit + 32 e2e tests pass.
- Fake Twirp e2e covers kick, leave, room delete (owner/admin), disable, reset-password, sweeper expiry, 404 success, and retry/failure metrics.
- Real isolated LiveKit: `RemoveParticipant` with `roomAdmin`/`roomCreate` removed a live browser participant (`ListParticipants` empty); `DeleteRoom` returned 404 for a missing room and succeeded end-to-end for a real room.
- `scripts/check.sh`: 18 PASS / 0 FAIL / 0 SKIP; SFU 19 PASS / 0 FAIL; UI voice smoke PASS.
- Static musl: both binaries are static-pie with no NEEDED/INTERP.
- Not manually repeated in a real browser: admin disable and password reset paths (covered by fake Twirp e2e; both use the same `schedule_user` participant removal).
