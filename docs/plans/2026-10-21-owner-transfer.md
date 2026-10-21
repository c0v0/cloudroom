# Owner transfer role consistency

## Context

`app::admin::transfer_owner` makes the target an owner and updates `rooms.owner_user_id`, but it never demotes the previous owner. Both membership rows keep `room_role='owner'`, while `owner_user_id` points only to the target. Authorization is based on the member role, so the previous owner retains all management powers. This plan makes transfer leave exactly one active owner and corrects historical mismatched rows; it does not redesign multi-owner rooms.

Execution also writes this plan to `docs/plans/2026-10-21-owner-transfer.md`.

## Current behavior

### Where each source of truth is used

`room_members.room_role` is the authorization source:

- `app::access::can` / `require_cap`: `RoomRole::Owner` grants every capability; members only `send_text` and `join_voice`.
- `api::rooms::capabilities` exposes the same capability list to the client.
- `get_room`: `is_owner` controls mutes/speak lists; `room.my_role` comes from the member role.
- `update_settings` (standard room): `manage_room`; direct room: explicit `room_role == Owner`.
- `delete_room`, `set_member_role`, invite create/list/revoke, member remove, nickname management, mute create/list/remove: owner-only checks via role or capability.
- `app::requests` change-media-mode request: requester must be `RoomRole::Owner`.
- `app::messages::send`: owner is exempt from mutes; `app::rooms::create_mute` rejects muting an owner.
- `api::ws.rs`: `RoomMuteToggle`, `SpeakRequest`, `SpeakReply`, `SpeakRevoke` and owner notification lists use `room_role == Owner`.

`rooms.owner_user_id` is used for:

- `app::rooms::join_member`: permits the recorded owner to join without an invite.
- `app::admin::transfer_owner`: updates the recorded owner.
- `api::admin::list_rooms`: displays `owner_username`; `api::json::room_json`: exposes `owner_user_id`.
- It is **not** consulted by room permission checks. Option B would have to replace every check above.

Direct rooms are a separate model: both members are owners, `owner_user_id` is NULL, and `transfer_owner` rejects non-standard rooms. Any correction must exclude `rooms.kind='direct'`.

### Transfer's current effect

`app::admin::transfer_owner`:

1. Loads a standard room and target user; rejects guests.
2. If target already has a membership row, sets its role to owner; otherwise inserts an owner member.
3. Calls `repo::set_room_owner` to set `owner_user_id`.
4. Audits `room.owner_transferred` with target_type `room`, target_id room_id, actor, and detail `{"new_owner": target, "room_code": code}`.
5. API broadcasts `{"type":"owner_transferred","owner_user_id":target}` to the room. No previous-owner ID or role events are sent.
6. Frontend has no `owner_transferred` case; role badges use `member.room_role` and `isOwner` uses `room.my_role`. Both users therefore display “房主” and have owner capabilities until a full reload.

The audit record does not currently store the previous owner.

### Current data

No `cloudroom.db` exists under the repository or the usual local data paths in this workspace. The only available database is the temporary upgrade-smoke DB at `/tmp/cloudroom-upgrade/data/cloudroom.db`; it has 0 active mismatch rows, but it is not a deployment snapshot and cannot be used as a production count.

Run against the deployment database (read-only) before changing data:

```sql
SELECT COUNT(*) AS owner_role_mismatches
FROM room_members m
JOIN rooms r ON r.room_id = m.room_id
WHERE m.room_role = 'owner'
  AND r.kind = 'standard'
  AND r.owner_user_id IS NOT NULL
  AND m.user_id <> r.owner_user_id;

-- Diagnostic: standard rooms with no canonical owner (not auto-fixed).
SELECT r.room_id, r.room_code
FROM rooms r
WHERE r.kind = 'standard' AND r.owner_user_id IS NULL;
```

The count includes left rows as well as active rows; left rows do not grant access, but are still inconsistent. If only active mismatches are wanted, add `AND m.left_at IS NULL`.

## Semantics decision

| Option | Meaning | Impact / cost | Frontend |
|---|---|---|---|
| **A. Demote previous owner** | One canonical owner; previous owner becomes a normal member. | Change one transfer workflow, add one repo demotion update, add one frontend event case; no schema change. Low. | Reload room on `owner_transferred`; existing UI then shows correct roles/capabilities. |
| B. Keep owner roles, authorize by `owner_user_id` | Multiple role-owner rows may remain, but only `owner_user_id` has management rights. | Must replace every role check listed above, including WS and direct-room multi-owner paths; high regression risk and confusing display. | Role badges must be derived from owner_user_id, not room_role. |
| C. Formal multi-owner | Multiple owners are a supported model. | Schema/API/UI/permission model changes, conflict with direct-room null owner semantics; highest. | Multi-owner management UI required. |

