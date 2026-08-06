//! `ohpm cache clean` — clean the ohpm cache folder (mirrors
//! `lib/commands/cache.js` + `lib/core/cache/index.js`: the `content-v1` and
//! `harball` directories are removed).

use anyhow::{anyhow, Result};
use ohpm_core::config::default::{default_cache, types};
use ohpm_core::error::OhpmError;
use std::path::PathBuf;

use super::{load_config, output};

pub async fn run(action: Option<String>) -> Result<()> {
    let config = load_config()?;
    let Some(action) = action else {
        return Err(anyhow!(OhpmError::new(
            "CacheSubcommandIsEmpty",
            "Missing subcommand for \"ohpm\" cache - available subcommands: clean, -h.",
        )));
    };
    if action != "clean" {
        return Err(anyhow!(OhpmError::new(
            "CacheSubcommandNotSupport",
            format!(
                "Not support subcommand \"{action}\" for \"ohpm\" cache - available subcommands: clean, -h."
            ),
        )));
    }
    let cache = config.get_string(types::CACHE);
    let cache_root = if cache.is_empty() {
        default_cache()
    } else {
        PathBuf::from(cache)
    };
    let start = std::time::Instant::now();
    // `cleanAllCaches` — the content dir + the harball dir.
    for dir in ["content-v1", "harball"] {
        let path = cache_root.join(dir);
        if path.exists() {
            std::fs::remove_dir_all(&path)?;
        }
    }
    let elapsed = start.elapsed().as_millis();
    output::succeed(&format!(
        "cache clean completed in {}s {}ms",
        elapsed / 1000,
        elapsed % 1000
    ));
    Ok(())
}
