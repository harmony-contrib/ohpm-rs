//! Package version bumping, including workspace-wide (unified) updates and
//! pre-release versions (e.g. `1.0.1-beta.1`).
//!
//! Two modes mirror the `ohpm version [--all]` command:
//! * **independent** — bump a single package (`bump_manifest_file`).
//! * **unified** — bump every workspace member to the same new version
//!   (`bump_workspace`), with the base version coming from the workspace root
//!   manifest or the highest member version (`unified_base_version`).

use std::cmp::Ordering;
use std::fmt;
use std::path::Path;

use crate::constants::MY_PACKAGE_JSON;
use crate::error::{OhpmError, Result};
use crate::package::validate::is_valid_version;
use crate::workspace::{Member, Workspace};

/// A single pre-release identifier: numeric (`beta.1` → 1) or alphanumeric.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrereleaseIdent {
    Numeric(u64),
    Alpha(String),
}

impl Ord for PrereleaseIdent {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (PrereleaseIdent::Numeric(a), PrereleaseIdent::Numeric(b)) => a.cmp(b),
            (PrereleaseIdent::Alpha(a), PrereleaseIdent::Alpha(b)) => a.cmp(b),
            // Per semver, numeric identifiers always sort lower than alpha.
            (PrereleaseIdent::Numeric(_), PrereleaseIdent::Alpha(_)) => Ordering::Less,
            (PrereleaseIdent::Alpha(_), PrereleaseIdent::Numeric(_)) => Ordering::Greater,
        }
    }
}

impl PartialOrd for PrereleaseIdent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A parsed semantic version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub prerelease: Vec<PrereleaseIdent>,
}

impl Version {
    /// Parse a full semantic version (with optional pre-release).
    pub fn parse(v: &str) -> Option<Version> {
        let v = v.trim();
        if v.is_empty() || v.len() > 256 {
            return None;
        }
        // Drop build metadata, split off pre-release.
        let core = v.split('+').next()?;
        let (main, pre) = match core.split_once('-') {
            Some((m, p)) => (m, Some(p)),
            None => (core, None),
        };
        let num = |s: &str| -> Option<u64> {
            if s.is_empty() || s.len() > 10 || !s.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            if s.len() > 1 && s.starts_with('0') {
                return None; // no leading zeros
            }
            s.parse().ok()
        };
        let parts: Vec<&str> = main.split('.').collect();
        if parts.len() > 3 || parts.is_empty() {
            return None;
        }
        let major = num(parts[0])?;
        let minor = parts.get(1).and_then(|s| num(s)).unwrap_or(0);
        let patch = parts.get(2).and_then(|s| num(s)).unwrap_or(0);

        let prerelease = match pre {
            None => Vec::new(),
            Some(p) => {
                if p.is_empty() {
                    return None;
                }
                let mut idents = Vec::new();
                for ident in p.split('.') {
                    if ident.is_empty() {
                        return None;
                    }
                    if ident.bytes().all(|b| b.is_ascii_digit()) {
                        idents.push(PrereleaseIdent::Numeric(num(ident)?));
                    } else if ident.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                        idents.push(PrereleaseIdent::Alpha(ident.to_string()));
                    } else {
                        return None;
                    }
                }
                idents
            }
        };
        Some(Version { major, minor, patch, prerelease })
    }

    /// Increment per `major | minor | patch | premajor | preminor | prepatch |
    /// prerelease`, with an optional pre-release identifier (`--preid`).
    pub fn inc(&self, action: &str, preid: Option<&str>) -> Option<Version> {
        let pre = |preid: Option<&str>| match preid {
            Some(id) => vec![PrereleaseIdent::Alpha(id.to_string()), PrereleaseIdent::Numeric(0)],
            None => vec![PrereleaseIdent::Numeric(0)],
        };
        let mut out = self.clone();
        match action {
            "major" => {
                out.major += 1;
                out.minor = 0;
                out.patch = 0;
                out.prerelease.clear();
            }
            "minor" => {
                out.minor += 1;
                out.patch = 0;
                out.prerelease.clear();
            }
            "patch" => {
                out.patch += 1;
                out.prerelease.clear();
            }
            "premajor" => {
                out.major += 1;
                out.minor = 0;
                out.patch = 0;
                out.prerelease = pre(preid);
            }
            "preminor" => {
                out.minor += 1;
                out.patch = 0;
                out.prerelease = pre(preid);
            }
            "prepatch" => {
                out.patch += 1;
                out.prerelease = pre(preid);
            }
            "prerelease" => {
                if self.prerelease.is_empty() {
                    out.patch += 1;
                    out.prerelease = pre(preid);
                } else {
                    // With a preid, reset when the leading identifier is a
                    // different preid (or numeric); otherwise increment.
                    let reset = match preid {
                        Some(id) => match self.prerelease.first() {
                            Some(PrereleaseIdent::Alpha(a)) => a != id,
                            _ => true,
                        },
                        None => false,
                    };
                    if reset {
                        out.prerelease = pre(preid);
                    } else {
                        let mut next = self.prerelease.clone();
                        match next.last_mut() {
                            Some(PrereleaseIdent::Numeric(n)) => *n += 1,
                            _ => next.push(PrereleaseIdent::Numeric(0)),
                        }
                        out.prerelease = next;
                    }
                }
            }
            _ => return None,
        }
        Some(out)
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        self.major
            .cmp(&other.major)
            .then(self.minor.cmp(&other.minor))
            .then(self.patch.cmp(&other.patch))
            .then_with(|| self.cmp_prerelease(other))
    }
}

