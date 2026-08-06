//! Command handlers: thin wrappers over `ohpm-core`.

pub mod cache;
pub mod config;
pub mod info;
pub mod init;
pub mod install;
pub mod uninstall;
pub mod update;
pub mod list;
pub mod login;
pub mod pack;
pub mod ping;
pub mod prepublish;
pub mod publish;
pub mod root;
pub mod unpublish;
pub mod version;

use anyhow::{anyhow, Result};
use ohpm_core::config::default::types;
use ohpm_core::config::Config;
use ohpm_core::OhpmError;

pub use crate::output;

/// Load configuration from the current working directory.
pub fn load_config() -> Result<Config> {
    let cwd = std::env::current_dir()?;
    let mut cfg = Config::new();
    cfg.load(&cwd, None)?;
    Ok(cfg)
}

/// CLI network/concurrency overrides feed into the config, mirroring the
/// reference's CLI config layer; `ConcurrentExecutor.check*` parameter bounds
/// are validated (the defaults are in range, so only CLI overrides can fail).
#[allow(clippy::too_many_arguments)]
pub fn apply_cli_options(
    config: &mut Config,
    fetch_timeout: Option<u64>,
    strict_ssl: Option<bool>,
    max_concurrent: Option<u64>,
    retry_times: Option<u32>,
    retry_interval: Option<u64>,
    registry: Option<&str>,
) -> Result<()> {
    if let Some(v) = fetch_timeout {
        config.set_cli(types::FETCH_TIMEOUT, &v.to_string());
    }
    if let Some(v) = strict_ssl {
        config.set_cli(types::STRICT_SSL, &v.to_string());
    }
    if let Some(v) = max_concurrent {
        config.set_cli(types::MAX_CONCURRENT, &v.to_string());
    }
    if let Some(v) = retry_times {
        config.set_cli(types::RETRY_TIMES, &v.to_string());
    }
    if let Some(v) = retry_interval {
        config.set_cli(types::RETRY_INTERVAL, &v.to_string());
    }
    if let Some(v) = registry {
        config.set_cli(types::REGISTRY, &ohpm_core::config::ensure_trailing_slash(v));
    }
    // The CLI option ranges are validated by `valid_cli_options` (per
    // command, with the reference's messages) before the pipeline runs.
    Ok(())
}

/// Resolve the install prefix: `--prefix` must contain oh-package.json5,
/// otherwise the nearest directory upward (mirrors `getRootDir`).
pub fn resolve_prefix(prefix: Option<&str>, command: &str) -> Result<std::path::PathBuf> {
    match prefix {
        Some(p) => {
            let p = std::path::PathBuf::from(p);
            if !p.join(ohpm_core::constants::MY_PACKAGE_JSON).is_file() {
                return Err(anyhow!(OhpmError::invalid_prefix_option(command)));
            }
            Ok(p)
        }
        None => find_local_prefix(&std::env::current_dir()?).ok_or_else(|| {
            anyhow!(OhpmError::file_not_exist(
                &std::env::current_dir()
                    .unwrap_or_default()
                    .join(ohpm_core::constants::MY_PACKAGE_JSON)
            ))
        }),
    }
}

use ohpm_core::config::find_local_prefix;
