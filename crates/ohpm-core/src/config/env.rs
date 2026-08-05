//! `OHPM_*` environment variable overrides.
//!
//! These allow the whole publish auth chain (registry, publish id, key path,
//! passphrase) to be driven from environment variables so CI/CD can publish
//! without any interactive input. Priority: env vars win over `.ohpmrc`
//! config files.

use super::default::types;

/// Map a config key to its `OHPM_*` environment variable name.
pub fn env_var_for(config_key: &str) -> Option<&'static str> {
    Some(match config_key {
        types::REGISTRY => "OHPM_REGISTRY",
        types::PUBLISH_REGISTRY => "OHPM_PUBLISH_REGISTRY",
        types::PUBLISH_ID => "OHPM_PUBLISH_ID",
        types::KEY_PATH => "OHPM_KEY_PATH",
        types::KEY_PASSPHRASE => "OHPM_KEY_PASSPHRASE",
        types::STRICT_SSL => "OHPM_STRICT_SSL",
        types::CA_FILES => "OHPM_CA_FILES",
        types::LOG_LEVEL => "OHPM_LOG_LEVEL",
        types::CACHE => "OHPM_CACHE",
        types::HTTP_PROXY => "OHPM_HTTP_PROXY",
        types::HTTPS_PROXY => "OHPM_HTTPS_PROXY",
        types::NO_PROXY => "OHPM_NO_PROXY",
        types::FETCH_TIMEOUT => "OHPM_FETCH_TIMEOUT",
        _ => return None,
    })
}

/// Collect all `OHPM_*` overrides that are set in the current environment,
/// as `(config_key, value)` pairs.
pub fn overrides() -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let keys = [
        types::REGISTRY,
        types::PUBLISH_REGISTRY,
        types::PUBLISH_ID,
        types::KEY_PATH,
        types::KEY_PASSPHRASE,
        types::STRICT_SSL,
        types::CA_FILES,
        types::LOG_LEVEL,
        types::CACHE,
        types::HTTP_PROXY,
        types::HTTPS_PROXY,
        types::NO_PROXY,
        types::FETCH_TIMEOUT,
    ];
    for key in keys {
        if let Some(env) = env_var_for(key) {
            if let Ok(v) = std::env::var(env) {
                if !v.is_empty() {
                    out.push((key, v));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_is_consistent() {
        assert_eq!(env_var_for(types::REGISTRY), Some("OHPM_REGISTRY"));
        assert_eq!(env_var_for(types::KEY_PASSPHRASE), Some("OHPM_KEY_PASSPHRASE"));
        assert_eq!(env_var_for("unknown_key"), None);
    }
}
