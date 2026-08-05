//! `ohpm root` — print the effective `oh_modules` folder.

use anyhow::{anyhow, Result};
use ohpm_core::config::find_local_prefix;
use ohpm_core::constants::MY_PACKAGE_JSON;

use super::{load_config, output};

pub async fn run() -> Result<()> {
    let _config = load_config()?;
    let cwd = std::env::current_dir()?;
    let local = find_local_prefix(&cwd)
        .ok_or_else(|| anyhow!("No {} found in the current directory.", MY_PACKAGE_JSON))?;
    output::output(&local.join(ohpm_core::constants::MY_MODULES).to_string_lossy());
    Ok(())
}
