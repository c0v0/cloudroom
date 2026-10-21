# WS multi-step events migration decision

## Context

The app-layer migration left 30 direct repo calls in `backend/src/api/ws.rs`, grouped into eight event handlers. This decision records each call, classifies it by transport vs. orchestration vs. timing sensitivity, and chooses whether to move any of them to `app/`. The outcome is a deliberate no-migration decision unless a handler can be extracted without changing event order or broadcast points.

Execution also writes this decision to `docs/plans/2026-10-21-ws-events-decision.md` and updates `docs/debt.md`.

## Inventory: 30 repo calls in ws event handling

Line numbers are current in `backend/src/api/ws.rs`.

### RoomMuteToggle — lines 186-257

| Line | Operation | Responsibility | Multi-step? | Tested? |
|---:|---|---|---|---|
| 190 | `get_member` | owner authorization | read | no WS test |
| 211 | `get_room_by_id` | current `room_muted` idempotency check | read | no |
| 217 | `set_room_muted` | persist mute flag | write | no |
| 218 | `clear_speak_grants` | clear grants and pending requests | write, same DB closure | no |
| 242 | `insert_system_message` | system message after `room_muted` broadcast | write, separate closure | no |

Current responsibilities: parse event, authorize owner by member role, idempotency check, write two resources, broadcast `room_muted`, then create/broadcast system `message`. **Multi-step and timing-sensitive.** No WS coverage; HTTP mutes do not exercise this path.

### SpeakRequest — lines 258-360

| Line | Operation | Responsibility | Multi-step? | Tested? |
|---:|---|---|---|---|
| 261 | `get_member` | owner check | read | no |
| 270 | `has_speak_grant` | already-granted check | read | no |
| 276 | `get_room_by_id` | room mute state | read | no |
| 299 | `grant_speak` | owner auto-approval | write | no |
| 318 | `insert_speak_request` | pending request for non-owner | write | no |
| 328 | `get_member` | requester nickname for owner notification | read | no |
| 343 | `list_members` | all active owners to notify | read | no |

Current responsibilities: validate room mute and existing grant, auto-grant owner, insert pending request, notify owners, ack requester. **Multi-step, cross-resource, timing-sensitive.** No coverage.

### SpeakReply — lines 361-442

| Line | Operation | Responsibility | Multi-step? | Tested? |
|---:|---|---|---|---|
| 364 | `get_member` | approver owner check | read | no |
| 383 | `grant_speak` | approve grant | write | no |
| 398 | `get_member` | target nickname for system message | read | no |
| 407 | `insert_system_message` | system message after `speak_grant` broadcast | write | no |
| 429 | `delete_speak_request` | reject/remove pending request | write | no |

Current responsibilities: owner auth, grant or reject, broadcast grant/rejected, then fetch nickname and insert/broadcast system message. **Multi-step and timing-sensitive.**

### SpeakRevoke — lines 443-514

| Line | Operation | Responsibility | Multi-step? | Tested? |
|---:|---|---|---|---|
| 446 | `get_member` | owner check | read | no |
| 472 | `revoke_speak` | remove grant | write | no |
| 487 | `get_member` | target nickname | read | no |
| 496 | `insert_system_message` | system message after `speak_revoked` | write | no |

**Multi-step; broadcast is interleaved before the system-message write.** No coverage.

### SpeakRelinquish — lines 515-562

| Line | Operation | Responsibility | Multi-step? | Tested? |
|---:|---|---|---|---|
| 520 | `revoke_speak` | remove caller's grant | write | no |
| 537 | `get_member` | caller nickname | read | no |
| 545 | `insert_system_message` | system message after `speak_revoked` | write | no |

**Multi-step; broadcast interleaved before system message.** No coverage.

### Reaction — lines 563-619

| Line | Operation | Responsibility | Multi-step? | Tested? |
|---:|---|---|---|---|
| 577 | `message_belongs_to_room` | validate target message | read | no |
| 584 | `toggle_reaction` | insert/delete reaction | write, same closure as validation | no |
| 591 | `message_reactions` | aggregate snapshot for the broadcast | read, separate closure | no |

**Multi-step; no broadcast between the DB phases.** This is the only handler that could be extracted cleanly as a single snapshot-returning app function. No coverage.

### Signal — lines 620-652

| Line | Operation | Responsibility | Multi-step? | Tested? |
|---:|---|---|---|---|
| 634 | `get_member` | target must be an active member | read | only unknown-target liveness in WS test |

**Transport relay**; one validation read, then `hub.send_to`. No cross-resource write; no ordering writes. **Category A.**

### Presence heartbeat — lines 654-668

| Line | Operation | Responsibility | Multi-step? | Tested? |
|---:|---|---|---|---|
| 663 | `touch_user_online` | user last_online | write | no |
| 664 | `touch_member_online` | member last_online | write, same closure | no |

**Two simple writes with the same timestamp**, throttled by `heartbeat_interval`; no broadcast. **Category B but low value.**

### Outside the 30 (not part of this decision)

- Connection snapshot: lines 40/45 (`get_room_by_code`, `get_member`).
- `build_hello`: lines 762/765 (`list_members`, `get_user`).
- Disconnect teardown: lines 700/711 (`get_member`, `insert_system_message`) after an 8-second timer, plus reconnect check and broadcasts. This is timing-sensitive transport lifecycle and remains in API; it is not in the 30-call inventory.

