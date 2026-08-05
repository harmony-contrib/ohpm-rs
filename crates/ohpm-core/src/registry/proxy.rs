//! HTTP client construction honoring proxy / strict-SSL / CA config.
//! Mirrors `lib/core/registry/proxy.js`.

use std::sync::Arc;
use std::time::Duration;

use crate::config::{default::types, Config};

/// Build a `reqwest::Client` from ohpm config plus standard proxy env vars.
pub fn build_client(config: &Config) -> crate::Result<reqwest::Client> {
    let strict_ssl = config.get_bool(types::STRICT_SSL);

    let mut builder = reqwest::Client::builder()
        .user_agent(crate::constants::user_agent())
        .timeout(Duration::from_millis(config.get_number(types::FETCH_TIMEOUT).max(1) as u64));

    // CA files (only honored when strict SSL is enabled, like the reference).
    let ca_files = config.get_string(types::CA_FILES);
    if strict_ssl && !ca_files.is_empty() {
        let mut buf = Vec::new();
        for path in ca_files.split(',') {
            let p = path.trim();
            if p.is_empty() {
                continue;
            }
            let bytes = std::fs::read(p)
                .map_err(|e| crate::OhpmError::new("CaFileReadError", format!("{p}: {e}")))?;
            buf.extend_from_slice(&bytes);
        }
        if !buf.is_empty() {
            let cert = reqwest::Certificate::from_pem(&buf)
                .map_err(|e| crate::OhpmError::new("CaFileParseError", e.to_string()))?;
            builder = builder.add_root_certificate(cert);
        }
    }

    if !strict_ssl {
        builder = builder
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true);
    }

    // Proxy: config value wins; otherwise standard env vars (lower/upper).
    let https_proxy = first_nonempty(&[
        &config.get_string(types::HTTPS_PROXY),
        &env_or("HTTPS_PROXY"),
        &env_or("https_proxy"),
    ]);
    let http_proxy = first_nonempty(&[
        &config.get_string(types::HTTP_PROXY),
        &env_or("HTTP_PROXY"),
        &env_or("http_proxy"),
    ]);
    let no_proxy = first_nonempty(&[
        &config.get_string(types::NO_PROXY),
        &env_or("NO_PROXY"),
        &env_or("no_proxy"),
    ]);

    let no_proxy_str = no_proxy.as_deref().unwrap_or_default();
    let no_proxy_guard = reqwest::NoProxy::from_string(no_proxy_str);

    let apply_proxy = |builder: reqwest::ClientBuilder,
                       url: &str,
                       no_proxy: Option<reqwest::NoProxy>|
     -> reqwest::ClientBuilder {
        match reqwest::Proxy::all(url) {
            Ok(proxy) => builder.proxy(proxy.no_proxy(no_proxy)),
            Err(_) => builder,
        }
    };

    if let Some(p) = https_proxy {
        builder = apply_proxy(builder, &p, no_proxy_guard);
    } else if let Some(p) = http_proxy {
        builder = apply_proxy(builder, &p, no_proxy_guard);
    }

    let client = builder
        .build()
        .map_err(|e| crate::OhpmError::new("ClientBuildError", e.to_string()))?;
    Ok(client)
}

fn env_or(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

fn first_nonempty(vals: &[&String]) -> Option<String> {
    vals.iter().find(|v| !v.is_empty()).map(|v| v.to_string())
}

/// Convenience wrapper so callers can share one client.
pub type SharedClient = Arc<reqwest::Client>;
