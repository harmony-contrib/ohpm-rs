//! ohpm-core: a Rust reimplementation of the ohpm (OpenHarmony package manager)
//! capabilities, focused on publish with environment-variable authentication.
//!
//! Reference implementation: the ohpm source bundled with DevEco Studio
//! 6.1.1.280 (`lib/...`). Module layout mirrors its core subsystems.

pub mod archive;
pub mod cache;
pub mod clean;
pub mod config;
pub mod constants;
pub mod error;
pub mod install;
pub mod pack;
pub mod package;
pub mod publish;
pub mod registry;
pub mod version;
pub mod workspace;

pub use error::{OhpmError, Result};
