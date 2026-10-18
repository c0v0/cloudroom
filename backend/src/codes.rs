//! Room code generation and validation.
//!
//! Auto-generated codes: CSPRNG picks `generated_code_length` characters from
//! `generated_code_alphabet` (default lowercase Crockford Base32). Collisions
//! (live rooms or tombstones) retry. Settings changes only affect codes
//! generated after the change.
//!
//! Custom codes (super_admin only): normalized lowercase, must match
//! `custom_code_pattern`, must not be a reserved word, must not collide with a
//! live room or tombstone. Codes are locators, not credentials.

use rand::RngCore;

use crate::config::Config;

/// Generate a random code from the configured alphabet.
pub fn generate_code(cfg: &Config) -> String {
    let alphabet: Vec<char> = cfg.rooms.generated_code_alphabet.chars().collect();
    let n = alphabet.len();
    let len = cfg.rooms.generated_code_length;
    let mut out = String::with_capacity(len);
    let mut buf = [0u8; 32];
    for _ in 0..len {
        rand::rngs::OsRng.fill_bytes(&mut buf);
        // Rejection sampling is unnecessary here: we re-roll the whole code on
        // collision, and a modulo bias over 32+ symbols is cryptographically
        // irrelevant for a locator. Keep it simple.
        let idx = (u64::from_le_bytes(buf[0..8].try_into().unwrap()) % n as u64) as usize;
        out.push(alphabet[idx]);
    }
    out
}

/// Validate a custom code against pattern + reserved list. Returns normalized form.
pub fn validate_custom_code(cfg: &Config, input: &str) -> Result<String, String> {
    let normalized = crate::util::normalize_custom_code(input)
        .ok_or_else(|| "自定义房间号需为 3-32 位：a-z、0-9、'-'（不能以 '-' 开头）".to_string())?;
    let re = regex_lite::Regex::new(&cfg.rooms.custom_code_pattern)
        .map_err(|e| format!("invalid rooms.custom_code_pattern: {e}"))?;
    if !re.is_match(&normalized) {
        return Err(format!("自定义房间号需匹配 {}", cfg.rooms.custom_code_pattern));
    }
    if cfg.rooms.reserved_codes.iter().any(|r| r == &normalized) {
        return Err(format!("房间号 {normalized:?} 是保留词"));
    }
    Ok(normalized)
}

pub fn code_is_reserved(cfg: &Config, code: &str) -> bool {
    cfg.rooms.reserved_codes.iter().any(|r| r == code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::default()
    }

    #[test]
    fn generated_codes_have_configured_length_and_alphabet() {
        let c = cfg();
        for _ in 0..100 {
            let code = generate_code(&c);
            assert_eq!(code.len(), c.rooms.generated_code_length);
            assert!(code.chars().all(|ch| c.rooms.generated_code_alphabet.contains(ch)));
        }
    }

    #[test]
    fn custom_codes_accept_666_and_ccc() {
        let c = cfg();
        assert_eq!(validate_custom_code(&c, "666").unwrap(), "666");
        assert_eq!(validate_custom_code(&c, "CCC").unwrap(), "ccc");
        assert_eq!(validate_custom_code(&c, "my-room-2").unwrap(), "my-room-2");
    }

    #[test]
    fn custom_codes_reject_reserved_and_invalid() {
        let c = cfg();
        for bad in ["api", "admin", "register", "health", "ws", "-abc", "ab", "a_b", "中文房"] {
            assert!(validate_custom_code(&c, bad).is_err(), "{bad} should fail");
        }
    }
}
