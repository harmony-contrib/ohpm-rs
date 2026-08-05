//! `ohpm config set/get/delete/list` — manage the `.ohpmrc` file.
//!
//! Token keys (ending in `:_auth` / `:_read_auth`) can be set and persisted,
//! e.g. `ohpm config set "//registry/:_auth" <token>`.

use anyhow::{anyhow, Result};
use ohpm_core::config::default::{access_token_type, ConfigValue};
use ohpm_core::config::Config;

use super::{load_config, output};
use crate::cli::ConfigArgs;

/// Keys hidden from `get`/`list` (mirrors the `m()` filter in `config.js`).
fn is_protected_key(key: &str) -> bool {
    key.starts_with('_') || key == access_token_type::READ || key == access_token_type::READ_WRITE
}

pub async fn run(args: &ConfigArgs) -> Result<()> {
    let mut config = load_config()?;
    let action = args.action.as_deref().unwrap_or_default();

    match action {
        "set" => {
            let (key, value) = match (&args.key, &args.value) {
                (Some(k), Some(v)) => (k, v),
                _ => return Err(anyhow!("Usage: ohpm config set <key> <value>")),
            };
            config.set(key, value);
            config.save()?;
        }
        "get" => {
            let key = args.key.as_deref().ok_or_else(|| anyhow!("Usage: ohpm config get <key>"))?;
            if is_protected_key(key) {
                return Err(anyhow!("The key \"{key}\" is protected and cannot be read."));
            }
            output::output(&config.get_string(key));
        }
        "delete" => {
            let key = args.key.as_deref().ok_or_else(|| anyhow!("Usage: ohpm config delete <key>"))?;
            if is_protected_key(key) {
                return Err(anyhow!("The key \"{key}\" is protected and cannot be deleted."));
            }
            if !config.delete(key) {
                return Err(anyhow!("The config key \"{key}\" does not exist."));
            }
            config.save()?;
        }
        "list" | "ls" => {
            list(&config, args.json)?;
        }
        "encrypt" => {
            return Err(anyhow!("The \"encrypt\" subcommand requires a crypto component and is not \
                                 supported in this Rust reimplementation."))
        }
        other => {
            return Err(anyhow!(
                "The config subcommand \"{other}\" is not supported. Use set/get/delete/list."
            ))
        }
    }
    Ok(())
}

fn list(config: &Config, json: bool) -> Result<()> {
    let mut visible: Vec<(String, ConfigValue)> = Vec::new();
    for key in config.effective_keys() {
        if is_protected_key(&key) {
            continue;
        }
        if let Some(v) = config.get(&key) {
            visible.push((key, v.clone()));
        }
    }

    if json {
        let mut map = serde_json::Map::new();
        for (k, v) in &visible {
            map.insert(k.clone(), serde_json::Value::String(v.as_str()));
        }
        output::output(&serde_json::to_string_pretty(&serde_json::Value::Object(map))?);
    } else {
        for (k, v) in &visible {
            output::output(&format!("{k} = {}", v.as_str()));
        }
    }
    Ok(())
}
