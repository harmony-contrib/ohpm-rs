//! SSH-key-based login: sign `v1-{publishId}-{timestamp}-{nonce}` with the
//! private key (PSS first, PKCS#1v15 fallback) and exchange it for an access
//! token, mirroring `lib/core/registry/loginRequest.js`.
//!
//! Unlike the reference, this module is **strictly non-interactive**: the
//! passphrase must come from `OHPM_KEY_PASSPHRASE` or the `key_passphrase`
//! config item. If it is missing, it returns an error instead of prompting.

use std::path::PathBuf;

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
    /// The private key PEM content directly (instead of `key_path`).
    pub key_content: Option<String>,
    pub passphrase: Option<String>,
}

/// Everything needed to run the login flow.
#[derive(Debug)]
pub struct LoginContext {
    pub publish_id: String,
    pub key_path: Option<PathBuf>,
    /// The private key PEM content, when provided directly.
    pub key_content: Option<String>,
    pub passphrase: String,
}

impl LoginContext {
    /// Resolve from overrides (CLI) > env > config. Returns an error when
    /// anything required is missing. The key may be given either as a file
    /// path (`key_path`) or as inline PEM content (`key_content`).
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

        let key_content = overrides
            .key_content
            .clone()
            .or_else(|| std::env::var("OHPM_KEY_CONTENT").ok().filter(|s| !s.is_empty()))
            .or_else(|| {
                let v = config.get_string(types::KEY_CONTENT);
                (!v.is_empty()).then_some(v)
            });

        let key_path = if key_content.is_none() {
            Some(
                overrides
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
                    .ok_or_else(OhpmError::key_path_is_empty)?,
            )
        } else {
            None
        };

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
            key_content,
            passphrase,
        })
    }
}

/// Read and parse the (encrypted) private key, returning the raw key. The PEM
/// comes either from `key_content` (inline) or from the `key_path` file.
fn load_private_key(ctx: &LoginContext) -> Result<RsaPrivateKey> {
    let source = || {
        ctx.key_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "inline key content".to_string())
    };
    let pem = match &ctx.key_content {
        Some(content) => content.clone(),
        None => {
            let path = ctx.key_path.as_ref().ok_or_else(OhpmError::key_path_is_empty)?;
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
            pem
        }
    };
    // The reference only accepts keys whose PEM contains "ENCRYPTED".
    if !pem.contains("ENCRYPTED") {
        return Err(OhpmError::not_support_private_key(&source()));
    }
    let passphrase = ctx.passphrase.as_str();

    let key = if pem.contains("BEGIN ENCRYPTED PRIVATE KEY") {
        // PKCS#8 encrypted (modern OpenSSL default with -traditional absent).
        RsaPrivateKey::from_pkcs8_encrypted_pem(&pem, passphrase.as_bytes())
            .map_err(|e| signing_failed_with(&e.to_string()))?
    } else if pem.contains("BEGIN PRIVATE KEY") {
        RsaPrivateKey::from_pkcs8_pem(&pem).map_err(|e| signing_failed_with(&e.to_string()))?
    } else if pem.contains("BEGIN RSA PRIVATE KEY") {
        // Traditional PKCS#1, possibly with the legacy OpenSSL encryption
        // headers (`Proc-Type: 4,ENCRYPTED` + `DEK-Info:`).
        let der = decrypt_traditional_pem(&pem, passphrase)?;
        pkcs1::DecodeRsaPrivateKey::from_pkcs1_der(&der)
            .map_err(|e| signing_failed_with(&e.to_string()))?
    } else {
        return Err(OhpmError::not_support_private_key(&source()));
    };

    // Validate the key really is RSA (the reference signs with RSA).
    let _: RsaPublicKey = key.to_public_key();
    Ok(key)
}

