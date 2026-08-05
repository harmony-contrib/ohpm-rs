//! SSH-key-based login: sign `v1-{publishId}-{timestamp}-{nonce}` with the
//! private key (PSS first, PKCS#1v15 fallback) and exchange it for an access
//! token, mirroring `lib/core/registry/loginRequest.js`.
//!
//! Unlike the reference, this module is **strictly non-interactive**: the
//! passphrase must come from `OHPM_KEY_PASSPHRASE` or the `key_passphrase`
//! config item. If it is missing, it returns an error instead of prompting.

use std::path::{Path, PathBuf};

use base64::Engine;
use rand::rngs::OsRng;
use rsa::pkcs1v15::SigningKey as Pkcs1v15SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::pss::{SigningKey as PssSigningKey, Signature as PssSignature};
use rsa::{RsaPrivateKey, RsaPublicKey};
use rsa::signature::{RandomizedSigner, SignatureEncoding};
use sha2::{Digest, Sha256};

use crate::config::{default::types, Config};
use crate::constants;
use crate::error::{OhpmError, Result};

/// Options that override config-sourced auth values (from CLI flags).
#[derive(Debug, Clone, Default)]
pub struct LoginOverrides {
    pub publish_id: Option<String>,
    pub key_path: Option<String>,
    pub passphrase: Option<String>,
}

/// Everything needed to run the login flow.
#[derive(Debug)]
pub struct LoginContext {
    pub publish_id: String,
    pub key_path: PathBuf,
    pub passphrase: String,
}

impl LoginContext {
    /// Resolve from overrides (CLI) > env > config. Returns an error when
    /// anything required is missing.
    pub fn resolve(config: &Config, overrides: &LoginOverrides) -> Result<Self> {
        let publish_id = overrides
            .publish_id
            .clone()
            .or_else(|| std::env::var("OHPM_PUBLISH_ID").ok().filter(|s| !s.is_empty()))
            .or_else(|| {
                let v = config.get_string(types::PUBLISH_ID);
                (!v.is_empty()).then_some(v)
            })
            .ok_or_else(OhpmError::publish_id_is_empty)?;

        let key_path = overrides
            .key_path
            .clone()
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var("OHPM_KEY_PATH")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
            })
            .or_else(|| {
                let v = config.get_string(types::KEY_PATH);
                (!v.is_empty()).then(|| PathBuf::from(v))
            })
            .ok_or_else(OhpmError::key_path_is_empty)?;

        // Passphrase: CLI/env override, then config; **no interactive prompt**.
        let passphrase = overrides
            .passphrase
            .clone()
            .or_else(|| {
                std::env::var("OHPM_KEY_PASSPHRASE")
                    .ok()
                    .filter(|s| !s.is_empty())
            })
            .or_else(|| {
                let v = config.get_string(types::KEY_PASSPHRASE);
                (!v.is_empty()).then_some(v)
            })
            .ok_or_else(OhpmError::key_passphrase_missing)?;

        Ok(Self {
            publish_id,
            key_path,
            passphrase,
        })
    }
}

/// Read and parse the (encrypted) private key, returning the raw key.
fn load_private_key(path: &Path, passphrase: &str) -> Result<RsaPrivateKey> {
    if !path.exists() {
        return Err(OhpmError::private_key_file_not_exist(&path.to_string_lossy()));
    }
    if path.is_dir() {
        return Err(OhpmError::key_path_is_dir(&path.to_string_lossy()));
    }
    let pem = std::fs::read_to_string(path)?;
    if pem.trim().is_empty() {
        return Err(OhpmError::private_key_content_is_empty(&path.to_string_lossy()));
    }
    // The reference only accepts keys whose PEM contains "ENCRYPTED".
    if !pem.contains("ENCRYPTED") {
        return Err(OhpmError::not_support_private_key(&path.to_string_lossy()));
    }

    let key = if pem.contains("BEGIN ENCRYPTED PRIVATE KEY") {
        RsaPrivateKey::from_pkcs8_encrypted_pem(&pem, passphrase.as_bytes())
            .map_err(|e| signing_failed_with(&e.to_string()))?
    } else if pem.contains("BEGIN PRIVATE KEY") || pem.contains("BEGIN RSA PRIVATE KEY") {
        RsaPrivateKey::from_pkcs8_pem(&pem).or_else(|_| {
            pkcs1::DecodeRsaPrivateKey::from_pkcs1_pem(&pem)
                .map_err(|e| signing_failed_with(&e.to_string()))
        })?
    } else {
        return Err(OhpmError::not_support_private_key(&path.to_string_lossy()));
    };

    // Validate the key really is RSA (the reference signs with RSA).
    let _: RsaPublicKey = key.to_public_key();
    Ok(key)
}

