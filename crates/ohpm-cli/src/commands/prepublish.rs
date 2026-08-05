//! `ohpm prepublish <file>` — validate a package without publishing.

use anyhow::Result;
use ohpm_core::publish::{self, PublishRequest};

use super::{load_config, output};
use crate::cli::PrepublishArgs;

pub async fn run(args: &PrepublishArgs) -> Result<()> {
    let config = load_config()?;
    let package_root = std::env::current_dir()
        .ok()
        .map(|cwd| ohpm_core::config::find_local_prefix(&cwd).unwrap_or(cwd));
    let req = PublishRequest {
        file: args.file.clone(),
        package_root,
        ..Default::default()
    };
    let outcome = publish::prepublish(&config, &req).await?;
    output::succeed(&format!("prepublish {} {} succeed.", outcome.name, outcome.version));
    Ok(())
}