/// Legacy ciphers used by OpenSSL traditional encrypted PEM (`DEK-Info:`).
#[derive(Debug, Clone, Copy)]
#[allow(clippy::enum_variant_names)] // cipher names all end in "Cbc"
enum LegacyCipher {
    Aes128Cbc,
    Aes192Cbc,
    Aes256Cbc,
    DesEde3Cbc,
    DesCbc,
}

impl LegacyCipher {
    fn parse(name: &str) -> Option<Self> {
        Some(match name.to_ascii_uppercase().as_str() {
            "AES-128-CBC" => Self::Aes128Cbc,
            "AES-192-CBC" => Self::Aes192Cbc,
            "AES-256-CBC" => Self::Aes256Cbc,
            "DES-EDE3-CBC" => Self::DesEde3Cbc,
            "DES-CBC" => Self::DesCbc,
            _ => return None,
        })
    }

    fn key_len(&self) -> usize {
        match self {
            Self::Aes128Cbc => 16,
            Self::Aes192Cbc => 24,
            Self::Aes256Cbc => 32,
            Self::DesEde3Cbc => 24,
            Self::DesCbc => 8,
        }
    }

    fn iv_len(&self) -> usize {
        match self {
            Self::Aes128Cbc | Self::Aes192Cbc | Self::Aes256Cbc => 16,
            Self::DesEde3Cbc | Self::DesCbc => 8,
        }
    }

    fn decrypt(self, key: &[u8], iv: &[u8], data: &[u8]) -> Option<Vec<u8>> {
        use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
        let mut buf = data.to_vec();
        let plain = match self {
            Self::Aes128Cbc => cbc::Decryptor::<aes::Aes128>::new_from_slices(key, iv).ok()?
                .decrypt_padded_mut::<Pkcs7>(&mut buf).ok()?,
            Self::Aes192Cbc => cbc::Decryptor::<aes::Aes192>::new_from_slices(key, iv).ok()?
                .decrypt_padded_mut::<Pkcs7>(&mut buf).ok()?,
            Self::Aes256Cbc => cbc::Decryptor::<aes::Aes256>::new_from_slices(key, iv).ok()?
                .decrypt_padded_mut::<Pkcs7>(&mut buf).ok()?,
            Self::DesEde3Cbc => cbc::Decryptor::<des::TdesEde3>::new_from_slices(key, iv).ok()?
                .decrypt_padded_mut::<Pkcs7>(&mut buf).ok()?,
            Self::DesCbc => cbc::Decryptor::<des::Des>::new_from_slices(key, iv).ok()?
                .decrypt_padded_mut::<Pkcs7>(&mut buf).ok()?,
        };
        Some(plain.to_vec())
    }
}

/// OpenSSL `EVP_BytesToKey` with MD5 and count 1 (the legacy PEM KDF): the
/// key+IV are built by repeatedly hashing `D_i = MD5(D_{i-1} || passphrase ||
/// salt)`, where the salt is the first 8 bytes of the IV.
fn evp_bytes_to_key_md5(passphrase: &[u8], salt: &[u8], key_len: usize, iv_len: usize) -> (Vec<u8>, Vec<u8>) {
    use md5::{Digest, Md5};
    let mut derived = Vec::new();
    let mut prev: Vec<u8> = Vec::new();
    while derived.len() < key_len + iv_len {
        let mut h = Md5::new();
        h.update(&prev);
        h.update(passphrase);
        h.update(salt);
        prev = h.finalize().to_vec();
        derived.extend_from_slice(&prev);
    }
    (
        derived[..key_len].to_vec(),
        derived[key_len..key_len + iv_len].to_vec(),
    )
}