**Recommendation: A.** Standard rooms already have one canonical owner in `owner_user_id`; transfer should make that canonical owner the only owner role. A keeps every existing permission check intact, matches the current UI’s owner badge, and removes the inconsistency at its source. Direct rooms remain the existing explicit two-owner exception.

## Approach

### 1. Demote every other owner during transfer

File: `backend/src/app/admin.rs::transfer_owner`.

Inside the existing single `state.db.with` closure, before setting the target owner:

```rust
if crate::infra::repo::get_member(conn, &room.room_id, &target)?.is_some() {
    crate::infra::repo::demote_other_owners(conn, &room.room_id, &target)?;
    crate::infra::repo::set_member_role(conn, &room.room_id, &target, RoomRole::Owner)?;
} else {
    crate::infra::repo::demote_other_owners(conn, &room.room_id, &target)?;
    let nick = target_user
        .username
        .unwrap_or_else(|| format!("guest-{}", &target[..8]));
    crate::infra::repo::add_member(
        conn,
        &room.room_id,
        &target,
        RoomRole::Owner,
        &nick,
    )?;
}
crate::infra::repo::set_room_owner(conn, &room.room_id, &target)?;
```

File: `backend/src/infra/repo.rs`, new helper:

```rust
pub fn demote_other_owners(
    conn: &mut Connection,
    room_id: &str,
    keep_user_id: &str,
) -> ApiResult<()> {
    conn.execute(
        "UPDATE room_members SET room_role = 'member'
         WHERE room_id = ?1 AND room_role = 'owner' AND user_id <> ?2",
        params![room_id, keep_user_id],
    )
    .map_err(ApiError::from)?;
    Ok(())
}
```

- Update all owner rows except the target, including left/history rows, so role data matches the canonical room owner. This is a single UPDATE inside the existing transfer transaction; no new transaction boundary.
- Keep the existing target handling: an existing member becomes owner; a non-member is inserted as owner. Do not add an active-membership check or change guest/standard-room rules.
- Keep the current audit fields and broadcast unchanged: `room.owner_transferred`, detail `{"new_owner": target, "room_code": code}`. The previous owner can be derived from the room row in audit history if needed; no audit format change.
- The API still broadcasts `owner_transferred` with `owner_user_id: target`; no extra event is required because the frontend will reload.

### 2. Make the frontend react to ownership transfer

File: `frontend/src/pages/Room.tsx`, the `onSocketEvent` switch (near the existing `member_updated` case).

Add:

```ts
case "owner_transferred":
  void loadRoom();
  break;
```

`loadRoom` refetches `room`, `members`, `capabilities`, and `room.my_role`; both the previous owner and the new owner therefore update immediately without a page reload. Existing `member_updated` handling remains unchanged. No API/JSON shape changes.

Direct-room socket behavior is unchanged; transfer is rejected for direct rooms.

### 3. One-time historical data correction

Run the count query from “Current data” first. Do not edit the database in application code; use a manual backup table and a transaction so the correction is reversible. Exclude direct rooms and only correct standard rooms with a non-NULL canonical owner.

```sql
BEGIN IMMEDIATE;

CREATE TABLE room_members_owner_role_backup_20261021 AS
SELECT m.room_id, m.user_id, m.room_role
FROM room_members m
JOIN rooms r ON r.room_id = m.room_id
WHERE m.room_role = 'owner'
  AND r.kind = 'standard'
  AND r.owner_user_id IS NOT NULL
  AND m.user_id <> r.owner_user_id;

UPDATE room_members
SET room_role = 'member'
WHERE room_role = 'owner'
  AND EXISTS (
    SELECT 1
    FROM rooms r
    WHERE r.room_id = room_members.room_id
      AND r.kind = 'standard'
      AND r.owner_user_id IS NOT NULL
      AND r.owner_user_id <> room_members.user_id
  );

COMMIT;
```

After the update, rerun the count query; expected `0`. Also verify:

```sql
SELECT room_id, COUNT(*) AS owner_count
FROM room_members
WHERE room_role = 'owner' AND left_at IS NULL
  AND room_id IN (SELECT room_id FROM rooms WHERE kind = 'standard')
GROUP BY room_id
HAVING COUNT(*) <> 1;
```

Expected: no rows. Direct rooms are intentionally excluded and retain their two-owner rows.