impl Version {
    fn cmp_prerelease(&self, other: &Self) -> Ordering {
        match (self.prerelease.is_empty(), other.prerelease.is_empty()) {
            (true, true) => Ordering::Equal,
            // A release outranks any pre-release of the same version.
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                for (a, b) in self.prerelease.iter().zip(other.prerelease.iter()) {
                    let ord = a.cmp(b);
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }
                self.prerelease.len().cmp(&other.prerelease.len())
            }
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.prerelease.is_empty() {
            write!(f, "-")?;
            for (i, ident) in self.prerelease.iter().enumerate() {
                if i > 0 {
                    write!(f, ".")?;
                }
                match ident {
                    PrereleaseIdent::Numeric(n) => write!(f, "{n}")?,
                    PrereleaseIdent::Alpha(s) => write!(f, "{s}")?,
                }
            }
        }
        Ok(())
    }
}

/// Validate a `--preid` value: `[0-9A-Za-z-]+`, no leading zeros when numeric.
fn validate_preid(preid: &str) -> Result<()> {
    let ok = !preid.is_empty()
        && preid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !(preid.len() > 1
            && preid.starts_with('0')
            && preid.bytes().all(|b| b.is_ascii_digit()));
    if !ok {
        return Err(OhpmError::new(
            "InvalidPreid",
            format!("The pre-release identifier \"{preid}\" is invalid. Use [0-9A-Za-z-]+."),
        ));
    }
    Ok(())
}

