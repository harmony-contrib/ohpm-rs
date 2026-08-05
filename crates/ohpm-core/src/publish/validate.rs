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

/// Mirror `validPkgDependencies`: reject bare local paths (specs that are not
/// version ranges, URLs, tags or `file:./` inner paths), `file:../` external
/// paths, specs longer than 128 chars and invalid `tag:` specs. The
/// classification matrix was verified against the reference implementation.
pub fn validate_dependency_specs(deps: &std::collections::BTreeMap<String, String>) -> Result<()> {
    let mut errors = Vec::new();
    for (name, spec) in deps {
        if spec.len() > 128 {
            errors.push(format!("{name}: {spec} (spec exceeds 128 chars)"));
            continue;
        }
        if let Some(tag) = spec.strip_prefix("tag:") {
            if !crate::package::validate::is_standard_tag(tag) {
                errors.push(format!("{name}: {spec}"));
            }
            continue;
        }
        if is_local_reject(spec) {
            errors.push(format!("{name}: {spec}"));
        }
    }
    if !errors.is_empty() {
        return Err(OhpmError::new(
            "InvalidPkgDepSpec",
            format!(
                "The oh-package.json5 dependency's spec is invalid: {}",
                errors.join(", ")
            ),
        ));
    }
    Ok(())
}

/// `isLocalDependency(spec) && !isInnerPkgDependency(spec)` — a path-like spec
/// that cannot be published as-is.
fn is_local_reject(spec: &str) -> bool {
    if spec == "latest" || is_valid_url(spec) || is_valid_range(spec) {
        return false;
    }
    !is_inner_spec(spec)
}

/// `isInnerPkgDependency`: `./`, `file:./`, `/`, `file:/`, `\` prefixes, or a
/// lowercase-starting spec without `:` (treated as a registry package name).
fn is_inner_spec(spec: &str) -> bool {
    if let Some(rest) = spec.to_ascii_lowercase().strip_prefix("file:") {
        // file:./  file:/  file:\  are inner; file:../ is NOT
        return rest.starts_with("./") || rest.starts_with('/') || rest.starts_with('\\');
    }
    if spec.starts_with("./") || spec.starts_with(".\\") || spec.starts_with('/') || spec.starts_with('\\') {
        return true;
    }
    let first = spec.chars().next().unwrap_or(' ');
    first.is_ascii_alphabetic() && !spec.contains(':')
}

fn is_valid_url(spec: &str) -> bool {
    url::Url::parse(spec).map(|u| u.host_str().is_some()).unwrap_or(false)
}

/// A practical npm-semver range checker covering `^1.2.3`, `~1.2`, `>=1.0.0
/// <2.0.0`, `1.2.x`, `1.2.3 - 2.0.0`, `||` unions and `*`.
pub fn is_valid_range(spec: &str) -> bool {
    let t = spec.trim();
    if t.is_empty() {
        return false;
    }
    if t == "*" || t.eq_ignore_ascii_case("x") {
        return true;
    }
    t.split("||").all(|group| {
        let group = group.trim();
        if let Some((a, b)) = group.split_once(" - ") {
            return is_version_or_x(a.trim()) && is_version_or_x(b.trim());
        }
        group.split_whitespace().all(is_range_part)
    })
}

fn is_range_part(part: &str) -> bool {
    let ver = if let Some(rest) = part.strip_prefix(">=") {
        rest
    } else if let Some(rest) = part.strip_prefix("<=") {
        rest
    } else if let Some(rest) = part.strip_prefix('^') {
        rest
    } else if let Some(rest) = part.strip_prefix('~') {
        rest
    } else if let Some(rest) = part.strip_prefix('>') {
        rest
    } else if let Some(rest) = part.strip_prefix('<') {
        rest
    } else if let Some(rest) = part.strip_prefix('=') {
        rest
    } else {
        part
    };
    is_version_or_x(ver)
}