fn signing_failed_with(detail: &str) -> OhpmError {
    OhpmError::new(
        "SignatureFailed",
        format!("Failed to parse/decrypt the private key or generate the signature: {detail}"),
    )
}

/// PSS signature: `SHA256withRSA/PSS:<base64>`, salt length = digest length
/// (matches `RSA_PSS_SALTLEN_DIGEST` in the reference).
fn pss_signature(key: &RsaPrivateKey, message: &str) -> String {
    let signing_key = PssSigningKey::<Sha256>::new_with_salt_len(key.clone(), Sha256::output_size());
    let sig: PssSignature = signing_key.sign_with_rng(&mut OsRng, message.as_bytes());
    let b64 = base64::engine::general_purpose::STANDARD.encode(sig.to_bytes());
    format!("SHA256withRSA/PSS:{b64}")
}

/// Default (PKCS#1 v1.5 / RSA-SHA256) signature: bare base64.
fn default_signature(key: &RsaPrivateKey, message: &str) -> String {
    let signing_key = Pkcs1v15SigningKey::<Sha256>::new(key.clone());
    let sig = signing_key.sign_with_rng(&mut OsRng, message.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(sig.to_bytes())
}

/// Run the login flow against `registry`, returning the access token.
///
/// Tries the PSS `login_pss` endpoint first; on HTTP 400/404 falls back to the
/// default `login` endpoint (mirroring `CODE_API_MISSING`).
pub async fn login(
    client: &reqwest::Client,
    registry: &str,
    ctx: &LoginContext,
) -> Result<String> {
    let key = load_private_key(&ctx.key_path, &ctx.passphrase)?;

    let timestamp = now_millis();
    let nonce = new_nonce();
    let message = format!("v1-{}-{}-{}", ctx.publish_id, timestamp, nonce);

    let pss_sig = pss_signature(&key, &message);
    match post_login(client, registry, "login_pss", &ctx.publish_id, timestamp, &nonce, &pss_sig)
        .await?
    {
        Some(resp) => Ok(extract_token(resp)?),
        None => {
            // 400/404 -> default login with PKCS#1v15 signature.
            let default_sig = default_signature(&key, &message);
            let resp = post_login(client, registry, "login", &ctx.publish_id, timestamp, &nonce, &default_sig)
                .await?
                .ok_or_else(|| {
                    OhpmError::login_failed("the default login endpoint is also unavailable")
                })?;
            Ok(extract_token(resp)?)
        }
    }
}

/// Post a login request. Returns `Some(body)` on success, `None` when the API
/// is missing (HTTP 400/404) so the caller can fall back.
async fn post_login(
    client: &reqwest::Client,
    registry: &str,
    endpoint: &str,
    publish_id: &str,
    timestamp: i64,
    nonce: &str,
    signature: &str,
) -> Result<Option<serde_json::Value>> {
    let url = format!("{}{}", crate::config::ensure_trailing_slash(registry), endpoint);
    let body = serde_json::json!({
        "publishId": publish_id,
        "timestamp": timestamp.to_string(),
        "nonce": nonce,
        "signature": signature,
        "version": constants::LOGIN_REQUEST_VERSION,
    });

    log::debug!("sending login request to: {url}");
    let resp = client
        .post(&url)
        .header("command", "login")
        .header("version", constants::LOGIN_REQUEST_VERSION)
        .header("user-agent", constants::user_agent())
        .header("authorization", "")
        .header("Content-Type", "application/json; charset=UTF-8")
        .body(serde_json::to_vec(&body)?)
        .send()
        .await?;

    let status = resp.status();
    if status.as_u16() == 400 || status.as_u16() == 404 {
        log::debug!("login api does not exist (status {status}), falling back");
        return Ok(None);
    }
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(OhpmError::login_failed(&format!("HttpCode {status}, {text}")));
    }
    let value: serde_json::Value = resp.json().await?;
    Ok(Some(value))
}

