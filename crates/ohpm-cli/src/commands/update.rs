//! `ohpm update [pkg...]` — update package(s) to their latest version
//! (mirrors `lib/commands/update.js` + `lib/core/install/service/update.js`).

use anyhow::Result;
use ohpm_core::install::InstallOptions;
use ohpm_core::registry::RegistryClient;

use super::{apply_cli_options, load_config, output, resolve_prefix};
use crate::cli::UpdateArgs;

pub async fn run(args: &UpdateArgs) -> Result<()> {
    let mut config = load_config()?;
    apply_cli_options(
        &mut config,
        args.fetch_timeout,
        args.strict_ssl,
        args.max_concurrent,
        args.retry_times,
        args.retry_interval,
        args.registry.as_deref(),
    )?;
    let prefix = resolve_prefix(args.prefix.as_deref(), "update")?;

    let opts = InstallOptions {
        all: args.all,
        all_modules: args.all_modules,
        tag_filter: args.tag_filter.clone(),
        prefix: Some(prefix.clone()),
        registry: args.registry.clone(),
        fetch_timeout: args.fetch_timeout,
        strict_ssl: args.strict_ssl,
        max_concurrent: args.max_concurrent,
        retry_times: args.retry_times,
        retry_interval: args.retry_interval,
        ..Default::default()
    };
    ohpm_core::install::valid_cli_options("update", &opts)?;


    let client = RegistryClient::from_config(&config)?;
    let start = std::time::Instant::now();
    let _outcome = ohpm_core::install::update(&client, &config, &prefix, &args.pkg, &opts).await?;
    let elapsed = start.elapsed().as_millis();
    output::succeed(&format!(
        "update completed in {}s {}ms",
        elapsed / 1000,
        elapsed % 1000
    ));
    Ok(())
}
