//! `ohpm clean [--kl|--keep-lockfile] [--workspace|--filter]` — delete the
//! `oh_modules` directories and the lockfiles of the project (mirrors
//! `lib/commands/clean.js`); `--workspace`/`--filter` extend it to the
//! workspace members (like the publish/version batch modes).

use anyhow::{anyhow, Result};

use super::{load_config, output};

pub async fn run(args: &crate::cli::CleanArgs) -> Result<()> {
    let _config = load_config()?;
    let start = std::time::Instant::now();
    if args.workspace || !args.filter.is_empty() {
        let cwd = std::env::current_dir()?;
        let ws = ohpm_core::workspace::Workspace::find(&cwd)?.ok_or_else(|| {
            anyhow!(
                "No {} found walking up from the current directory; --workspace/--filter require a \
                 workspace.",
                ohpm_core::workspace::WORKSPACE_CONFIG
            )
        })?;
        ohpm_core::clean::clean_workspace(&ws, &args.filter, args.keep_lockfile)?;
    } else {
        ohpm_core::clean::start_clean(args.keep_lockfile)?;
    }
    let elapsed = start.elapsed().as_millis();
    output::succeed(&format!(
        "clean completed in {}s {}ms",
        elapsed / 1000,
        elapsed % 1000
    ));
    Ok(())
}
