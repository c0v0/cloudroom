//! Web Push（RFC 8291 + RFC 8292）纯 Rust 实现，保持 static-pie 发布。
//! 使用 `ring`（rustls 已在用）：VAPID ES256 + ECDH + HKDF + AES-128-GCM。

use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use ring::aead;
use ring::agreement;
use ring::hkdf;
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushSub {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

#[derive(Debug, Clone)]
pub struct PushKeys {
    /// base64url（无填充）的 P-256 公钥（65 字节未压缩点），下发给浏览器。
    pub public_key: String,
    /// base64（标准）的 PKCS#8 DER 私钥。
    pub private_key_pkcs8: String,
    pub subject: String,
}

impl PushKeys {
    pub fn enabled(&self) -> bool {
        !self.public_key.is_empty() && !self.private_key_pkcs8.is_empty()
    }

    fn signing_key(&self) -> Result<EcdsaKeyPair, String> {
        let der = STANDARD
            .decode(self.private_key_pkcs8.trim())
            .map_err(|e| format!("invalid VAPID PKCS#8 base64: {e}"))?;
        EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &SystemRandom::new())
            .map_err(|e| format!("invalid VAPID PKCS#8 key: {e}"))
    }

    /// 由 PKCS#8 私钥推导公钥（base64url 未压缩点）；失败时回退配置值。
    pub fn derived_public_key(&self) -> String {
        if let Ok(pair) = self.signing_key() {
            URL_SAFE_NO_PAD.encode(pair.public_key().as_ref())
        } else {
            self.public_key.clone()
        }
    }
}

fn b64(s: &str) -> Result<Vec<u8>, String> {
    URL_SAFE_NO_PAD
        .decode(s.trim())
        .map_err(|e| format!("invalid base64url: {e}"))
}

struct Len(usize);
impl hkdf::KeyType for Len {
    fn len(&self) -> usize {
        self.0
    }
}

/// VAPID JWT（RFC 8292）：ES256（固定 r||s），`aud` 为推送服务 origin。
pub fn vapid_jwt(keys: &PushKeys, endpoint: &str, exp: u64) -> Result<String, String> {
    let url = reqwest::Url::parse(endpoint).map_err(|e| format!("invalid endpoint: {e}"))?;
    let aud = format!(
        "{}://{}",
        url.scheme(),
        url.host_str().ok_or("endpoint without host")?
    );
    let header = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = URL_SAFE_NO_PAD
        .encode(serde_json::json!({ "aud": aud, "exp": exp, "sub": keys.subject }).to_string());
    let signing_input = format!("{header}.{claims}");
    let pair = keys.signing_key()?;
    let sig = pair
        .sign(&SystemRandom::new(), signing_input.as_bytes())
        .map_err(|e| format!("VAPID sign failed: {e}"))?;
    Ok(format!(
        "{signing_input}.{}",
        URL_SAFE_NO_PAD.encode(sig.as_ref())
    ))
}

/// RFC 8291 aes128gcm 单记录加密。`salt` 与临时私钥由调用方生成（测试可注入）。
pub fn encrypt_payload(
    p256dh: &str,
    auth: &str,
    plaintext: &[u8],
    salt: &[u8; 16],
    eph: agreement::EphemeralPrivateKey,
) -> Result<Vec<u8>, String> {
    let ua_bytes = b64(p256dh)?;
    let auth_bytes = b64(auth)?;
    let as_public = eph
        .compute_public_key()
        .map_err(|_| "ephemeral public key failed".to_string())?;
    let peer = agreement::UnparsedPublicKey::new(&agreement::ECDH_P256, &ua_bytes);
    let shared: Vec<u8> = agreement::agree_ephemeral(
        eph,
        &peer,
        |secret| -> Result<Vec<u8>, ring::error::Unspecified> { Ok(secret.to_vec()) },
    )
    .map_err(|_| "ECDH failed".to_string())?
    .map_err(|_| "ECDH failed".to_string())?;

    let mut info = Vec::with_capacity(14 + ua_bytes.len() + as_public.as_ref().len());
    info.extend_from_slice(b"WebPush: info\0");
    info.extend_from_slice(&ua_bytes);
    info.extend_from_slice(as_public.as_ref());
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, &auth_bytes).extract(&shared);
    let mut ikm = [0u8; 32];
    prk.expand(&[&info], Len(32))
        .map_err(|_| "hkdf ikm failed".to_string())?
        .fill(&mut ikm)
        .map_err(|_| "hkdf ikm fill failed".to_string())?;

    let prk2 = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(&ikm);
    let mut cek = [0u8; 16];
    prk2.expand(&[b"Content-Encoding: aes128gcm\0"], Len(16))
        .map_err(|_| "hkdf cek failed".to_string())?
        .fill(&mut cek)
        .map_err(|_| "hkdf cek fill failed".to_string())?;
    let mut nonce = [0u8; 12];
    prk2.expand(&[b"Content-Encoding: nonce\0"], Len(12))
        .map_err(|_| "hkdf nonce failed".to_string())?
        .fill(&mut nonce)
        .map_err(|_| "hkdf nonce fill failed".to_string())?;

    let mut record = Vec::with_capacity(plaintext.len() + 1);
    record.extend_from_slice(plaintext);
    record.push(0x02);

    let key =
        aead::UnboundKey::new(&aead::AES_128_GCM, &cek).map_err(|_| "invalid cek".to_string())?;
    aead::LessSafeKey::new(key)
        .seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::empty(),
            &mut record,
        )
        .map_err(|_| "encrypt failed".to_string())?;

    let mut body = Vec::with_capacity(16 + 4 + 1 + as_public.as_ref().len() + record.len());
    body.extend_from_slice(salt);
    body.extend_from_slice(&4096u32.to_be_bytes());
    body.push(as_public.as_ref().len() as u8);
    body.extend_from_slice(as_public.as_ref());
    body.extend_from_slice(&record);
    Ok(body)
}

