//! `ohpm init` — create an `oh-package.json5`.
//!
//! Non-interactive: writes the same defaults the reference `init` writes when
//! run with `-y`, honoring `-g <group>` for the package scope.

use anyhow::{anyhow, Result};
use ohpm_core::constants::MY_PACKAGE_JSON;
use ohpm_core::package::validate::is_standard_package_name;

use super::{load_config, output};
use crate::cli::InitArgs;

pub async fn run(args: &InitArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let _config = load_config()?;

    // Default name: the directory name, lowercased; or the existing manifest name.
    let mut name = cwd
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| "unnamed".to_string());

    let existing = cwd.join(MY_PACKAGE_JSON);
    if existing.exists() {
        if let Ok(text) = std::fs::read_to_string(&existing) {
            if let Ok(m) = ohpm_core::package::Manifest::from_json5(&text) {
                if !m.name.is_empty() {
                    name = m.name;
                }
            }
        }
    }

    // Apply the group scope, matching `init.js`.
    if let Some(group) = &args.group {
        if let Some(local) = name.strip_prefix('@').and_then(|s| s.split_once('/')) {
            name = local.1.to_string();
        }
        name = format!("@{group}/{name}");
    }

    if !is_standard_package_name(&name) {
        return Err(anyhow!(
            "The package name \"{name}\" is invalid. It must be a valid ohpm package name \
             (lowercase letters, digits, '-', '_', '.'; at most 128 chars)."
        ));
    }

    let manifest = serde_json::json!({
        "name": name,
        "version": "1.0.0",
        "description": "",
        "main": "index.ets",
        "author": "",
        "license": "ISC",
        "dependencies": {},
    });
    let pretty = serde_json::to_string_pretty(&manifest)?;
    let path = cwd.join(MY_PACKAGE_JSON);
    std::fs::write(&path, format!("{pretty}\n"))?;

    output::output(&format!("Wrote to {}:\n", path.display()));
    output::output(&pretty);
    Ok(())
}