/// Decrypt a traditional encrypted PKCS#1 PEM (OpenSSL legacy format with
/// `Proc-Type: 4,ENCRYPTED` / `DEK-Info: <cipher>,<hex-iv>` headers) into the
/// PKCS#1 DER private key.
fn decrypt_traditional_pem(pem: &str, passphrase: &str) -> Result<Vec<u8>> {
    let mut dek_info: Option<(LegacyCipher, Vec<u8>)> = None;
    let mut body = String::new();
    for line in pem.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("DEK-Info:") {
            let rest = rest.trim();
            let (cipher, iv_hex) = rest.split_once(',').ok_or_else(|| {
                signing_failed_with("malformed DEK-Info header in the private key PEM")
            })?;
            let cipher = LegacyCipher::parse(cipher.trim()).ok_or_else(|| {
                OhpmError::new(
                    "UnsupportedKeyCipher",
                    format!(
                        "The private key uses the unsupported legacy cipher \"{}\" (supported: \
                         AES-128/192/256-CBC, DES-EDE3-CBC, DES-CBC).",
                        cipher.trim()
                    ),
                )
            })?;
            let iv = decode_hex(iv_hex.trim()).ok_or_else(|| {
                signing_failed_with("malformed DEK-Info IV in the private key PEM")
            })?;
            dek_info = Some((cipher, iv));
        } else if !line.starts_with("-----") && !line.starts_with("Proc-Type:") {
            body.push_str(line);
        }
    }
    let (cipher, iv) = dek_info.ok_or_else(|| {
        OhpmError::new(
            "NotSupportPrivateKey",
            "The traditional PKCS#1 private key is not encrypted (no DEK-Info header); only \
             encrypted private keys are supported.",
        )
    })?;

    let data = base64::engine::general_purpose::STANDARD
        .decode(body.as_bytes())
        .map_err(|e| signing_failed_with(&e.to_string()))?;

    // The KDF salt is the first 8 bytes of the IV (OpenSSL legacy behavior);
    // the CBC IV itself is the full DEK-Info IV.
    let salt = &iv[..iv.len().min(8)];
    let (key, _derived_iv) =
        evp_bytes_to_key_md5(passphrase.as_bytes(), salt, cipher.key_len(), cipher.iv_len());
    cipher
        .decrypt(&key, &iv, &data)
        .ok_or_else(|| OhpmError::new("SignatureFailed", "Failed to decrypt the private key — the \
                                      passphrase is likely incorrect."))
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Validate the key material locally (readable + parseable with the given
/// passphrase) without performing any login request. Used by `--dry-run`.
pub fn validate_key(ctx: &LoginContext) -> Result<()> {
    let _ = load_private_key(ctx)?;
    Ok(())
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
    let key = load_private_key(ctx)?;

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

/// Traditional encrypted PKCS#1 PEM generated with
    /// `openssl genrsa -traditional -aes256` (passphrase "test-pass").
    const TRAD_AES256: &str = concat!(
        "-----BEGIN RSA PRIVATE KEY-----\n",
        "Proc-Type: 4,ENCRYPTED\n",
        "DEK-Info: AES-256-CBC,B27B5A83FCBD739CEE15E5D0CE221AC5\n",
        "\n",
        "8ZoxYtVFVE9RM1cz3+XKTWd21K71/wdKHD6LRL8xKYdrHHP7dMzFvEKMIo3N3jB+\n",
        "+VXNyk9fftTcMlJhIKVjEno23mgRzTiBlaPeZc1HeWgtD4Fb4glAKUqJDPtMhuh5\n",
        "H6IWj2Up1WFh6Yc5NRyUdFVWHdE++c+ZEs/o9Pd4k87nX2+LBOBWrJiwdbXs0kWV\n",
        "2jw7co1SgoklfVEKaYz055JoS2IZlae7mHXXUiM//Fn6I4eFNV0LM4JCnIbTzybU\n",
        "rhSii0n7+18vBk9pT1cdYKIjqJEgZWEOgF5zDdU8H2I5Tf8Gyr0UG1H5XBucuXLr\n",
        "a4d5UnpS5GvzXMchBMtmbb6LeiREE5HAYOCqlQOGK5tOuX/iiAZH5TLHhRvJG7BT\n",
        "X6M0uTFioC9qET6YjNJT6N16otxneGIB5vMgNNJg6TRN601+8Fmkhsmnaiaa80ZL\n",
        "FyyaCJ6OfbjHDLRxdTGBRvjbZhb/w5S2aaN3aoQttYGkyE+4OtkhnsDNSH9tk1Jo\n",
        "fizh0H3j9iaItOyQdJVIXRv+YLqM2gTMPOGQppXne1eW5i7nFhFsJqSVqY+HuvDX\n",
        "1HkxOje0HNt9slkfcOlc1Ulus145AGLRfgX6Ht7cOZ2rRsE0/xymXk0JaqIKwChg\n",
        "Xn8dDB82InQzZk8Ofl+dLb4zGATh8kIh2Tt5GsZ6P/9J+4WG6T89SxGIuj0FNG0C\n",
        "/VNkmsYQl303o5lxkuiII1TdtvR1DtiK/p93j2dnqIsg/ZoqiuBfcn0F4Jj7npqw\n",
        "bDTL7UQvSG1oMeHrgjzY1c0zwZIDLkxgtpKL2GtMpvAr/+F/pX2kOjxIGas47qgC\n",
        "U4ApjtKQKpBwEs0zK6AAvDkLi8y1xOoNxVEmkGGcwvITVp8qW6ytqiaKp3XxPZ5z\n",
        "/DstR6NZjZ7wPwxMvRbnGmi8vGetwfVXBD/hhhjbfiwW77WwP7ALue78yi+63O38\n",
        "QwlVbRPxUk4aTFUZ+nSeDspZvOC3GQpEqAYA631OMjEXZ4PRtlqFhO8FtEn5unvK\n",
        "sB8YEALJ80OyHdcKjeCd4Zvn4hhQ+ph/QY40iNpCpmRN6E6HVva48s4Sk0jLFgJ/\n",
        "ZC3FAwQnnk9XSB/yjrFLRv8bW/5sQeBdf98Glo91V7C/Mhl/4KPnlRX79zbtT/Tw\n",
        "zNvRPVNj7g49YyB6spW98zLzhMB3HZuOfiHwX9mXgjm1pVoiyafS6YobRAwaVJoL\n",
        "JaLdxlEc0NHXzKTY1B7eraZXz0Q98AFj4ohS78ZT+VosVppXVGBhfekIlyyrkxNr\n",
        "fC57uAtyr38CieYLNPiuNuUFC9MCsQdVn7t9RgM+MGy01QC5aoZZzpzjMW77zApy\n",
        "GLlEqiDFx2rbMvBtlGx1nAwPUNn+4ga83/P9DsGYVwiDuVH3hYTkbZl/tK40uiRA\n",
        "UsExvx3IR/ziLKRYIlSmu2YtxpCcz1s3iCWzGhJFEtQv5VF93HIu24rBa6Biw9VG\n",
        "tZ9iMwtlNhfubjPbTVysaaAaLAS0J56NjV14hMqQvaZN/VR0pj2rY1dLlh9wS0Jh\n",
        "b8G8coANSX1tcUiSly8A+oFrSnVLBSnO9HLFsZQ8pt0NIG8qmEXCHdKHM3UxGyKK\n",
        "-----END RSA PRIVATE KEY-----\n",
    );

/// Same key encrypted with `openssl genrsa -traditional -des3`.
    const TRAD_3DES: &str = concat!(
        "-----BEGIN RSA PRIVATE KEY-----\n",
        "Proc-Type: 4,ENCRYPTED\n",
        "DEK-Info: DES-EDE3-CBC,51662F6ADADF20EF\n",
        "\n",
        "wFoz1L8O0q4/qZ4yzhvITmXEuZsYwVEQGADosIaUJqJCCiM4PgzGcS8yiAl9GypN\n",
        "hG69VOvUB38mjfhQLiNGlh3Hrwy3qYlqdrVYvC5ZCYbH+VB2DJ8Ou63h0dbmVGYm\n",
        "Gt0nAZgy6OPCnYiTNHPe1cg1PSXkSOBcibKIvO1/ksTBz5mIar37UQqPIkl3q88F\n",
        "6EWEMb56S5esDmjrqmElkTXaEX9WD8vWcdDO98/IfexFxZMjXg5Ld2+T6TnDM7fY\n",
        "dQV5y4s2RKrc2UxFONcUFR0QMl3BrMGB3UxPRC/orQ3xmq8wC0C1QTSMb/er+/ii\n",
        "kghSuz2lFaOgAreAWSGsLA8YmU2hh6BirMCsbtsdS4duqz9QZuWh7dM9yzT37ycz\n",
        "MuKV3zahZmFLdQAK3zkk50jN5h5jcQgQppygOIE2FhQjb0wXvjStz9WO98wyaagM\n",
        "YNOcVGyw9+Y19fSYqfR7vTGO/ciG4UNAeJl6xDgJ7emaNs4kxidNtcSoxqkRyR7P\n",
        "oxY/KfRvWjxtptg1DfGueSc09nt2729E73lVD+YdLQDLI+C8gUGWUhlJCdH+62a5\n",
        "FInr8s126joHzDio5fzuurUwqJjcn6wTclcreYOR4YYovmfFbDksPiCqXGgLMwBN\n",
        "o9RBxlBKtP1RHjprlMgsQJojybs3cLFrMZSaGa+H/DngDg1hfuJ0jirdJIPlkZKF\n",
        "8T/jrP3yA3BDGd02qnO+GrprBTJQPojakzxfR5rA7hdltbR1FkVTGLoKBRi2YjrQ\n",
        "7SULEqcevyfLALIlyhIrV496SdrJdvokcAdgw4ZQcQIdH0QUD/MZQU9K0Y1sbFdS\n",
        "c/VA4HEsxasTyS9+PipP+nu0MS9NskXb9Q4L8BFC0Yr+uQAqXrCQnGweH/VAI+Gb\n",
        "kq9iTZJU2Bx+QZcXmAQ+vSprNDMwBhnzO+trNhoZiDGLR7cSyiVXLY2bBLJ6gwW+\n",
        "1ZDKjrzNJmMNmancIs5Vbf4kJlLxcOahGnN/YsTwH8cphRkf1bv+vvpq6nBMwnKg\n",
        "AvUB6oX7DiPruehTxp9RGUKh3hgKdaGhnogHJKtg+jR+65ykEzshULJY+tYA7SGe\n",
        "skAzNdhPGfA+EPCxQ+QHDhootW78QU17lmudIqQ15ymAZV2CtACD+TQqcyJ5Gr1a\n",
        "xKGhFPT6DjYDvvVKHbbQBjWMIhL9BBLk6FD6yykkJoSFl1IIv4cGWH00iyE2tJfi\n",
        "TnnUQVRPvp6Jsrh2hacuMAzezjrRi9vHEYU0E5+yRIAWcrvwawUrg9TwYloCFYzq\n",
        "PSYbbOAPofR4BnEyLmNaSXNg0pYsF4uhJRT7xgs5CYEEamDQltZqBCD81lvmyrsh\n",
        "0FGaMKI1S6g+R7qLgEbz7Ki0lOrMx0U/BC29M/HW/Vj9BWC5S/vJ33YCKhAY9g88\n",
        "+KaZYjCAINdB4CgZJgS8vz2Ch/RhGasHz6x1vsAkNbClUFBnMn3Gf5BtPuYLwWv4\n",
        "+n/wPu2g3gh3sog7NEX53Qn+As/jpEwA/QEcx8b50v6Fvta8SsmIlpBAjUHRmzXP\n",
        "s7IizJtYK+wLX9n2m1Q5WgvxNYTCR4DinmH1QJdT2YFQlfpKdgLZAcsgBfxzYVJg\n",
        "-----END RSA PRIVATE KEY-----\n",
    );

    #[test]
    fn traditional_aes256_pem_decrypts_and_signs() {
        let ctx = LoginContext {
            publish_id: "pid".into(),
            key_path: None,
            key_content: Some(TRAD_AES256.to_string()),
            passphrase: "test-pass".into(),
        };
        let key = super::load_private_key(&ctx).unwrap();
        let msg = "v1-pid-1700000000000-abc";
        let sig = pss_signature(&key, msg);
        let pub_pem = key
            .to_public_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        assert!(verify_signature(&pub_pem, msg, &sig, true));
    }

    #[test]
    fn traditional_3des_pem_decrypts() {
        let ctx = LoginContext {
            publish_id: "pid".into(),
            key_path: None,
            key_content: Some(TRAD_3DES.to_string()),
            passphrase: "test-pass".into(),
        };
        let key = super::load_private_key(&ctx).unwrap();
        let _: RsaPublicKey = key.to_public_key();
    }

    #[test]
    fn traditional_pem_wrong_passphrase_errors() {
        let ctx = LoginContext {
            publish_id: "pid".into(),
            key_path: None,
            key_content: Some(TRAD_AES256.to_string()),
            passphrase: "wrong".into(),
        };
        let err = super::load_private_key(&ctx).unwrap_err();
        assert_eq!(err.code, "SignatureFailed");
    }

    #[test]
    fn login_context_resolves_overrides_then_config() {
        let mut cfg = Config::new();
        cfg.set(types::PUBLISH_ID, "cfg-pid");
        cfg.set(types::KEY_PATH, "/tmp/cfg-key");
        cfg.set(types::KEY_PASSPHRASE, "cfg-secret");

        let ctx = LoginContext::resolve(&cfg, &LoginOverrides::default()).unwrap();
        assert_eq!(ctx.publish_id, "cfg-pid");
        assert_eq!(ctx.key_path, Some(PathBuf::from("/tmp/cfg-key")));
        assert!(ctx.key_content.is_none());
        assert_eq!(ctx.passphrase, "cfg-secret");

        // CLI overrides win.
        let overrides = LoginOverrides {
            publish_id: Some("cli-pid".into()),
            key_path: Some("/tmp/cli-key".into()),
            passphrase: Some("cli-secret".into()),
            ..Default::default()
        };
        let ctx = LoginContext::resolve(&cfg, &overrides).unwrap();
        assert_eq!(ctx.publish_id, "cli-pid");
        assert_eq!(ctx.passphrase, "cli-secret");
    }

    #[test]
    fn key_content_supplies_the_key_without_a_path() {
        let mut cfg = Config::new();
        cfg.set(types::PUBLISH_ID, "pid");
        cfg.set(types::KEY_PASSPHRASE, "secret");

        // Inline content via config; key_path is not required.
        cfg.set(types::KEY_CONTENT, "-----BEGIN ENCRYPTED PRIVATE KEY-----\n...");
        let ctx = LoginContext::resolve(&cfg, &LoginOverrides::default()).unwrap();
        assert!(ctx.key_path.is_none());
        assert_eq!(
            ctx.key_content.as_deref(),
            Some("-----BEGIN ENCRYPTED PRIVATE KEY-----\n...")
        );

        // CLI override wins over config content.
        let overrides = LoginOverrides {
            key_content: Some("-----BEGIN ENCRYPTED PRIVATE KEY-----\ncli".into()),
            ..Default::default()
        };
        let ctx = LoginContext::resolve(&cfg, &overrides).unwrap();
        assert_eq!(
            ctx.key_content.as_deref(),
            Some("-----BEGIN ENCRYPTED PRIVATE KEY-----\ncli")
        );
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
