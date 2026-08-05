//! `ohpm login` — run the SSH-key login and persist the access token into the
//! user `.ohpmrc` as `{registry}:_auth`.

use anyhow::Result;
use ohpm_core::config::ensure_trailing_slash;
use ohpm_core::registry::auth;
use ohpm_core::registry::login::LoginOverrides;
use ohpm_core::registry::RegistryClient;

use super::{load_config, output};
use crate::cli::LoginArgs;

pub async fn run(args: &LoginArgs) -> Result<()> {
    let config = load_config()?;
    let client = RegistryClient::from_config(&config)?;

    let registry = login_registry(&config, args.publish_registry.as_deref());
    let overrides = LoginOverrides {
        publish_id: args.publish_id.clone(),
        key_path: args.key_path.clone(),
        passphrase: args.passphrase.clone(),
    };

    let token = auth::resolve_write_token(client.http(), &config, &registry, &overrides).await?;

    let mut config = config;
    config.set(&auth::write_token_key(&registry), &token);
    config.save()?;

    output::succeed(&format!("Login succeed, access token saved for {registry}"));
    Ok(())
}

/// Registry to log in against: `--publish_registry` > `publish_registry`
/// config > `registry` config (which defaults to the public registry).
pub fn login_registry(config: &ohpm_core::config::Config, cli: Option<&str>) -> String {
    if let Some(r) = cli.filter(|s| !s.is_empty()) {
        return ensure_trailing_slash(r);
    }
    let pr = config.publish_registry();
    if !pr.is_empty() {
        return pr;
    }
    config.registry()
}
