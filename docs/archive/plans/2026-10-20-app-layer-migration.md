# API → infra::repo migration plan (2026-10-20)

## Context

Inventory the 189 direct `crate::infra::repo::*` calls under `backend/src/api/`,
decide which should move to `app/`, and define test-backed migration batches.
The default is to keep simple reads/writes in the API; migration is only for
multi-step workflows, transactions, and cross-resource consistency.

Do not extract a generic repository, change public HTTP contracts, alter SQLite
schema, or rename public functions. This document is the planning deliverable;
no backend code moves in this round.

## Decision rules

1. **Move to app** when a handler performs multiple DB writes/reads as one
   use case, needs a transaction across resources, or coordinates several
   resources (membership, messages, audit, broadcast) behind one operation.
2. **Keep in API** for a single resource read or write, including simple
   list/count/get/insert/update handlers. Reuse an existing app workflow when
   one exists instead of adding a one-line wrapper.
3. **Keep auth/CSRF/Origin/rate-limit/session transport checks in API.**
   `extract.rs`, `auth_routes.rs`, `security.rs`, and the HTTP/WS extractors stay
   transport-owned.
4. **Keep read-only composition** (list/detail/stats/N+1 enrichment) in the
   API unless a second mutation must share the same consistency boundary. These
   are read models, not empty wrappers.
5. **Do not move WS event dispatch wholesale.** `ws.rs` owns socket lifecycle,
   event ordering, and broadcast timing. Its multi-step blocks are marked
   deferred; they need WS-specific tests before any extraction.
6. Every batch is at most 20 repo calls, has a concrete test gate, and must
   reduce the API repo count by exactly its batch size.

## Inventory (189 calls, function level)

Coverage column: `none` means no existing API/e2e test directly exercises the
handler; `unit` would be listed if a unit test covered it (none do for these
call sites). `full` = `full_flow_e2e`; `media` =
`media_mode_change_and_gating`; `ws` = `ws_chat_and_voice_events`; `myrooms` =
`my_rooms_returns_complete_room_fields`.

### `api/admin.rs` — 32

| Function | Calls | Category | Coverage |
|---|---:|---|---|
| `list_room_requests` | 1 | single read | none |
| `approve_room_request` | 1 | single read, then app workflow | full, media |
| `reject_room_request` | 2 | multi-step write + audit | none |
| `list_rooms` | 3 | read model | full |
| `room_detail` | 3 | read model | none |
| `force_delete_room` | 1 | single read, then app workflow | none |
| `transfer_owner` | 5 | multi-step write; raw SQL | none |
| `list_users` | 1 | single read | none |
| `set_user_disabled` | 3 | multi-step write + audit | none |
| `reset_password` | 3 | multi-step write + audit | none |
| `set_user_role` | 2 | multi-step write + audit | none |
| `create_registration_invite` | 1 | single write | full, media |
| `list_registration_invites` | 1 | single read | none |
| `revoke_registration_invite` | 2 | multi-step write | none |
| `list_audit` | 1 | single read | none |
| `stats` | 2 | read model + raw counts | none |

### `api/auth_routes.rs` — 15

All auth/session flows: `register` 7, `login` 1, `logout` 1, `new_session` 1,
`upgrade_guest` 5. Category: auth/session; keep in API. Coverage: register/logout
by `full`, login by `ws`/`media`; `upgrade_guest` none.

### `api/extract.rs` — 2

`resolve_session` 2: auth session read; keep. Covered by every authenticated
test.

### `api/json.rs` — 2

`requests_json` 2: read-only enrichment shared by admin/my request responses;
keep (presentation read model).

### `api/me.rs` — 7

| Function | Calls | Category | Coverage |
|---|---:|---|---|
| `change_password` | 1 | single write | none |
| `upload_avatar` | 2 | multi-step write + file/audit | full |
| `delete_avatar` | 2 | multi-step write + file/audit | none |
| `my_rooms` | 2 | single read (row mapping helper) | myrooms |

### `api/routes.rs` — 2

`health` 1 and `public_config` 1: bootstrap/health reads; keep.

### `api/rooms.rs` — 93

