//! Default configuration values and config key constants.
//! Mirrors `lib/config/DefaultConfig.js`.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Known config keys (subset that matters for publish + core commands).
pub mod types {
    pub const REGISTRY: &str = "registry";
    pub const PUBLISH_ID: &str = "publish_id";
    pub const CACHE: &str = "cache";
    pub const KEY_PATH: &str = "key_path";
    /// Inline private-key PEM content (alternative to `key_path`).
    pub const KEY_CONTENT: &str = "key_content";
    pub const KEY_PASSPHRASE: &str = "key_passphrase";
    pub const NO_PROXY: &str = "no_proxy";
    pub const HTTP_PROXY: &str = "http_proxy";
    pub const HTTPS_PROXY: &str = "https_proxy";
    pub const STRICT_SSL: &str = "strict_ssl";
    pub const LOG_LEVEL: &str = "log_level";
    pub const PUBLISH_REGISTRY: &str = "publish_registry";
    pub const CA_FILES: &str = "ca_files";
    pub const FETCH_TIMEOUT: &str = "fetch_timeout";
    pub const LOCAL_PREFIX: &str = "prefix";
    pub const MAX_CONCURRENT: &str = "max_concurrent";
    pub const RETRY_TIMES: &str = "retry_times";
    pub const RETRY_INTERVAL: &str = "retry_interval";
    pub const USE_STREAM_THRESHOLD_SIZE: &str = "use_stream_threshold_size";
    pub const COMPABILITY_LOG_LEVEL: &str = "compability_log_level";
    pub const CRYPTO_PATH: &str = "crypto_path";
    pub const RESOLVE_CONFLICT: &str = "resolve_conflict";
    pub const RESOLVE_CONFLICT_STRICT: &str = "resolve_conflict_strict";
    pub const ENFORCE_DEPENDENCY_KEY: &str = "enforce_dependency_key";
    pub const INSTALL_ALL: &str = "install_all";
    pub const ENABLE_UNIFIED_LOCKFILE: &str = "enable_unified_lockfile";
    pub const LOCKFILE_STABLE_ORDER: &str = "lockfile_stable_order";
    pub const ENABLE_LOCK_INNER_PKG_VERSION: &str = "enable_lock_inner_pkg_version";
    pub const ENABLE_CROSS_PROCESS_LOCK: &str = "enable_cross_process_lock";
    pub const PARAMETER_FILE: &str = "parameter_file";
}

/// Suffixes appended to a stripped registry URL for access tokens.
pub mod access_token_type {
    /// Read-only token suffix: `:<suffix>` appended to `//host/path/`.
    pub const READ: &str = ":_read_auth";
    /// Read-write token suffix (used for publish).
    pub const READ_WRITE: &str = ":_auth";
}

/// Config-source precedence (highest first). Mirrors `configType`.
pub const SOURCES: [&str; 5] = ["cli", "cwd", "project", "user", "default"];

pub const LOG_LEVELS: [&str; 4] = ["error", "warn", "info", "debug"];

/// Maximum number of auth records that can be configured.
pub const MAX_AUTH: usize = 3;

pub fn default_cache() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(super::super::constants::PM_DIR)
        .join("cache")
}

