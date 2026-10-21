# Left-at room access fix

## Context

`app::access::active_member_room` returns a `room_members` row even when `left_at` is set, so a user who voluntarily left or was removed can still call room HTTP endpoints. The defect is confirmed by `room_member_lifecycle_paths`, which currently asserts `GET /rooms/life-room == 200` after leave and after removal. The intended end state: they receive the same **403** as an authenticated non-member for an existing room, while `POST /rooms/{code}/join` continues to restore a left membership.

Execution also writes this plan to `docs/plans/2026-10-21-left-at-access.md`; plan-mode review itself is read-only.

## Current behavior (from code and existing tests)

### HTTP

- Authenticated non-member, existing active room: **403** (`full_flow_e2e` asserts Bob GET `/rooms/666` is 403).
- Authenticated, unknown room: **404** (`active_member_room` → `ApiError::room_not_found`).
- Unauthenticated GET room: **403** (`get_room` rejects missing `MaybeAuth` before room lookup; `full_flow_e2e` asserts anonymous `/rooms/zzzzzzz` is 403).
- Left or removed member: **200**. `active_member_room` fetches `(room_id,user_id)` without checking `left_at`; both lifecycle assertions lock this.
- `GET /rooms/{code}/messages` and `POST /rooms/{code}/messages`: currently **200** for a left member. Both call `active_member_room`; send also passes the left row to `app::messages::send`.
- `POST /rooms/{code}/leave` on a member who already left: currently **200** (the row is fetched and `leave_room` is a no-op update).
- `GET /rooms/{code}/ice-servers` and `POST /rooms/{code}/media-token`: currently **200** for a left member because both use `active_member_room`. Already-issued LiveKit tokens remain valid until their existing TTL; this fix only blocks new token/ICE requests.
- All other room endpoints using `active_member_room` (settings, delete, invites, members, mutes, direct, request media-mode changes) currently accept a left member according to the member's stored room role.
- `GET /me/rooms` already filters `m.left_at IS NULL`; admin endpoints do not use membership and are unaffected.

### WebSocket

- New WS connection: `handle_socket` has its own snapshot using `repo::get_member` without `left_at`; a left member therefore gets a successful upgrade, a `hello`, and can send events. This is the one membership gate that bypasses `active_member_room`.
- Existing WS connection after leave/removal: `leave_room`/`remove_member` call `hub.unregister_all`, which removes the user's hub sender but does not close the reader loop. The loop never rechecks membership, so an already-open socket can keep processing text events.
- Current non-member WS behavior: upgrade succeeds, then the snapshot fails and the socket is dropped; there is no HTTP 403 after upgrade.

### Frontend

- `frontend/src/pages/Room.tsx::loadRoom`: HTTP 403 sets `notMember=true` and renders the invite form; it does **not** redirect. HTTP 404 (and 410) falls through to `setError`, rendering an error card with a “返回首页” link.
- `loadMessages` catches and ignores all errors; no message content is shown after a 403.
- `manual actions (send/settings/leave/members)` display the API error via `setError`.
- `useRoomSocket` reconnects with backoff on every close; there is no special 403/404 handling because the WebSocket close event does not expose the HTTP status. This is the existing non-member behavior.
- No frontend edit is required for the access fix: the 403 branch already presents the non-member/join view. The form requires an invite token; `POST /join` without a token still restores a left membership, but the UI does not offer an empty-token submit.

## Target semantics

