# CloudRoom configuration reference

`cloudroom-server` reads one TOML file: `/etc/cloudroom/config.toml`, or
`./config.toml` relative to the working directory, or the path given with
`--config`. Every key has a default; only keys you need to change must appear.
Durations are humantime strings (`30d`, `1h`, `24h`, `60s`).

Example:

```toml
[server]
listen = "127.0.0.1:8080"
public_origin = "https://chat.example.com"
frontend_origin = "https://chat.example.com"
trusted_proxy = "127.0.0.1"
insecure_cookies = false

[data]
dir = "/var/lib/cloudroom"

[auth]
registration_mode = "invite_only"

[rooms]
dm_idle_timeout = "1h"
expired_cleanup_grace = "30d"
generated_code_length = 7
generated_code_alphabet = "0123456789abcdefghjkmnpqrstvwxyz"

[media]
livekit_url = "wss://lk.example.com"
livekit_api_key = "CHANGE_ME"
livekit_api_secret = "CHANGE_ME"
p2p_voice_max_participants = 4

[voice]
# 音频码率档位（kbps）及房主可设置该档所需的最低账号角色。
# 0（自动）恒可用。部署方可放宽（例如全部改为 "user"）。
bitrate_tiers = { 32 = "user", 64 = "user", 96 = "user", 128 = "user", 160 = "user", 192 = "admin", 256 = "super_admin" }
```

## `[server]`

| Key | Default | Meaning |
|---|---|---|
| `listen` | `127.0.0.1:8080` | Bind address. Use `"unix:/run/cloudroom/cloudroom.sock"` for a Unix socket. Never bind a public interface — TLS/WSS terminates at Nginx. |
| `public_origin` | — (required) | Origin browsers use, e.g. `https://chat.example.com`. Drives cookies, CORS, WS origin checks, bootstrap links. |
| `frontend_origin` | `http://127.0.0.1:5173` | Origin that serves the frontend. Equal to `public_origin` when Nginx hosts both (recommended). |
| `allowed_origins` | `[]` | Extra origins allowed for CORS / WebSocket / Origin checks when the frontend is on another domain. |
| `trusted_proxy` | `127.0.0.1` | Peer allowed to set `X-Forwarded-*` (the local Nginx). `null` disables forwarding headers. |
| `insecure_cookies` | `false` | Dev only: allow cookies without `Secure` and drop the `__Host-` prefix for plain-HTTP testing. MUST stay `false` in production. |
| `max_body_bytes` | `4194304` | Max request body (avatar uploads + JSON). |

## `[data]`

| Key | Default | Meaning |
|---|---|---|
| `dir` | `data` | Directory holding `cloudroom.db` (SQLite), `avatars/`, `instance.lock`. Production: `/var/lib/cloudroom`, owned by the service user, mode 0750. |

## `[auth]`

| Key | Default | Meaning |
|---|---|---|
| `session_ttl` | `30d` | Session cookie lifetime; sessions are server-side and revocable. |
| `bootstrap_ttl` | `24h` | Validity of the one-time first-run bootstrap link. |
| `registration_mode` | `invite_only` | `invite_only` (admins issue registration invites) or `open` (public registration — discouraged). |
| `argon2_memory_kib` | `19456` | Argon2id memory in KiB (OWASP baseline). |
| `argon2_iterations` | `2` | Argon2id iterations. |
| `argon2_parallelism` | `1` | Argon2id lanes. |
| `password_min_bytes` | `8` | Minimum password length in bytes. |
| `password_max_bytes` | `1024` | Maximum password length in bytes (never truncated silently). |

## `[rooms]`

| Key | Default | Meaning |
|---|---|---|
| `dm_idle_timeout` | `1h` | Direct room expiry: if either participant is continuously offline longer than this, the direct room is deleted (both participants are owners). Heartbeats refresh online state, so brief disconnects do not destroy rooms. |
| `expired_cleanup_grace` | `30d` | How long deleted/expired room data is retained (admin audit/restore window) before physical cleanup. Codes stay tombstoned forever. |
| `empty_room_policy` | `keep` | Must stay `keep`: standard rooms are never destroyed for being empty. Only owner deletion or `expires_at` terminates them. |
| `generated_code_length` | `7` | Length of auto-generated room codes. **Only affects codes generated after the change**; existing codes are stored verbatim. |
| `generated_code_alphabet` | `0123456789abcdefghjkmnpqrstvwxyz` | Literal character set for auto codes (lowercase Crockford Base32: no `i l o u`). **Only affects new rooms.** |
| `custom_code_pattern` | `^[a-z0-9][a-z0-9-]{2,31}$` | Regex for custom codes (super_admin only), matched after lowercasing. |
| `reserved_codes` | `["api","admin","register","health","ws","login","logout","me","rooms","room-requests","invites","assets","static","settings","favicon.ico"]` | Words custom codes may never use (frontend routes and infrastructure). |
| `invite_ttl` | `24h` | Default lifetime of room and registration invites. |
| `max_note_chars` | `500` | Room-request note limit. |
| `max_message_chars` | `4000` | Text message limit. |

