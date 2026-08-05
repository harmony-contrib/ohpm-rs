//! `oh-package.json5` manifest model.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Author as an object (`{ name, email, url }`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Author {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub url: String,
}

/// Author accepts either the object form or a string form
/// `"Name <email> (url)"` (see `fixer.js`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AuthorValue {
    String(String),
    Object(Author),
}

impl Default for AuthorValue {
    fn default() -> Self {
        AuthorValue::String(String::new())
    }
}

/// A parsed `oh-package.json5` manifest. Unknown fields are preserved so the
/// published metadata round-trips them.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub main: String,
    #[serde(default)]
    pub author: AuthorValue,
    #[serde(default)]
    pub license: String,
    /// `packageType`; must be `InterfaceHar` for `.tgz` packages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_type: Option<String>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub dev_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub dynamic_dependencies: BTreeMap<String, String>,
    /// `publish: false` marks a package as not publishable; it is skipped by
    /// workspace batch operations and rejected by an explicit publish.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish: Option<bool>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,

    // ---- publish-only fields (not read from the file) ----
    #[serde(skip)]
    pub har_cache: Option<PathBuf>,
    /// Dist tag for the publish metadata (`getPublishOptions` sets it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
}

impl Manifest {
    /// Parse a manifest from a JSON5 string.
    pub fn from_json5(text: &str) -> crate::Result<Self> {
        let m: Manifest = json5::from_str(text)?;
        Ok(m)
    }

    /// The effective `packageType`, defaulting to `""`.
    pub fn package_type(&self) -> &str {
        self.package_type.as_deref().unwrap_or("")
    }

    /// Whether this package may be published. `publish: false` opts out.
    pub fn publishable(&self) -> bool {
        self.publish != Some(false)
    }

    /// Serialize to JSON (used for the publish metadata version entry).
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_json5_with_comments_and_unquoted_keys() {
        let text = r#"
// a comment
{
  name: "com.example.demo",
  version: '1.0.0',
  description: "demo",
  main: "index.ets",
  author: "A <a@b.c> (https://x)",
  license: "ISC",
  dependencies: { "@ohos/foo": "^1.0.0", },
  extraField: 42,
}
"#;
        let m = Manifest::from_json5(text).unwrap();
        assert_eq!(m.name, "com.example.demo");
        assert_eq!(m.version, "1.0.0");
        assert_eq!(m.dependencies.get("@ohos/foo").unwrap(), "^1.0.0");
        assert_eq!(m.extra.get("extraField").unwrap(), &serde_json::json!(42));
    }

    #[test]
    fn parse_author_object() {
        let text = r#"{ name: "x", author: { name: "N", email: "e@x.y", url: "https://u" } }"#;
        let m = Manifest::from_json5(text).unwrap();
        match m.author {
            AuthorValue::Object(a) => {
                assert_eq!(a.name, "N");
                assert_eq!(a.email, "e@x.y");
            }
            _ => panic!("expected object author"),
        }
    }

    #[test]
    fn package_type_roundtrip() {
        let text = r#"{ name: "x", version: "1.0.0", packageType: "InterfaceHar" }"#;
        let m = Manifest::from_json5(text).unwrap();
        assert_eq!(m.package_type(), "InterfaceHar");
    }
}
