# CloudRoom security model

This document states the threat model, the boundaries, and what each control
protects. Version 1 provides transport encryption only: HTTPS/WSS and WebRTC
DTLS-SRTP. There is **no E2EE** — the server stores text history and can read
it under authorization. Protect `data/` and backups with filesystem
permissions, host disk encryption, and encrypted backups.

## Boundary

```
Browser ── HTTPS/WSS (TLS) ──> Nginx ── HTTP/WS (loopback) ──> cloudroom-server ──> SQLite
Browser ── DTLS-SRTP (WebRTC) ──> peer (P2P) or LiveKit (SFU)
Browser ── LiveKit WSS/UDP/TURN (media plane) ──> LiveKit
```

- Nginx terminates public TLS; the backend listens only on loopback/Unix
  socket and trusts forwarded headers only from the configured
  `trusted_proxy`. If Nginx and backend ever run on different hosts, that
  hop must become HTTPS or mTLS — never plaintext on an untrusted network.
- Nginx proxies only HTTP/WSS control plane. WebRTC UDP media (P2P or
  LiveKit) is never proxied by Nginx.
- LiveKit is the single media infrastructure: p2p rooms reuse its embedded
  TURN (short-lived credentials minted by the backend), sfu rooms use
  short-lived JWTs. There is no independent coturn; no coturn shared secret
  exists anywhere in the system.
- Room codes (`https://example.com/aaaa8888`) are **locators, not
  credentials**. Authentication, membership checks, invite validation, rate
  limiting, and enumeration resistance are enforced regardless of the code.
  A code stays occupied while its room row exists (including the
  `expired_cleanup_grace` retention window); after the row is purged the code
  becomes allocatable again, so a stale link may later resolve to a different
  room — membership and invites are still enforced per `room_id`.

## Authentication and sessions

- Passwords: Argon2id PHC hashes, baseline `m=19456 KiB, t=2, p=1` (OWASP
  Password Storage Cheat Sheet). Unicode passwords are never truncated or
  silently trimmed; login is rate limited per IP and per account.
- Sessions: server-side revocable opaque cookie
  (`__Host-cloudroom_session`; `Secure; HttpOnly; SameSite=Lax`). Logout and
  admin disable revoke sessions immediately; password change revokes all
  other sessions but keeps the current session, and both disable/password
  change schedule LiveKit participant removal for current rooms. No long tokens in URL or
  localStorage; bootstrap/invite tokens are single-use, hashed at rest, and
  stripped from the address bar after use.
- CSRF: every cookie-authenticated state change requires the session-bound
  `X-CSRF-Token`; `Origin` and `Sec-Fetch-Site` are checked; GET requests
  never change room/business state (the only exception is sliding session
  renewal, which updates `last_seen_at`/`expires_at`).
- Bootstrap: first-run link is written once to a 0600 file, hashed in
  the DB, valid 24 h, and atomically consumed by the first registration
  (that account becomes `super_admin`). Only a local `--rotate-bootstrap`
  can mint a new one, and only while no super_admin exists.
- Rate limiting covers login, registration, invite redemption, room
  requests, and messages.

## Authorization

- Global roles: `super_admin`, `admin`, `user`, `guest`. Only
  `super_admin` grants/revokes admin. Admins see global metadata and audit,
  not room contents (unless room roles grant access). Password reset is
  restricted to `super_admin` because it is equivalent to account takeover.
- Room roles: `owner` and `member`, mapped to capabilities
  (`manage_room`, `manage_invites`, `manage_nicknames`, `mute_member`,
  `mute_room`, `delete_room`, `send_text`, `join_voice`). The UI hides
  nothing critical: every action is re-checked server-side.
- Room creation is approval-gated: registered users submit requests with an
  explicit `media_mode` (`p2p` or `sfu`); an admin approves (requester
  becomes owner). Custom short codes are super_admin-only and validated
  against a reserved-word list.
- **Media mode changes use the same approval channel.** Only the standard
  room owner may submit `change_media_mode`; the requester cannot approve
  their own change request; approval re-validates that the room is active
  and the requester is still the owner. Direct rooms inherit the parent
  room's mode and cannot be changed separately. `settings` no longer accepts
  `media_mode`, so a client cannot bypass the approval flow.
- The admin panel (`/admin`) is invisible to non-admins and enforced by the
  router and by every admin API (403 otherwise).
- Direct rooms: both participants are owners; either may delete. The room
  expires when either participant is offline past `dm_idle_timeout`
  (heartbeat-based — brief disconnects do not destroy rooms).

