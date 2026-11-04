//! Uninstall dependencies from a package or a selected set of workspace members.

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use ohpm_core::config::Config;
use ohpm_core::error::OhpmError;
use ohpm_core::install::modules::ModuleScope;
use ohpm_core::install::InstallOptions;
use ohpm_core::registry::RegistryClient;
use ohpm_core::workspace::{Workspace, WORKSPACE_CONFIG};

use super::{apply_cli_options, load_config, output, resolve_prefix};
use crate::cli::UninstallArgs;

pub async fn run(args: &UninstallArgs) -> Result<()> {
    UninstallRunner::new(args)?.run().await
}

/// Validate the whole request before executing members in a stable order.
struct UninstallRunner<'a> {
    args: &'a UninstallArgs,
    prefixes: Vec<PathBuf>,
    config: Config,
    client: RegistryClient,
    options: InstallOptions,
}

impl<'a> UninstallRunner<'a> {
    fn new(args: &'a UninstallArgs) -> Result<Self> {
        if args.pkg.is_empty() {
            return Err(OhpmError::uninstall_no_pkg().into());
        }
        for package in &args.pkg {
            if let Some(version) = ohpm_core::install::root::parse_cli_pkg_version(package) {
                return Err(OhpmError::uninstall_has_version(&version).into());
            }
            ohpm_core::install::spec::validate_name(package)?;
        }
        let workspace_mode = args.workspace || !args.filter.is_empty();
        let prefixes = if workspace_mode {
            Self::workspace_prefixes(args)?
        } else {
            vec![std::env::current_dir()?.join(resolve_prefix(args.prefix.as_deref(), "uninstall")?)]
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
            all: args.all,
            module_scope: if workspace_mode {
                ModuleScope::Package
            } else {
                ModuleScope::Project
            },
            registry: args.registry.clone(),
            fetch_timeout: args.fetch_timeout,
            strict_ssl: args.strict_ssl,
            max_concurrent: args.max_concurrent,
            retry_times: args.retry_times,
            retry_interval: args.retry_interval,
            ..Default::default()
        };
        ohpm_core::install::valid_cli_options("uninstall", &options)?;
        let client = RegistryClient::from_config(&config)?;
        Ok(Self {
            args,
            prefixes,
            config,
            client,
            options,
        })
    }

    fn workspace_prefixes(args: &UninstallArgs) -> Result<Vec<PathBuf>> {
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
            ohpm_core::install::uninstall(
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
            "uninstall completed in {}s {}ms",
            elapsed / 1000,
            elapsed % 1000
        ));
        Ok(())
    }
}
