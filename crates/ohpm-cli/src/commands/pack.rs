//! `ohpm-rs pack [source_dir] [--output <dir>]` — build a `<name>-<version>.har`
//! from a source directory (npm-pack style).

use anyhow::{anyhow, Result};
use ohpm_core::config::find_local_prefix;
use ohpm_core::pack;

use super::{load_config, output};
use crate::cli::PackArgs;

pub async fn run(args: &PackArgs) -> Result<()> {
    let _config = load_config()?;
    let cwd = std::env::current_dir()?;

    let source = match &args.source {
        Some(dir) => std::path::PathBuf::from(dir),
        None => find_local_prefix(&cwd).unwrap_or_else(|| cwd.clone()),
    };
    if !source.is_dir() {
        return Err(anyhow!(
            "The source \"{}\" is not a directory containing oh-package.json5.",
            source.display()
        ));
    }

    let output_dir = args
        .output
        .as_ref()
        .map(std::path::PathBuf::from)
        .unwrap_or(cwd);

    let outcome = pack::pack(&source, &output_dir)?;
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