Rollback (only before later role changes):

```sql
BEGIN IMMEDIATE;

UPDATE room_members
SET room_role = (
  SELECT b.room_role
  FROM room_members_owner_role_backup_20261021 b
  WHERE b.room_id = room_members.room_id
    AND b.user_id = room_members.user_id
)
WHERE EXISTS (
  SELECT 1 FROM room_members_owner_role_backup_20261021 b
  WHERE b.room_id = room_members.room_id
    AND b.user_id = room_members.user_id
);

DROP TABLE room_members_owner_role_backup_20261021;
COMMIT;
```

If the count is already 0, skip the update and backup/rollback entirely. If a standard room has `owner_user_id IS NULL`, do not guess; list it for manual handling and exclude it from the automatic update.

## Critical files & anchors

- `backend/src/app/admin.rs::transfer_owner` — demote others before assigning target owner; keep audit/broadcast.
- `backend/src/infra/repo.rs` — add `demote_other_owners`; reuse `set_member_role`, `add_member`, `set_room_owner`.
- `frontend/src/pages/Room.tsx` — add `owner_transferred` case that calls `loadRoom`.
- `backend/tests/e2e.rs::admin_reject_force_delete_and_transfer` — existing transfer assertions must change from two owners to one.
- `docs/debt.md` — currently records the previous-owner-remains-owner behavior; mark it resolved after verification.

## Verification

From `/home/hui/agents/omp/cloudroom/backend`:

```bash
cargo test --test e2e admin_reject_force_delete_and_transfer
cargo test --test e2e room_member_lifecycle_paths
cargo test --all-targets
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
```

Update `admin_reject_force_delete_and_transfer` after the fix:

- After transferring `transfer-room` to Carol: Carol GET is 200 with `my_role=="owner"` and `owner_user_id==carol_id`; Alice GET is 200 with `my_role=="member"`, not owner.
- Only one member in the room has `room_role=="owner"` (Carol).
- Alice’s management writes fail with 403, e.g. `POST /rooms/transfer-room/settings` with `{"text_enabled":false}`; Carol’s equivalent succeeds (200) or reaches the expected validation path.
- Repeating the transfer to Carol returns 200 and leaves exactly one owner; audit `room.owner_transferred` still has `actor_user_id`, `target_type=="room"`, `target_id==transfer_room_id`, `detail.new_owner==carol_id`, `detail.room_code=="transfer-room"`.
- Transfer to a non-member (a new user Dave) still adds Dave as owner and demotes Carol; verify one owner and previous owner member.

Add a focused e2e case if the existing test is already too long: `owner_transfer_demotes_previous` with owner/member/outsider; transfer to member, transfer back, transfer to outsider, assert one owner and old owner member each time.

Frontend verification:

```bash
cd /home/hui/agents/omp/cloudroom/frontend
npm run typecheck
```

Manual browser check (two sessions, owner and target): transfer via admin/API; both clients receive `owner_transferred`, call `loadRoom`, and roles/capabilities update without a full page reload. No component test framework exists; do not add one for this case.

Historical SQL verification against a copy of the deployment DB:

```bash
sqlite3 /path/to/cloudroom.db < count_query.sql
# apply backup/update/rollback exactly as above
sqlite3 /path/to/cloudroom.db < verification_query.sql
```

Expected: initial mismatch count is reported; after update it is 0; the owner-count verification returns no rows; rollback in a disposable copy restores the backup roles.

Full gate:

```bash
cd /home/hui/agents/omp/cloudroom
scripts/check.sh
```

Expected `16 PASS / 0 FAIL / 2 SKIP`.

## Assumptions & contingencies

- Option A is selected. B is rejected because every owner permission would need rewriting; C is rejected because direct rooms already have a separate two-owner model.
- Direct rooms keep multiple owner rows and NULL `owner_user_id`; the transfer helper and historical SQL both exclude `kind='direct'`.
- Historical standard rooms with `owner_user_id IS NULL` are not auto-corrected; they are reported for manual handling.
- No schema migration is needed; the fix changes existing rows and transfer behavior. The one-time SQL is deliberately manual because it needs a backup and a deployment-specific count.
- The existing audit fields and `owner_transferred` wire event remain unchanged; frontend reload uses the existing room GET endpoint.
- If a target user already has a left membership, current transfer behavior (role owner while left) is preserved; this plan does not add a target-membership check.

## Rollback

Revert the transfer/demotion code and the frontend case. For data, run the backup-table rollback SQL above before any later role changes. No schema or migration rollback is required.
