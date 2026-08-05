//! `ohpm-rs publish [<har_or_tgz_file> | <source_dir>]` — publish a package.
//!
//! With no argument the current package directory is published directly from
//! source (auto-packed first, the default behavior). Passing a `.har`/`.tgz`
//! publishes the pre-built package; passing a directory packs and publishes it.
//! Authentication works entirely from environment variables or CLI flags —
//! no interactive prompts.

use anyhow::{anyhow, Result};
use ohpm_core::config::default::types;
use ohpm_core::config::find_local_prefix;
use ohpm_core::publish::{self, PublishRequest};
use ohpm_core::registry::login::LoginOverrides;
use ohpm_core::registry::RegistryClient;

use super::{load_config, output};
use crate::cli::PublishArgs;

pub async fn run(args: &PublishArgs) -> Result<()> {
    let mut config = load_config()?;
    if let Some(timeout) = args.timeout {
        config.set_cli(types::FETCH_TIMEOUT, &timeout.to_string());
    }
    let client = RegistryClient::from_config(&config)?;

    let cwd = std::env::current_dir()?;
    let (input, package_root) = resolve_input(args.file.as_deref(), &cwd)?;

    let req = PublishRequest {
        file: input,
        tag: args.tag.clone(),
        publish_registry: args.publish_registry.clone(),
        login: LoginOverrides {
            publish_id: args.publish_id.clone(),
            key_path: args.key_path.clone(),
            key_content: args.key_content.clone(),
            passphrase: args.passphrase.clone(), // or OHPM_KEY_PASSPHRASE / key_passphrase
        },
        timeout: args.timeout,
        package_root: Some(package_root),
    };

    let outcome = publish::publish(&client, &config, &req).await?;
    output::succeed(&format!("+{} {}", outcome.name, outcome.version));
    if let Some(msg) = outcome.additional_msg {
        output::output(&msg);
    }
    Ok(())
}

/// Resolve the publish input: an explicit file/directory argument, or the
/// current package directory (publish-from-source default).
fn resolve_input(input: Option<&str>, cwd: &std::path::Path) -> Result<(String, std::path::PathBuf)> {
    match input {
        Some(arg) => {
            let p = std::path::PathBuf::from(arg);
            if p.is_dir() {
                Ok((arg.to_string(), p))
            } else {
                Ok((arg.to_string(), package_source_root(cwd)))
            }
        }
        None => {
            let local = find_local_prefix(cwd).ok_or_else(|| {
                anyhow!(
                    "No {} found in the current directory. Run publish from a package directory \
                     (publish-from-source), or pass a har/tgz file explicitly.",
                    ohpm_core::constants::MY_PACKAGE_JSON
                )
            })?;
            Ok((local.to_string_lossy().into_owned(), local))
        }
    }
}

/// The source root used to resolve `file:` workspace dependencies: the nearest
/// dir with `oh-package.json5` walking up from the cwd.
fn package_source_root(cwd: &std::path::Path) -> std::path::PathBuf {
    find_local_prefix(cwd).unwrap_or_else(|| cwd.to_path_buf())
}