| Function | Calls | Category | Coverage |
|---|---:|---|---|
| `create_room_request` | 5 | multi-step request + auto-approval | full, media |
| `my_room_requests` | 1 | single read | none |
| `cancel_room_request` | 1 | single write | none |
| `get_room` | 8 | read model | full, media |
| `join_room` | 14 (2 auth + 12 workflow) | multi-step; raw restore SQL | full, media |
| `default_nickname` | 2 | helper reads for join | full, media |
| `redeem_invite` | 6 | multi-step invite/member flow | full, media |
| `leave_room` | 3 | multi-step membership + message | none |
| `update_settings` | 2 | multi-step settings + seq | full |
| `create_invite` | 1 | single write | full, media |
| `list_invites` | 1 | single read | none |
| `revoke_invite` | 2 | multi-step write | none |
| `set_member_role` | 3 | multi-step write | none |
| `set_nickname` | 3 | multi-step write | none |
| `remove_member` | 4 | multi-step membership + message | none |
| `create_mute` | 3 | multi-step write | full |
| `list_mutes` | 1 | single read | none |
| `remove_mute` | 3 | multi-step write | full |
| `send_message_inner` | 10 | multi-step message + reply/mute/seq | full, ws |
| `get_messages` | 5 | read model | full |
| `create_direct_room` | 15 | multi-step + `claim_code` transaction | full, media |

### `api/ws.rs` — 36

`handle_socket` 34: 4 auth/single reads (initial room/member and one-off member
lookup) + 30 multi-step event blocks (room mute, speak request/approve/revoke,
reactions, presence). `build_hello` 2 read-only. All kept for this plan:
transport event sequencing and missing targeted tests. Coverage: `ws` only for
chat/voice; mute/speak/reaction/presence none.

## Keep vs move summary

- **Keep: 95**
  - 17 auth/extract
  - 4 health/public config/json read models
  - 15 admin read/single handlers
  - 3 me read/single handlers
  - 20 rooms read/single/helper calls (including the 2 join guest-auth calls)
  - 36 WS transport calls (30 multi-step deferred)
- **Move: 94**, in 8 batches below. This is not a wholesale cleanup; it is the
  set of multi-step workflows with an actual coordination boundary.
- **Deferred:** 30 WS multi-step calls; separate task after adding WS tests.

## Batches

### Batch 1 — join membership workflow (20 calls, covered)

- Calls: `rooms::join_room` lines 473, 479, 484, 490, 491, 498, 499, 500,
  505, 518, 519, 538 (12); `rooms::default_nickname` 574, 579 (2);
  `rooms::redeem_invite` 593, 609, 615, 617, 619, 622 (6).
- Keep in API: guest `create_user`/`create_session` at lines 460-461, rate limit,
  CSRF/transport, response cookies.
- Add `app::rooms::join_member(state, conn, room, uid, invite_token, nickname)`
  performing room state/member/rejoin/owner/full checks, invite redemption,
  nickname derivation, and sequence bump. Move the raw `UPDATE room_members`
  restore at line 485 into a new internal repo helper `restore_member` (no
  public HTTP change).
- Tests: existing `full_flow_e2e`, `media_mode_change_and_gating`; add
  `join_room_membership_paths` e2e covering owner-no-invite, already-member,
  rejoin-after-leave, max_members full, invalid/used invite. Estimated 5 cases.
- Acceptance: `cargo test --all-targets`; API count 189 → 169; join/guest
  tests pass; no API/JSON/schema change. Rollback: restore handler body and
  delete app helper/repo helper in one revert.

### Batch 2 — message send workflow (10 calls, covered)

- Calls: `rooms::send_message_inner` 1317, 1339, 1347, 1370, 1372, 1380, 1381,
  1399, 1403, 1408.
- Move to `app::rooms::send_message`; keep API validation, CSRF, mute/role
  checks that belong to transport; reuse existing app event broadcast. The WS
  call site at `ws.rs:99` calls the app function too.
- Tests: add `message_send_permissions_and_reply` e2e (estimated 4 cases:
  normal send, muted send, reply snapshot, invalid/empty body) plus existing
  `full_flow_e2e` and `ws_chat_and_voice_events`. Acceptance: count 169 → 159;
  HTTP and WS message tests pass. Rollback: revert app function and
  handler/WS call site.

### Batch 3 — room request creation (5 calls, covered)

- Calls: `rooms::create_room_request` 115, 139, 195, 198, 230.
- Move request-row construction/insert/read and the super-admin auto-approval
  decision into `app::rooms::create_room_request`; keep rate limit, CSRF,
  role/bitrate validation, and response JSON in API. Reuse existing
  `app::rooms::create_room_for_request`/`apply_media_mode_change_for_request`.
- Tests: `full_flow_e2e`, `media_mode_change_and_gating`; add
  `room_request_pending_and_auto_approval` e2e (2 cases: normal user pending,
  super-admin auto-approved); add it regardless of existing assertions so the
  app cutover has a direct regression gate. Acceptance:
  count 159 → 154; no HTTP contract change. Rollback: revert handler plus app
  helper.

### Batch 4 — settings and member lifecycle (15 calls, tests first)

- Calls: `rooms::update_settings` 763, 773 (2); `rooms::leave_room` 653, 654,
  680 (3); `rooms::set_member_role` 950, 958, 959 (3);
  `rooms::set_nickname` 1011, 1016, 1017 (3); `rooms::remove_member` 1056,
  1064, 1065, 1089 (4).
