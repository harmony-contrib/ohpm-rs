//! Publish metadata construction.
//! Mirrors `buildHarMetaData` / `buildTgzMeta` / `mergeResolves` and
//! `clearUnUsedFieldInMetadata` in `lib/core/publish/common.js`.

use serde_json::{json, Map, Value};

use crate::archive::integrity::Integrity;
use crate::constants;
use crate::error::{OhpmError, Result};
use crate::package::Manifest;

/// Options describing what is being published.
pub struct MetaContext {
    pub registry: String,
    pub tag: String,
    /// Integrity of the `.har` archive (`dist.integrity`).
    pub har_integrity: Integrity,
    /// For `.tgz` bundles, the hsp archive's integrity/hspType.
    pub hsp: Option<HspMeta>,
    /// Absolute path of the `.har` (used only to derive attachment names).
    pub har_abs: Option<std::path::PathBuf>,
}

/// Metadata of the `.hsp` part of a `.tgz` bundle.
pub struct HspMeta {
    pub integrity: Integrity,
    pub hsp_type: String,
}

/// Serialize the `Manifest` as a JSON object (the `versions[version]` entry).
fn manifest_to_object(m: &Manifest) -> Value {
    let mut v = m.to_json();
    if let Value::Object(map) = &mut v {
        map.insert("_id".into(), json!(format!("{}@{}", m.name, m.version)));
    }
    v
}

/// Build the metadata document for a plain `.har` package.
pub fn build_har_metadata(m: &Manifest, ctx: &MetaContext) -> Result<Value> {
    let mut doc = Map::new();
    doc.insert("_id".into(), json!(m.name));
    doc.insert("name".into(), json!(m.name));
    doc.insert("packageType".into(), json!(m.package_type()));
    doc.insert("description".into(), json!(m.description));

    let mut dist_tags = Map::new();
    dist_tags.insert(ctx.tag.clone(), json!(m.version));
    doc.insert("dist-tags".into(), Value::Object(dist_tags));

    // versions[version] = manifest + _id + dist
    let tarball = build_tarball_url(&ctx.registry, &m.name, &m.version)?;
    let mut version_entry = manifest_to_object(m);
    if let Value::Object(ve) = &mut version_entry {
        let mut dist = Map::new();
        // The registry validates the attachment against this single sha512
        // ssri entry (the reference's `getIntegrity()` picks sha512).
        dist.insert("integrity".into(), json!(ctx.har_integrity.to_ssri()));
        dist.insert("tarball".into(), json!(tarball));
        if let Some(hsp) = &ctx.hsp {
            dist.insert("integrity_hsp".into(), json!(hsp.integrity.to_ssri()));
        }
        ve.insert("dist".into(), Value::Object(dist));
    }

    let mut versions = Map::new();
    versions.insert(m.version.clone(), version_entry);
    doc.insert("versions".into(), Value::Object(versions));

    if let Some(hsp) = &ctx.hsp {
        doc.insert("hspType".into(), json!(hsp.hsp_type));
    }

    Ok(Value::Object(doc))
}

/// The `dist.tarball` URL: `{registry}{name}/-/{name}-{version}.har` with
/// `http://` normalized to `https://` (matching `buildHarMetaData`).
pub fn build_tarball_url(registry: &str, name: &str, version: &str) -> Result<String> {
    let base = url::Url::parse(registry).map_err(|e| {
        OhpmError::new("InvalidRegistryUrl", format!("{registry}: {e}"))
    })?;
    let relative = format!("{name}/-/{name}-{version}{}", constants::HAR_SUFFIX);
    let joined = base
        .join(&relative)
        .map_err(|e| OhpmError::new("InvalidTarballUrl", e.to_string()))?;
    let mut s = joined.to_string();
    if let Some(rest) = s.strip_prefix("http://") {
        s = format!("https://{rest}");
    }
    Ok(s)
}

/// Remove internal-only fields before upload, mirroring
/// `clearUnUsedFieldInMetadata`.
pub fn clear_unused_fields(doc: &mut Value) {
    if let Value::Object(map) = doc {
        map.remove("isTgz");
        map.remove("pkg");
        if let Some(versions) = map.get_mut("versions").and_then(|v| v.as_object_mut()) {
            for v in versions.values_mut() {
                if let Value::Object(ve) = v {
                    ve.remove("hspPkg");
                    ve.remove("cache");
                    ve.remove("harCache");
                    ve.remove("hspCache");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> Manifest {
        Manifest {
            name: "@ohos/demo".into(),
            version: "1.0.0".into(),
            description: "demo".into(),
            main: "index.ets".into(),
            author: crate::package::manifest::AuthorValue::String("A <a@b.c>".into()),
            license: "ISC".into(),
            package_type: Some("InterfaceHar".into()),
            dependencies: [("@ohos/foo".to_string(), "^1.0.0".to_string())].into(),
            ..Default::default()
        }
    }

    fn fake_integrity() -> Integrity {
        Integrity {
            sha1: "c2hhMQ==".into(),
            sha512: "c2hhNTEy".into(),
        }
    }

    #[test]
    fn build_har_metadata_shape() {
        let mut m = sample_manifest();
        let ctx = MetaContext {
            registry: "https://ohpm.openharmony.cn/ohpm/".into(),
            tag: "latest".into(),
            har_integrity: fake_integrity(),
            hsp: None,
            har_abs: None,
        };
        let doc = build_har_metadata(&mut m, &ctx).unwrap();
        assert_eq!(doc["_id"], "@ohos/demo");
        assert_eq!(doc["packageType"], "InterfaceHar");
        assert_eq!(doc["dist-tags"]["latest"], "1.0.0");
        assert_eq!(doc["versions"]["1.0.0"]["_id"], "@ohos/demo@1.0.0");
        let tarball = doc["versions"]["1.0.0"]["dist"]["tarball"].as_str().unwrap();
        assert_eq!(
            tarball,
            "https://ohpm.openharmony.cn/ohpm/@ohos/demo/-/@ohos/demo-1.0.0.har"
        );
        assert_eq!(
            doc["versions"]["1.0.0"]["dist"]["integrity"].as_str().unwrap(),
            "sha512-c2hhNTEy"
        );
        assert!(doc.get("hspType").is_none());
    }

    #[test]
    fn clear_unused_fields_removes_internals() {
        let mut m = sample_manifest();
        let ctx = MetaContext {
            registry: "https://r/".into(),
            tag: "latest".into(),
            har_integrity: fake_integrity(),
            hsp: None,
            har_abs: None,
        };
        let mut doc = build_har_metadata(&mut m, &ctx).unwrap();
        doc["isTgz"] = json!(true);
        doc["pkg"] = json!("x");
        doc["versions"]["1.0.0"]["harCache"] = json!("/tmp/x");
        clear_unused_fields(&mut doc);
        assert!(doc.get("isTgz").is_none());
        assert!(doc.get("pkg").is_none());
        assert!(doc["versions"]["1.0.0"].get("harCache").is_none());
    }
}
