//! `ohpm unpublish <pkg[@version]>` — delete a package version.

use anyhow::Result;
use ohpm_core::config::default::types;
use ohpm_core::publish::{self, UnpublishRequest};
use ohpm_core::registry::login::LoginOverrides;
use ohpm_core::registry::RegistryClient;

use super::{load_config, output};
use crate::cli::UnpublishArgs;

pub async fn run(args: &UnpublishArgs) -> Result<()> {
    let mut config = load_config()?;
    if let Some(timeout) = args.timeout {
        config.set_cli(types::FETCH_TIMEOUT, &timeout.to_string());
    }
    let client = RegistryClient::from_config(&config)?;

    let req = UnpublishRequest {
        pkg: args.pkg.clone().unwrap_or_default(),
        force: args.force,
        publish_registry: args.publish_registry.clone(),
        login: LoginOverrides {
            publish_id: args.publish_id.clone(),
            key_path: args.key_path.clone(),
            passphrase: None,
        },
    };
    publish::unpublish(&client, &config, &req).await?;
    output::succeed("unpublish succeed.");
    Ok(())
}