/// Compute the new version for `base` given `major | minor | patch | pre* |
/// prerelease`, an exact version string, and an optional pre-release id.
pub fn resolve_new_version(base: &str, action: &str, preid: Option<&str>) -> Result<String> {
    if let Some(id) = preid {
        validate_preid(id)?;
    }
    let v = Version::parse(base).ok_or_else(|| {
        OhpmError::new(
            "InvalidOriginVersion",
            format!("The current version \"{base}\" is not a valid semantic version."),
        )
    })?;
    let next = match action {
        "major" | "minor" | "patch" | "premajor" | "preminor" | "prepatch" | "prerelease" => {
            v.inc(action, preid).map(|nv| nv.to_string())
        }
        exact if is_valid_version(exact) => Some(exact.to_string()),
        _ => None,
    };
    next.ok_or_else(|| {
        // Helpful hint: `--preid prerelease beta` — the action was passed to
        // --preid and a preid-like string became the action.
        let hint = match preid {
            Some(p) if is_action_name(p) => format!(
                " (note: \"{p}\" looks like an action — did you mean `ohpm version {p} --preid {action}`?)"
            ),
            _ => String::new(),
        };
        OhpmError::new(
            "InvalidVersionAction",
            format!(
                "The argument \"{action}\" is invalid. Use major, minor, patch, premajor, preminor, \
                 prepatch, prerelease, or a valid semantic version.{hint}"
            ),
        )
    })
}

fn is_action_name(s: &str) -> bool {
    matches!(s, "major" | "minor" | "patch" | "premajor" | "preminor" | "prepatch" | "prerelease")
}

/// Read the `oh-package.json5` at `path`, set its `version` to `new_version`,
/// and write it back (preserving every other field exactly). Returns
/// `(old_version, new_version)`.
pub fn bump_manifest_file(path: &Path, new_version: &str) -> Result<(String, String)> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        OhpmError::new(
            "ReadManifestFailed",
            format!("Failed to read {}: {e}", path.display()),
        )
    })?;
    let mut doc: serde_json::Value = json5::from_str(&text)?;
    let name = doc["name"].as_str().unwrap_or_default();
    if name.is_empty() {
        return Err(OhpmError::new(
            "PkgNameNotExist",
            format!("The package name does not exist in {}", path.display()),
        ));
    }
    let old = doc["version"].as_str().unwrap_or_default().to_string();
    doc["version"] = serde_json::json!(new_version);
    let pretty = serde_json::to_string_pretty(&doc)?;
    std::fs::write(path, format!("{pretty}\n"))?;
    Ok((old, new_version.to_string()))
}

/// The base version for a unified workspace bump: the workspace root's own
/// manifest version if the root is itself a publishable package, otherwise the
/// highest publishable member version. `publish: false` packages are ignored.
pub fn unified_base_version(ws: &Workspace) -> Result<String> {
    let root_manifest = ws.root.join(MY_PACKAGE_JSON);
    if root_manifest.is_file() {
        if let Ok(text) = std::fs::read_to_string(&root_manifest) {
            if let Ok(doc) = json5::from_str::<serde_json::Value>(&text) {
                let publishable = doc
                    .get("publish")
                    .and_then(|p| p.as_bool())
                    .unwrap_or(true);
                if publishable {
                    if let Some(v) = doc.get("version").and_then(|v| v.as_str()).filter(|v| !v.is_empty())
                    {
                        return Ok(v.to_string());
                    }
                }
            }
        }
    }
    let max = ws
        .members
        .iter()
        .filter(|m| m.manifest.publishable())
        .filter_map(|m| {
            let v = Version::parse(&m.manifest.version)?;
            Some((v, m.manifest.version.clone()))
        })
        .max()
        .map(|(_, version)| version);
    max.ok_or_else(|| {
        OhpmError::new(
            "NoPackageInWorkspace",
            "The workspace has no publishable packages with a valid version to bump.",
        )
    })
}

/// Set `new_version` on every publishable workspace member's
/// `oh-package.json5`, and on the workspace root's own manifest if present.
/// `publish: false` packages are skipped. Returns the directories bumped.
pub fn bump_workspace(ws: &Workspace, new_version: &str) -> Result<Vec<std::path::PathBuf>> {
    let selected: Vec<&Member> = ws.members.iter().filter(|m| m.manifest.publishable()).collect();
    let mut bumped = bump_members(&selected, new_version)?;
    let root_manifest = ws.root.join(MY_PACKAGE_JSON);
    if root_manifest.is_file() {
        if let Ok(text) = std::fs::read_to_string(&root_manifest) {
            if let Ok(doc) = json5::from_str::<serde_json::Value>(&text) {
                let publishable = doc.get("publish").and_then(|p| p.as_bool()).unwrap_or(true);
                if publishable {
                    bump_manifest_file(&root_manifest, new_version)?;
                    bumped.push(ws.root.clone());
                }
            }
        }
    }
    Ok(bumped)
}

