//! `ohpm install [pkg...] [--workspace|--filter]` — install dependencies.

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use ohpm_core::config::Config;
use ohpm_core::install::InstallOptions;
use ohpm_core::registry::RegistryClient;
use ohpm_core::workspace::{Workspace, WORKSPACE_CONFIG};

use super::{apply_cli_options, load_config, output, resolve_prefix};
use crate::cli::InstallArgs;

pub async fn run(args: &InstallArgs) -> Result<()> {
    InstallRunner::new(args)?.run().await
}

/// Compose ordinary installs for a validated, deterministic member selection.
struct InstallRunner<'a> {
    args: &'a InstallArgs,
    prefixes: Vec<PathBuf>,
    config: Config,
    client: RegistryClient,
    options: InstallOptions,
}

impl<'a> InstallRunner<'a> {
    fn new(args: &'a InstallArgs) -> Result<Self> {
        let prefixes = if args.workspace || !args.filter.is_empty() {
            Self::workspace_prefixes(args)?
        } else {
            vec![resolve_prefix(args.prefix.as_deref(), "install")?]
        };
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

        let options = InstallOptions {
            save: !args.no_save,
            save_dev: args.save_dev,
            save_prod: args.save_prod,
            save_dynamic: args.save_dynamic,
            link: !args.no_link,
            all: args.all,
            lockfile_only: args.lockfile_only,
            parameter_file: args.parameter_file.as_deref().map(PathBuf::from),
            target_path: args.target_path.as_deref().map(PathBuf::from),
            registry: args.registry.clone(),
            fetch_timeout: args.fetch_timeout,
            strict_ssl: args.strict_ssl,
            max_concurrent: args.max_concurrent,
            retry_times: args.retry_times,
            retry_interval: args.retry_interval,
            ..Default::default()
        };
        ohpm_core::install::valid_cli_options("install", &options)?;
        let client = RegistryClient::from_config(&config)?;
        Ok(Self {
            args,
            prefixes,
            config,
            client,
            options,
        })
    }

    fn workspace_prefixes(args: &InstallArgs) -> Result<Vec<PathBuf>> {
        let cwd = std::env::current_dir()?;
        let anchor = args
            .prefix
            .as_ref()
            .map(|prefix| cwd.join(prefix))
            .unwrap_or(cwd);
        if !anchor.is_dir() {
            anyhow::bail!(
                "The workspace prefix is not a directory: {}",
                anchor.display()
            );
        }
        let workspace = Workspace::find(&anchor)?.ok_or_else(|| {
            anyhow!(
                "No {WORKSPACE_CONFIG} found walking up from {}; --workspace/--filter require a workspace.",
                anchor.display()
            )
        })?;
        let mut prefixes: Vec<PathBuf> = workspace
            .filtered_members(&args.filter)?
            .into_iter()
            .map(|member| member.dir.clone())
            .collect();
        prefixes.sort();
        prefixes.dedup();
        Ok(prefixes)
    }

    async fn run(self) -> Result<()> {
        let start = std::time::Instant::now();
        for prefix in &self.prefixes {
            let options = InstallOptions {
                prefix: Some(prefix.clone()),
                ..self.options.clone()
            };
            ohpm_core::install::install(
                &self.client,
                &self.config,
                prefix,
                &self.args.pkg,
                &options,
            )
            .await?;
        }
        let elapsed = start.elapsed().as_millis();
        output::succeed(&format!(
            "install completed in {}s {}ms",
            elapsed / 1000,
            elapsed % 1000
        ));
        Ok(())
    }
}
