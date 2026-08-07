//! Config value validation, mirroring `lib/config/TypeValidate.js`.
//!
//! `validate` returns `None` when the value is acceptable, or a message
//! describing the problem. The caller (the `config set` command) warns and
//! skips invalid values, exactly like the reference.

use super::default::{access_token_type, types, ConfigValue, LOG_LEVELS};

/// Values that count as a boolean set when the config file/CLI passes them.
const BOOL_FALSY: [&str; 3] = ["null", "false", "\"\""];

/// `crypto/constants.js` KEY_HEAD: proxy values already encrypted with a
/// crypto component start with this prefix and are accepted as-is.
const KEY_HEAD: &str = "security:";

fn is_url(value: &str) -> bool {
    url::Url::parse(value).map(|u| u.host_str().is_some()).unwrap_or(false)
}

fn is_abs_path(value: &str) -> bool {
    std::path::Path::new(value).is_absolute()
}

fn is_integer_in(value: &str, min: i64, max: i64) -> bool {
    let Ok(n) = value.parse::<i64>() else { return false };
    (min..=max).contains(&n)
}

fn contains_cjk(s: &str) -> bool {
    s.chars().any(|c| ('\u{4E00}'..='\u{9FA5}').contains(&c) || ('\u{FE30}'..='\u{FFA0}').contains(&c))
}

/// Parse a bool-ish value the way the reference does, rejecting CJK content.
fn parse_bool(value: &str) -> Option<bool> {
    let v = value.trim();
    if contains_cjk(v) {
        return None;
    }
    Some(match v {
        "null" | "false" | "\"\"" | "" => false,
        "true" => true,
        _ => {
            // numeric coercion: "0" -> false, "1"/"2" -> true
            if let Ok(n) = v.parse::<f64>() {
                n != 0.0
            } else {
                !BOOL_FALSY.contains(&v)
            }
        }
    })
}

/// Validate a `key = value` pair for `config set`. Returns an error message
/// when the value is rejected, otherwise `None`.
pub fn validate(key: &str, value: &str) -> Option<String> {
    let key = key.trim();
    let value = value.trim();
    match key {
        types::REGISTRY => validate_registry_list(value),
        types::PUBLISH_ID | types::KEY_CONTENT | types::KEY_PASSPHRASE => None,
        types::CACHE | types::KEY_PATH => {
            if value.is_empty() || is_abs_path(value) {
                None
            } else {
                Some(" - invalid filesystem path.".into())
            }
        }
        types::NO_PROXY => {
            if value.is_empty() {
                None
            } else {
                Some(" - invalid no_proxy address.".into())
            }
        }
        types::HTTP_PROXY | types::HTTPS_PROXY => {
            if value.is_empty() || value.starts_with(KEY_HEAD) {
                None
            } else if !value.starts_with("http://") && !value.starts_with("https://") || !is_url(value) {
                Some(" - full url with \"http://\" or \"https://\".".into())
            } else {
                None
            }
        }
        types::STRICT_SSL | types::CACHE_HARDLINK => {
            if parse_bool(value).is_none() {
                Some(format!(" - invalid {} value.", key))
            } else {
                None
            }
        }
        types::LOG_LEVEL => {
            if LOG_LEVELS.contains(&value.to_ascii_lowercase().as_str()) {
                None
            } else {
                Some(" - must be one of: error, warn, info, debug.".into())
            }
        }
        types::PUBLISH_REGISTRY => {
            if !value.starts_with("https://") && !value.starts_with("http://") || !is_url(value) {
                Some(" - full url with \"http:// or \"https://\".".into())
            } else {
                None
            }
        }
        types::CA_FILES => {
            if value.is_empty() {
                None
            } else if value.ends_with(',') || value.split(',').any(|p| !is_abs_path(p.trim())) {
                Some(" - invalid CaFiles path.".into())
            } else {
                None
            }
        }
        types::FETCH_TIMEOUT => {
            if is_integer_in(value, 10_000, 360_000) {
                None
            } else {
                Some(format!(
                    " - invalid {} value, reference value: [10000, 360000].",
                    types::FETCH_TIMEOUT
                ))
            }
        }
        types::MAX_CONCURRENT => {
            if is_integer_in(value, 1, 200) {
                None
            } else {
                Some(format!(
                    " - invalid {} value, reference value: [1, 200].",
                    types::MAX_CONCURRENT
                ))
            }
        }
        types::RETRY_TIMES => {
            if is_integer_in(value, 0, 5) {
                None
            } else {
                Some(format!(
                    " - invalid {} value, reference value: [0, 5].",
                    types::RETRY_TIMES
                ))
            }
        }
        types::RETRY_INTERVAL => {
            if is_integer_in(value, 1, 6) {
                None
            } else {
                Some(format!(
                    " - invalid {} value, reference value: [1, 6].",
                    types::RETRY_INTERVAL
                ))
            }
        }
        types::USE_STREAM_THRESHOLD_SIZE => {
            if is_integer_in(value, 0, 500) {
                None
            } else {
                Some(format!(
                    " - invalid {} value, reference value: [0, 500].",
                    types::USE_STREAM_THRESHOLD_SIZE
                ))
            }
        }
        types::COMPABILITY_LOG_LEVEL => {
            let levels = ["error", "warn", "info", "close"];
            if levels.contains(&value.to_ascii_lowercase().as_str()) {
                None
            } else {
                Some(" - must be one of: error, warn, info, close.".into())
            }
        }
        _ => {
            // Scoped registry keys: `@group:registry`.
            if key.starts_with('@') && key.ends_with(":registry") {
                return validate_registry_list(value);
            }
            // Access-token keys: `//host/path/:_auth` / `:_read_auth`.
            // Accepted as-is (Node's url.parse is lenient about scheme-less
            // keys, and the registry token lookup uses them verbatim).
            if key.ends_with(access_token_type::READ_WRITE) || key.ends_with(access_token_type::READ)
            {
                None
            } else {
                Some(format!(
                    "Invalid key: {key} - enter \"{} config list -j\" to get the defaults.",
                    crate::constants::PM
                ))
            }
        }
    }
}

