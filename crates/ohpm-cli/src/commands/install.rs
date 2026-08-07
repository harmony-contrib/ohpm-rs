//! `ohpm install [pkg...]` — install dependencies (mirrors
//! `lib/commands/install.js` + `lib/core/install/service/install.js`).

use anyhow::Result;
use ohpm_core::install::InstallOptions;
use ohpm_core::registry::RegistryClient;

use super::{apply_cli_options, load_config, output, resolve_prefix};
use crate::cli::InstallArgs;

pub async fn run(args: &InstallArgs) -> Result<()> {
    let mut config = load_config()?;
    apply_cli_options(
        &mut config,
        args.fetch_timeout,
        args.strict_ssl,
        args.max_concurrent,
        args.retry_times,
        args.retry_interval,
        args.registry.as_deref(),
        args.cache.as_deref(),
    )?;
    let prefix = resolve_prefix(args.prefix.as_deref(), "install")?;

    let opts = InstallOptions {
        save: !args.no_save,
        save_dev: args.save_dev,
        save_prod: args.save_prod,
        save_dynamic: args.save_dynamic,
        link: !args.no_link,
        all: args.all,
        prefix: Some(prefix.clone()),
        parameter_file: args.parameter_file.as_deref().map(std::path::PathBuf::from),
        target_path: args.target_path.as_deref().map(std::path::PathBuf::from),
        registry: args.registry.clone(),
        fetch_timeout: args.fetch_timeout,
        strict_ssl: args.strict_ssl,
        max_concurrent: args.max_concurrent,
        retry_times: args.retry_times,
        retry_interval: args.retry_interval,
        ..Default::default()
    };
    ohpm_core::install::valid_cli_options("install", &opts)?;


    let client = RegistryClient::from_config(&config)?;
    let start = std::time::Instant::now();
    let _outcome = ohpm_core::install::install(&client, &config, &prefix, &args.pkg, &opts).await?;

    let elapsed = start.elapsed().as_millis();
    output::succeed(&format!(
        "install completed in {}s {}ms",
        elapsed / 1000,
        elapsed % 1000
    ));
    Ok(())
}