## Data and lifecycle

- Standard rooms are never destroyed for being empty. Only owner
  `delete_room` or `expires_at` terminates; deleted/expired rooms stay
  readable by admins for `expired_cleanup_grace` (30 d), then rows are
  physically removed. Codes become allocatable again after the purge.
- `text_enabled=false` rejects new messages server-side (409
  `text_disabled`); history is retained and reappears when reopened.
- Text mutes (member and whole-room) are enforced server-side; the
  whole-room mute exempts the owner. Voice muting is cooperative: P2P mutes
  only constrain honest clients, and the SFU path mutes the local track.
- Avatar uploads: size limit (2 MiB), real magic-byte sniffing, decode with
  the `image` crate (PNG/JPEG/WebP), dimension cap 1024×1024, metadata
  stripped, re-encoded, stored outside the webroot with random names, and
  served through an authenticated handler with `nosniff`. SVG and
  script-bearing files are rejected.
- Input validation: schema/length/enum checks on all endpoints, parameterized
  SQL everywhere, plain-text rendering with HTML escaping in the frontend
  (no `dangerouslySetInnerHTML`, no `eval`, no arbitrary HTML/Markdown), no
  shell execution with user input, strict WebSocket event schemas.

## Media plane (LiveKit)

- The backend mints only short-lived credentials after room authorization:
  - sfu rooms: HS256 JWT at `POST /api/v1/rooms/{code}/media-token`
    (`media_token_ttl`), grant scoped to the room and participant, and
    `canPublishSources: ["microphone", "screen_share"]` — camera and data
    publishing are NOT granted; screen sharing is SFU-only.
  - p2p rooms: LiveKit-format TURN credentials at
    `GET /api/v1/rooms/{code}/ice-servers` (TTL 300 s) for LiveKit's
    embedded TURN; no coturn shared secret exists.
- Endpoints are mode-gated: `ice-servers` only for p2p rooms,
  `media-token` only for sfu rooms; mismatches return
  `409 media_mode_mismatch` and never leak whether a room exists otherwise.
- Signaling for p2p runs over authenticated WSS with room authorization;
  SDP/ICE is relayed only between current members (RFC 8827 threat model:
  media is DTLS-SRTP protected end to end between browsers). SFU signaling
  happens directly with LiveKit over WSS.
- The LiveKit API key/secret live only in `/etc/livekit.yaml` and the
  server-side `config.toml`; browsers never receive them. Production
  requires `wss://` (`media.livekit_url`) so signaling is TLS-protected.
- Kicks, leaves, account disable, password reset, room deletion, and expiry
  schedule a best-effort `RemoveParticipant`/`DeleteRoom` through the internal
  Twirp API (`media.livekit_api_url`, loopback/Compose DNS). The request uses
  a 60-second admin JWT; `/twirp` is never proxied publicly. DB/HTTP success
  does not depend on LiveKit, and the media-token TTL remains the fallback.
- The p2p mesh is capped (`p2p_voice_max_participants=4`); above it the
  server returns `sfu_required` and the room must request an sfu mode
  change through the approval channel rather than silently degrading.

## Server-side protection

- Structured logs are redacted: raw tokens, passwords, and WebRTC
  credentials never appear.
- Nginx adds HSTS, CSP, `X-Content-Type-Options: nosniff`,
  `Referrer-Policy: no-referrer`; the backend rejects cross-origin requests
  unless the origin is allowlisted.
- SQLite: single writer enforced by a file lock (a second instance refuses
  to start), WAL, foreign keys, parameterized queries, migrations recorded
  and abort-on-failure, online consistent backups for upgrades.
- Health checks distinguish SQLite from the LiveKit configuration state —
  an external outage is never disguised as an auth or permission error.

## What version 1 does not promise

- Media encryption is opt-in per SFU room and uses a server-generated key handed to
  members with the media token: it protects media from the LiveKit node and the
  network, not from the server operator, who can still read text history and
  audit events.
- No SQLite file-level encryption (protect via permissions/disk
  encryption/encrypted backups; SQLCipher-style encryption is a later
  evaluation).
- Voice moderation is cooperative on the p2p path (mutes stop local tracks
  on honest clients); authoritative media muting applies to the SFU path.
- Screen sharing is implemented for SFU rooms (`features.screen_share=true`);
  the P2P path still has no video track. Multimedia file playback is a later
  phase (`features.media_playback=false`): it will use object storage with
  signed URLs and play-state sync (LiveKit data message / WSS), never large
  P2P DataChannel transfers.
