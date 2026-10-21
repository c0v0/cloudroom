# Kicked-member rejoin semantics

## Context

A room `remove_member` currently writes the same `left_at` value used by voluntary leave. `join_member` treats any left row as a rejoin and restores it without an invite, so a kicked user can immediately call `POST /rooms/{code}/join` with `{}` and recover membership. This plan chooses a semantics, updates the membership state model, and makes the join path enforce it. Room joins are invite-only in the current code; no public room-join setting exists.

Execution also writes this plan to `docs/plans/2026-10-21-kicked-rejoin.md`.

## Current behavior

- **New-member join** (`app::rooms::join_member`):
  1. Room must exist and be active; otherwise 404/410.
  2. If the requester has no membership row and is the room owner, add owner without an invite.
  3. If `room.max_members > 0`, reject when active members already reach the limit with 409 `room_full`.
  4. Otherwise require a valid room invite (`invite_token`); missing invite is 403, invalid/expired/used/wrong-room invite is 400 `invite_invalid`.
  5. Invite redemption inserts a member row with `RoomRole::Member`; `max_role` is ignored.
- **Voluntary leave**: `app::rooms::leave_member` calls `repo::leave_room`, which only sets `left_at`; audit action `room.left`.
- **Kick**: `app::rooms::remove_member` also calls `repo::leave_room`; audit action `member.removed` records actor/target in the audit log, but the membership row stores only `left_at`—no reason, operator, or ban marker.
- **Rejoin**: any row with `left_at` set takes the early `restore_member` branch in `join_member`: it clears `left_at`, resets `joined_at`, bumps the sequence, emits the join system message, and returns 200. It bypasses invite validation and `max_members`. First join and rejoin therefore share `join_member` but not the same authorization path.
- **Owner**: `remove_member` rejects attempts to remove another owner; an owner who voluntarily leaves is restored through the same early branch. Owner removal is not available.
- **Room types**: standard rooms are invite-only; no public join configuration exists. Direct rooms are created with both users as owners, and the owner-removal guard makes kick ineffective there. The public/invite distinction mentioned in option C does not exist in this codebase.
- **Frontend**: `Room.tsx::removeMember` calls the owner-only remove endpoint; no reason is entered. The target is unregistered from the WS hub and receives no explicit “kicked” event. On the next room load, 403 renders the existing non-member invite form; there is no dedicated kicked/rejoin entry point. The form sends a user-entered invite token.

## Semantics decision

|Option|Meaning|Impact / cost|Frontend|
|---|---|---|---|
|**A. Require a new invite**|Kick ends membership; rejoin must present a valid room invite **created after the kick timestamp**.|One nullable membership column, repo helpers, and join/redeem branching; no schema rewrite of existing rows. Medium-low.|No source change; existing invite form works, and a new invite succeeds.|
|B. Permanent ban|Kick prevents any future join, even with an invite; requires ban state and an unban path.|`banned_at`/ban table + admin unban API/UI + review of room deletion/expiry; highest.|Needs “banned” messaging and an owner unban control.|
|C. Keep current|Kick only removes the current membership/session; empty `POST /join` restores immediately.|No code, but kick has no durable effect; because rooms are invite-only, C contradicts the kick action.|No change.|

**Recommendation: A.** Every new entry into a standard room already requires an invite; a kick should invalidate the current membership and require a fresh grant of entry. B is stronger than the product currently supports (no unban surface or ban state) and would need a separate moderation design. C makes kick meaningless in the only room type that supports it.

“New invite” is enforced as `invite.created_at > member.left_at`: a currently valid but pre-kick invite cannot be used by the removed user. Invites remain unscoped per user (any holder can redeem), so this is a per-rejoin timestamp check, not a per-user invite binding. Revoking all other room invites is not part of A.

## Data model

Add one nullable column to `room_members`:

```sql
ALTER TABLE room_members ADD COLUMN left_reason TEXT;
```

