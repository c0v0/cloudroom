//! Minimal OIDC authorization-code client (PKCE + ID-token verification).
//! Discovery, token exchange and JWKS are fetched over HTTPS; redirects are
//! disabled so a compromised endpoint cannot bounce tokens elsewhere.

use base64::Engine;
use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::config::OidcCfg;

#[derive(Debug, Clone, Deserialize)]
pub struct Provider {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
}

#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    pub id_token: String,
}

#[derive(Debug, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub iss: String,
    pub aud: serde_json::Value,
    pub exp: i64,
    pub nonce: Option<String>,
    pub email: Option<String>,
    pub email_verified: Option<bool>,
    pub preferred_username: Option<String>,
    pub name: Option<String>,
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("OIDC HTTP client: {e}"))
}

fn normalized(url: &str) -> &str {
    url.trim_end_matches('/')
}

pub async fn discover(issuer: &str) -> Result<Provider, String> {
    let url = format!(
        "{}/.well-known/openid-configuration",
        normalized(issuer)
    );
    let res = client()?
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("OIDC discovery failed: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("OIDC discovery returned {}", res.status()));
    }
    let p: Provider = res
        .json()
        .await
        .map_err(|e| format!("OIDC discovery JSON: {e}"))?;
    if normalized(&p.issuer) != normalized(issuer) {
        return Err("OIDC discovery issuer mismatch".into());
    }
    Ok(p)
}

/// PKCE S256 challenge: base64url(sha256(verifier)).
pub fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

pub fn authorization_url(
    provider: &Provider,
    cfg: &OidcCfg,
    redirect_uri: &str,
    state: &str,
    nonce: &str,
    verifier: &str,
) -> Result<String, String> {
    let mut url =
        url::Url::parse(&provider.authorization_endpoint).map_err(|e| format!("OIDC auth URL: {e}"))?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &cfg.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", &cfg.scopes)
        .append_pair("state", state)
        .append_pair("nonce", nonce)
        .append_pair("code_challenge", &pkce_challenge(verifier))
        .append_pair("code_challenge_method", "S256");
    Ok(url.to_string())
}

pub async fn exchange_code(
    provider: &Provider,
    cfg: &OidcCfg,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<TokenResponse, String> {
    let res = client()?
        .post(&provider.token_endpoint)
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", &cfg.client_id),
            ("client_secret", &cfg.client_secret),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .map_err(|e| format!("OIDC token exchange failed: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("OIDC token exchange returned {}", res.status()));
    }
    res.json::<TokenResponse>()
        .await
        .map_err(|e| format!("OIDC token response JSON: {e}"))
}

pub async fn verify_id_token(
    provider: &Provider,
    cfg: &OidcCfg,
    id_token: &str,
    expected_nonce: &str,
) -> Result<Claims, String> {
    let header = decode_header(id_token).map_err(|e| format!("OIDC ID token header: {e}"))?;
    let res = client()?
        .get(&provider.jwks_uri)
        .send()
        .await
        .map_err(|e| format!("OIDC JWKS failed: {e}"))?;
    if !res.status().is_success() {
        return Err(format!("OIDC JWKS returned {}", res.status()));
    }
    let jwks: JwkSet = res
        .json()
        .await
        .map_err(|e| format!("OIDC JWKS JSON: {e}"))?;
    let jwk: &Jwk = jwks
        .keys
        .iter()
        .find(|k| header.kid.as_deref().is_none_or(|kid| k.common.key_id.as_deref() == Some(kid)))
        .ok_or_else(|| "OIDC JWKS: no matching key".to_string())?;
    let key = DecodingKey::from_jwk(jwk).map_err(|e| format!("OIDC JWK decode: {e}"))?;
    let mut validation = Validation::new(header.alg);
    validation.set_audience(&[cfg.client_id.as_str()]);
    validation.set_issuer(&[provider.issuer.as_str()]);
    validation.validate_exp = true;
    validation.validate_nbf = true;
    let data = decode::<Claims>(id_token, &key, &validation)
        .map_err(|e| format!("OIDC ID token invalid: {e}"))?;
    if data.claims.nonce.as_deref() != Some(expected_nonce) {
        return Err("OIDC ID token nonce mismatch".into());
    }
    Ok(data.claims)
}

#[cfg(test)]
mod tests {
    use super::pkce_challenge;

    #[test]
    fn pkce_s256_is_stable() {
        // RFC 7636 Appendix B test vector.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
