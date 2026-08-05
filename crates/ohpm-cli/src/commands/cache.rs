//! `ohpm cache clean` — clean the ohpm cache folder.

use anyhow::{anyhow, Result};
use ohpm_core::config::default::{default_cache, types};
use std::path::PathBuf;

use super::{load_config, output};

pub async fn run(action: Option<String>) -> Result<()> {
    let config = load_config()?;
    match action.as_deref() {
        Some("clean") => {
            let cache = config.get_string(types::CACHE);
            let path = if cache.is_empty() {
                default_cache()
            } else {
                PathBuf::from(cache)
            };
            if path.exists() {
                std::fs::remove_dir_all(&path)?;
            }
            output::succeed(&format!("cache cleaned: {}", path.display()));
            Ok(())
        }
        _ => Err(anyhow!("Usage: ohpm cache clean")),
    }
}
