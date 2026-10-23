//! Password hashing (Argon2id), session tokens, CSRF tokens, cookie codec.

use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};

use crate::config::Config;
use crate::domain::models::Session;
use crate::error::{ApiError, ApiResult};
use crate::util;
use std::sync::{Arc, OnceLock};

pub const COOKIE_NAME_SECURE: &str = "__Host-cloudroom_session";
pub const COOKIE_NAME_INSECURE: &str = "cloudroom_session";

pub fn cookie_name(cfg: &Config) -> &'static str {
    if cfg.server.insecure_cookies {
        COOKIE_NAME_INSECURE
    } else {
        COOKIE_NAME_SECURE
    }
}

#[derive(Clone)]
pub struct Auth {
    pub cfg: Config,
}

impl Auth {
    pub fn hash_password(&self, password: &str) -> ApiResult<String> {
        let a = Argon2::new(
            argon2::Algorithm::Argon2id,
            argon2::Version::V0x13,
            argon2::Params::new(
                self.cfg.auth.argon2_memory_kib,
                self.cfg.auth.argon2_iterations,
                self.cfg.auth.argon2_parallelism,
                None,
            )
            .map_err(|e| ApiError::internal(format!("密码哈希参数无效: {e}")))?,
        );
        let salt = SaltString::generate(&mut OsRng);
        a.hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| ApiError::internal(format!("密码哈希失败: {e}")))
    }

    /// Verify against a process-local dummy hash so unknown usernames take the
    /// same Argon2 path as wrong passwords (timing enumeration mitigation).
    pub fn verify_dummy(&self, password: &str) -> bool {
        static DUMMY: OnceLock<String> = OnceLock::new();
        let hash = DUMMY.get_or_init(|| {
            self.hash_password("cloudroom-dummy-password")
                .unwrap_or_default()
        });
        self.verify_password(password, hash)
    }

    pub fn verify_password(&self, password: &str, phc: &str) -> bool {
        let Ok(parsed) = PasswordHash::new(phc) else {
            return false;
        };
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    }

    pub fn new_session(
        &self,
        user_id: &str,
        ip: Option<String>,
        user_agent: Option<String>,
    ) -> Session {
        let now = util::now();
        let expires = now + chrono::Duration::from_std(self.cfg.session_ttl()).unwrap_or_default();
        Session {
            session_id: util::random_id(),
            user_id: user_id.to_string(),
            token_hash: util::sha256_hex(util::random_token().as_bytes()),
            csrf_token: util::random_token(),
            expires_at: expires,
            created_at: now,
            last_seen_at: now,
            ip,
            user_agent,
        }
    }

    /// Set-Cookie value for a raw session token. Raw token is NOT stored; only
    /// its hash is persisted.
    pub fn session_cookie(&self, raw_token: &str) -> String {
        let secure = if self.cfg.server.insecure_cookies {
            ""
        } else {
            " Secure"
        };
        format!(
            "{}={raw_token}; Path=/; HttpOnly; SameSite=Lax{secure}; Max-Age={}",
            cookie_name(&self.cfg),
            self.cfg.session_ttl().as_secs()
        )
    }

    pub fn clear_cookie(&self) -> String {
        let secure = if self.cfg.server.insecure_cookies {
            ""
        } else {
            " Secure"
        };
        format!(
            "{}=; Path=/; HttpOnly; SameSite=Lax{secure}; Max-Age=0",
            cookie_name(&self.cfg)
        )
    }
}

pub async fn hash_password_blocking(auth: Arc<Auth>, password: String) -> ApiResult<String> {
    tokio::task::spawn_blocking(move || auth.hash_password(&password))
        .await
        .map_err(|e| ApiError::internal(format!("密码哈希任务失败: {e}")))?
}

pub async fn verify_password_blocking(
    auth: Arc<Auth>,
    password: String,
    hash: String,
) -> ApiResult<bool> {
    tokio::task::spawn_blocking(move || auth.verify_password(&password, &hash))
        .await
        .map_err(|e| ApiError::internal(format!("密码校验任务失败: {e}")))
}

pub async fn verify_dummy_blocking(auth: Arc<Auth>, password: String) -> ApiResult<bool> {
    tokio::task::spawn_blocking(move || auth.verify_dummy(&password))
        .await
        .map_err(|e| ApiError::internal(format!("密码校验任务失败: {e}")))
}

/// Header value for the session cookie (split on ';').
pub fn cookie_value(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(axum::http::header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some((k, v)) = part.split_once('=') {
            if k.trim() == name {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}
