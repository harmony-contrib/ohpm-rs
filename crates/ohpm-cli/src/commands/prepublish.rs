//! `ohpm-rs prepublish [<har_or_tgz_file> | <source_dir>]` — validate a
//! package without publishing. With no argument the current package directory
//! is validated directly from source.

use anyhow::{anyhow, Result};
use ohpm_core::config::find_local_prefix;
use ohpm_core::publish::{self, PublishRequest};

use super::{load_config, output};
use crate::cli::PrepublishArgs;

pub async fn run(args: &PrepublishArgs) -> Result<()> {
    let config = load_config()?;
    let cwd = std::env::current_dir()?;

    let (input, package_root) = match args.file.as_deref() {
        Some(arg) => {
            let p = std::path::PathBuf::from(arg);
            if p.is_dir() {
                (arg.to_string(), p)
            } else {
                (arg.to_string(), find_local_prefix(&cwd).unwrap_or(cwd))
            }
        }
        None => {
            let local = find_local_prefix(&cwd).ok_or_else(|| {
                anyhow!(
                    "No {} found in the current directory. Run prepublish from a package \
                     directory, or pass a har/tgz file explicitly.",
                    ohpm_core::constants::MY_PACKAGE_JSON
                )
            })?;
            (local.to_string_lossy().into_owned(), local)
        }
    };

    let req = PublishRequest {
        file: input,
        package_root: Some(package_root),
        ..Default::default()
    };
    let outcome = publish::prepublish(&config, &req).await?;
    output::succeed(&format!("prepublish {} {} succeed.", outcome.name, outcome.version));
    Ok(())
}
