//! `ohpm-rs prepublish [<har_or_tgz_file> | <source_dir>] [--workspace]` —
//! validate a package without publishing. With no argument the current package
//! directory is validated directly from source; `--workspace`/`--filter`
//! validate every (selected) publishable workspace member.

use anyhow::{anyhow, Result};
use ohpm_core::config::find_local_prefix;
use ohpm_core::publish::{self, PublishRequest};
use ohpm_core::workspace::Workspace;

use super::{load_config, output};
use crate::cli::PrepublishArgs;

pub async fn run(args: &PrepublishArgs) -> Result<()> {
    let config = load_config()?;
    let cwd = std::env::current_dir()?;

    let batch = args.workspace || !args.filter.is_empty();
    if batch {
        if args.file.is_some() {
            anyhow::bail!("--workspace/--filter cannot be combined with a file argument.");
        }
        return run_workspace(&config, args, &cwd).await;
    }

    let (input, package_root) = match args.file.as_deref() {
        Some(arg) => {
            let p = std::path::PathBuf::from(arg);
            if p.is_dir() {
                (arg.to_string(), p)
            } else {
                (arg.to_string(), find_local_prefix(&cwd).unwrap_or(cwd))
            }
        }
        None => {
            let local = find_local_prefix(&cwd).ok_or_else(|| {
                anyhow!(
                    "No {} found in the current directory. Run prepublish from a package \
                     directory, or pass a har/tgz file explicitly.",
                    ohpm_core::constants::MY_PACKAGE_JSON
                )
            })?;
            (local.to_string_lossy().into_owned(), local)
        }
    };

    let req = PublishRequest {
        file: input,
        package_root: Some(package_root),
        ..Default::default()
    };
    let outcome = publish::prepublish(&config, &req).await?;
    output::succeed(&format!("prepublish {} {} succeed.", outcome.name, outcome.version));
    Ok(())
}

/// Validate every (filtered, publishable) workspace member.
async fn run_workspace(
    config: &ohpm_core::config::Config,
    args: &PrepublishArgs,
    cwd: &std::path::Path,
) -> Result<()> {
    let ws = Workspace::find(cwd)?.ok_or_else(|| {
        anyhow!(
            "No {} found walking up from the current directory; --workspace/--filter require a \
             workspace.",
            ohpm_core::workspace::WORKSPACE_CONFIG
        )
    })?;
    let base = PublishRequest::default();
    let outcomes = publish::prepublish_workspace(config, &ws, &args.filter, &base).await?;
    for outcome in &outcomes {
        output::succeed(&format!("prepublish {} {} succeed.", outcome.name, outcome.version));
    }
    if !outcomes.is_empty() {
        output::succeed(&format!("validated {} package(s)", outcomes.len()));
    } else {
        output::output("no package was validated (all selected members are publish: false)");
    }
    Ok(())
}
