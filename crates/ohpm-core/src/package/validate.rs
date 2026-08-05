//! Name / tag / version / package-type validation.
//! Mirrors `lib/common/Regex.js`, `isStandardTag.js` and the publish validators.

use crate::constants;
use crate::error::{OhpmError, Result};

/// `OHPM_PACKAGE_NAME_REGEX` from `Regex.js`, rewritten without look-around
/// (the Rust `regex` crate does not support it):
///
/// `(@(?![0-9\-_])[a-z0-9\-_]+(?<![\-_])\/)?(?![0-9\-_.])[a-z0-9\-_.]+(?<![\-_.])`
///
/// i.e. an optional `@group/` prefix plus a name; both are lowercase
/// alphanumeric (`-`/`_`, plus `.` for the name), the first char must be a
/// lowercase letter, and the last char must not be a separator.
pub fn is_standard_package_name(name: &str) -> bool {
    if name.is_empty() || name.chars().count() > 128 {
        return false;
    }
    let (group, local) = match name.split_once('/') {
        Some((g, rest)) => (Some(g), rest),
        None => (None, name),
    };
    if let Some(g) = group {
        let g = g.strip_prefix('@').unwrap_or_default();
        if !is_segment(g, true) {
            return false;
        }
    }
    is_segment(local, false)
}

fn is_segment(seg: &str, scoped: bool) -> bool {
    if seg.is_empty() {
        return false;
    }
    let allowed = |c: char| {
        c.is_ascii_lowercase()
            || c.is_ascii_digit()
            || c == '-'
            || c == '_'
            || (!scoped && c == '.')
    };
    if !seg.chars().all(allowed) {
        return false;
    }
    let first = seg.chars().next().unwrap();
    if !first.is_ascii_lowercase() {
        return false;
    }
    let last = seg.chars().last().unwrap();
    if last == '-' || last == '_' || (!scoped && last == '.') {
        return false;
    }
    true
}

/// `REGEX_TAG` from `Regex.js`; tag must also not be `latest`.
pub fn is_standard_tag(tag: &str) -> bool {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,59}$").unwrap());
    re.is_match(tag) && tag != constants::LATEST
}

/// Validate a tag argument (`validateTag.js`).
pub fn validate_tag(tag: Option<&str>) -> Result<()> {
    if let Some(t) = tag {
        if !is_standard_tag(t) {
            return Err(OhpmError::tag_invalid(t));
        }
    }
    Ok(())
}

/// Validate a package name.
pub fn validate_package_name(name: &str) -> Result<()> {
    if name.is_empty() || !is_standard_package_name(name) {
        return Err(OhpmError::package_name_invalid(name));
    }
    Ok(())
}

/// Validate a package version is a valid semantic version string.
/// Implements the `semver` core subset used by ohpm (no ranges at publish time).
pub fn is_valid_version(version: &str) -> bool {
    let v = version.trim();
    if v.is_empty() || v.len() > 256 {
        return false;
    }
    // strip build metadata
    let core = v.split('+').next().unwrap_or("");
    // split prerelease
    let (main, pre) = match core.split_once('-') {
        Some((m, p)) => (m, Some(p)),
        None => (core, None),
    };
    let parts: Vec<&str> = main.split('.').collect();
    if parts.len() > 3 {
        return false;
    }
    // Allow 1-3 numeric dot components (npm semver permits x, x.y, x.y.z).
    let mut seen = [0u64; 3];
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        // Avoid overflow / absurd lengths, and reject leading zeros.
        if part.len() > 10 || (part.len() > 1 && part.starts_with('0')) {
            return false;
        }
        seen[i] = part.parse().unwrap_or(0);
    }
    if let Some(pre) = pre {
        if pre.is_empty() {
            return false;
        }
        // prerelease: dot-separated alnum + hyphen identifiers, numeric ones
        // must not have leading zeros.
        for ident in pre.split('.') {
            if ident.is_empty() {
                return false;
            }
            if ident.bytes().all(|b| b.is_ascii_digit()) {
                if ident.len() > 1 && ident.starts_with('0') {
                    return false;
                }
            } else if !ident.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                return false;
            }
        }
    }
    let _ = seen;
    true
}

/// Validate a package version.
pub fn validate_version(version: &str) -> Result<()> {
    if !is_valid_version(version) {
        return Err(OhpmError::new(
            "InvalidVersionError",
            format!("The version \"{version}\" is not a valid semantic version."),
        ));
    }
    Ok(())
}

/// Validate `packageType` for a `.tgz` package (`PublishCore.validPackageType`).
pub fn validate_package_type_for_tgz(package_type: &str) -> Result<()> {
    if package_type.is_empty() {
        return Err(OhpmError::package_type_empty());
    }
    if package_type != constants::HSP_PACKAGE_TYPE {
        return Err(OhpmError::package_type_invalid());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_names() {
        assert!(is_standard_package_name("com.example.demo"));
        assert!(is_standard_package_name("@ohos/foo-bar"));
        assert!(!is_standard_package_name("Bad_Name"));
        assert!(!is_standard_package_name("@ohos/9bad"));
        assert!(!is_standard_package_name(""));
    }

    #[test]
    fn tags() {
        assert!(is_standard_tag("beta1"));
        assert!(is_standard_tag("a"));
        assert!(is_standard_tag("x.y_z-1"));
        assert!(!is_standard_tag("latest"));
        assert!(!is_standard_tag(""));
        assert!(!is_standard_tag("has space"));
        assert!(!is_standard_tag("-leading"));
    }

    #[test]
    fn versions() {
        assert!(is_valid_version("1.0.0"));
        assert!(is_valid_version("1.2"));
        assert!(is_valid_version("1"));
        assert!(is_valid_version("1.0.0-beta.1"));
        assert!(is_valid_version("1.0.0-beta.1+build"));
        assert!(!is_valid_version("1.0.0-beta.01"));
        assert!(!is_valid_version("1.0.0-"));
        assert!(!is_valid_version("abc"));
        assert!(!is_valid_version("1.0.0.0"));
        assert!(!is_valid_version(""));
    }
}
