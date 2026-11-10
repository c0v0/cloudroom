//! RFC 6238 TOTP (SHA-1, 30s, 6 digits), base32 secrets, recovery codes.
//! Uses ring's HMAC-SHA1 implementation; no third-party OTP dependency.

use ring::hmac;

const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
const STEP: u64 = 30;

fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for &b in bytes {
        buffer = (buffer << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 0x1f) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 0x1f) as usize] as char);
    }
    out
}

fn base32_decode(input: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for c in input.chars() {
        let c = c.to_ascii_uppercase();
        if c == '=' || c == ' ' || c == '-' {
            continue;
        }
        let v = ALPHABET
            .iter()
            .position(|&a| a as char == c)
            .ok_or_else(|| "invalid base32 secret".to_string())? as u32;
        buffer = (buffer << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    if out.is_empty() {
        return Err("empty base32 secret".into());
    }
    Ok(out)
}

/// 160-bit random secret, base32 without padding.
pub fn generate_secret() -> String {
    let mut bytes = [0u8; 20];
    use rand::RngCore;
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base32_encode(&bytes)
}

pub fn otpauth_url(account: &str, secret: &str) -> String {
    let label = format!("CloudRoom:{account}");
    let label = url::form_urlencoded::byte_serialize(label.as_bytes()).collect::<String>();
    format!(
        "otpauth://totp/{label}?secret={secret}&issuer=CloudRoom&algorithm=SHA1&digits=6&period=30"
    )
}

fn hotp(secret: &[u8], counter: u64) -> Result<u32, String> {
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let tag = hmac::sign(&key, &counter.to_be_bytes());
    let bytes = tag.as_ref();
    let offset = (bytes[19] & 0x0f) as usize;
    let bin = (u32::from(bytes[offset] & 0x7f) << 24)
        | (u32::from(bytes[offset + 1]) << 16)
        | (u32::from(bytes[offset + 2]) << 8)
        | u32::from(bytes[offset + 3]);
    Ok(bin % 1_000_000)
}

/// Current code for tests/CLI; production verifies with `verify`.
pub fn code_now(secret: &str) -> Result<String, String> {
    let raw = base32_decode(secret)?;
    let counter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs()
        / STEP;
    Ok(format!("{:06}", hotp(&raw, counter)?))
}

/// Verify a 6-digit code with a ±1 step window and return the matched step.
pub fn verify_step(secret: &str, code: &str, unix_time: u64) -> Option<u64> {
    let code = code.trim();
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let raw = base32_decode(secret).ok()?;
    let counter = unix_time / STEP;
    for delta in [-1i64, 0, 1] {
        let Some(c) = counter.checked_add_signed(delta) else {
            continue;
        };
        if let Ok(expected) = hotp(&raw, c) {
            if format!("{expected:06}") == code {
                return Some(c);
            }
        }
    }
    None
}

/// Verify a 6-digit code with a ±1 step window.
pub fn verify(secret: &str, code: &str, unix_time: u64) -> bool {
    verify_step(secret, code, unix_time).is_some()
}

/// Ten recovery codes (100-bit secret, displayed as four 5-char groups).
pub fn generate_recovery_codes() -> Vec<String> {
    use rand::RngCore;
    let mut out = Vec::with_capacity(10);
    for _ in 0..10 {
        let mut bytes = [0u8; 13];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let raw = base32_encode(&bytes);
        let code = format!(
            "{}-{}-{}-{}",
            &raw[..5],
            &raw[5..10],
            &raw[10..15],
            &raw[15..20]
        );
        out.push(code);
    }
    out
}

pub fn normalize_recovery_code(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

pub fn hash_recovery_code(code: &str) -> String {
    crate::util::sha256_hex(normalize_recovery_code(code).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6238_sha1_vectors() {
        // RFC 6238 test secret "12345678901234567890" (ASCII) base32.
        let secret = base32_encode(b"12345678901234567890");
        let raw = base32_decode(&secret).unwrap();
        assert_eq!(raw, b"12345678901234567890");
        assert_eq!(format!("{:06}", hotp(&raw, 59 / 30).unwrap()), "287082");
        assert_eq!(
            format!("{:06}", hotp(&raw, 1111111109 / 30).unwrap()),
            "081804"
        );
        assert_eq!(
            format!("{:06}", hotp(&raw, 2000000000 / 30).unwrap()),
            "279037"
        );
    }

    #[test]
    fn recovery_codes_are_ten_and_normalized() {
        let codes = generate_recovery_codes();
        assert_eq!(codes.len(), 10);
        assert!(codes.iter().all(|c| c.len() == 23 && c.matches('-').count() == 3));
        assert_eq!(normalize_recovery_code(" ab-cd "), "ABCD");
        assert_eq!(
            hash_recovery_code("ab-cd"),
            hash_recovery_code("ABCD")
        );
    }
}
