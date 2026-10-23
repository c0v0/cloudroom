//! LiveKit adapter: the single media integration for both room modes.
//!
//! - `livekit_token` issues short-lived room JWTs (HS256) for sfu rooms after
//!   room authorization. The API secret never reaches the browser.
//! - `livekit_turn_credentials` mints short-lived LiveKit-format TURN
//!   credentials for p2p rooms, so the P2P mesh reuses LiveKit's embedded
//!   TURN server (no independent coturn).

use std::sync::OnceLock;
use std::time::Duration;

use base64::Engine;
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::{Digest as _, Sha256};

use crate::config::Config;
use crate::error::{ApiError, ApiResult};

type HmacSha256 = Hmac<Sha256>;

fn b64url(data: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

fn sign(secret: &str, data: &str) -> ApiResult<String> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .map_err(|e| ApiError::internal(format!("媒体令牌签名初始化失败: {e}")))?;
    mac.update(data.as_bytes());
    Ok(b64url(&mac.finalize().into_bytes()))
}

#[derive(Debug)]
pub enum RevocationError {
    /// Network, timeout, 5xx, or other condition worth retrying.
    Transient(String),
    /// The request was rejected in a way retries cannot fix.
    Permanent(String),
}

fn livekit_api_base(cfg: &Config) -> &str {
    cfg.media.livekit_api_url.trim_end_matches('/')
}

fn http_client() -> Option<&'static reqwest::Client> {
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(2))
                .build()
                .ok()
        })
        .as_ref()
}

/// Server-to-server admin JWT for LiveKit RoomService. Keep exp short: it is
/// minted only for one revocation request and never sent to browsers.
fn livekit_admin_token(cfg: &Config, room_name: &str) -> ApiResult<String> {
    let now = chrono::Utc::now().timestamp();
    let header = json!({ "alg": "HS256", "typ": "JWT" });
    let payload = json!({
        "iss": cfg.media.livekit_api_key,
        "sub": cfg.media.livekit_api_key,
        "nbf": now - 10,
        "exp": now + 60,
        "video": {
            "room": room_name,
            "roomAdmin": true,
            // DeleteRoom requires roomCreate; RemoveParticipant needs roomAdmin.
            "roomCreate": true,
        },
    });
    let h = b64url(header.to_string().as_bytes());
    let p = b64url(payload.to_string().as_bytes());
    let signing = format!("{h}.{p}");
    let sig = sign(&cfg.media.livekit_api_secret, &signing)?;
    Ok(format!("{signing}.{sig}"))
}

async fn post_room_service(
    cfg: &Config,
    method: &str,
    room_name: &str,
    body: serde_json::Value,
) -> Result<(), RevocationError> {
    let token = livekit_admin_token(cfg, room_name)
        .map_err(|e| RevocationError::Permanent(format!("admin token: {}", e.message)))?;
    let Some(client) = http_client() else {
        return Err(RevocationError::Permanent(
            "LiveKit HTTP client initialization failed".into(),
        ));
    };
    let url = format!(
        "{}/twirp/livekit.RoomService/{method}",
        livekit_api_base(cfg)
    );
    let response = client
        .post(url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|e| RevocationError::Transient(format!("transport: {e}")))?;
    let status = response.status();
    if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
        return Ok(());
    }
    let message = format!("HTTP {}", status.as_u16());
    if status.is_server_error()
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        Err(RevocationError::Transient(message))
    } else {
        Err(RevocationError::Permanent(message))
    }
}

/// Remove one participant from a room. A 404 means the room/participant is
/// already gone and is treated as success.
pub async fn remove_participant(
    cfg: &Config,
    room_name: &str,
    identity: &str,
) -> Result<(), RevocationError> {
    post_room_service(
        cfg,
        "RemoveParticipant",
        room_name,
        json!({ "room": room_name, "identity": identity }),
    )
    .await
}