/// The default configuration, mirroring `defaultConfig` in `DefaultConfig.js`.
pub fn default_config() -> BTreeMap<String, ConfigValue> {
    let mut m = BTreeMap::new();
    m.insert(types::REGISTRY.to_string(), ConfigValue::String(String::new()));
    m.insert(types::PUBLISH_ID.to_string(), ConfigValue::String(String::new()));
    m.insert(types::CACHE.to_string(), ConfigValue::String(default_cache().to_string_lossy().into_owned()));
    m.insert(types::KEY_PATH.to_string(), ConfigValue::String(String::new()));
    m.insert(types::KEY_CONTENT.to_string(), ConfigValue::String(String::new()));
    m.insert(types::KEY_PASSPHRASE.to_string(), ConfigValue::String(String::new()));
    m.insert(types::NO_PROXY.to_string(), ConfigValue::String(String::new()));
    m.insert(types::HTTP_PROXY.to_string(), ConfigValue::String(String::new()));
    m.insert(types::HTTPS_PROXY.to_string(), ConfigValue::String(String::new()));
    m.insert(types::STRICT_SSL.to_string(), ConfigValue::Bool(true));
    m.insert(types::LOG_LEVEL.to_string(), ConfigValue::String("info".into()));
    m.insert(types::PUBLISH_REGISTRY.to_string(), ConfigValue::String(String::new()));
    m.insert(types::CA_FILES.to_string(), ConfigValue::String(String::new()));
    m.insert(types::FETCH_TIMEOUT.to_string(), ConfigValue::Number(60_000));
    m.insert(types::MAX_CONCURRENT.to_string(), ConfigValue::Number(50));
    m.insert(types::RETRY_TIMES.to_string(), ConfigValue::Number(1));
    m.insert(types::RETRY_INTERVAL.to_string(), ConfigValue::Number(1_000));
    m.insert(types::USE_STREAM_THRESHOLD_SIZE.to_string(), ConfigValue::Number(5));
    m.insert(types::COMPABILITY_LOG_LEVEL.to_string(), ConfigValue::String("warn".into()));
    m.insert(types::CRYPTO_PATH.to_string(), ConfigValue::String(String::new()));
    m.insert(types::RESOLVE_CONFLICT.to_string(), ConfigValue::Bool(true));
    m.insert(types::RESOLVE_CONFLICT_STRICT.to_string(), ConfigValue::Bool(false));
    m.insert(types::ENFORCE_DEPENDENCY_KEY.to_string(), ConfigValue::Bool(false));
    m.insert(types::INSTALL_ALL.to_string(), ConfigValue::Bool(true));
    m.insert(types::ENABLE_UNIFIED_LOCKFILE.to_string(), ConfigValue::Bool(false));
    m.insert(types::LOCKFILE_STABLE_ORDER.to_string(), ConfigValue::Bool(false));
    m.insert(types::ENABLE_LOCK_INNER_PKG_VERSION.to_string(), ConfigValue::Bool(true));
    m.insert(types::ENABLE_CROSS_PROCESS_LOCK.to_string(), ConfigValue::Bool(false));
    m.insert(types::PARAMETER_FILE.to_string(), ConfigValue::String(String::new()));
    m
}

/// A config value, preserving the source's value types (string/bool/number).
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigValue {
    String(String),
    Bool(bool),
    Number(i64),
}

impl ConfigValue {
    pub fn as_str(&self) -> String {
        match self {
            ConfigValue::String(s) => s.clone(),
            ConfigValue::Bool(b) => b.to_string(),
            ConfigValue::Number(n) => n.to_string(),
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            ConfigValue::Bool(b) => *b,
            ConfigValue::String(s) => matches!(
                s.trim().to_lowercase().as_str(),
                "true" | "1" | "yes" | "on"
            ),
            ConfigValue::Number(n) => *n != 0,
        }
    }

    pub fn as_i64(&self) -> i64 {
        match self {
            ConfigValue::Number(n) => *n,
            ConfigValue::String(s) => s.trim().parse().unwrap_or(0),
            ConfigValue::Bool(b) => i64::from(*b),
        }
    }
}

impl From<&str> for ConfigValue {
    fn from(s: &str) -> Self {
        // Tolerate empty strings; keep booleans/numbers typed like the JS impl.
        let t = s.trim();
        if t.is_empty() {
            return ConfigValue::String(String::new());
        }
        match t.to_ascii_lowercase().as_str() {
            "true" => return ConfigValue::Bool(true),
            "false" => return ConfigValue::Bool(false),
            _ => {}
        }
        if let Ok(n) = t.parse::<i64>() {
            // Keep small integers typed, but "latest"/names stay strings.
            return ConfigValue::Number(n);
        }
        ConfigValue::String(s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_type_inference() {
        assert_eq!(ConfigValue::from("true"), ConfigValue::Bool(true));
        assert_eq!(ConfigValue::from("FALSE"), ConfigValue::Bool(false));
        assert_eq!(ConfigValue::from("60000"), ConfigValue::Number(60_000));
        assert_eq!(ConfigValue::from("latest"), ConfigValue::String("latest".into()));
        assert_eq!(ConfigValue::from(""), ConfigValue::String(String::new()));
    }
}
