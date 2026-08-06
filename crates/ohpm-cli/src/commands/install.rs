//! `ohpm install [pkg...]` — install dependencies (mirrors
//! `lib/commands/install.js` + `lib/core/install/service/install.js`).

use anyhow::{anyhow, Result};
use ohpm_core::config::{default::types, ensure_trailing_slash, find_local_prefix};
use ohpm_core::install::InstallOptions;
use ohpm_core::registry::RegistryClient;
use ohpm_core::OhpmError;

use super::{load_config, output};
use crate::cli::InstallArgs;

pub async fn run(args: &InstallArgs) -> Result<()> {
    let mut config = load_config()?;

    // CLI overrides feed into the config (the client and the pipeline read
    // from it), mirroring the reference's CLI config layer.
    if let Some(v) = args.fetch_timeout {
        config.set_cli(types::FETCH_TIMEOUT, &v.to_string());
    }
    if let Some(v) = args.strict_ssl {
        config.set_cli(types::STRICT_SSL, &v.to_string());
    }
    if let Some(v) = args.max_concurrent {
        config.set_cli(types::MAX_CONCURRENT, &v.to_string());
    }
    if let Some(v) = args.retry_times {
        config.set_cli(types::RETRY_TIMES, &v.to_string());
    }
    if let Some(v) = args.retry_interval {
        config.set_cli(types::RETRY_INTERVAL, &v.to_string());
    }
    if let Some(v) = &args.registry {
        config.set_cli(types::REGISTRY, &ensure_trailing_slash(v));
    }

    // `ConcurrentExecutor` parameter ranges.
    validate_range("max_concurrent", 1, 200, &config)?;
    validate_range("retry_times", 0, 5, &config)?;
    validate_range("retry_interval", 1_000, 60_000, &config)?;

    // Prefix: `--prefix` must contain oh-package.json5; otherwise the nearest
    // directory upward (mirrors `getRootDir` + `InvalidPrefixOption`).
    let prefix = match &args.prefix {
        Some(p) => {
            let p = std::path::PathBuf::from(p);
            if !p.join(ohpm_core::constants::MY_PACKAGE_JSON).is_file() {
                return Err(anyhow!(OhpmError::invalid_prefix_option("install")));
            }
            p
        }
        None => find_local_prefix(&std::env::current_dir()?)
            .ok_or_else(|| {
                anyhow!(OhpmError::file_not_exist(
                    &std::env::current_dir()
                        .unwrap_or_default()
                        .join(ohpm_core::constants::MY_PACKAGE_JSON)
                ))
            })?,
    };

    let opts = InstallOptions {
        save: !args.no_save,
        save_dev: args.save_dev,
        save_prod: args.save_prod,
        save_dynamic: args.save_dynamic,
        link: !args.no_link,
        all: args.all,
        prefix: Some(prefix.clone()),
        registry: args.registry.clone(),
        fetch_timeout: args.fetch_timeout,
        strict_ssl: args.strict_ssl,
        max_concurrent: args.max_concurrent,
        retry_times: args.retry_times,
        retry_interval: args.retry_interval,
    };

    let client = RegistryClient::from_config(&config)?;
    let outcome = ohpm_core::install::install(&client, &config, &prefix, &args.pkg, &opts).await?;

    output::succeed(&format!(
        "install success, {} packages installed, {} modules",
        outcome.installed,
        outcome.module_roots.len()
    ));
    Ok(())
}

/// `ConcurrentExecutor.check*` — parameter bounds.
fn validate_range(key: &str, min: i64, max: i64, config: &ohpm_core::config::Config) -> Result<()> {
    let value = config.get_number(key);
    if value < min || value > max {
        return Err(anyhow!(OhpmError::executor_number_invalid(key, min, max)));
    }
    Ok(())
}
