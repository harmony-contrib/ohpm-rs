//! `ohpm-rs pack [source_dir] [--output <dir>] [--workspace] [--filter <pkgs>]` —
//! build `<name>-<version>.har` packages (npm-pack style).
//!
//! Single mode packs one source directory; `--workspace` / `--filter` pack every
//! (selected) publishable workspace member.

use anyhow::{anyhow, Result};
use ohpm_core::config::find_local_prefix;
use ohpm_core::pack;
use ohpm_core::workspace::Workspace;

use super::{load_config, output};
use crate::cli::PackArgs;

pub async fn run(args: &PackArgs) -> Result<()> {
    let _config = load_config()?;
    let cwd = std::env::current_dir()?;

    let batch = args.workspace || !args.filter.is_empty();
    if batch && args.source.is_some() {
        anyhow::bail!("--workspace/--filter cannot be combined with a source_dir argument.");
    }

    let output_dir = args
        .output
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or(cwd.clone());

    if batch {
        run_batch(&cwd, &output_dir, &args.filter)
    } else {
        run_single(&cwd, &output_dir, args.source.as_deref())
    }
}

/// Pack every (filtered, publishable) workspace member.
fn run_batch(cwd: &std::path::Path, output_dir: &std::path::Path, filter: &[String]) -> Result<()> {
    let ws = Workspace::find(cwd)?.ok_or_else(|| {
        anyhow!(
            "No {} found walking up from the current directory; --workspace/--filter require a workspace.",
            ohpm_core::workspace::WORKSPACE_CONFIG
        )
    })?;
    let selected = ws.filtered_members(filter)?;

    let mut packed = 0usize;
    for member in &selected {
        if !member.manifest.publishable() {
            output::output(&format!(
                "skip {}: publish is false",
                member.manifest.name
            ));
            continue;
        }
        let outcome = pack::pack(&member.dir, output_dir)?;
        output::output(&format!(
            "packed {}@{} -> {} ({} files)",
            outcome.name,
            outcome.version,
            outcome.har_path.display(),
            outcome.entry_count
        ));
        packed += 1;
    }
    if packed > 0 {
        output::succeed(&format!("packed {packed} package(s)"));
    } else {
        output::output("no package was packed (all selected members are publish: false)");
    }
    Ok(())
}

/// Pack a single source directory.
fn run_single(cwd: &std::path::Path, output_dir: &std::path::Path, source: Option<&str>) -> Result<()> {
    let source = match source {
        Some(dir) => std::path::PathBuf::from(dir),
        None => find_local_prefix(cwd).unwrap_or_else(|| cwd.to_path_buf()),
    };
    if !source.is_dir() {
        return Err(anyhow!(
            "The source \"{}\" is not a directory containing oh-package.json5.",
            source.display()
        ));
    }

    let outcome = pack::pack(&source, output_dir)?;
    output::succeed(&format!(
        "packed {}@{} -> {} ({} files, {} bytes)",
        outcome.name,
        outcome.version,
        outcome.har_path.display(),
        outcome.entry_count,
        outcome.size_bytes
    ));
    Ok(())
}
