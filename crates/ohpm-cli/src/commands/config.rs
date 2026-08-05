//! `ohpm-rs config <action> [key] [value]` — manage the `.ohpmrc` file,
//! compatible with the reference ohpm `config` command:
//!
//! * `set <key> <value>` — validates the value (type/ranges) and warns on
//!   invalid input instead of storing it; unknown keys are warned about.
//! * `get <key>` — prints the effective value; `get` with no key prints the
//!   full list, like the reference.
//! * `delete <key>` — removes the key from the user config.
//! * `list` / `ls` — per-source sections with override annotations;
//!   `-j/--json` prints the effective values as typed JSON.
//! * `encrypt` — requires the native crypto component; not supported here.

use anyhow::{anyhow, Result};
use ohpm_core::config::default::access_token_type;
use ohpm_core::config::{type_validate, Config};

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
                (Some(k), Some(v)) => (k.trim(), v.trim()),
                _ => return Err(anyhow!("Usage: ohpm config set <key> <value>")),
            };
            if config.set(key, value) {
                config.save()?;
            }
        }
        "get" => match args.key.as_deref() {
            Some(key) => {
                let key = key.trim();
                if is_protected_key(key) {
                    return Err(anyhow!("The key \"{key}\" is protected and cannot be read."));
                }
                // Like the reference: a `get` without a key prints the list.
                if key.is_empty() {
                    print_list(&config, args.json);
                } else {
                    output::output(&config.get_string(key));
                }
            }
            None => print_list(&config, args.json),
        },
        "delete" => {
            let key = args.key.as_deref().ok_or_else(|| anyhow!("Usage: ohpm config delete <key>"))?;
            let key = key.trim();
            if is_protected_key(key) {
                return Err(anyhow!("The key \"{key}\" is protected and cannot be deleted."));
            }
            if !config.delete(key) {
                return Err(anyhow!("The config key \"{key}\" does not exist."));
            }
            config.save()?;
        }
        "list" | "ls" => print_list(&config, args.json),
        "encrypt" => {
            return Err(anyhow!(
                "The \"encrypt\" subcommand requires the native crypto component and is not \
                 supported in this Rust reimplementation."
            ))
        }
        other => {
            return Err(anyhow!(
                "The config subcommand \"{other}\" is not supported. Use set/get/delete/list."
            ))
        }
    }
    Ok(())
}

/// The effective config as typed JSON (`config list -j`).
fn json_list(config: &Config) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for key in config.effective_keys() {
        if is_protected_key(&key) {
            continue;
        }
        if let Some(v) = config.get(&key) {
            map.insert(key, type_validate::to_json(v));
        }
    }
    serde_json::Value::Object(map)
}

/// `config list` / `config get` without a key: per-source sections with
/// override annotations, mirroring `config.js` `h()`.
fn print_list(config: &Config, json: bool) {
    if json {
        output::output(&serde_json::to_string_pretty(&json_list(config)).unwrap_or_default());
        return;
    }
    let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
    let home = dirs::home_dir().map(|p| p.display().to_string()).unwrap_or_default();

    let mut lines: Vec<String> = Vec::new();
    for (source, label, entries) in config.list_sections() {
        if source == "default" {
            continue; // the default section is only shown with --long
        }
        lines.push(format!("; \"{source}\" config from {label}"));
        lines.push(String::new());
        for (key, value) in &entries {
            if is_protected_key(key) {
                continue;
            }
            let holder = config.find(key);
            let (comment, note) = if holder != source && !holder.is_empty() {
                ("; ", format!("; overridden by {holder}"))
            } else {
                ("", String::new())
            };
            // Values are JSON-stringified like the reference (strings quoted,
            // booleans/numbers raw).
            lines.push(format!(
                "{comment}{key} = {}{note}",
                type_validate::to_json(value)
            ));
        }
        lines.push(String::new());
    }

    lines.push(format!(
        "; \"user\" config from {}",
        config.user_rc().display()
    ));
    lines.push(format!("; node bin location = {}", std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default()));
    lines.push(format!("; node version = (rust {} {})", std::env::consts::ARCH, std::env::consts::OS));
    lines.push(format!("; {} local prefix = {home}", ohpm_core::constants::PM));
    lines.push(format!("; {} version = {}", ohpm_core::constants::PM, ohpm_core::constants::PM_DISPLAY_VERSION));
    lines.push(format!("; cwd = {cwd}"));
    lines.push(format!("; HOME = {home}"));

    output::output(lines.join("\n").trim());
}
