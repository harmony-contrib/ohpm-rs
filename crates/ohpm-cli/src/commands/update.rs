//! `ohpm update [pkg...] [--workspace|--filter]` — update within declared ranges.

use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use ohpm_core::config::Config;
use ohpm_core::install::modules::ModuleScope;
use ohpm_core::install::{root::parse_cli_pkg_version, spec::validate_name, InstallOptions};
use ohpm_core::registry::RegistryClient;
use ohpm_core::workspace::{Workspace, WORKSPACE_CONFIG};
use ohpm_core::OhpmError;

use super::{apply_cli_options, load_config, output, resolve_prefix};
use crate::cli::UpdateArgs;

pub async fn run(args: &UpdateArgs) -> Result<()> {
    UpdateRunner::new(args)?.run().await
}

/// Validate the entire selection before updating members in a stable order.
struct UpdateRunner<'a> {
    args: &'a UpdateArgs,
    prefixes: Vec<PathBuf>,
    config: Config,
    client: RegistryClient,
    options: InstallOptions,
}

impl<'a> UpdateRunner<'a> {
    fn new(args: &'a UpdateArgs) -> Result<Self> {
        for name in &args.pkg {
            if let Some(version) = parse_cli_pkg_version(name) {
                return Err(OhpmError::update_has_version(&version).into());
            }
            validate_name(name)?;
        }

        let workspace = if args.workspace || !args.filter.is_empty() {
            Some(Self::find_workspace(args)?)
        } else {
            None
        };
        let prefixes = if let Some(workspace) = &workspace {
            let mut prefixes: Vec<PathBuf> = workspace
                .filtered_members(&args.filter)?
                .into_iter()
                .map(|member| member.dir.clone())
                .collect();
            prefixes.sort();
            prefixes.dedup();
            prefixes
        } else {
            vec![resolve_prefix(args.prefix.as_deref(), "update")?]
        };

        let mut config = if let Some(workspace) = &workspace {
            let mut config = Config::new();
            config.load(&workspace.root, None)?;
            config
        } else {
            load_config()?
        };
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
            all: args.all,
            all_modules: args.all_modules,
            module_scope: if workspace.is_some() {
                ModuleScope::Package
            } else {
                ModuleScope::Project
            },
            tag_filter: args.tag_filter.clone(),
            registry: args.registry.clone(),
            fetch_timeout: args.fetch_timeout,
            strict_ssl: args.strict_ssl,
            max_concurrent: args.max_concurrent,
            retry_times: args.retry_times,
            retry_interval: args.retry_interval,
            ..Default::default()
        };
        ohpm_core::install::valid_cli_options("update", &options)?;
        let client = RegistryClient::from_config(&config)?;
        Ok(Self {
            args,
            prefixes,
            config,
            client,
            options,
        })
    }

    fn find_workspace(args: &UpdateArgs) -> Result<Workspace> {
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
        Workspace::find(&anchor)?.ok_or_else(|| {
            anyhow!(
                "No {WORKSPACE_CONFIG} found walking up from {}; --workspace/--filter require a workspace.",
                anchor.display()
            )
        })
    }

    async fn run(self) -> Result<()> {
        let start = std::time::Instant::now();
        for prefix in &self.prefixes {
            let options = InstallOptions {
                prefix: Some(prefix.clone()),
                ..self.options.clone()
            };
            ohpm_core::install::update(
                &self.client,
                &self.config,
                prefix,
                &self.args.pkg,
                &options,
            )
            .await
            .with_context(|| format!("Failed to update {}", prefix.display()))?;
        }
        let elapsed = start.elapsed().as_millis();
        output::succeed(&format!(
            "update completed in {}s {}ms",
            elapsed / 1000,
            elapsed % 1000
        ));
        Ok(())
    }
}