- Move DB orchestration to app functions; keep permission/CSRF checks in API.
  Use a transaction where a membership/sequence/system-message set must be
  atomic; do not add a generic transaction wrapper.
- Add `room_member_lifecycle` e2e before migration: leave, owner/role change,
  nickname change, remove member, settings update + sequence/message effects.
- Tests: existing `full_flow_e2e` settings; new lifecycle e2e (estimated 5
  cases). Acceptance: count 154 → 139; lifecycle e2e + full flow pass. Rollback:
  revert handler/app helper per operation; no schema.

### Batch 5 — mutes and invite revocation (8 calls, tests first)

- Calls: `rooms::create_mute` 1149, 1155, 1156 (3);
  `rooms::remove_mute` 1224, 1229, 1230 (3);
  `rooms::revoke_invite` 911, 916 (2).
- Move to app; keep permission/CSRF validation. Add `invite_revoke_and_list`
  e2e for revoke/list/expired/already-used behavior. Existing mute coverage in
  `full_flow_e2e` remains.
- Acceptance: count 139 → 131; mute/invite tests pass. Rollback: revert per
  operation; no schema.

### Batch 6 — direct-room creation (15 calls, covered; transaction)

- Calls: `rooms::create_direct_room` 1517, 1522, 1529, 1531, 1536, 1542, 1543,
  1547, 1569, 1570, 1571, 1574, 1577, 1578, 1598.
- Move orchestration to `app::rooms::create_direct_room`; keep CSRF/auth/role
  checks in API. Preserve the existing `claim_code` transaction boundary; do
  not wrap `claim_code` in a second transaction. Add
  `direct_room_idempotency_and_collision` e2e (existing room, peer membership,
  code collision, non-member parent).
- Acceptance: count 131 → 116; `full_flow_e2e`, `media_mode_change_and_gating`,
  new direct e2e pass. Rollback: revert handler/app helper; no schema.

### Batch 7 — avatar workflows (4 calls, tests first)

- Calls: `me::upload_avatar` 103, 125 (2); `me::delete_avatar` 155, 166 (2).
- Move the DB/audit/member-room orchestration to `app::profile::update_avatar`
  and `remove_avatar`; keep multipart parsing, auth, and response JSON in API.
  Add `avatar_delete` e2e; upload/serve is already in `full_flow_e2e`.
- Acceptance: count 116 → 112; avatar upload/serve/delete tests pass. Rollback:
  revert app module and handlers; avatar files are existing state, no schema.

### Batch 8 — admin multi-step writes (17 calls, tests first; last)

- Calls: `admin::reject_room_request` 124, 129 (2);
  `admin::transfer_owner` 273, 278, 284, 285, 290 (5);
  `admin::set_user_disabled` 371, 379, 382 (3);
  `admin::reset_password` 410, 421, 422 (3);
  `admin::set_user_role` 463, 468 (2);
  `admin::revoke_registration_invite` 548, 553 (2).
- Create `app/admin.rs` workflows; keep auth/CSRF/role checks and response JSON
  in API. Move the raw owner `UPDATE` at admin.rs:298 into an internal repo
  helper. Password hashing stays in API or is passed as a hash; do not make app
  depend on transport. Use transactions for multi-write user/room operations.
- Add e2e tests first: `admin_reject_and_transfer`, `admin_user_management`
  (disable/reset/role), `admin_invite_revoke`. These endpoints currently have
  no coverage.
- Acceptance: count 112 → 95; new admin e2e + `scripts/check.sh` pass. Rollback:
  revert app module and handlers; no schema/public API.

## First batch recommendation

Start with **Batch 1 (20 calls)**: join is covered by both existing e2e suites,
has a clear consistency boundary (membership + invite + sequence), and is concentrated
in one file. It removes the largest single block while leaving auth/session
creation and transport checks in API. Add the five-case
`join_room_membership_paths` e2e first, then implement the app helper and
handler cutover. This is the smallest useful migration with a real test gate.

## Verification for the migration execution

- Before each batch: record `grep -rF 'crate::infra::repo' backend/src/api | wc -l`
  (current 189) and the batch's exact count.
- Implement tests first; prove they fail if the new app function is absent or
  the handler is still direct (or at least prove the new behavior path is
  exercised), then migrate.
- After each batch:
  - `cargo test --all-targets`
  - `scripts/check.sh`
  - `test-server/run-tests.sh`
  - API count equals previous count minus the batch size.
- Do not change `docs/api.md`, route paths, response fields, SQLite schema, or
  public function names. Roll back by reverting the batch commit; no data
  migration is involved.
