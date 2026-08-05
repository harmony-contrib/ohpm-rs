//! Command handlers: thin wrappers over `ohpm-core`.

pub mod cache;
pub mod config;
pub mod info;
pub mod init;
pub mod list;
pub mod login;
pub mod ping;
pub mod prepublish;
pub mod publish;
pub mod root;
pub mod unpublish;
pub mod version;

use anyhow::Result;
use ohpm_core::config::Config;

pub use crate::output;

/// Load configuration from the current working directory.
pub fn load_config() -> Result<Config> {
    let cwd = std::env::current_dir()?;
    let mut cfg = Config::new();
    cfg.load(&cwd, None)?;
    Ok(cfg)
}
