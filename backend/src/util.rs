//! Small shared helpers: time, randomness, hashing, text validation.

use base64::Engine;
use chrono::{DateTime, Utc};
use rand::RngCore;
use sha2::{Digest, Sha256};

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

pub fn now() -> DateTime<Utc> {
    Utc::now()
}

/// 128-bit random hex identifier (opaque, non-enumerable).
pub fn random_id() -> String {
    let mut b = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    hex(&b)
}

/// 256-bit random raw token, base64url without padding (for URLs).
pub fn random_token() -> String {
    let mut b = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut b);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex(&h.finalize())
}

/// Normalize + validate a room code (custom codes). Returns lowercase form.
pub fn normalize_custom_code(input: &str) -> Option<String> {
    let s = input.trim().to_lowercase();
    if s.is_empty() || s.len() > 32 {
        return None;
    }
    let mut chars = s.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphanumeric() {
        return None;
    }
    let rest_ok = s
        .chars()
        .skip(1)
        .all(|c| c.is_ascii_alphanumeric() || c == '-');
    if !rest_ok || s.len() < 3 {
        return None;
    }
    Some(s)
}

/// Count Unicode scalar values (not graphemes, but a safe upper bound for
/// nickname limits; grapheme clusters are <= scalar count).
pub fn count_scalars(s: &str) -> usize {
    s.chars().count()
}

/// Reject control characters (except none — full strip for display safety).
pub fn has_control_chars(s: &str) -> bool {
    s.chars().any(|c| c.is_control())
}

/// Reject Unicode bidi overrides/embeddings that can make labels render as a
/// different identity. Zero-width joiners used by emoji are intentionally
/// allowed.
pub fn has_bidi_controls(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(
            c,
            '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
        )
    })
}

/// Validate a username: 3..=32 Unicode scalars, no control chars, no leading/trailing whitespace.
pub fn validate_username(s: &str) -> bool {
    if s.trim() != s || s.is_empty() {
        return false;
    }
    let n = count_scalars(s);
    (3..=32).contains(&n) && !has_control_chars(s) && !has_bidi_controls(s)
}

/// Validate a nickname: 1..=32 scalars, no control chars.
pub fn validate_nickname(s: &str, max: usize) -> bool {
    if s.trim().is_empty() || has_control_chars(s) || has_bidi_controls(s) {
        return false;
    }
    count_scalars(s) <= max
}

/// 消息/公告正文允许换行与制表符（Markdown 代码块需要），其余控制字符仍拒绝。
pub fn has_forbidden_controls(s: &str) -> bool {
    s.chars()
        .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
}
/// 安全取前 8 个字符作为显示用短 id（不按字节切，短 id 原样返回）。
pub fn short_id(s: &str) -> &str {
    s.get(..8).unwrap_or(s)
}

/// True only for globally routable unicast addresses. Used to keep
/// server-initiated requests (Web Push) away from loopback, private,
/// link-local and other reserved ranges.
pub fn is_public_ip(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || o[0] == 0
                || (o[0] == 100 && (64..=127).contains(&o[1]))
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || (o[0] == 192 && o[1] == 0 && o[2] == 2)
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
                || (o[0] == 198 && o[1] == 51 && o[2] == 100)
                || (o[0] == 203 && o[1] == 0 && o[2] == 113)
                || (o[0] == 192 && o[1] == 88 && o[2] == 99)
                // 240.0.0.0/4 is reserved; never a valid public target.
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(v4.into());
            }
            let seg = v6.segments();
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || seg[0] & 0xfe00 == 0xfc00
                || seg[0] & 0xffc0 == 0xfe80
                // Deprecated IPv6 site-local.
                || seg[0] & 0xffc0 == 0xfec0
                // IPv4-compatible (deprecated) and NAT64 well-known/local-use.
                || seg[..6].iter().all(|s| *s == 0)
                || (seg[0] == 0x0064 && seg[1] == 0xff9b)
                // Documentation range.
                || (seg[0] == 0x2001 && seg[1] == 0x0db8))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bidi_controls_are_rejected_from_identity_fields() {
        assert!(!validate_username("ali\u{202e}ce"));
        assert!(!validate_nickname("\u{2066}owner\u{2069}", 32));
        assert!(validate_nickname("Alice", 32));
    }

    #[test]
    fn message_body_allows_newlines_but_not_other_controls() {
        assert!(!has_forbidden_controls("line1\nline2\tcode"));
        assert!(has_forbidden_controls("bell\u{7}"));
    }

    #[test]
    fn public_ip_rejects_nat64_and_reserved_ranges() {
        for ip in [
            "64:ff9b::1",
            "64:ff9b:1::1",
            "2001:db8::1",
            "fec0::1",
            "240.0.0.1",
            "255.255.255.255",
            "10.0.0.1",
            "127.0.0.1",
            "::1",
        ] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(is_public_ip("8.8.8.8".parse().unwrap()));
        assert!(is_public_ip("2606:4700:4700::1111".parse().unwrap()));
    }
}