fn validate_registry_list(value: &str) -> Option<String> {
    for item in value.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        if !item.starts_with("https://") && !item.starts_with("http://") {
            return Some(" - full url with \"http:// or \"https://\".".into());
        }
        if !is_url(item) {
            return Some(" - full url with \"http:// or \"https://\".".into());
        }
    }
    None
}

/// The parsed bool for keys that store booleans (used by `Config::set`).
pub fn parsed_bool(value: &str) -> Option<bool> {
    parse_bool(value)
}

/// `ConfigValue` -> JSON for `config list -j` (preserves types).
pub fn to_json(value: &ConfigValue) -> serde_json::Value {
    match value {
        ConfigValue::String(s) => serde_json::Value::String(s.clone()),
        ConfigValue::Bool(b) => serde_json::Value::Bool(*b),
        ConfigValue::Number(n) => serde_json::Value::Number((*n).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_keys() {
        assert!(validate(types::PUBLISH_ID, "abc").is_none());
        assert!(validate(types::KEY_PASSPHRASE, "secret").is_none());
        assert!(validate(types::KEY_CONTENT, "-----BEGIN...").is_none());
        assert!(validate(types::STRICT_SSL, "true").is_none());
        assert!(validate(types::STRICT_SSL, "false").is_none());
        assert!(validate(types::LOG_LEVEL, "DEBUG").is_none());
        assert!(validate(types::FETCH_TIMEOUT, "60000").is_none());
    }

    #[test]
    fn invalid_values_rejected() {
        assert!(validate(types::REGISTRY, "not-a-url").is_some());
        assert!(validate(types::REGISTRY, "https://host/path/").is_none());
        assert!(validate(types::REGISTRY, "https://a/,http://b/").is_none());
        assert!(validate(types::PUBLISH_REGISTRY, "ftp://x/").is_some());
        assert!(validate(types::LOG_LEVEL, "verbose").is_some());
        assert!(validate(types::FETCH_TIMEOUT, "abc").is_some());
        assert!(validate(types::FETCH_TIMEOUT, "1").is_some());
        assert!(validate(types::MAX_CONCURRENT, "1000").is_some());
        assert!(validate(types::RETRY_TIMES, "100").is_some());
        assert!(validate(types::USE_STREAM_THRESHOLD_SIZE, "5").is_none());
        assert!(validate(types::USE_STREAM_THRESHOLD_SIZE, "501").is_some());
        assert!(validate(types::KEY_PATH, "relative/path").is_some());
        assert!(validate(types::KEY_PATH, "/abs/path").is_none());
        assert!(validate(types::HTTP_PROXY, "https://proxy:8080").is_none());
        assert!(validate(types::HTTP_PROXY, "security:abc").is_none());
        // CJK rejected for bool keys
        assert!(validate(types::STRICT_SSL, "中文").is_some());
    }

    #[test]
    fn token_and_unknown_keys() {
        assert!(validate("//repo.harmonyos.com/ohpm/:_auth", "token").is_none());
        assert!(validate("//repo.harmonyos.com/ohpm/:_read_auth", "token").is_none());
        let msg = validate("unknown_key", "x").unwrap();
        assert!(msg.contains("Invalid key"));
        assert!(msg.contains("config list -j"));
        assert!(validate("@mygroup:registry", "https://repo/").is_none());
    }

    #[test]
    fn bool_parsing() {
        assert_eq!(parsed_bool("true"), Some(true));
        assert_eq!(parsed_bool("false"), Some(false));
        assert_eq!(parsed_bool("0"), Some(false));
        assert_eq!(parsed_bool("1"), Some(true));
        assert_eq!(parsed_bool("null"), Some(false));
        assert_eq!(parsed_bool("中文"), None);
    }
}