fn is_version_or_x(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if s == "*" || s.eq_ignore_ascii_case("x") {
        return true;
    }
    let core = s.split('+').next().unwrap_or("");
    let main = core.split_once('-').map(|(m, _)| m).unwrap_or(core);
    let parts: Vec<&str> = main.split('.').collect();
    parts.len() <= 3
        && !parts.is_empty()
        && parts.iter().all(|p| {
            p.eq_ignore_ascii_case("x") || !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())
        })
}

/// The reference rejects package names ending in `.har`/`.tgz`/`.tar`/`.tar.gz`.
pub fn validate_name_suffix(name: &str) -> Result<()> {
    let lower = name.to_ascii_lowercase();
    for suffix in [".har", ".tgz", ".tar", ".tar.gz"] {
        if lower.ends_with(suffix) {
            return Err(OhpmError::new(
                "InvalidPackageName",
                format!("The package name \"{name}\" cannot end with \"{suffix}\"."),
            ));
        }
    }
    Ok(())
}

/// Field length limits from `OhPkgValidationConfig.json`.
pub fn validate_field_lengths(m: &crate::package::Manifest) -> Result<()> {
    if m.description.chars().count() > 512 {
        return Err(OhpmError::over_maximum_length("description"));
    }
    if m.license.chars().count() > 256 {
        return Err(OhpmError::over_maximum_length("license"));
    }
    for key in ["homepage", "repository"] {
        if let Some(v) = m.extra.get(key).and_then(|v| v.as_str()) {
            if v.chars().count() > 1024 {
                return Err(OhpmError::over_maximum_length(key));
            }
        }
    }
    Ok(())
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
    use std::collections::BTreeMap;

    #[test]
    fn dependency_spec_matrix_matches_reference() {
        // Classification verified against the reference implementation.
        for ok in [
            "^1.0.0",
            "1.2.3",
            "latest",
            "tag:beta",
            "file:./x",
            "file:/abs/x",
            "./x",
            "/abs",
            "libfoo.so",
            "com.example.lib",
            ">=1.0.0 <2.0.0",
            "1.2.x",
            "1.2.3 - 2.0.0",
            "*",
            "https://example.com/pkg",
        ] {
            let mut deps = BTreeMap::new();
            deps.insert("x".to_string(), ok.to_string());
            assert!(validate_dependency_specs(&deps).is_ok(), "{ok} should be ok");
        }
        for reject in ["../x", "file:../x", "~/x", "@ohos/foo", "C:\\x"] {
            let mut deps = BTreeMap::new();
            deps.insert("x".to_string(), reject.to_string());
            let err = validate_dependency_specs(&deps).unwrap_err();
            assert_eq!(err.code, "InvalidPkgDepSpec", "{reject} should be rejected");
        }
        // over-long spec and invalid tag
        let mut deps = BTreeMap::new();
        deps.insert("x".to_string(), "y".repeat(129));
        assert!(validate_dependency_specs(&deps).is_err());
        let mut deps = BTreeMap::new();
        deps.insert("x".to_string(), "tag:bad tag!".to_string());
        assert!(validate_dependency_specs(&deps).is_err());
    }

    #[test]
    fn name_suffix_and_field_lengths() {
        assert!(validate_name_suffix("com.example.lib").is_ok());
        for bad in ["a.har", "b.tgz", "c.tar", "d.tar.gz", "e.HAR"] {
            assert!(validate_name_suffix(bad).is_err(), "{bad} should be rejected");
        }

        let mut m = crate::package::Manifest::default();
        m.description = "x".repeat(513);
        assert!(validate_field_lengths(&m).is_err());
        m.description = "x".repeat(512);
        assert!(validate_field_lengths(&m).is_ok());
        m.extra.insert("repository".into(), serde_json::json!("https://x".repeat(400)));
        assert!(validate_field_lengths(&m).is_err());
    }

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
