//! Lightweight `.ohpmrc` INI reader/writer.
//!
//! The npm `ini` format used by ohpm stores access tokens under keys that
//! contain `/` and `:` (e.g. `"//repo.harmonyos.com/repos/ohpm/:_auth"`), so
//! keys are quoted on write and both quoted/unquoted forms are accepted on
//! read. Values keep their inferred type (bool/number/string) like `ini.parse`.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;

use super::default::ConfigValue;
use crate::error::Result;

fn needs_quoted_key(key: &str) -> bool {
    !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
}

fn strip_inline_comment(line: &str) -> &str {
    let mut in_quote = false;
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'"' {
            in_quote = !in_quote;
        } else if !in_quote && (b == b'#' || b == b';') && i > 0 && bytes[i - 1].is_ascii_whitespace() {
            return &line[..i];
        }
    }
    line
}

fn unquote(s: &str) -> String {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        // Basic escaped-quote unescape used by ini.js for quoted keys/values.
        let inner = &t[1..t.len() - 1];
        return inner.replace("\\\"", "\"").replace("\\\\", "\\");
    }
    t.to_string()
}

/// Parse INI text into an ordered key/value map.
pub fn parse(text: &str) -> BTreeMap<String, ConfigValue> {
    let mut map = BTreeMap::new();
    for raw in text.lines() {
        let line = strip_inline_comment(raw).trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        // Split on first `=` (preferred) or `:` (npm ini also allows `:`).
        let (key, value) = match line.find('=') {
            Some(idx) => (&line[..idx], &line[idx + 1..]),
            None => match line.find(':') {
                Some(idx) => (&line[..idx], &line[idx + 1..]),
                None => (line, ""),
            },
        };
        if key.trim().is_empty() {
            continue;
        }
        let key = unquote(key);
        let value = unquote(value);
        map.insert(key, ConfigValue::from(value.as_str()));
    }
    map
}

/// Serialize a config map into INI text, quoting keys with special characters.
pub fn stringify(map: &BTreeMap<String, ConfigValue>) -> String {
    let mut out = String::new();
    for (k, v) in map {
        let key = if needs_quoted_key(k) {
            format!("\"{}\"", k.replace('"', "\\\""))
        } else {
            k.clone()
        };
        let value = match v {
            ConfigValue::String(s) => {
                if s.contains('#') || s.contains(';') || s.contains('"') || s != s.trim() {
                    format!("\"{}\"", s.replace('"', "\\\""))
                } else {
                    s.clone()
                }
            }
            ConfigValue::Bool(b) => b.to_string(),
            ConfigValue::Number(n) => n.to_string(),
        };
        out.push_str(&format!("{key} = {value}\n"));
    }
    out
}

/// Read and parse an INI file if it exists, otherwise return an empty map.
pub fn read_file(path: &Path) -> Result<BTreeMap<String, ConfigValue>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let mut content = String::new();
    let mut f = std::fs::File::open(path)?;
    f.read_to_string(&mut content)?;
    Ok(parse(&content))
}

/// Write a config map to a file, creating parent directories as needed.
pub fn write_file(path: &Path, map: &BTreeMap<String, ConfigValue>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = stringify(map);
    let mut f = std::fs::File::create(path)?;
    f.write_all(text.as_bytes())?;
    Ok(())
}

/// Guard so `parse`/`stringify` round-trip token-style keys exactly.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_token_key() {
        let mut m = BTreeMap::new();
        m.insert(
            "//ohpm.openharmony.cn/ohpm/:_auth".to_string(),
            ConfigValue::String("sometoken".into()),
        );
        m.insert("registry".to_string(), ConfigValue::String("https://ohpm.openharmony.cn/ohpm/".into()));
        m.insert("strict_ssl".to_string(), ConfigValue::Bool(true));

        let text = stringify(&m);
        assert!(text.contains("\"//ohpm.openharmony.cn/ohpm/:_auth\" = sometoken"), "{text}");
        assert!(text.contains("registry = https://ohpm.openharmony.cn/ohpm/"), "{text}");

        let parsed = parse(&text);
        assert_eq!(parsed.get("//ohpm.openharmony.cn/ohpm/:_auth").unwrap().as_str(), "sometoken");
        assert_eq!(parsed.get("registry").unwrap().as_str(), "https://ohpm.openharmony.cn/ohpm/");
        assert_eq!(parsed.get("strict_ssl").unwrap().as_bool(), true);
    }

    #[test]
    fn parse_unquoted_token_key() {
        let text = "//ohpm.openharmony.cn/ohpm/:_auth=abc\npublish_registry=https://x/";
        let parsed = parse(text);
        assert_eq!(parsed.get("//ohpm.openharmony.cn/ohpm/:_auth").unwrap().as_str(), "abc");
    }

    #[test]
    fn parse_comments_and_blank_lines() {
        let text = "# comment\n; also comment\nregistry=https://x/  # trailing\n\nfoo=bar";
        let parsed = parse(text);
        assert_eq!(parsed.get("registry").unwrap().as_str(), "https://x/");
        assert_eq!(parsed.get("foo").unwrap().as_str(), "bar");
    }
}
