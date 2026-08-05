//! `ohpm publish` — the flagship command.
//!
//! Authentication works entirely from environment variables
//! (`OHPM_ACCESS_TOKEN`, or `OHPM_PUBLISH_ID` + `OHPM_KEY_PATH` +
//! `OHPM_KEY_PASSPHRASE` for the SSH-key login flow). No interactive prompts.

use anyhow::Result;
use ohpm_core::config::default::types;
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

    let req = PublishRequest {
        file: args.file.clone(),
        tag: args.tag.clone(),
        publish_registry: args.publish_registry.clone(),
        login: LoginOverrides {
            publish_id: args.publish_id.clone(),
            key_path: args.key_path.clone(),
            passphrase: None, // publish only reads the passphrase from env/config
        },
        timeout: args.timeout,
        package_root: package_source_root(),
    };

    let outcome = publish::publish(&client, &config, &req).await?;
    output::succeed(&format!("+{} {}", outcome.name, outcome.version));
    if let Some(msg) = outcome.additional_msg {
        output::output(&msg);
    }
    Ok(())
}

/// The source root of the package being published: the nearest dir with
/// `oh-package.json5` walking up from the cwd (used to resolve `file:`
/// workspace dependencies).
fn package_source_root() -> Option<std::path::PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    Some(ohpm_core::config::find_local_prefix(&cwd).unwrap_or(cwd))
}