## `[media]`

LiveKit is the single media infrastructure for both p2p rooms (embedded-TURN
relay credentials) and sfu rooms (JWTs); all three keys are required.

| Key | Default | Meaning |
|---|---|---|
| `livekit_url` | `""` (required) | Browser-reachable LiveKit WSS URL. Must start with `ws://` or `wss://`; production (`insecure_cookies=false`) requires `wss://`. Also supplies the host used for the p2p embedded-TURN URLs. |
| `livekit_api_key` | `""` (required) | LiveKit API key (must match `/etc/livekit.yaml`). Never sent to browsers. |
| `livekit_api_secret` | `""` (required) | LiveKit API secret (must match `/etc/livekit.yaml`). Never sent to browsers; the backend mints short-lived JWTs and TURN credentials. |
| `p2p_voice_max_participants` | `4` | Max participants in a P2P voice mesh. Above this, joining voice in a p2p room returns `sfu_required`; the room must request an sfu mode change through the approval channel. |

## `[voice]`

| Key | Default | Meaning |
|---|---|---|
| `bitrate_tiers` | `{32:"user",48:"user",64:"user",96:"user",128:"user",160:"admin",192:"admin",256:"super_admin",320:"super_admin",384:"super_admin",448:"super_admin",510:"super_admin"}` | Audio bitrate tiers (kbps) a room owner may select, keyed by the minimum account role required (`user`/`admin`/`super_admin`). `0` (automatic) is always available. Tiers apply to SFU rooms (P2P bitrate is decided by the browser and cannot be enforced). `510` is the Opus encoder hard limit; tiers above 256 are near-transparent for speech and reserved for super-admin owners. Deployments may relax any tier (e.g. set all to `"user"`). |

Role hierarchy: `user` < `admin` < `super_admin`. The role checked is the current room owner's account role, at room creation and at settings save. The frontend fetches the tier list from `GET /api/v1/voice/options` and filters by the logged-in user's role.

## `[security]`

| Key | Default | Meaning |
|---|---|---|
| `max_avatar_bytes` | `2097152` | Upload limit (2 MiB). |
| `max_avatar_dimension` | `1024` | Max avatar width/height in pixels. |
| `max_nickname_graphemes` | `32` | Per-room nickname limit. |
| `media_token_ttl` | `2h` | Lifetime of issued LiveKit JWTs. |
| `presence_heartbeat_interval` | `60s` | How often WS heartbeats persist online state (feeds `dm_idle_timeout`). |

## `[rate_limit]`

| Key | Default | Meaning |
|---|---|---|
| `login_per_minute_per_ip` | `10` | Login attempts per minute per IP. |
| `login_per_hour_per_account` | `30` | Login attempts per hour per account. |
| `register_per_hour_per_ip` | `5` | Registration attempts per hour per IP. |
| `invite_redeem_per_hour_per_ip` | `20` | Invite redemptions per hour per IP. |
| `room_request_per_hour_per_user` | `10` | Room requests per hour per user. |
| `message_per_minute_per_user` | `60` | Messages per minute per user. |
| `room_join_per_minute_per_user` | `20` | Room joins per minute per user (guards against high-frequency join/leave churn). |
| `room_leave_per_minute_per_user` | `20` | Room leaves per minute per user. |

## `[log]`

| Key | Default | Meaning |
|---|---|---|
| `level` | `info` | `tracing` filter (e.g. `info`, `debug`, `cloudroom_server=debug`). Logs never contain tokens, passwords, or WebRTC credentials. |

## Validation rules

Startup fails fast (with the reason) when: `generated_code_length` is outside
4–64; `generated_code_alphabet` has fewer than 16 chars or repeated chars;
`empty_room_policy != "keep"`; `registration_mode` is not
`invite_only`/`open`; `livekit_url`/`livekit_api_key`/`livekit_api_secret`
are missing; `livekit_url` is not `ws://`/`wss://`; production uses a
non-`wss://` URL; or `public_origin` is empty.
