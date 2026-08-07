//! `ohpm cache <clean|path|status|add>` — manage the local store (the `cache`
//! config directory, default `~/.ohpm/cache`).
//!
//! `clean` mirrors `lib/commands/cache.js` + `lib/core/cache/index.js` (the
//! `content-v1` / `harball` directories are removed); `path` / `status` /
//! `add` are pnpm-`pnpm store`-aligned extensions (≈ `pnpm store path` /
//! `pnpm store status` / `pnpm store add`).

use anyhow::{anyhow, Result};
use ohpm_core::cache;
use ohpm_core::error::OhpmError;
use ohpm_core::registry::RegistryClient;

use super::{load_config, output};

/// The subcommands accepted by `ohpm cache` (the reference only has `clean`).
const SUBCOMMANDS: [&str; 4] = ["clean", "path", "status", "add"];

fn available_list() -> String {
    format!("{}.", SUBCOMMANDS.join(", "))
}

pub async fn run(action: Option<String>, pkgs: &[String]) -> Result<()> {
    let config = load_config()?;
    let Some(action) = action else {
        return Err(anyhow!(OhpmError::new(
            "CacheSubcommandIsEmpty",
            format!(
                "Missing subcommand for \"ohpm\" cache - available subcommands: {}",
                available_list()
            ),
        )));
    };
    let start = std::time::Instant::now();
    match action.as_str() {
        "clean" => {
            cache::clean_all(&config)?;
            let elapsed = start.elapsed().as_millis();
            output::succeed(&format!(
                "cache clean completed in {}s {}ms",
                elapsed / 1000,
                elapsed % 1000
            ));
        }
        "path" => {
            // `pnpm store path` — print the effective store dir.
            println!("{}", cache::store_path(&config).display());
        }
        "status" => {
            let corrupted = cache::status(&config)?;
            if corrupted.is_empty() {
                output::succeed("cache status: the store is valid");
            } else {
                for p in &corrupted {
                    eprintln!("cache status: corrupted file: {}", p.display());
                }
                return Err(anyhow!(OhpmError::new(
                    "CacheStatusCorrupted",
                    format!("The store contains {} corrupted file(s).", corrupted.len()),
                )));
            }
        }
        "add" => {
            if pkgs.is_empty() {
                return Err(anyhow!(OhpmError::new(
                    "CacheAddPackageIsEmpty",
                    "Missing package for \"ohpm\" cache add - e.g. \"ohpm cache add @ohos/foo@1.2.3\".",
                )));
            }
            let client = RegistryClient::from_config(&config)?;
            let fetched = cache::add(&client, &config, pkgs).await?;
            let elapsed = start.elapsed().as_millis();
            output::succeed(&format!(
                "cache add completed in {}s {}ms ({} package(s))",
                elapsed / 1000,
                elapsed % 1000,
                fetched
            ));
        }
        _ => {
            return Err(anyhow!(OhpmError::new(
                "CacheSubcommandNotSupport",
                format!(
                    "Not support subcommand \"{action}\" for \"ohpm\" cache - available subcommands: {}",
                    available_list()
                ),
            )));
        }
    }
    Ok(())
}