fn extract_token(body: serde_json::Value) -> Result<String> {
    let token = body
        .get("token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            OhpmError::login_failed("the login response did not contain a \"token\" field")
        })?;
    Ok(token.to_string())
}

/// Verify a signature for tests: returns true when `signature` matches
/// `message` under the given public key with the given algorithm.
pub fn verify_signature(pem: &str, message: &str, signature: &str, pss: bool) -> bool {
    use rsa::signature::Verifier;
    use rsa::pkcs8::DecodePublicKey;
    let Ok(public) = RsaPublicKey::from_public_key_pem(pem) else {
        return false;
    };
    if pss {
        let vk = rsa::pss::VerifyingKey::<Sha256>::new_with_salt_len(public, Sha256::output_size());
        match signature.strip_prefix("SHA256withRSA/PSS:") {
            Some(b64) => {
                let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(b64) else {
                    return false;
                };
                let sig = match rsa::pss::Signature::try_from(bytes.as_slice()) {
                    Ok(s) => s,
                    Err(_) => return false,
                };
                vk.verify(message.as_bytes(), &sig).is_ok()
            }
            None => false,
        }
    } else {
        let vk = rsa::pkcs1v15::VerifyingKey::<Sha256>::new(public);
        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(signature) else {
            return false;
        };
        let sig = match rsa::pkcs1v15::Signature::try_from(bytes.as_slice()) {
            Ok(s) => s,
            Err(_) => return false,
        };
        vk.verify(message.as_bytes(), &sig).is_ok()
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn new_nonce() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use rsa::pkcs8::{EncodePublicKey, LineEnding};

    fn gen_key() -> (RsaPrivateKey, String) {
        use rand::SeedableRng;
        let mut rng = rand::rngs::StdRng::seed_from_u64(42);
        let key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let pub_pem = key
            .to_public_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        (key, pub_pem)
    }

    #[test]
    fn pss_signature_format_and_verify() {
        let (key, pub_pem) = gen_key();
        let msg = "v1-myid-1700000000000-abcdef";
        let sig = pss_signature(&key, msg);
        assert!(sig.starts_with("SHA256withRSA/PSS:"));
        assert!(verify_signature(&pub_pem, msg, &sig, true));
        // tampering must fail verification
        assert!(!verify_signature(&pub_pem, "tampered", &sig, true));
    }

    #[test]
    fn default_signature_verify() {
        let (key, pub_pem) = gen_key();
        let msg = "v1-myid-1700000000000-abcdef";
        let sig = default_signature(&key, msg);
        assert!(!sig.starts_with("SHA256withRSA/PSS:"));
        assert!(verify_signature(&pub_pem, msg, &sig, false));
    }

    #[test]
    fn login_context_resolves_overrides_then_config() {
        let mut cfg = Config::new();
        cfg.set(types::PUBLISH_ID, "cfg-pid");
        cfg.set(types::KEY_PATH, "/tmp/cfg-key");
        cfg.set(types::KEY_PASSPHRASE, "cfg-secret");

        let ctx = LoginContext::resolve(&cfg, &LoginOverrides::default()).unwrap();
        assert_eq!(ctx.publish_id, "cfg-pid");
        assert_eq!(ctx.key_path, PathBuf::from("/tmp/cfg-key"));
        assert_eq!(ctx.passphrase, "cfg-secret");

        // CLI overrides win.
        let overrides = LoginOverrides {
            publish_id: Some("cli-pid".into()),
            key_path: Some("/tmp/cli-key".into()),
            passphrase: Some("cli-secret".into()),
        };
        let ctx = LoginContext::resolve(&cfg, &overrides).unwrap();
        assert_eq!(ctx.publish_id, "cli-pid");
        assert_eq!(ctx.passphrase, "cli-secret");
    }

    #[test]
    fn missing_passphrase_is_an_error_not_a_prompt() {
        let mut cfg = Config::new();
        cfg.set(types::PUBLISH_ID, "pid");
        cfg.set(types::KEY_PATH, "/tmp/key");
        // no passphrase anywhere
        let err = LoginContext::resolve(&cfg, &LoginOverrides::default()).unwrap_err();
        assert_eq!(err.code, "KeyPassphraseMissing");
        assert!(err.message.contains("OHPM_KEY_PASSPHRASE"));
    }
}
