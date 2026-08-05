//! `ohpm version [<newversion> | major | minor | patch] [--workspace] [--filter <pkgs>]`.
//!
//! Unified mode (bump every member to the same version) is triggered by
//! `--workspace`, or by `version.mode: unified` in `ohpm-workspace.yaml`. Otherwise
//! only the package containing the current directory is bumped (independent).

use anyhow::{anyhow, Result};
use ohpm_core::config::find_local_prefix;
use ohpm_core::constants::MY_PACKAGE_JSON;
use ohpm_core::version::{
    bump_manifest_file, bump_members, resolve_new_version, unified_base_version,
};
use ohpm_core::workspace::{VersionMode, Workspace, WORKSPACE_CONFIG};

use super::{load_config, output};
use crate::cli::VersionArgs;

pub async fn run(args: &VersionArgs) -> Result<()> {
    let _config = load_config()?;
    let cwd = std::env::current_dir()?;
    let action = args.action.as_deref().ok_or_else(|| {
        anyhow!(
            "Usage: ohpm version [--workspace] [--filter <pkgs>] [--preid <id>] \
             [<newversion> | major | minor | patch | pre* | prerelease]"
        )
    })?;
    let preid = args.preid.as_deref();

    let ws = Workspace::find(&cwd)?;
    let force_unified = args.workspace
        || ws
            .as_ref()
            .map(|w| w.version_mode == VersionMode::Unified)
            .unwrap_or(false);

    if force_unified {
        let ws = ws.ok_or_else(|| {
            anyhow!(
                "No {} found walking up from the current directory; unified versioning requires a \
                 workspace.",
                WORKSPACE_CONFIG
            )
        })?;
        run_unified(&ws, action, &args.filter, preid)
    } else {
        if !args.filter.is_empty() {
            anyhow::bail!("--filter only applies in unified version mode (use --workspace).");
        }
        run_independent(&cwd, action, preid)
    }
}

/// Bump the selected (filtered, publishable) workspace members to one version.
fn run_unified(ws: &Workspace, action: &str, filter: &[String], preid: Option<&str>) -> Result<()> {
    let selected = ws.filtered_members(filter)?;
    if selected.is_empty() {
        return Err(anyhow!("No publishable workspace packages to bump."));
    }
    let base = unified_base_version(ws)?;
    let new_version = resolve_new_version(&base, action, preid)?;
    let bumped = bump_members(&selected, &new_version)?;

    for dir in &bumped {
        output::output(&format!("{base} -> {new_version}  {}", dir.display()));
    }
    if bumped.is_empty() {
        output::output("no package was bumped (all selected packages are publish: false)");
    } else {
        output::succeed(&format!("updated {} package(s) to {new_version}", bumped.len()));
    }
    Ok(())
}

/// Bump only the package containing the current directory.
fn run_independent(cwd: &std::path::Path, action: &str, preid: Option<&str>) -> Result<()> {
    let local = find_local_prefix(cwd)
        .ok_or_else(|| anyhow!("No {} found in the current directory.", MY_PACKAGE_JSON))?;
    let path = local.join(MY_PACKAGE_JSON);
    let text = std::fs::read_to_string(&path)?;
    let doc: serde_json::Value = json5::from_str(&text)?;
    let current = doc["version"].as_str().unwrap_or_default();
    let new_version = resolve_new_version(current, action, preid)?;
    let (old, new) = bump_manifest_file(&path, &new_version)?;
    output::output(&format!("{old} -> {new}"));
    Ok(())
}
