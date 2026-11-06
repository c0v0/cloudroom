# CloudRoom deployment guide

CloudRoom is a self-hosted real-time room system: an independent
React/TypeScript frontend plus a single static Rust backend (`cloudroom-server`)
with SQLite storage, and LiveKit as the media plane. Public HTTPS/WSS is
terminated by Nginx; the backend only listens on loopback or a Unix socket and
is never exposed directly. LiveKit can run on the same host or on a separate
host/domain.

Rooms are created with an explicit media mode (`p2p` or `sfu`) chosen in the
room request; changing a room's mode later requires an admin-approved request
(the same channel as room creation). There is no coturn anywhere.

This guide walks a bare Linux x86_64 host to a working service.

## Components

| Component | Role | Ports (defaults) |
|---|---|---|
| Nginx | Public TLS termination; serves the frontend; proxies `/api/v1/` + WSS | 80/tcp, 443/tcp |
| `cloudroom-server` | Control plane (auth, rooms, chat, approvals, media credentials) | loopback 8080 only (never exposed) |
| `livekit-server` | Media plane: WebRTC signaling, UDP media, embedded TURN | 7881/tcp, 50000–50100/udp (signaling via Nginx WSS /rtc), 3478/udp, 50200–50300/udp |

LiveKit media is UDP and is never proxied by Nginx; browsers connect to
LiveKit directly.

## Prerequisites