- Values written by application code: `'left'` for voluntary leave, `'removed'` for kick. Active rows and legacy rows remain `NULL`.
- `left_at` already records when the row was closed; use it for the “invite created after kick” check. No `banned_at` for option A.
- Existing left rows become `NULL` because history cannot distinguish old voluntary leaves from old kicks. Treat `NULL` as voluntary (`left_reason != 'removed'`) so existing rejoin behavior is preserved for pre-migration data. State this one-time legacy gap in the migration comment.
- Migration id: `2026-10-21-0006-room-member-left-reason`, appended to `MIGRATIONS` in `backend/src/infra/db.rs` after `2026-08-09-0005-audio-bitrate-and-room-mute`.
- No new index: `get_member` already uses the `(room_id, user_id)` primary key; the new column is read from that one row. Do not add `banned_at` or a ban table.

## Approach

### 1. Persist the leave reason

File: `backend/src/domain/models.rs`, `RoomMember`.

Add:

```rust
pub left_reason: Option<String>,
```

File: `backend/src/infra/repo.rs`.

- Update both member SELECT column lists (`get_member`, `list_members`) to append `left_reason` after `last_online_at`; `row_to_member` reads it as `Option<String>` at index 7.
- Change `leave_room` (voluntary) to set both fields:

```sql
UPDATE room_members
SET left_at = ?1, left_reason = 'left'
WHERE room_id = ?2 AND user_id = ?3 AND left_at IS NULL
```

- Add `pub fn remove_member(conn, room_id, user_id) -> ApiResult<()>` with the same update but `left_reason = 'removed'`. No new transaction; callers already run inside `with_sync`/`with`.
- Change `restore_member` (voluntary rejoin) to `SET left_at = NULL, left_reason = NULL, joined_at = ?1`.
- Add `pub fn restore_member_with_nickname(conn, room_id, user_id, nickname) -> ApiResult<()>` for a removed user redeeming an invite:

```sql
UPDATE room_members
SET left_at = NULL, left_reason = NULL, joined_at = ?1, nickname = ?2
WHERE room_id = ?3 AND user_id = ?4 AND left_at IS NOT NULL
```

- Keep `add_member` unchanged; a fresh insert leaves `left_reason` NULL.

### 2. Branch join/rejoin on the reason and invite timestamp

File: `backend/src/app/rooms.rs`.

In `join_member`, replace the existing left-row branch:

```rust
if let Some(member) = crate::infra::repo::get_member(conn, &room.room_id, uid)? {
    if member.left_at.is_none() {
        return Err(ApiError::conflict("您已是该房间成员"));
    }
    if member.left_reason.as_deref() != Some("removed") {
        crate::infra::repo::restore_member(conn, &room.room_id, uid)?;
        // existing bump_room_seq + join system message + return
    }
    // removed: fall through to the invite-required path
}
```

For the removed branch, preserve the existing owner, `max_members`, and invite checks below:

- Owner path is unchanged (owner rows can only be voluntary leaves, handled above).
- `max_members` applies to a removed rejoin; do not bypass capacity.
- Missing invite continues to return 403 `forbidden("加入该房间需要有效邀请")`.
- Pass the removed member’s `left_at` to `redeem_invite` as `min_created_at`:

```rust
redeem_invite(
    state,
    conn,
    token,
    &room,
    uid,
    requested_nickname,
    member_left_at, // Some(member.left_at) only for removed rejoin
)?;
```

Change `redeem_invite` signature to accept `min_created_at: Option<chrono::DateTime<chrono::Utc>>`. After the existing invite/room/active checks:

```rust
if let Some(after) = min_created_at {
    if inv.created_at <= after {
        return Err(ApiError::bad_request(
            "invite_invalid",
            "被移出后需要新邀请",
        ));
    }
}
```

After `consume_invite`, replace the unconditional `add_member` with:

```rust
if let Some(existing) = crate::infra::repo::get_member(conn, &room.room_id, uid)? {
    if existing.left_at.is_none() {
        return Err(ApiError::conflict("您已是该房间成员"));
    }
    crate::infra::repo::restore_member_with_nickname(
        conn,
        &room.room_id,
        uid,
        &nickname,
    )?;
} else {
    crate::infra::repo::add_member(conn, &room.room_id, uid, role, &nickname)?;
}
```

The existing `invite.redeemed` audit call remains. For a removed rejoin, the consumed invite timestamp must be strictly later than `left_at`; otherwise the transaction returns 400 and the invite is not consumed. The existing join system message and sequence bump after `redeem_invite` still run.

