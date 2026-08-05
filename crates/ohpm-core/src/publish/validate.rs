//! Package validation helpers for publish/prepublish.
//! Mirrors `lib/core/publish/PublishCore.js` validators.

use std::path::Path;

use crate::archive::{integrity, list};
use crate::constants;
use crate::error::{OhpmError, Result};
use crate::package::Manifest;

/// Content summary of a `.har`/`.hsp` archive (`getPkgContent`).
#[derive(Debug, Clone)]
pub struct PkgContent {
    /// Compressed archive size in bytes.
    pub size: u64,
    /// Sum of entry sizes.
    pub unpacked_size: u64,
    /// Base64 SHA-1 digest.
    pub shasum: String,
    pub integrity: integrity::Integrity,
    /// `(path, size)` for every non-directory entry, `package/` prefix stripped.
    pub files: Vec<(String, u64)>,
    pub entry_count: usize,
}

/// Validate that a package path exists. A directory is allowed (it is packed
/// into a har first); missing or empty paths are rejected.
pub fn valid_pkg_path(path: &str) -> Result<()> {
    let p = Path::new(path);
    if path.is_empty() || !p.exists() {
        return Err(OhpmError::pkg_empty());
    }
    Ok(())
}

/// Compute the content summary of an archive (`getPkgContent`).
pub fn get_pkg_content(path: &Path) -> Result<PkgContent> {
    let size = std::fs::metadata(path)?.len();
    let integrity = integrity::integrity_of_file(path)?;

    let entries = list(path)?;
    let mut entry_count = 0usize;
    let mut unpacked_size = 0u64;
    let mut files = Vec::new();
    for e in entries {
        if e.is_dir {
            continue;
        }
        entry_count += 1;
        unpacked_size += e.size;
        files.push((e.path.strip_prefix("package/").unwrap_or(&e.path).to_string(), e.size));
    }

    Ok(PkgContent {
        size,
        unpacked_size,
        shasum: integrity.shasum().to_string(),
        integrity,
        files,
        entry_count,
    })
}

/// Validate the package size is within [0, 500MB] (`validatePkgSize`).
pub fn validate_pkg_size(pkg: &PkgContent) -> Result<()> {
    if pkg.size > constants::MAX_PACK_SIZE_B {
        return Err(OhpmError::new(
            "OverMaxPackageSize",
            format!(
                "The package size {}MB exceeds the maximum allowed {}MB.",
                pkg.size >> 20,
                constants::MAX_PACK_SIZE_MB
            ),
        ));
    }
    Ok(())
}

/// Validate the manifest of a `.har` (lightweight stand-in for
/// `validateHmPackage` + the `PackageValidator`): name and version must be
/// present and well-formed.
pub fn validate_manifest_basics(m: &Manifest) -> Result<()> {
    if m.name.is_empty() {
        return Err(OhpmError::new("NameEmptyError", "The package \"name\" cannot be empty."));
    }
    crate::package::validate::validate_package_name(&m.name)?;
    if m.version.is_empty() {
        return Err(OhpmError::new("VersionEmptyError", "The package \"version\" cannot be empty."));
    }
    crate::package::validate::validate_version(&m.version)
}

/// Validate a `.hsp` archive is non-empty and within size limits
/// (`validHspContent`).
pub fn valid_hsp_content(path: &Path) -> Result<PkgContent> {
    let pkg = get_pkg_content(path)?;
    if pkg.size == 0 {
        return Err(OhpmError::hsp_file_is_empty(&path.to_string_lossy()));
    }
    validate_pkg_size(&pkg)?;
    Ok(pkg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkg_content_counts_entries() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("package/sub")).unwrap();
        std::fs::write(src.join("package/a.ets"), "aa").unwrap();
        std::fs::write(src.join("package/sub/b.ets"), "bbb").unwrap();

        let har = dir.path().join("pkg.har");
        let f = std::fs::File::create(&har).unwrap();
        let enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
        let mut tar = tar::Builder::new(enc);
        tar.append_dir_all("package", &src.join("package")).unwrap();
        let enc = tar.into_inner().unwrap();
        enc.finish().unwrap();

        let pkg = get_pkg_content(&har).unwrap();
        assert_eq!(pkg.entry_count, 2);
        assert_eq!(pkg.unpacked_size, 5);
        assert!(pkg.files.iter().any(|(p, _)| p == "a.ets"));
        assert!(pkg.files.iter().any(|(p, _)| p == "sub/b.ets"));
        validate_pkg_size(&pkg).unwrap();
    }
}