- Linux x86_64 host, systemd, Nginx, DNS name(s) pointing at the host.
- Backend binary: `cloudroom-server` (static musl build, no runtime
  dependencies — see [Build](#build)).
- LiveKit binary: `livekit-server` (from the [LiveKit GitHub
  Releases](https://github.com/livekit/livekit/releases) `linux_amd64`
  tarball; version ≥ 1.5 recommended).
- Frontend build: `frontend/dist/` from `npm run build` in `frontend/`.

## 1. Users and directories

```bash
sudo useradd --system --home /var/lib/cloudroom --shell /usr/sbin/nologin cloudroom
sudo useradd --system --home /var/lib/livekit --shell /usr/sbin/nologin livekit
sudo install -d -o cloudroom -g cloudroom -m 0750 /var/lib/cloudroom   # data dir (SQLite, avatars)
sudo install -d -o cloudroom -g cloudroom -m 0750 /var/lib/cloudroom/avatars
sudo install -d -o root -g root -m 0755 /etc/cloudroom                 # config
sudo install -d -o cloudroom -g cloudroom -m 0750 /run/cloudroom       # runtime (sockets, pid)
sudo install -d -o root -g root -m 0755 /var/www/cloudroom             # frontend static files
```

Deploy artifacts:

```bash
sudo install -m 0750 cloudroom-server /usr/local/bin/cloudroom-server
sudo install -m 0755 livekit-server /usr/local/bin/livekit-server
sudo install -m 0640 release/nginx/cloudroom.conf.example /etc/nginx/sites-available/cloudroom.conf
# 模板已含 frame-src 'self' blob:（PDF 预览）；自定义 CSP 时需保留该指令。
sudo install -m 0640 release/systemd/cloudroom-server.service /etc/systemd/system/
sudo install -m 0640 -o root -g livekit release/livekit/livekit.yaml.example /etc/livekit.yaml
sudo install -m 0644 release/livekit/livekit-server.service.example /etc/systemd/system/livekit-server.service
sudo cp -r frontend/dist/* /var/www/cloudroom/
```

## 2. LiveKit configuration

Edit `/etc/livekit.yaml` (see `release/livekit/livekit.yaml.example`):

- Generate a key pair: `openssl rand -hex 32` for the API key and
  `openssl rand -hex 32` for the API secret; put them in the `keys:` section.
- Set the signaling domain and (optionally) TLS: either terminate TLS at
  LiveKit (`tls.cert_file`/`tls.key_file`) or put LiveKit behind its own
  Nginx/Caddy WSS reverse proxy. `media.livekit_url` in the CloudRoom config
  must match the browser-reachable WSS URL.
- The embedded TURN server is enabled by default (`turn.enabled: true`,
  UDP 3478, relay 50200–50300). This is the only TURN relay CloudRoom uses —
  both the p2p mesh and SFU clients rely on it. No coturn is installed.

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now livekit-server
sudo systemctl status livekit-server
```

Firewall: keep `7880/tcp` internal to Nginx/backend (not public); open `7881/tcp`, `50000–50100/udp`, `3478/udp`,
`50200–50300/udp` (or scope them to the LiveKit host when it is remote).
`/twirp` is server-only and the Nginx template returns 404 for it.

## 3. CloudRoom configuration

Copy the example (or the keys listed in `docs/configuration.md`) to
`/etc/cloudroom/config.toml` and set:

```toml
[server]
listen = "127.0.0.1:8080"          # or "unix:/run/cloudroom/cloudroom.sock"
public_origin = "https://chat.example.com"
frontend_origin = "https://chat.example.com"
cross_site_cookies = false         # true when the frontend is on another site
trusted_proxy = "127.0.0.1"
insecure_cookies = false            # MUST stay false in production

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
livekit_api_url = "http://127.0.0.1:7880"
livekit_api_key = "<same key as /etc/livekit.yaml>"
livekit_api_secret = "<same secret as /etc/livekit.yaml>"
p2p_voice_max_participants = 4
```

Permissions: `sudo chown root:cloudroom /etc/cloudroom/config.toml && sudo chmod 0640 /etc/cloudroom/config.toml`.

`livekit_url`, `livekit_api_key`, and `livekit_api_secret` are required at
startup; production must use `wss://`. `livekit_api_url` is the internal
server-to-server Twirp base (default `http://127.0.0.1:7880`), not the browser
URL. Keep `/twirp` internal and never proxy it through Nginx. LiveKit
key/secret never reach browsers — the backend mints short-lived JWTs
(`media-token`) and TURN credentials (`ice-servers`), and best-effort
participant/room revocation calls use a short-lived admin JWT.

Note: `generated_code_length` / `generated_code_alphabet` changes apply only to
codes generated after the change. Existing room codes are stored verbatim and
are never rewritten.

## 4. DNS and TLS (ACME)

```bash
# DNS: A record chat.example.com -> host IP (and AAAA if IPv6).
# If LiveKit is remote: A record lk.example.com -> LiveKit host IP.
sudo apt-get install -y nginx certbot python3-certbot-nginx
sudo certbot --nginx -d chat.example.com     # issues + configures TLS + auto-renewal
```

## 5. Nginx

Edit `/etc/nginx/sites-available/cloudroom.conf` (see the example template):
set `server_name`, the certificate paths (or keep the `certbot`-managed
include), and the upstream (`127.0.0.1:8080` or the Unix socket). Enable and
test:

```bash
sudo ln -s /etc/nginx/sites-available/cloudroom.conf /etc/nginx/sites-enabled/
sudo nginx -t
sudo systemctl reload nginx
```

The Nginx block must: redirect 80→443, serve the frontend from
`/var/www/cloudroom`, proxy `/api/v1/` with `proxy_http_version 1.1`,
`Upgrade`/`Connection` headers for WSS, long timeouts, `client_max_body_size
24m` (20 MiB media + multipart overhead), and pass `X-Forwarded-Host/Proto/For`. It must NOT proxy WebRTC UDP
media (P2P or LiveKit are reached by browsers directly on their own ports).
When LiveKit is behind its own reverse proxy, use the same Upgrade pattern
for `wss://lk.example.com`.

## 6. Service

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now cloudroom-server
sudo systemctl status cloudroom-server
journalctl -u cloudroom-server -f
```

The service unit has no `ExecReload`: the server handles SIGTERM only. Use
`sudo systemctl restart cloudroom-server` after config or binary changes.
(If a SIGHUP hot reload is added later, update the unit and this section.)

First start writes the one-time bootstrap URL to a 0600 file, never to the
journal:

```bash
sudo cat /var/lib/cloudroom/bootstrap-url
# https://chat.example.com/register?bootstrap=<token>
```

Open it, register — that account becomes `super_admin`. The raw token is only
in that root/service-readable file. If the link is lost before use, regenerate
on the host (it overwrites the file):

```bash
sudo -u cloudroom cloudroom-server --config /etc/cloudroom/config.toml --rotate-bootstrap
```

## 7. Verification

```bash
curl -fsS https://chat.example.com/api/v1/health          # {"status":"ok","checks":{"sqlite":"ok","livekit":"configured"}}
curl -fsS https://chat.example.com/api/v1/config/public    # registration mode, features (voice_modes: p2p/sfu)
```

Browser: register via the bootstrap link, request a room (choose `p2p` or
`sfu`), approve it in the **Admin** panel (`/admin`), open the room URL
(`https://chat.example.com/<code>`), send a message, join voice. In a p2p
room, the browser fetches short-lived LiveKit TURN credentials from
`/api/v1/rooms/{code}/ice-servers`; in an sfu room it fetches a LiveKit JWT
from `/api/v1/rooms/{code}/media-token`.

WebSocket upgrade check:

```bash
curl -fsS -N -H "Connection: Upgrade" -H "Upgrade: websocket" -H "Sec-WebSocket-Version: 13" \
  -H "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==" \
  https://chat.example.com/api/v1/health   # 426 expected (upgrade required) = proxy OK
```

To switch a room between p2p and sfu, the owner submits a
`change_media_mode` request from the room page; an admin approves it in the
Admin panel. Media mode cannot be changed through room settings.

## 8. Backups, upgrades, restore

Backups are online SQLite backups (safe while the service runs):

```bash
sudo -u cloudroom mkdir -p /var/lib/cloudroom/backups
sudo -u cloudroom sqlite3 /var/lib/cloudroom/cloudroom.db \
  ".backup '/var/lib/cloudroom/backups/cloudroom-$(date +%F).db'"
```

Encrypt the backups (restic/borg/age) — SQLite is not encrypted at rest.
Restore: stop the service, replace the db file, start it.

Upgrade:

```bash
sudo install -m 0750 new-cloudroom-server /usr/local/bin/cloudroom-server
sudo -u cloudroom sqlite3 /var/lib/cloudroom/cloudroom.db ".backup '/var/lib/cloudroom/backups/pre-upgrade.db'"
sudo systemctl restart cloudroom-server    # migrations run automatically; failure aborts startup
```

Migrations run in order and are recorded; a failed migration aborts startup
and leaves the old database untouched. This version's
`2026-08-08-0004-explicit-room-media-mode` migration converts legacy `auto`
rooms to `sfu` and adds the request kind/target/mode columns; old clients
that still send `media_mode` via settings or omit it from create requests
receive 422 and must be upgraded together.

## 9. Troubleshooting

| Symptom | Check |
|---|---|
| 502 from Nginx | `systemctl status cloudroom-server`; `journalctl -u cloudroom-server` |
| Secure cookie not set | `X-Forwarded-Proto: https` must reach the backend; `trusted_proxy` matches Nginx |
| WSS drops quickly | `proxy_read_timeout` / `proxy_send_timeout` at least 3600s in the Nginx block |
| Backend refuses to start | `[media]` keys missing or `livekit_url` not `wss://` in production |
| Voice fails on public internet | verify LiveKit ports (Nginx `/rtc`, 7881/tcp, 50000–50100/udp) and embedded TURN 3478/udp + 50200–50300/udp are open |
| `sfu_required` on voice join | room is p2p and above `p2p_voice_max_participants`; request an sfu mode change |
| `media_mode_mismatch` | calling `ice-servers` on an sfu room or `media-token` on a p2p room |
| Avatars rejected | only PNG/JPEG/WebP ≤ 2 MiB and ≤ 1024×1024 are accepted |
| Bootstrap link lost | `cloudroom-server --rotate-bootstrap` on the host (only before first registration) |

## Build

### Static binaries (musl, one command)

Prerequisites on Ubuntu/Debian:

```bash
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  musl-tools musl-dev cmake file
rustup target add x86_64-unknown-linux-musl
```

Build both static binaries and refresh the frontend assets:

```bash
./scripts/build-musl.sh
```

This is the release build. The `cargo build --release --target
x86_64-unknown-linux-musl` command shown in README's development section is a
local development build only: it produces a non-PIE static binary with a
different hash. Do not publish that binary; use the wrapper above.

Build only one binary:

```bash
./scripts/build-musl.sh backend
./scripts/build-musl.sh frontend-server
```

The wrapper sets `CLOUDROOM_TARGET`, the musl C compiler/linker, CMake compiler,
`AWS_LC_SYS_PREBUILT_NASM=1`, `RUSTFLAGS=-C target-feature=+crt-static`, and
single-job build settings. `clang` and `nasm` are not required for this
dependency set; the AWS-LC bindings and NASM objects are prebuilt.

Verify:

```bash
file release/cloudroom-server release/cloudroom-frontend-server
readelf -d release/cloudroom-server | grep NEEDED       # no output
readelf -l release/cloudroom-server | grep INTERP       # no output
ldd release/cloudroom-server                            # statically linked
```

Frontend:

```bash
cd frontend && npm ci && npm run build   # output: frontend/dist/
```

## Backups (restic, encrypted + retention)

Back up the SQLite database, attachments/avatars, and deployment secrets:

```bash
export RESTIC_REPOSITORY=/backup/cloudroom   # or s3:...
export RESTIC_PASSWORD_FILE=/root/.restic-pass
restic init                                   # once

restic backup \
  /var/lib/cloudroom/cloudroom.db* \
  /var/lib/cloudroom/attachments /var/lib/cloudroom/avatars \
  /etc/cloudroom /etc/livekit.yaml

restic forget --prune --keep-daily 7 --keep-weekly 4 --keep-monthly 6
restic check
```

- Keep `RESTIC_PASSWORD_FILE` offline; losing it makes backups unrecoverable.
- Test restore quarterly into a temp dir before relying on it.
- SQLite WAL: stop the service (or use `sqlite3 .backup`) for a consistent snapshot;
  copying `-wal`/`-shm` alongside is also acceptable.
- Backups contain password hashes and LiveKit/VAPID keys — encrypt and restrict access.