## Classification summary

- **A — transport/parse/relay (1 call):** Signal (line 634). Keep in API.
- **B — multi-step but cleanly extractable (5 calls):** Reaction (3) and presence heartbeat (2). Reaction is a valid future extraction; presence is trivial and timing-throttled. Both have no targeted tests.
- **C — multi-step + timing/broadcast-sensitive (24 calls):** RoomMuteToggle (5), SpeakRequest (7), SpeakReply (5), SpeakRevoke (4), SpeakRelinquish (3). All interleave DB writes with broadcasts/system messages and have no dedicated WS sequence tests.
- `SendMessage` already delegates to `app::messages::send`; no direct repo calls.
- VoiceJoin/Leave, MediaState, Ping, SendMessage have no direct repo calls in the 30; they are transport/hub operations or existing app calls.

## Decision

**Keep all 30 calls in `api::ws`; do not migrate them to `app/` in this change.**

Reasons:

1. 24 of 30 calls are in handlers whose externally visible order is `DB write → broadcast role/state → system-message DB write → message broadcast` (or grant → notify → system message). Extracting the DB parts would either split one use case across app calls or require app callbacks for hub broadcasts, adding indirection without a consistency boundary.
2. The WebSocket loop is a long-lived transport state machine: socket lifecycle, `voice_member` teardown, 8-second offline timer, reconnect takeover, and per-user hub routing live in `api/ws.rs`. Moving individual DB snippets does not move the use case and makes ordering harder to see.
3. Existing tests do not cover RoomMuteToggle, SpeakRequest/Reply/Revoke/Relinquish, Reaction, presence, or teardown. Migrating timing-sensitive code before sequence tests would risk silent protocol changes.
4. Reaction is the only clean B candidate, but it is three calls and would need a new app module plus two-socket sequence tests for marginal benefit; presence is two trivial writes in a throttled heartbeat. Keep both for now.
5. The app-layer goal is multi-step business workflows with a consistency boundary. WS event handlers are transport-triggered state machines; the transport and broadcast timing are the boundary.

This is an explicit **accepted decision**, not unfinished migration debt. The 30-call count remains 30 by design; it must not be used as a “remaining migration” metric.

## Tests added (sequence protection, no migration)

`ws_mute_speak_and_reaction_sequences` uses two authenticated sockets (owner and member) in a standard room and asserts:

1. **Room mute**
   - Owner sends `{"type":"room_mute_toggle","muted":true}`.
   - Both clients receive `room_muted` with `muted:true` **before** a `message` whose body is `"房主开启了全员禁言"`.
   - Repeat the same event: neither client receives another `room_muted` or system message (idempotency).
   - Non-owner sends the event: non-owner receives `error.code=="forbidden"`; no broadcast.
   - Owner sends `muted:false`; both receive `room_muted` before `"房主关闭了全员禁言"`.
2. **Speak request/approval**
   - Member sends `{"type":"speak_request"}`; member receives `speak_requested`; owner receives `speak_request_notify` with member ID.
   - Owner approves; both receive `speak_grant` **before** `message` `"ws-member 获得了发言权限"`.
   - Repeat approval: no duplicate events.
3. **Revoke/relinquish**
   - Owner revokes; both receive `speak_revoked` before the matching system message.
   - Re-grant, then member sends `speak_relinquish`; both receive `speak_revoked` with `relinquished:true` before the matching system message.
4. **Reaction**
   - Member sends a message; both receive it.
   - Member reacts with `👍`; both receive one `reaction` with `added:true` and count 1.
   - Repeat: `reaction` with `added:false` and count 0.
   - Invalid emoji `💥`: sender receives `error.code=="validation"`; no `reaction` broadcast.

The test uses a 250ms read timeout for negative assertions and 2s for positive reads. The 8-second offline timer remains untested and out of scope.

## Verification

From `/home/hui/agents/omp/cloudroom/backend`:

```bash
cargo test --test e2e ws_mute_speak_and_reaction_sequences
cargo test --test e2e ws_chat_and_voice_events
cargo test --all-targets
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
```

Expected:

- The new sequence test passes; it fails if a broadcast moves after its system message or idempotent events duplicate.
- Existing WS test remains green.
- Full suite/lint green.

From `/home/hui/agents/omp/cloudroom`:

```bash
scripts/check.sh
```

Expected `16 PASS / 0 FAIL / 2 SKIP`.

## Assumptions & contingencies

- No production code migration is made; the 30 repo calls remain in `api/ws.rs`.
- `Reaction` and presence are the only category B candidates; they stay because extraction does not eliminate the transport state machine and adds test/maintenance cost.
- The sequence test uses two sockets and a 250ms negative read timeout. If the WebSocket environment cannot reliably observe “no event” with a socket timeout, use the existing bounded-reader fallback; do not weaken positive ordering assertions.
- Existing room role/ownership semantics are unrelated; no schema, API JSON, frontend, or hub changes.
- If a future change needs to extract a WS handler, first land/keep the sequence test, then move only DB phases while preserving broadcast call sites in `api/ws.rs`.

## Rollback

Revert the test/doc commit. No production behavior or schema changes exist to roll back.
