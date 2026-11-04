//! Clean a project/package or a selected set of workspace members.

use anyhow::{anyhow, Result};
use ohpm_core::clean::Cleaner;
use ohpm_core::workspace::{Workspace, WORKSPACE_CONFIG};

use super::{load_config, output};

pub async fn run(args: &crate::cli::CleanArgs) -> Result<()> {
    let _config = load_config()?;
    let cwd = std::env::current_dir()?;
    let anchor = args
        .prefix
        .as_ref()
        .map(|prefix| cwd.join(prefix))
        .unwrap_or(cwd);
    if !anchor.is_dir() {
        anyhow::bail!("The clean prefix is not a directory: {}", anchor.display());
    }
    let cleaner = if args.workspace || !args.filter.is_empty() {
        let workspace = Workspace::find(&anchor)?.ok_or_else(|| {
            anyhow!(
                "No {WORKSPACE_CONFIG} found walking up from {}; --workspace/--filter require a workspace.",
                anchor.display()
            )
        })?;
        Cleaner::for_workspace(&workspace, &args.filter, args.keep_lockfile)?
    } else {
        Cleaner::for_directory(&anchor, args.keep_lockfile)?
    };
    let start = std::time::Instant::now();
    cleaner.run()?;
    let elapsed = start.elapsed().as_millis();
    output::succeed(&format!(
        "clean completed in {}s {}ms",
        elapsed / 1000,
        elapsed % 1000
    ));
    Ok(())
}
