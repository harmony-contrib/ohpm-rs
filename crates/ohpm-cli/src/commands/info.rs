//! `ohpm info <pkg> [field]` — fetch package metadata from the registry.

use anyhow::{anyhow, Result};
use ohpm_core::config::{default::types, ensure_trailing_slash};
use ohpm_core::registry::{auth, url_encode_pkg_name, RegistryClient};
use ohpm_core::{constants, OhpmError};

use super::{load_config, output};
use crate::cli::InfoArgs;

pub async fn run(args: &InfoArgs) -> Result<()> {
    let mut config = load_config()?;
    if let Some(timeout) = args.timeout {
        config.set_cli(types::FETCH_TIMEOUT, &timeout.to_string());
    }
    let client = RegistryClient::from_config(&config)?;

    let registry = args
        .registry
        .as_deref()
        .map(ensure_trailing_slash)
        .unwrap_or_else(|| config.registry());

    let token = auth::read_token(&config, &registry);
    let url = format!("{}{}", registry, url_encode_pkg_name(&args.pkg));

    let mut req = client
        .http()
        .get(&url)
        .header("user-agent", constants::user_agent());
    if !token.is_empty() {
        req = req.header("Authorization", token);
    }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!(OhpmError::response_status(status.as_u16(), &text)));
    }
    let value: serde_json::Value = resp.json().await?;

    match args.field.as_deref() {
        None => output::output(&serde_json::to_string_pretty(&value)?),
        Some(field) => {
            let picked = value.get(field).ok_or_else(|| {
                anyhow!("The field \"{field}\" does not exist in the package metadata.")
            })?;
            if picked.is_string() {
                output::output(picked.as_str().unwrap_or_default());
            } else {
                output::output(&serde_json::to_string_pretty(picked)?);
            }
        }
    }
    Ok(())
}