static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default()
});

/// 发送一条推送。`Ok(false)` 表示订阅失效（404/410）。
pub async fn send(keys: &PushKeys, sub: &PushSub, payload: &[u8]) -> Result<bool, String> {
    use rand::RngCore;
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let eph = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &SystemRandom::new())
        .map_err(|_| "ephemeral key failed".to_string())?;
    let body = encrypt_payload(&sub.p256dh, &sub.auth, payload, &salt, eph)?;
    let jwt = vapid_jwt(
        keys,
        &sub.endpoint,
        chrono::Utc::now().timestamp() as u64 + 12 * 3600,
    )?;
    let res = CLIENT
        .post(&sub.endpoint)
        .header("TTL", "86400")
        .header("Content-Encoding", "aes128gcm")
        .header("Content-Type", "application/octet-stream")
        .header(
            "Authorization",
            format!("vapid t={jwt}, k={}", keys.derived_public_key()),
        )
        .body(body)
        .send()
        .await
        .map_err(|e| format!("push request failed: {e}"))?;
    match res.status().as_u16() {
        200..=204 => Ok(true),
        404 | 410 => Ok(false),
        other => Err(format!("push endpoint returned {other}")),
    }
}

/// 向一组用户的所有订阅发送同一份 JSON 通知；失效订阅自动清理。
pub async fn notify_users(
    db: &crate::infra::db::Db,
    cfg: &crate::config::PushCfg,
    user_ids: Vec<String>,
    payload: serde_json::Value,
) {
    if !cfg.enabled() || user_ids.is_empty() {
        return;
    }
    let keys = PushKeys {
        public_key: cfg.vapid_public_key.clone(),
        private_key_pkcs8: cfg.vapid_private_key.clone(),
        subject: cfg.subject.clone(),
    };
    let subs = match db
        .with(move |conn| crate::infra::repo::push_subscriptions_for_users(conn, &user_ids))
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e.message, "push: cannot load subscriptions");
            return;
        }
    };
    let body = payload.to_string();
    for (_uid, endpoint, p256dh, auth) in subs {
        let sub = PushSub {
            endpoint: endpoint.clone(),
            p256dh,
            auth,
        };
        match send(&keys, &sub, body.as_bytes()).await {
            Ok(true) => {}
            Ok(false) => {
                let e = endpoint.clone();
                let _ = db
                    .with(move |conn| {
                        crate::infra::repo::delete_push_subscription_by_endpoint(conn, &e)
                    })
                    .await;
                tracing::info!(endpoint, "push: removed expired subscription");
            }
            Err(e) => tracing::warn!(endpoint, error = %e, "push: send failed"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> PushKeys {
        // 生成一次性 PKCS#8 私钥用于测试。
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        PushKeys {
            public_key: URL_SAFE_NO_PAD.encode(pair.public_key().as_ref()),
            private_key_pkcs8: STANDARD.encode(pkcs8.as_ref()),
            subject: "mailto:test@example.com".into(),
        }
    }

    #[test]
    fn vapid_jwt_is_es256_and_verifies() {
        let keys = keys();
        let jwt = vapid_jwt(&keys, "https://push.example.com/send/x", 1_900_000_000).unwrap();
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(header["alg"], "ES256");
        let claims: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://push.example.com");
        let pubkey = URL_SAFE_NO_PAD.decode(keys.derived_public_key()).unwrap();
        let sig = URL_SAFE_NO_PAD.decode(parts[2]).unwrap();
        assert_eq!(sig.len(), 64);
        ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, &pubkey)
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig)
            .unwrap();
    }

    #[test]
    fn encrypted_body_has_rfc8291_layout() {
        let rng = SystemRandom::new();
        let ua = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
        let ua_public = ua.compute_public_key().unwrap();
        let p256dh = URL_SAFE_NO_PAD.encode(ua_public.as_ref());
        let auth = URL_SAFE_NO_PAD.encode([3u8; 16]);
        let salt = [5u8; 16];
        let eph = agreement::EphemeralPrivateKey::generate(&agreement::ECDH_P256, &rng).unwrap();
        let eph_public = eph.compute_public_key().unwrap().as_ref().to_vec();
        let body = encrypt_payload(&p256dh, &auth, b"hello", &salt, eph).unwrap();
        assert_eq!(&body[0..16], &salt);
        assert_eq!(u32::from_be_bytes(body[16..20].try_into().unwrap()), 4096);
        assert_eq!(body[20], 65);
        assert_eq!(&body[21..86], eph_public.as_slice());
        assert_eq!(body.len(), 86 + 5 + 1 + 16);
    }
}