- Existing active room + user has no active membership (never joined, left, or removed): **403** with the existing `forbidden` error body. Reason: matches the current non-member contract, avoids leaking whether the user once belonged, and does not disclose room membership state. Do not return 404 for an existing room.
- Unknown room: **404**.
- Inactive room (deleted/expired): **410** via existing `room_unavailable`; this check stays before membership.
- Unauthenticated HTTP: **401** from `AuthUser` for authenticated endpoints; `GET /rooms/{code}` is intentionally 403 because it accepts `MaybeAuth` and returns the invite flow. Do not change that.
- New WS connection for a left/removed user: same observable behavior as a non-member today — upgrade then immediate close, no `hello`, no accepted events.
- Existing WS connection after leave/removal: close on the next received text event; no further ping/pong or room mutations.
- `POST /rooms/{code}/join` is out of this defect and remains unchanged: a left row is restored without an invite, including a row created by removal. This preserves voluntary rejoin behavior from Batch 1; blocking removed-only rejoin would require distinguishing leave from removal (new state) and is not part of this fix.

## Approach

### 1. Reject left rows in the shared HTTP access gate

File: `backend/src/app/access.rs`, `active_member_room`.

After the existing `get_member` lookup:

```rust
let member = crate::infra::repo::get_member(conn, &room.room_id, &uid)?
    .ok_or_else(|| ApiError::forbidden("you are 不是房间成员 of this room"))?;
if member.left_at.is_some() {
    return Err(ApiError::forbidden("you are 不是房间成员 of this room"));
}
Ok((room, member))
```

- Reuse the exact existing message so clients see no new error shape; status is 403 via `ApiError::forbidden`.
- Do not change `repo::get_member` (join, admin transfer, nickname lookups, and message enrichment intentionally need left rows).
- This automatically fixes every endpoint listed above; no per-handler edits.
- No SQL/index change: `room_members` is keyed by `(room_id, user_id)` (`PRIMARY KEY` in the initial migration), so this is a single PK row fetch plus an in-memory `Option` check. No new index or migration.

### 2. Fix the two WS bypasses

File: `backend/src/api/ws.rs`.

1. In `handle_socket`'s membership snapshot, after `get_member` succeeds:

```rust
if member.left_at.is_some() {
    return Err(crate::error::ApiError::forbidden("不是房间成员"));
}
```

This makes a new connection from a left/removed user follow the existing non-member path: socket dropped before `hello`.

2. In the `Message::Text` arm of the event loop, before `serde_json::from_str`:

```rust
if !state.hub.is_connected(&room_id, &uid) {
    break;
}
```

`hub.unregister_all` is already called by `leave_room` and `remove_member`; this in-memory check closes an already-open reader on its next event without adding a DB query per event. Do not change the hub registry or the WS handshake to a pre-upgrade 403: current non-member WS behavior is upgrade-then-close, and this fix stays consistent with it.

### 3. Update the characterization test and add coverage

File: `backend/tests/e2e.rs`.

Change `room_member_lifecycle_paths`:

- After Bob leaves: change the GET room assertion from `200` to `403`; remove the old “existing behavior” comment.
- After Carol is removed: change Carol's GET room assertion from `200` to `403`; remove the old comment.
- After Alice (owner) leaves: change Alice's GET room assertion from `200` to `403`. Do not read `room.owner_user_id`/`my_role` from that response. Instead:
  - assert Alice's GET is 403;
  - read the room directly with `state.db.with_sync(|conn| repo::get_room_by_code(conn, "life-room"))` and assert `owner_user_id == Some(alice_id)`;
  - assert Bob (still active) GET room is 200 and `room.owner_user_id == alice_id`, `room.my_role == "member"`.

Add `left_member_access_paths`:

- Setup owner Alice, member Bob, non-member Carol in a standard room; Bob joins.
- As Bob before leaving: GET room 200 and GET messages 200.
- Bob leaves (200), then assert all are **403**:
  - `GET /rooms/{code}`;
  - `GET /rooms/{code}/messages?after=0`;
  - `POST /rooms/{code}/messages` with `{"body":"after leave"}`;
  - `POST /rooms/{code}/settings` with `{"text_enabled":false}`;
  - `POST /rooms/{code}/direct` with `{"user_id": carol_id}`;
  - `GET /rooms/{code}/ice-servers`;
  - `POST /rooms/{code}/media-token` (CSRF token from `csrf_opt`);
  - `POST /rooms/{code}/leave` again.