/// Set `new_version` on the given members' manifests. `publish: false`
/// members are skipped. Returns the directories bumped.
pub fn bump_members(members: &[&Member], new_version: &str) -> Result<Vec<std::path::PathBuf>> {
    let mut bumped = Vec::new();
    for member in members {
        if !member.manifest.publishable() {
            log::info!("skip {}: publish is false", member.manifest.name);
            continue;
        }
        let path = member.dir.join(MY_PACKAGE_JSON);
        bump_manifest_file(&path, new_version)?;
        bumped.push(member.dir.clone());
    }
    Ok(bumped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    #[test]
    fn resolve_actions() {
        assert_eq!(resolve_new_version("1.2.3", "major", None).unwrap(), "2.0.0");
        assert_eq!(resolve_new_version("1.2.3", "minor", None).unwrap(), "1.3.0");
        assert_eq!(resolve_new_version("1.2.3", "patch", None).unwrap(), "1.2.4");
        assert_eq!(resolve_new_version("1.2.3", "2.0.0", None).unwrap(), "2.0.0");
        assert_eq!(resolve_new_version("1", "minor", None).unwrap(), "1.1.0");
        assert!(resolve_new_version("1.2.3", "nonsense", None).is_err());
        assert!(resolve_new_version("not-a-version", "patch", None).is_err());
    }

    #[test]
    fn version_parse_valid_and_invalid() {
        assert!(Version::parse("1.2.3").is_some());
        assert!(Version::parse("1.2").is_some());
        assert!(Version::parse("1").is_some());
        assert!(Version::parse("1.0.0-beta.1").is_some());
        assert!(Version::parse("1.0.0-beta").is_some());
        assert!(Version::parse("1.0.0-0").is_some());
        assert!(Version::parse("1.0.0-beta.1+build.5").is_some());
        // invalid
        assert!(Version::parse("").is_none());
        assert!(Version::parse("01.0.0").is_none());
        assert!(Version::parse("1.0.0-01").is_none());
        assert!(Version::parse("1.0.0-beta..1").is_none());
        assert!(Version::parse("1.0.0-").is_none());
        assert!(Version::parse("1.0.0.0").is_none());
        assert!(Version::parse("1.0.0-beta_1").is_none());
    }

    #[test]
    fn pre_release_increments() {
        // pre actions with default and explicit preid
        assert_eq!(resolve_new_version("1.2.3", "premajor", None).unwrap(), "2.0.0-0");
        assert_eq!(resolve_new_version("1.2.3", "premajor", Some("beta")).unwrap(), "2.0.0-beta.0");
        assert_eq!(resolve_new_version("1.2.3", "preminor", Some("rc")).unwrap(), "1.3.0-rc.0");
        assert_eq!(resolve_new_version("1.2.3", "prepatch", Some("beta")).unwrap(), "1.2.4-beta.0");

        // prerelease on a release version == prepatch
        assert_eq!(resolve_new_version("1.0.0", "prerelease", None).unwrap(), "1.0.1-0");
        assert_eq!(resolve_new_version("1.0.0", "prerelease", Some("beta")).unwrap(), "1.0.1-beta.0");
        // prerelease increments the trailing numeric identifier
        assert_eq!(resolve_new_version("1.0.1-beta.1", "prerelease", None).unwrap(), "1.0.1-beta.2");
        assert_eq!(resolve_new_version("1.0.1-beta", "prerelease", None).unwrap(), "1.0.1-beta.0");
        assert_eq!(resolve_new_version("1.0.1-rc.1", "prerelease", Some("rc")).unwrap(), "1.0.1-rc.2");
        // a different preid resets
        assert_eq!(resolve_new_version("1.0.1-alpha.1", "prerelease", Some("rc")).unwrap(), "1.0.1-rc.0");

        // exact pre-release versions are accepted
        assert_eq!(resolve_new_version("1.0.0", "1.0.0-beta.1", None).unwrap(), "1.0.0-beta.1");

        // invalid preid
        assert!(resolve_new_version("1.0.0", "prerelease", Some("0beta!")).is_err());
        assert!(resolve_new_version("1.0.0", "prerelease", Some("01")).is_err());
    }

    #[test]
    fn swapped_preid_action_hint() {
        // `version --preid prerelease beta`: the action was swallowed by --preid.
        let err = resolve_new_version("1.0.0", "beta", Some("prerelease")).unwrap_err();
        assert_eq!(err.code, "InvalidVersionAction");
        assert!(err.message.contains("ohpm version prerelease --preid beta"));
    }

    #[test]
    fn version_ordering() {
        let cmp = |a: &str, b: &str| Version::parse(a).unwrap().cmp(&Version::parse(b).unwrap());
        use std::cmp::Ordering;
        assert_eq!(cmp("1.0.0", "1.0.0-beta"), Ordering::Greater);
        assert_eq!(cmp("1.0.0-beta", "1.0.0-rc"), Ordering::Less);
        assert_eq!(cmp("1.0.0-rc.1", "1.0.0-rc.0"), Ordering::Greater);
        assert_eq!(cmp("1.0.0-0", "1.0.0-beta"), Ordering::Less);
        assert_eq!(cmp("1.0.0-beta.1", "1.0.0-beta.1"), Ordering::Equal);
    }

    #[test]
    fn bump_manifest_preserves_fields() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(MY_PACKAGE_JSON);
        write(
            dir.path(),
            MY_PACKAGE_JSON,
            "{ name: \"pkg.a\", version: \"1.0.0\", description: \"keep\", deps: {} }\n",
        );
        let (old, new) = bump_manifest_file(&path, "2.0.0").unwrap();
        assert_eq!(old, "1.0.0");
        assert_eq!(new, "2.0.0");

        let doc: serde_json::Value = json5::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(doc["version"], "2.0.0");
        assert_eq!(doc["description"], "keep");
        assert!(doc.get("deps").is_some());
    }

    #[test]
    fn unified_base_version_prefers_root_manifest() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "ohpm-workspace.yaml",
            "packages:\n  - packages/*\n",
        );
        write(
            dir.path(),
            MY_PACKAGE_JSON,
            "{ name: \"root\", version: \"5.0.0\" }\n",
        );
        write(dir.path(), "packages/a/oh-package.json5", "{ name: \"pkg.a\", version: \"1.0.0\" }\n");
        write(dir.path(), "packages/b/oh-package.json5", "{ name: \"pkg.b\", version: \"9.0.0\" }\n");

        let ws = Workspace::load(dir.path()).unwrap();
        assert_eq!(unified_base_version(&ws).unwrap(), "5.0.0");
    }

    #[test]
    fn unified_base_version_falls_back_to_max_member() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "ohpm-workspace.yaml",
            "packages:\n  - packages/*\n",
        );
        write(dir.path(), "packages/a/oh-package.json5", "{ name: \"pkg.a\", version: \"1.0.0\" }\n");
        write(dir.path(), "packages/b/oh-package.json5", "{ name: \"pkg.b\", version: \"3.2.1\" }\n");

        let ws = Workspace::load(dir.path()).unwrap();
        assert_eq!(unified_base_version(&ws).unwrap(), "3.2.1");
    }

    #[test]
    fn unified_base_version_prefers_release_over_prerelease() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "ohpm-workspace.yaml",
            "packages:\n  - packages/*\n",
        );
        write(dir.path(), "packages/a/oh-package.json5", "{ name: \"pkg.a\", version: \"1.0.0\" }\n");
        write(dir.path(), "packages/b/oh-package.json5", "{ name: \"pkg.b\", version: \"1.0.0-beta.1\" }\n");

        let ws = Workspace::load(dir.path()).unwrap();
        // A release outranks a pre-release of the same version.
        assert_eq!(unified_base_version(&ws).unwrap(), "1.0.0");
    }

    #[test]
    fn bump_workspace_unifies_all_members() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "ohpm-workspace.yaml",
            "packages:\n  - packages/*\n",
        );
        write(dir.path(), "packages/a/oh-package.json5", "{ name: \"pkg.a\", version: \"1.0.0\" }\n");
        write(dir.path(), "packages/b/oh-package.json5", "{ name: \"pkg.b\", version: \"1.0.0\" }\n");

        let ws = Workspace::load(dir.path()).unwrap();
        let new = resolve_new_version(&unified_base_version(&ws).unwrap(), "minor", None).unwrap();
        assert_eq!(new, "1.1.0");
        let bumped = bump_workspace(&ws, &new).unwrap();
        assert_eq!(bumped.len(), 2);

        for rel in ["packages/a/oh-package.json5", "packages/b/oh-package.json5"] {
            let text = std::fs::read_to_string(dir.path().join(rel)).unwrap();
            let doc: serde_json::Value = json5::from_str(&text).unwrap();
            assert_eq!(doc["version"], "1.1.0", "{rel} must be unified");
        }
    }

    #[test]
    fn unified_skips_publish_false_members() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "ohpm-workspace.yaml",
            "packages:\n  - packages/*\n",
        );
        write(dir.path(), "packages/a/oh-package.json5", "{ name: \"pkg.a\", version: \"1.0.0\" }\n");
        write(
            dir.path(),
            "packages/priv/oh-package.json5",
            "{ name: \"pkg.priv\", version: \"1.0.0\", publish: false }\n",
        );

        let ws = Workspace::load(dir.path()).unwrap();
        // base comes from the only publishable member
        assert_eq!(unified_base_version(&ws).unwrap(), "1.0.0");
        let new = resolve_new_version(&unified_base_version(&ws).unwrap(), "minor", None).unwrap();
        let bumped = bump_workspace(&ws, &new).unwrap();
        assert_eq!(bumped.len(), 1, "publish:false member is skipped");

        let priv_text = std::fs::read_to_string(dir.path().join("packages/priv/oh-package.json5")).unwrap();
        let doc: serde_json::Value = json5::from_str(&priv_text).unwrap();
        assert_eq!(doc["version"], "1.0.0", "publish:false member must not be bumped");
    }

    #[test]
    fn bump_members_respects_filter_and_publish() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "ohpm-workspace.yaml",
            "packages:\n  - packages/*\n",
        );
        write(dir.path(), "packages/a/oh-package.json5", "{ name: \"pkg.a\", version: \"1.0.0\" }\n");
        write(dir.path(), "packages/b/oh-package.json5", "{ name: \"pkg.b\", version: \"1.0.0\" }\n");

        let ws = Workspace::load(dir.path()).unwrap();
        let selected = ws.filtered_members(&["pkg.b".to_string()]).unwrap();
        let bumped = bump_members(&selected, "9.9.9").unwrap();
        assert_eq!(bumped.len(), 1);

        let a: serde_json::Value =
            json5::from_str(&std::fs::read_to_string(dir.path().join("packages/a/oh-package.json5")).unwrap())
                .unwrap();
        let b: serde_json::Value =
            json5::from_str(&std::fs::read_to_string(dir.path().join("packages/b/oh-package.json5")).unwrap())
                .unwrap();
        assert_eq!(a["version"], "1.0.0");
        assert_eq!(b["version"], "9.9.9");
    }
}
