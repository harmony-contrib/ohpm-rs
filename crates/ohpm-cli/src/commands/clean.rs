//! `ohpm clean [--kl|--keep-lockfile]` — delete the `oh_modules` directories
//! and the lockfiles of the project (mirrors `lib/commands/clean.js`).

use anyhow::Result;
use ohpm_core::clean::start_clean;

use super::{load_config, output};
use crate::cli::CleanArgs;

pub async fn run(args: &CleanArgs) -> Result<()> {
    let config = load_config()?;
    let start = std::time::Instant::now();
    start_clean(&config, args.keep_lockfile)?;
    let elapsed = start.elapsed().as_millis();
    output::succeed(&format!(
        "clean completed in {}s {}ms",
        elapsed / 1000,
        elapsed % 1000
    ));
    Ok(())
}
