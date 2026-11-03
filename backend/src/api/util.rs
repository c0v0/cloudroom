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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bidi_controls_are_rejected_from_identity_fields() {
        assert!(!validate_username("ali\u{202e}ce"));
        assert!(!validate_nickname("\u{2066}owner\u{2069}", 32));
        assert!(validate_nickname("Alice", 32));
    }
}

/// 消息/公告正文允许换行与制表符（Markdown 代码块需要），其余控制字符仍拒绝。
pub fn has_forbidden_controls(s: &str) -> bool {
    s.chars()
        .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
}
