//! Package manifest reading and patching.

pub mod author;
pub mod manifest;
pub mod validate;

use std::path::Path;

use crate::archive;
use crate::constants::MY_PACKAGE_JSON;
use crate::error::{OhpmError, Result};

pub use self::manifest::{Author, AuthorValue, Manifest};

/// Read a manifest from a directory containing `oh-package.json5`.
pub fn read_manifest_from_dir(dir: &Path) -> Result<Manifest> {
    let path = dir.join(MY_PACKAGE_JSON);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| OhpmError::manifest_read_failed(&path, &e.to_string()))?;
    let mut m = Manifest::from_json5(&text)?;
    m.har_cache = Some(dir.to_path_buf());
    Ok(m)
}

/// Extract the manifest from a `.har`/`.tgz` archive into `cache_dir`
/// (stripping the leading path component) and return it.
pub fn read_manifest_from_archive(path: &Path, cache_dir: &Path) -> Result<Manifest> {
    // Find and read only the manifest entry without full extraction.
    let entry = archive::find_manifest_entry(path)?;
    let bytes = archive::read_entry_content(path, &entry)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut m = Manifest::from_json5(&text)?;
    // Also materialize the extracted tree so validation can inspect contents.
    archive::extract(path, cache_dir, 1, None)?;
    m.har_cache = Some(cache_dir.to_path_buf());
    Ok(m)
}

/// Patch the manifest with tool versions, mirroring `patchManifest` in
/// `publish/common.js`. The registry requires `_ohpmVersion` in the published
/// version metadata (`checkRegistry`), so this must run before the metadata
/// is built.
pub fn patch_manifest(m: &mut Manifest) {
    m.extra.insert(
        "_nodeVersion".into(),
        serde_json::json!(crate::constants::NODE_VERSION),
    );
    m.extra.insert(
        format!("_{}Version", crate::constants::PM),
        serde_json::json!(crate::constants::PM_VERSION),
    );
}

/// Normalize the `author` field in place (string -> object).
pub fn fix_author(m: &mut Manifest) -> Result<()> {
    let fixed = author::fix_author(&m.author)?;
    m.author = AuthorValue::Object(fixed);
    Ok(())
}

/// Convenience for callers that want the fixed author as a plain struct.
pub fn fixed_author(author: &AuthorValue) -> Result<Author> {
    author::fix_author(author)
}