In `remove_member` (the app workflow), replace its `repo::leave_room` call with the new `repo::remove_member` call. In `leave_member`, keep `repo::leave_room` so voluntary leave writes `'left'`. No API handler change is needed: all joins already call `join_member`, and the existing 403/400/409 contracts stay.

### 3. Frontend

No source change. After a kick, the target’s next room load gets 403 and shows the existing invite form; entering a new invite succeeds through `POST /join`. A pre-kick invite fails with `invite_invalid` and the form displays the error via the existing `join` catch. The target does not receive a dedicated “kicked” event in this design; no new endpoint or UI state is added.

## Critical files & anchors

- `backend/src/infra/db.rs::MIGRATIONS` — append the new nullable column migration; do not edit old migrations.
- `backend/src/infra/repo.rs` — member SELECT/row mapping, `leave_room`, new `remove_member`, `restore_member`, new nickname restore.
- `backend/src/app/rooms.rs::join_member` and `redeem_invite` — reason/timestamp branch and existing-member restore.
- `backend/src/app/rooms.rs::remove_member` — switch to the reason-setting repo helper.
- `backend/tests/e2e.rs::join_membership_paths` / `room_member_lifecycle_paths` — voluntary rejoin stays 200; add kicked-rejoin coverage.

## Verification

From `/home/hui/agents/omp/cloudroom/backend`:

```bash
cargo test --test e2e join_membership_paths
cargo test --test e2e room_member_lifecycle_paths
cargo test --test e2e kicked_rejoin_requires_new_invite
cargo test --all-targets
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
```

Add `kicked_rejoin_requires_new_invite` (new e2e):

1. Owner Alice and member Bob join a standard room; create an unused invite `old_invite` before the kick.
2. Alice removes Bob; assert Bob’s `GET /rooms/{code}` is 403.
3. Bob `POST /join` with `{}` returns 403 (invite required).
4. Bob `POST /join` with `old_invite` returns 400 `invite_invalid` (created before `left_at`).
5. Read Bob’s member row directly: `left_at.is_some()` and `left_reason == Some("removed")`.
6. Wait `5ms`, create a fresh invite as Alice, and Bob `POST /join` with it returns 200; `GET /rooms/{code}` is 200, `my_role == "member"`; member row now has `left_at == None`, `left_reason == None`.
7. Bob voluntarily leaves, then `POST /join` with `{}` returns 200 (voluntary rejoin unchanged); member row `left_reason` is `None` after restore.

Also confirm in `room_member_lifecycle_paths` that the removed member’s earlier GET remains 403 and no existing assertion expects a successful kicked rejoin. `join_membership_paths` must keep passing for voluntary leave/rejoin without an invite.

From `/home/hui/agents/omp/cloudroom`:

```bash
scripts/check.sh
```

Expected `16 PASS / 0 FAIL / 2 SKIP` (ESLint and cargo-audit remain skipped).

## Assumptions & contingencies

- Option A is selected; B is not implemented because there is no unban surface or ban state. Do not add `banned_at`.
- “New invite” is strict: `invite.created_at > left_at`. Pre-kick invites are rejected for the kicked user; other users may still use them. If execution finds `created_at`/`left_at` clock resolution causes equal timestamps in tests, wait 5ms before creating the fresh invite; do not weaken the comparison to `>=`.
- Legacy rows with `left_reason == NULL` are treated as voluntary legacy leaves, preserving pre-migration rejoin behavior; they cannot be distinguished from old kicks.
- The existing owner-removal rule and both-owner direct rooms are unchanged; direct-room kick remains unavailable.
- No invite revocation, user-specific invite binding, new API error field, or frontend contract is added.
- A failed/expired/old invite must not consume the invite or restore membership: invite validation and timestamp check happen before `consume_invite`, and the whole `join_member` call is already inside the DB SAVEPOINT.

## Rollback

Revert the implementation commit(s). The forward-only migration leaves a nullable `left_reason` column that the old code ignores; no destructive migration is required and existing rows are unchanged. Reverting the code restores the current empty-`POST /join` rejoin behavior for left rows, including kicked rows.
