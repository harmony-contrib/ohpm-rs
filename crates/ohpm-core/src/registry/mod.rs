//! Registry HTTP client and authentication.

pub mod auth;
pub mod login;
pub mod proxy;

use crate::config::Config;
use crate::constants;

/// Shared HTTP client for registry requests, built from ohpm config
/// (proxy / strict-SSL / CA files / timeout).
#[derive(Debug, Clone)]
pub struct RegistryClient {
    http: reqwest::Client,
}

impl RegistryClient {
    /// Build a client from configuration.
    pub fn from_config(config: &Config) -> crate::Result<Self> {
        let http = proxy::build_client(config)?;
        Ok(Self { http })
    }

    /// Build a client with defaults (no config loaded).
    pub fn unconfigured() -> Self {
        Self {
            http: reqwest::Client::new(),
        }
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    /// The user-agent used for registry requests
    /// (`ohpm/<version> node/<node-version>` in the reference).
    pub fn user_agent() -> String {
        constants::user_agent()
    }
}

/// Normalize a package name for URL paths: `@group/name` -> `@group%2fname`
/// (only the first `/` is replaced, as in the reference uploaders).
pub fn url_encode_pkg_name(name: &str) -> String {
    match name.find('/') {
        Some(i) => format!("{}%2f{}", &name[..i], &name[i + 1..]),
        None => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkg_name_encoding() {
        assert_eq!(url_encode_pkg_name("@ohos/foo"), "@ohos%2ffoo");
        assert_eq!(url_encode_pkg_name("com.example.foo"), "com.example.foo");
    }
}
