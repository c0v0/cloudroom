//! Room code generation and validation.
//!
//! Auto-generated codes: CSPRNG picks `generated_code_length` characters from
//! `generated_code_alphabet` (default lowercase Crockford Base32). Collisions
//! with a currently existing room retry. Settings changes only affect codes
//! generated after the change.
//!
//! Custom codes (super_admin only): normalized lowercase, must match
//! `custom_code_pattern`, must not be a reserved word, must not collide with an
//! existing room row. Codes are locators, not credentials; a code freed by
//! deletion becomes allocatable again once the room row is purged.

use rand::RngCore;

use crate::config::Config;

/// Generate a random code from the configured alphabet.
pub fn generate_code(cfg: &Config) -> String {
    let alphabet: Vec<char> = cfg.rooms.generated_code_alphabet.chars().collect();
    let n = alphabet.len();
    let len = cfg.rooms.generated_code_length;
    let mut out = String::with_capacity(len);
    if n == 0 || len == 0 {
        return String::new();
    }
    // 一次系统调用取够所有随机字节；每 8 字节取一个索引。
    let mut buf = vec![0u8; len * 8];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    for i in 0..len {
        // Rejection sampling is unnecessary here: we re-roll the whole code on
        // collision, and a modulo bias over 32+ symbols is cryptographically
        // irrelevant for a locator. Keep it simple.
        let off = i * 8;
        let idx = (u64::from_le_bytes(buf[off..off + 8].try_into().unwrap()) % n as u64) as usize;
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
        return Err(format!(
            "自定义房间号需匹配 {}",
            cfg.rooms.custom_code_pattern
        ));
    }
    if cfg.rooms.reserved_codes.iter().any(|r| r == &normalized) {
        return Err(format!("房间号 {normalized:?} 是保留词"));
    }
    Ok(normalized)
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
            assert!(code
                .chars()
                .all(|ch| c.rooms.generated_code_alphabet.contains(ch)));
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
        for bad in [
            "api",
            "admin",
            "register",
            "health",
            "ws",
            "-abc",
            "ab",
            "a_b",
            "中文房",
        ] {
            assert!(validate_custom_code(&c, bad).is_err(), "{bad} should fail");
        }
    }
}