/// Delete a room, disconnecting all participants. A 404 is success.
pub async fn delete_room(cfg: &Config, room_name: &str) -> Result<(), RevocationError> {
    post_room_service(cfg, "DeleteRoom", room_name, json!({ "room": room_name })).await
}

/// LiveKit JWT per https://docs.livekit.io/home/get-started/authentication/
/// Claims: iss/sub = api_key, exp, nbf, video { room, roomJoin, participant,
/// canPublish, canSubscribe }. Publishing is restricted to the microphone so
/// this voice-only version cannot publish camera, screen-share or data.
pub fn livekit_token(cfg: &Config, room_name: &str, participant: &str) -> ApiResult<String> {
    let now = chrono::Utc::now().timestamp();
    let ttl = cfg.media_token_ttl().as_secs() as i64;
    let header = json!({ "alg": "HS256", "typ": "JWT" });
    let payload = json!({
        "iss": cfg.media.livekit_api_key,
        "sub": participant, // LiveKit 以 sub 作为 participant identity（必须唯一）
        "nbf": now - 10,
        "exp": now + ttl,
        "video": {
            "room": room_name,
            "roomJoin": true,
            "participant": participant,
            "canPublish": true,
            "canSubscribe": true,
            "canPublishSources": ["microphone"],
        },
    });
    let h = b64url(header.to_string().as_bytes());
    let p = b64url(payload.to_string().as_bytes());
    let signing = format!("{h}.{p}");
    let sig = sign(&cfg.media.livekit_api_secret, &signing)?;
    Ok(format!("{signing}.{sig}"))
}

/// jxskiss/base62 v1.1.0 V2 encoding (bit-stream, reversed, with the 0x1E
/// 5-bit compaction) — byte-for-byte compatible with LiveKit's TURN
/// username/credential format.
fn b62_encode(src: &[u8]) -> String {
    if src.is_empty() {
        return String::new();
    }
    const TABLE: &[u8; 62] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    const COMPACT_MASK: u8 = 0x1E;
    const MASK5: u8 = 0x1F;
    let mut out = Vec::with_capacity(src.len() * 9 / 5 + 1);
    let mut pos: isize = (src.len() * 8) as isize;
    loop {
        if pos - 6 <= 0 {
            // final partial group: take the remaining `pos` bits at the head
            let blen = pos as u8;
            let shift = 8 - blen;
            let mut b = (src[0] >> shift) & ((1u16 << blen) - 1) as u8;
            if b & COMPACT_MASK == COMPACT_MASK {
                b &= MASK5;
            }
            out.push(TABLE[b as usize]);
            break;
        }
        let p = pos - 6;
        let i = (p / 8) as usize;
        let j = (p % 8) as u8;
        let mut blen = (((i + 1) * 8) as isize - p) as u8;
        if blen > 6 {
            blen = 6;
        }
        let shift = 8 - j - blen;
        let mut b = (src[i] >> shift) & ((1u16 << blen) - 1) as u8;
        if blen < 6 && p > 0 {
            let blen1 = 6 - blen;
            b = (b << blen1) | (src[i + 1] >> (8 - blen1));
        }
        let has_more = p > 0;
        let mut p2 = p;
        if b & COMPACT_MASK == COMPACT_MASK {
            if has_more || b > MASK5 {
                p2 += 1;
            }
            b &= MASK5;
        }
        out.push(TABLE[b as usize]);
        if !has_more {
            break;
        }
        pos = p2;
    }
    String::from_utf8(out).unwrap()
}