- Bob rejoins with an empty `{}` body (200); assert GET room 200 and GET messages 200.
- Alice removes Bob; assert Bob GET room and GET messages are 403.
- Assert non-member Carol GET room is 403; authenticated GET `/rooms/no-such-room` is 404; anonymous GET room is 403.

Add `left_member_ws_closes`:

- Reuse the existing direct-DB setup pattern from `ws_chat_and_voice_events` (user with password, active room, owner membership).
- Login and open WS; read the `hello`.
- Obtain CSRF via `csrf_opt`, `POST /rooms/{code}/leave` with it.
- Send `{"type":"ping"}` on the still-open socket; read with a 2-second TCP read timeout (WS is plain `ws://`; match `MaybeTlsStream::Plain`, call `set_read_timeout`). Assert the read is `Err` or `Message::Close`, and never a `pong`.
- Open a fresh WS with the same cookie after leave; within the same 2-second timeout assert the first read is `Err`/`Close`, never a `hello`.
- This test requires the WS event-loop guard; without it the first read returns `pong`, and without the snapshot check the fresh read returns `hello`.

### 4. Frontend

No source edit. On the next room load, 403 takes the existing `notMember` branch (invite form), 404/410 take the existing error card with a back link, and there is no automatic redirect. Do not change `join`, `loadMessages`, or the WS reconnect policy in this fix.

## Critical files & anchors

- `backend/src/app/access.rs::active_member_room` — single shared gate; left check here.
- `backend/src/api/ws.rs::handle_socket` — snapshot check at entry and `Message::Text` guard before parsing.
- `backend/tests/e2e.rs::room_member_lifecycle_paths` — three old 200 assertions to replace.
- `backend/tests/e2e.rs` — append `left_member_access_paths` and `left_member_ws_closes`.
- `frontend/src/pages/Room.tsx::loadRoom` / `frontend/src/hooks/useRoomSocket.ts` — existing behavior reference; no edit.

## Verification

From `/home/hui/agents/omp/cloudroom/backend`:

```bash
cargo test --test e2e room_member_lifecycle_paths
cargo test --test e2e left_member_access_paths
cargo test --test e2e left_member_ws_closes
cargo test --all-targets
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
```

Expected:

- `room_member_lifecycle_paths` passes with the three changed 403 assertions.
- `left_member_access_paths` passes: all listed room/message/settings/direct/voice/leave calls return 403 after leave and after removal; rejoin restores 200; nonmember 403; unknown 404; anonymous 403.
- `left_member_ws_closes` passes: no hello for a fresh left-member socket; no pong after leaving on an existing socket; both close within 2 seconds.
- Full suite and clippy remain green.

From `/home/hui/agents/omp/cloudroom`:

```bash
scripts/check.sh
```

Expected `16 PASS / 0 FAIL / 2 SKIP` (the two existing skips remain ESLint and cargo-audit).

## Assumptions & contingencies

- 403 is chosen over 404 for an existing room because it matches the current non-member contract and avoids leaking membership history.
- WS keeps the existing upgrade-then-close behavior; no pre-upgrade 403. If the read timeout cannot be applied to the chosen tungstenite stream variant, the test fails rather than silently accepting a hung socket; inspect the local `ws://` stream as `MaybeTlsStream::Plain`.
- Rejoin semantics are intentionally unchanged: a removed user can still call `POST /join` without an invite because only `left_at` is stored. Do not add a schema field or alter `join_member` in this fix.
- Existing LiveKit tokens issued before removal remain valid until TTL; only new ICE/media-token requests are blocked. No token-revocation mechanism exists in this codebase.
- No schema, migration, route, response-field, or frontend-contract changes.

## Rollback

Revert the single implementation commit. This restores the old helper, WS snapshot/event-loop behavior, and test assertions; no schema or data migration exists. Existing `left_at` rows are untouched and remain historical membership records.
