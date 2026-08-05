//! `ohpm ping` — test connectivity to the registry.
//! Mirrors `lib/commands/ping.js`: GET `{registry}/-/ping` for each registry.

use anyhow::{anyhow, Result};
use ohpm_core::config::{default::types, ensure_trailing_slash};
use ohpm_core::registry::{auth, RegistryClient};
use ohpm_core::{constants, OhpmError};

use super::{load_config, output};
use crate::cli::PingArgs;

pub async fn run(args: &PingArgs) -> Result<()> {
    let mut config = load_config()?;
    if let Some(timeout) = args.timeout {
        config.set_cli(types::FETCH_TIMEOUT, &timeout.to_string());
    }
    let client = RegistryClient::from_config(&config)?;

    let registries: Vec<String> = if let Some(r) = args.registry.as_deref().filter(|s| !s.is_empty()) {
        vec![ensure_trailing_slash(r)]
    } else {
        config
            .get_string(types::REGISTRY)
            .split(',')
            .map(|s| ensure_trailing_slash(s.trim()))
            .filter(|s| !s.is_empty())
            .collect()
    };
    if registries.is_empty() {
        return Err(anyhow!(OhpmError::new("RegistryIsEmpty", "The registry list is empty.")));
    }

    let started = std::time::Instant::now();
    let mut failed: Vec<String> = Vec::new();

    for registry in &registries {
        output::output(&format!("PING {registry}"));
        let t = std::time::Instant::now();
        match ping_one(&client, &config, registry).await {
            Ok(Some(body)) => {
                if body.trim().is_empty() {
                    output::output(&format!("PONG {}ms", t.elapsed().as_millis()));
                } else {
                    output::output(&format!("PONG {body}"));
                    output::output(&format!("PONG {}ms", t.elapsed().as_millis()));
                }
            }
            Ok(None) | Err(_) => failed.push(registry.clone()),
        }
    }

    output::succeed(&format!("PONG Total {}ms", started.elapsed().as_millis()));
    if !failed.is_empty() {
        return Err(anyhow!(
            "The following registries failed to ping: {}",
            failed.join(", ")
        ));
    }
    Ok(())
}

async fn ping_one(
    client: &RegistryClient,
    config: &ohpm_core::config::Config,
    registry: &str,
) -> std::result::Result<Option<String>, OhpmError> {
    let url = format!("{}-/ping", registry.trim_end_matches('/'));
    let token = auth::read_token(config, registry);
    let mut req = client.http().get(&url).header("user-agent", constants::user_agent());
    if !token.is_empty() {
        req = req.header("Authorization", token);
    }
    let resp = req.send().await.map_err(OhpmError::from)?;
    log::debug!("STATUS_{} - GET {url}", resp.status());
    if resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Ok(Some(text));
    }
    if resp.status().as_u16() == 404 {
        return Ok(None);
    }
    Ok(None)
}