/// LiveKit-format TURN credentials for the P2P mesh.
///
/// LiveKit's embedded TURN server (same machine, UDP 3478) accepts the same
/// generic TURN protocol as coturn; its auth handler validates apiKey/expiry/
/// MAC only — the participant id inside the username is NOT checked against
/// any room, so the backend can mint credentials for P2P peers with a random
/// id. This lets a LiveKit deployment serve TURN for BOTH the SFU path and
/// the P2P mesh, making an independent coturn unnecessary.
pub fn livekit_turn_credentials(cfg: &Config) -> ApiResult<serde_json::Value> {
    let now = chrono::Utc::now().timestamp();
    let ttl: i64 = 300; // LiveKit default TURN TTL
    let expiry = now + ttl;
    let pid = crate::util::random_token();
    let username_raw = format!("{}|{}|{}", cfg.media.livekit_api_key, pid, expiry);
    let username = b62_encode(username_raw.as_bytes());
    let mut h = Sha256::new();
    h.update(format!("{}|{}|{}", cfg.media.livekit_api_secret, pid, expiry).as_bytes());
    let credential = b62_encode(&h.finalize());
    // TURN host = host part of livekit_url (browser-reachable); embedded TURN
    // listens on UDP 3478. TLS TURN (5349) is optional in production.
    let host = cfg
        .media
        .livekit_url
        .split("://")
        .nth(1)
        .unwrap_or(&cfg.media.livekit_url)
        .split([':', '/'])
        .next()
        .unwrap_or("localhost")
        .to_string();
    let urls = vec![format!("turn:{host}:3478?transport=udp")];
    Ok(json!({
        "iceServers": [{
            "urls": urls,
            "username": username,
            "credential": credential,
        }],
        "ttl": ttl,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        let mut c = Config::default();
        c.media.livekit_url = "wss://lk.example.com".into();
        c.media.livekit_api_key = "api-key".into();
        c.media.livekit_api_secret = "api-secret".into();
        c
    }

    #[test]
    fn livekit_admin_token_has_room_admin_claim() {
        let c = cfg();
        let token = livekit_admin_token(&c, "abc1234").unwrap();
        let payload = token.split('.').nth(1).unwrap();
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(v["iss"], c.media.livekit_api_key);
        assert_eq!(v["sub"], c.media.livekit_api_key);
        assert_eq!(v["video"]["room"], "abc1234");
        assert_eq!(v["video"]["roomAdmin"], true);
        assert_eq!(v["video"]["roomCreate"], true);
        assert!(v["exp"].as_i64().unwrap() > v["nbf"].as_i64().unwrap());
        assert!(!token.contains(&c.media.livekit_api_secret));
    }

    #[test]
    fn token_is_three_part_jwt() {
        let t = livekit_token(&cfg(), "abc1234", "user-1").unwrap();
        assert_eq!(t.split('.').count(), 3);
    }

    #[test]
    fn token_claims_allow_microphone_only() {
        let t = livekit_token(&cfg(), "abc1234", "user-1").unwrap();
        let payload = t.split('.').nth(1).unwrap();
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload)
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(v["video"]["room"], "abc1234");
        assert_eq!(v["sub"], "user-1");
        assert_eq!(v["video"]["canPublish"], true);
        assert_eq!(v["video"]["canSubscribe"], true);
        assert_eq!(v["video"]["canPublishSources"][0], "microphone");
    }

    #[test]
    fn turn_credentials_are_short_lived_livekit_format() {
        let a = livekit_turn_credentials(&cfg()).unwrap();
        let b = livekit_turn_credentials(&cfg()).unwrap();
        assert_ne!(
            a["iceServers"][0]["username"],
            b["iceServers"][0]["username"]
        );
        let urls = a["iceServers"][0]["urls"].as_array().unwrap();
        assert!(urls
            .iter()
            .any(|u| u.as_str().unwrap().contains("turn:lk.example.com:3478")));
        assert_eq!(a["ttl"], 300);
    }

    #[test]
    fn b62_matches_jxskiss() {
        // Vector from github.com/jxskiss/base62 v1.1.0 (Go) for this exact input.
        assert_eq!(
            b62_encode(b"devkey|test-participant-001|2000000000"),
            "wADMwADMwADMywXMwATL05WYwl2YpRnchBXL0NXZ0xXeKbN7KjM"
        );
        assert_eq!(b62_encode(b""), "");
        assert_eq!(b62_encode(b"a"), "hB");
    }
}
