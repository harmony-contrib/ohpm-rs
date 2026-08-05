//! Access-token resolution for registry requests.
//!
//! The core requirement of this reimplementation: `publish` must be able to
//! authenticate entirely from environment variables. Resolution order for the
//! **write** token is:
//!
//! 1. `OHPM_ACCESS_TOKEN`
//! 2. `{registry-stripped}:_auth` from `.ohpmrc` (mirrors `getAuth.js`)
//! 3. SSH-key login via `OHPM_PUBLISH_ID` / `OHPM_KEY_PATH` /
//!    `OHPM_KEY_PASSPHRASE` (non-interactive; see [`login`])
//!
//! If the SSH login path is entered but a required value (or the passphrase)
//! is missing, an error is returned — never a TUI prompt.

use crate::config::{default::access_token_type, Config};
use crate::error::{OhpmError, Result};

use super::login::{self, LoginOverrides};

/// Get the read token for `registry` (`:_read_auth`), or `""`.
pub fn read_token(config: &Config, registry: &str) -> String {
    // OHPM_READ_ACCESS_TOKEN overrides everything for read requests.
    if let Ok(t) = std::env::var("OHPM_READ_ACCESS_TOKEN") {
        if !t.is_empty() {
            return t;
        }
    }
    config.access_token(registry, false)
}

/// Get the write (read-write) token for `registry` from env or config, or `""`
/// when none is available.
pub fn configured_write_token(config: &Config, registry: &str) -> String {
    if let Ok(t) = std::env::var("OHPM_ACCESS_TOKEN") {
        if !t.is_empty() {
            return t;
        }
    }
    config.access_token(registry, true)
}

/// Resolve a write token for `registry`, running the SSH-key login flow when
/// no token is configured. Never prompts.
pub async fn resolve_write_token(
    client: &reqwest::Client,
    config: &Config,
    registry: &str,
    overrides: &LoginOverrides,
) -> Result<String> {
    let token = configured_write_token(config, registry);
    if !token.is_empty() {
        return Ok(token);
    }

    // No token available -> SSH-key login. This needs publish_id + key_path;
    // missing pieces (or a missing passphrase) raise a clear error.
    let ctx = login::LoginContext::resolve(config, overrides)?;
    let token = login::login(client, registry, &ctx).await?;
    if token.is_empty() {
        return Err(OhpmError::access_token_missing());
    }
    Ok(token)
}

/// The token config key for a registry, e.g. `//host/path/:_auth`.
pub fn write_token_key(registry: &str) -> String {
    format!(
        "{}{}",
        crate::config::strip_protocol(&crate::config::ensure_trailing_slash(registry)),
        access_token_type::READ_WRITE
    )
}

/// The read token config key, e.g. `//host/path/:_read_auth`.
pub fn read_token_key(registry: &str) -> String {
    format!(
        "{}{}",
        crate::config::strip_protocol(&crate::config::ensure_trailing_slash(registry)),
        access_token_type::READ
    )
}

/// Validate that a token is non-blank; used to build a friendly error.
pub fn ensure_token(token: &str) -> Result<()> {
    if token.is_empty() {
        Err(OhpmError::access_token_missing())
    } else {
        Ok(())
    }
}
