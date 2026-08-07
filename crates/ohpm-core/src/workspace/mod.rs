//! Workspace mode support.
//!
//! A workspace root is identified by an `ohpm-workspace.yaml` file (schema
//! modeled after `pnpm-workspace.yaml`):
//!
//! ```yaml
//! packages:
//!   - "packages/*"
//!   - "modules/**"
//! ```
//!
//! When publishing a package that lives in a workspace, `file:` protocol
//! dependencies are resolved to the target package's version before the
//! metadata is uploaded, so published manifests never reference local paths.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::constants::{HAR_SUFFIX, MY_PACKAGE_JSON, TGZ_SUFFIX};
use crate::error::{OhpmError, Result};
use crate::package::{self, Manifest};

/// The workspace config filename at the workspace root.
pub const WORKSPACE_CONFIG: &str = "ohpm-workspace.yaml";

/// Versioning policy declared in `ohpm-workspace.yaml` (`version.mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VersionMode {
    /// All member packages are bumped to the same version.
    Unified,
    /// Each package is bumped independently.
    #[default]
    Independent,
}

/// Parsed `ohpm-workspace.yaml` (minimal YAML subset).
#[derive(Debug, Clone, Default)]
pub struct WorkspaceConfig {
    /// `packages:` globs.
    pub packages: Vec<String>,
    /// `exclude:` globs or package names removed from membership.
    pub exclude: Vec<String>,
    /// `version.mode`: unified | independent.
    pub version_mode: VersionMode,
}

/// A package belonging to the workspace.
#[derive(Debug, Clone)]
pub struct Member {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

/// A loaded workspace.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// Directory containing `ohpm-workspace.yaml`.
    pub root: PathBuf,
    pub members: Vec<Member>,
    /// The versioning policy declared in the config.
    pub version_mode: VersionMode,
}

/// A parsed `workspace:` spec (mirrors pnpm's workspace protocol grammar).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceQuery {
    /// `workspace:*` / bare `workspace:` — any local version.
    Any,
    /// `workspace:^` — caret against the member's version.
    Caret,
    /// `workspace:~` — tilde against the member's version.
    Tilde,
    /// An explicit range/version (`workspace:^1.5.0`, `workspace:1.5.0`).
    Range(String),
    /// A path form (`workspace:./foo`, `workspace:../foo`).
    Path(String),
    /// The alias form (`workspace:foo@*`, `workspace:foo@^1.0.0`).
    Alias { member: String, range: Option<String> },
}

impl WorkspaceQuery {
    pub fn parse(spec: &str) -> WorkspaceQuery {
        let rest = spec
            .strip_prefix(crate::constants::WORKSPACE_PREFIX)
            .unwrap_or(spec);
        match rest {
            "" | "*" => WorkspaceQuery::Any,
            "^" => WorkspaceQuery::Caret,
            "~" => WorkspaceQuery::Tilde,
            _ if rest.starts_with('.') => WorkspaceQuery::Path(rest.to_string()),
            _ => {
                if let Some((member, range)) = rest.split_once('@') {
                    WorkspaceQuery::Alias {
                        member: member.to_string(),
                        range: Some(range.to_string()),
                    }
                } else {
                    WorkspaceQuery::Range(rest.to_string())
                }
            }
        }
    }
}

/// Resolve a workspace query against a member version (pnpm semantics:
/// `*`/`^`/`~` pick the version including prereleases; explicit ranges go
/// through max-satisfying).
pub fn resolve_workspace_version(
    query: &WorkspaceQuery,
    member_version: &str,
    name: &str,
    _spec: &str,
) -> Result<String> {
    match query {
        WorkspaceQuery::Any | WorkspaceQuery::Caret | WorkspaceQuery::Tilde => {
            Ok(member_version.to_string())
        }
        WorkspaceQuery::Alias { range: None, .. } => Ok(member_version.to_string()),
        WorkspaceQuery::Range(range) | WorkspaceQuery::Alias { range: Some(range), .. } => {
            crate::install::semver::semver_max_satisfying(&[member_version.to_string()], range)
                .ok_or_else(|| {
                    OhpmError::workspace_no_matching_version(name, range, member_version)
                })
        }
        WorkspaceQuery::Path(_) => Ok(member_version.to_string()),
    }
}

impl Workspace {
    /// Find the nearest workspace root walking up from `dir`. Returns
    /// `Ok(None)` when `dir` is not inside a workspace.
    pub fn find(from: &Path) -> Result<Option<Workspace>> {
        let mut cur = from.to_path_buf();
        loop {
            if cur.join(WORKSPACE_CONFIG).is_file() {
                return Ok(Some(Workspace::load(&cur)?));
            }
            if !cur.pop() {
                return Ok(None);
            }
        }
    }

    /// Load a workspace from its root directory.
    pub fn load(root: &Path) -> Result<Workspace> {
        // Canonicalize so member dirs (canonicalized during discovery) can be
        // compared against the root for exclude-path matching.
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let config_path = root.join(WORKSPACE_CONFIG);
        let text = std::fs::read_to_string(&config_path).map_err(|e| {
            OhpmError::new(
                "WorkspaceConfigReadFailed",
                format!("Failed to read {}: {e}", config_path.display()),
            )
        })?;
        let cfg = parse_config(&text);
        let members = discover_members(&root, &cfg.packages, &cfg.exclude)?;
        Ok(Workspace {
            root: root.to_path_buf(),
            members,
            version_mode: cfg.version_mode,
        })
    }

    /// Index of workspace members by package name.
    pub fn members_by_name(&self) -> BTreeMap<&str, &Member> {
        self.members
            .iter()
            .filter_map(|m| {
                let name = m.manifest.name.as_str();
                (!name.is_empty()).then_some((name, m))
            })
            .collect()
    }

    /// Select members by `filter` (exact package names). An empty filter means
    /// every member. Any filter name that matches nothing is an error.
    pub fn filtered_members(&self, filter: &[String]) -> Result<Vec<&Member>> {
        if filter.is_empty() {
            return Ok(self.members.iter().collect());
        }
        let mut out = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for name in filter {
            let matched: Vec<&Member> = self
                .members
                .iter()
                .filter(|m| m.manifest.name == *name)
                .collect();
            if matched.is_empty() {
                return Err(OhpmError::new(
                    "NoPkgMatchFilter",
                    format!("No workspace package matches the filter \"{name}\"."),
                ));
            }
            for m in matched {
                if seen.insert(m.dir.clone()) {
                    out.push(m);
                }
            }
        }
        Ok(out)
    }
}

/// Parse a minimal `ohpm-workspace.yaml`.
///
/// Supported: top-level `packages:` and `exclude:` block lists of scalar
/// strings (quoted or bare), and `version:`/`mode:` for the versioning policy.
/// Other top-level keys are ignored. Comments (`#`) and blank lines are
/// stripped.
pub fn parse_config(text: &str) -> WorkspaceConfig {
    let mut cfg = WorkspaceConfig::default();
    // The section the current line belongs to.
    let mut section = String::new();
    for raw in text.lines() {
        let line = strip_comment(raw).trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Top-level key (no leading whitespace, ends with ':').
        if !line.starts_with(char::is_whitespace) {
            if let Some(key) = trimmed.strip_suffix(':') {
                section = key.trim().to_string();
            } else {
                section.clear();
            }
            continue;
        }
        match section.as_str() {
            "packages" => push_list_item(&mut cfg.packages, trimmed),
            "exclude" => push_list_item(&mut cfg.exclude, trimmed),
            "version" => {
                if let Some((k, v)) = trimmed.split_once(':') {
                    if k.trim() == "mode" {
                        cfg.version_mode = parse_version_mode(v);
                    }
                }
            }
            _ => {}
        }
    }
    cfg
}

fn parse_version_mode(value: &str) -> VersionMode {
    match value.trim().to_ascii_lowercase().as_str() {
        "unified" | "fixed" => VersionMode::Unified,
        _ => VersionMode::Independent,
    }
}

fn push_list_item(list: &mut Vec<String>, trimmed: &str) {
    if let Some(item) = trimmed.strip_prefix('-') {
        let value = item.trim();
        if !value.is_empty() {
            list.push(unquote(value));
        }
    }
}

fn strip_comment(line: &str) -> &str {
    // A '#' starts a comment only when not inside quotes.
    let mut in_quote = false;
    let mut quote = ' ';
    for (i, c) in line.char_indices() {
        match c {
            '"' | '\'' if !in_quote => {
                in_quote = true;
                quote = c;
            }
            '"' | '\'' if in_quote && c == quote => in_quote = false,
            '#' if !in_quote => return &line[..i],
            _ => {}
        }
    }
    line
}

fn unquote(s: &str) -> String {
    if s.len() >= 2 {
        let b = s.as_bytes();
        if (b[0] == b'"' && b[s.len() - 1] == b'"') || (b[0] == b'\'' && b[s.len() - 1] == b'\'') {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

/// Expand workspace globs, drop `exclude`d entries, and keep directories that
/// contain an `oh-package.json5`.
fn discover_members(root: &Path, patterns: &[String], exclude: &[String]) -> Result<Vec<Member>> {
    let mut seen = std::collections::BTreeSet::new();
    let mut members = Vec::new();
    for pattern in patterns {
        let full = if Path::new(pattern).is_absolute() {
            PathBuf::from(pattern)
        } else {
            root.join(pattern)
        };
        let pattern_str = full.to_string_lossy().into_owned();
        let entries = glob::glob(&pattern_str)
            .map_err(|e| OhpmError::new("WorkspaceGlobError", format!("{pattern_str}: {e}")))?;
        for entry in entries.flatten() {
            // Lexical normalization only — the same basis as
            // `resolve_file_spec`, so member paths compare equal to
            // `file:`-derived paths and relativize cleanly against the
            // module root (`canonicalize()` would break the common-prefix
            // on symlinked roots like macOS `/var` -> `/private/var`).
            let dir = normalize_lexical(&entry);
            if !dir.is_dir() {
                continue;
            }
            if !dir.join(MY_PACKAGE_JSON).is_file() {
                continue;
            }
            if !seen.insert(dir.clone()) {
                continue;
            }
            match package::read_manifest_from_dir(&dir) {
                Ok(manifest) => {
                    if is_excluded(root, &dir, &manifest.name, exclude) {
                        log::debug!("exclude workspace member {}", dir.display());
                        continue;
                    }
                    members.push(Member { dir, manifest });
                }
                Err(e) => log::warn!("skip workspace member {}: {e}", dir.display()),
            }
        }
    }
    Ok(members)
}

/// An exclude entry matches a member when it equals its package name or when it
/// is a glob matching its path relative to the workspace root.
fn is_excluded(root: &Path, dir: &Path, name: &str, exclude: &[String]) -> bool {
    if exclude.iter().any(|e| e == name) {
        return true;
    }
    let rel = dir.strip_prefix(root).unwrap_or(dir).to_string_lossy().into_owned();
    // Glob patterns use '/' separators; normalize the path on Windows.
    let rel = rel.replace('\\', "/");
    exclude.iter().any(|e| {
        glob::Pattern::new(e)
            .map(|p| p.matches(&rel))
            .unwrap_or(false)
    })
}

/// Resolve a `file:` spec to an absolute path, mirroring the reference
/// (`REGEX_LOCAL`, `~` expansion, relative to `base`).
pub fn resolve_file_spec(base: &Path, spec: &str) -> PathBuf {
    let path = strip_file_prefix(spec);
    let path = path.trim();
    if let Some(rest) = path.strip_prefix('~') {
        let home = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
        let joined = home.join(rest.trim_start_matches(['/', '\\']));
        return joined;
    }
    let p = PathBuf::from(path);
    let joined = if p.is_absolute() { p } else { base.join(p) };
    normalize_lexical(&joined)
}

/// Lexically normalize a path: resolve `.` and `..` components without touching
/// the filesystem (`a/../b` -> `b`).
fn normalize_lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() && !out.has_root() {
                    out.push("..");
                }
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

/// Strip the `file:` protocol prefix (case-insensitive) and optional
/// whitespace.
fn strip_file_prefix(spec: &str) -> &str {
    // Manual case-insensitive prefix check to avoid an alloc.
    let lower = spec.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("file:") {
        let skip = spec.len() - rest.len();
        let tail = &spec[skip..];
        let tail = tail.trim_start();
        return tail;
    }
    spec
}

/// Whether a dependency spec uses the `file:` protocol.
pub fn is_file_spec(spec: &str) -> bool {
    let t = spec.trim();
    t.to_ascii_lowercase().starts_with("file:")
}

/// Read the name+version of a local package target (`dir` or har/tgz file).
fn read_target(dir_or_file: &Path) -> Option<(String, String)> {
    let target = dir_or_file;
    if target.is_dir() {
        let manifest_path = target.join(MY_PACKAGE_JSON);
        if manifest_path.is_file() {
            if let Ok(text) = std::fs::read_to_string(&manifest_path) {
                if let Ok(m) = Manifest::from_json5(&text) {
                    return (!m.name.is_empty() && !m.version.is_empty())
                        .then_some((m.name, m.version));
                }
            }
        }
        return None;
    }
    // A packaged `.har` / `.tgz`.
    let name = target.to_string_lossy().to_lowercase();
    if name.ends_with(HAR_SUFFIX) || name.ends_with(TGZ_SUFFIX) {
        let tmp = crate::config::default::default_cache()
            .join("ws-probe")
            .join(uuid::Uuid::new_v4().simple().to_string());
        if let Ok(m) = package::read_manifest_from_archive(target, &tmp) {
            let _ = std::fs::remove_dir_all(&tmp);
            if !m.name.is_empty() && !m.version.is_empty() {
                return Some((m.name, m.version));
            }
        }
    }
    None
}

/// Process `file:` dependencies in `manifest`, rewriting each to the target
/// package's version.
///
/// * `dependencies` entries that cannot be resolved produce an error.
/// * `devDependencies` entries that cannot be resolved are kept with a warning.
///
/// Returns the number of entries rewritten.
pub fn process_file_dependencies(
    ws: &Workspace,
    package_root: &Path,
    manifest: &mut Manifest,
) -> Result<usize> {
    let mut rewritten = 0usize;

    let deps = std::mem::take(&mut manifest.dependencies);
    manifest.dependencies = process_deps(ws, package_root, &deps, true, &mut rewritten)?;
    let dev = std::mem::take(&mut manifest.dev_dependencies);
    manifest.dev_dependencies = process_deps(ws, package_root, &dev, false, &mut rewritten)?;

    Ok(rewritten)
}

/// Rewrite `workspace:` dependencies for publishing, mirroring pnpm's
/// `exportable-manifest` replace table:
///
/// | declared | published (member at X.Y.Z) |
/// |---|---|
/// | `workspace:*` / `workspace:` | exact version |
/// | `workspace:^` | `^X.Y.Z` |
/// | `workspace:~` | `~X.Y.Z` |
/// | `workspace:<range>` | kept verbatim |
/// | `workspace:./dir` | exact version |
/// | `workspace:<member>@*` | `ohpm:<member>@X.Y.Z` |
pub fn process_workspace_dependencies(
    ws: &Workspace,
    _package_root: &Path,
    manifest: &mut Manifest,
) -> Result<usize> {
    let mut rewritten = 0usize;
    let deps = std::mem::take(&mut manifest.dependencies);
    manifest.dependencies = process_workspace_deps(ws, &deps, true, &mut rewritten)?;
    let dev = std::mem::take(&mut manifest.dev_dependencies);
    manifest.dev_dependencies = process_workspace_deps(ws, &dev, false, &mut rewritten)?;
    let dynamic = std::mem::take(&mut manifest.dynamic_dependencies);
    manifest.dynamic_dependencies = process_workspace_deps(ws, &dynamic, true, &mut rewritten)?;
    Ok(rewritten)
}

fn process_workspace_deps(
    ws: &Workspace,
    deps: &BTreeMap<String, String>,
    _strict: bool,
    rewritten: &mut usize,
) -> Result<BTreeMap<String, String>> {
    let _ = (ws, _strict);
    let members = ws.members_by_name();
    let mut out = BTreeMap::new();
    for (name, spec) in deps {
        let Some(rest) = spec.strip_prefix(crate::constants::WORKSPACE_PREFIX) else {
            out.insert(name.clone(), spec.clone());
            continue;
        };
        let version = match rest {
            "" | "*" => replace_workspace(name, spec, rest, ws, &members, false, None)?,
            "^" => replace_workspace(name, spec, rest, ws, &members, false, Some("^"))?,
            "~" => replace_workspace(name, spec, rest, ws, &members, false, Some("~"))?,
            _ if rest.starts_with('.') => {
                // Path form: resolve the member's exact version.
                let resolved = resolve_file_spec(
                    ws.root.as_path(),
                    &format!("file:{}", rest.trim_start_matches("./")),
                );
                let member = ws
                    .members
                    .iter()
                    .find(|m| m.dir == resolved || resolved.starts_with(&m.dir))
                    .ok_or_else(|| OhpmError::workspace_pkg_not_found(name, spec))?;
                member.manifest.version.clone()
            }
            _ => {
                if let Some((member, range)) = rest.split_once('@') {
                    let m = members
                        .get(member)
                        .copied()
                        .ok_or_else(|| OhpmError::workspace_pkg_not_found(member, spec))?;
                    let v = m.manifest.version.clone();
                    match range {
                        "*" | "" => format!("ohpm:{member}@{v}"),
                        r => format!("ohpm:{member}@{r}"),
                    }
                } else {
                    // Explicit range: kept verbatim (pnpm parity).
                    format!("workspace:{rest}")
                }
            }
        };
        if version == *spec {
            out.insert(name.clone(), spec.clone());
            continue;
        }
        out.insert(name.clone(), version);
        *rewritten += 1;
    }
    Ok(out)
}

fn replace_workspace(
    name: &str,
    spec: &str,
    _rest: &str,
    _ws: &Workspace,
    members: &BTreeMap<&str, &Member>,
    _strict: bool,
    prefix: Option<&str>,
) -> Result<String> {
    let member = members
        .get(name)
        .copied()
        .ok_or_else(|| OhpmError::workspace_pkg_not_found(name, spec))?;
    let v = member.manifest.version.clone();
    Ok(match prefix {
        Some("^") => format!("^{v}"),
        Some("~") => format!("~{v}"),
        _ => v,
    })
}

fn process_deps(
    ws: &Workspace,
    package_root: &Path,
    deps: &BTreeMap<String, String>,
    strict: bool,
    rewritten: &mut usize,
) -> Result<BTreeMap<String, String>> {
    let members = ws.members_by_name();
    let mut out = BTreeMap::new();
    for (name, spec) in deps {
        if !is_file_spec(spec) {
            out.insert(name.clone(), spec.clone());
            continue;
        }
        let target = resolve_file_spec(package_root, spec);
        let version = read_target(&target).map(|(tname, tversion)| {
            if !tname.is_empty() && tname != *name {
                log::warn!(
                    "file: dependency \"{name}\" resolves to package \"{tname}\" whose name differs; \
                     the dependency key is kept"
                );
            }
            tversion
        });
        match version {
            Some(v) => {
                out.insert(name.clone(), v);
                *rewritten += 1;
            }
            None => {
                // A workspace member that exists but is unreadable shouldn't
                // silently pass; still, only `dependencies` is strict.
                if strict {
                    return Err(OhpmError::new(
                        "FileDependencyUnresolvable",
                        format!(
                            "The file: dependency \"{name}\" ({spec}) cannot be resolved to a package \
                             (\"{}\" is not a directory with {} or a valid {}.har/.tgz). \
                             Workspace members: {}",
                            target.display(),
                            MY_PACKAGE_JSON,
                            "pkg",
                            member_list(&members),
                        ),
                    ));
                }
                log::warn!(
                    "file: devDependency \"{name}\" ({spec}) is kept as-is because it cannot be \
                     resolved to a local package"
                );
                out.insert(name.clone(), spec.clone());
            }
        }
    }
    Ok(out)
}

fn member_list(members: &BTreeMap<&str, &Member>) -> String {
    if members.is_empty() {
        "none".to_string()
    } else {
        members.keys().copied().collect::<Vec<_>>().join(", ")
    }
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

    fn write_pkg(dir: &Path, rel: &str, name: &str, version: &str) {
        write(
            dir,
            &format!("{rel}/{}", MY_PACKAGE_JSON),
            &format!("{{ name: \"{name}\", version: \"{version}\" }}\n"),
        );
    }

    #[test]
    fn parse_minimal_yaml() {
        let text = r#"
# workspace
packages:
  - "packages/*"
  - modules/**
  - 'legacy'
"#;
        let cfg = parse_config(text);
        assert_eq!(cfg.packages, vec!["packages/*", "modules/**", "legacy"]);
        assert!(cfg.exclude.is_empty());
        assert_eq!(cfg.version_mode, VersionMode::Independent);
    }

    #[test]
    fn parse_exclude_and_version_mode() {
        let text = r#"
packages:
  - "packages/*"
exclude:
  - packages/internal
  - "@scope/private"
version:
  mode: unified
"#;
        let cfg = parse_config(text);
        assert_eq!(cfg.exclude, vec!["packages/internal", "@scope/private"]);
        assert_eq!(cfg.version_mode, VersionMode::Unified);
    }

    #[test]
    fn parse_version_mode_unknown_defaults_independent() {
        let cfg = parse_config("version:\n  mode: something-else\n");
        assert_eq!(cfg.version_mode, VersionMode::Independent);
    }

    #[test]
    fn find_from_member_dir() {
        let root = TempDir::new().unwrap();
        write(&root.path(), WORKSPACE_CONFIG, "packages:\n  - packages/*\n");
        write_pkg(&root.path(), "packages/a", "pkg.a", "1.0.0");
        write_pkg(&root.path(), "packages/b", "pkg.b", "2.0.0");

        let ws = Workspace::find(&root.path().join("packages/a")).unwrap().unwrap();
        assert_eq!(ws.members.len(), 2);
        let by_name = ws.members_by_name();
        assert_eq!(by_name.get("pkg.b").unwrap().manifest.version, "2.0.0");
    }

    #[test]
    fn find_returns_none_outside_workspace() {
        let dir = TempDir::new().unwrap();
        assert!(Workspace::find(dir.path()).unwrap().is_none());
    }

    #[test]
    fn exclude_by_path_and_name() {
        let root = TempDir::new().unwrap();
        write(
            &root.path(),
            WORKSPACE_CONFIG,
            "packages:\n  - packages/*\nexclude:\n  - packages/internal\n  - pkg.b\n",
        );
        write_pkg(&root.path(), "packages/a", "pkg.a", "1.0.0");
        write_pkg(&root.path(), "packages/internal", "pkg.internal", "1.0.0");
        write_pkg(&root.path(), "packages/b", "pkg.b", "2.0.0");

        let ws = Workspace::load(root.path()).unwrap();
        let names: Vec<&str> = ws.members.iter().map(|m| m.manifest.name.as_str()).collect();
        assert_eq!(names, vec!["pkg.a"], "internal (path) and pkg.b (name) are excluded");
    }

    #[test]
    fn filtered_members_select_and_validate() {
        let root = TempDir::new().unwrap();
        write(&root.path(), WORKSPACE_CONFIG, "packages:\n  - packages/*\n");
        write_pkg(&root.path(), "packages/a", "pkg.a", "1.0.0");
        write_pkg(&root.path(), "packages/b", "pkg.b", "2.0.0");

        let ws = Workspace::load(root.path()).unwrap();
        // empty filter = all
        assert_eq!(ws.filtered_members(&[]).unwrap().len(), 2);
        // specific
        let selected = ws.filtered_members(&["pkg.b".to_string()]).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].manifest.name, "pkg.b");
        // unknown name errors
        let err = ws.filtered_members(&["nope".to_string()]).unwrap_err();
        assert_eq!(err.code, "NoPkgMatchFilter");
    }

    #[test]
    fn version_mode_read_from_yaml() {
        let root = TempDir::new().unwrap();
        write(
            &root.path(),
            WORKSPACE_CONFIG,
            "packages:\n  - packages/*\nversion:\n  mode: unified\n",
        );
        let ws = Workspace::load(root.path()).unwrap();
        assert_eq!(ws.version_mode, VersionMode::Unified);
    }

    #[test]
    fn resolve_file_spec_variants() {
        let base = Path::new("/tmp/ws/packages/a");
        assert_eq!(resolve_file_spec(base, "file:../b"), Path::new("/tmp/ws/packages/b"));
        assert_eq!(resolve_file_spec(base, "file:./lib"), Path::new("/tmp/ws/packages/a/lib"));
        assert_eq!(resolve_file_spec(base, "file:/abs/x"), Path::new("/abs/x"));
        assert_eq!(resolve_file_spec(base, "FILE: ../b"), Path::new("/tmp/ws/packages/b"));
        // ~ expansion
        assert!(resolve_file_spec(base, "file:~/pkg").starts_with(dirs::home_dir().unwrap()));
    }

    #[test]
    fn process_file_dependencies_rewrites() {
        let root = TempDir::new().unwrap();
        write(&root.path(), WORKSPACE_CONFIG, "packages:\n  - packages/*\n");
        write_pkg(&root.path(), "packages/a", "pkg.a", "1.0.0");
        write_pkg(&root.path(), "packages/b", "pkg.b", "2.3.4");

        let ws = Workspace::find(&root.path().join("packages/a")).unwrap().unwrap();
        let mut m = Manifest {
            name: "pkg.a".into(),
            version: "1.0.0".into(),
            dependencies: [
                ("pkg.b".to_string(), "file:../b".to_string()),
                ("pkg.remote".to_string(), "^3.0.0".to_string()),
            ]
            .into(),
            dev_dependencies: [("pkg.b".to_string(), "file:../b".to_string())].into(),
            ..Default::default()
        };

        let n = process_file_dependencies(&ws, &root.path().join("packages/a"), &mut m).unwrap();
        assert_eq!(n, 2);
        assert_eq!(m.dependencies.get("pkg.b").unwrap(), "2.3.4");
        assert_eq!(m.dependencies.get("pkg.remote").unwrap(), "^3.0.0");
        assert_eq!(m.dev_dependencies.get("pkg.b").unwrap(), "2.3.4");
    }

    #[test]
    fn unresolvable_dependency_is_error_devdep_kept() {
        let root = TempDir::new().unwrap();
        write(&root.path(), WORKSPACE_CONFIG, "packages:\n  - packages/*\n");
        write_pkg(&root.path(), "packages/a", "pkg.a", "1.0.0");

        let ws = Workspace::find(&root.path().join("packages/a")).unwrap().unwrap();
        let mut m = Manifest {
            name: "pkg.a".into(),
            version: "1.0.0".into(),
            dependencies: [("missing".to_string(), "file:../nope".to_string())].into(),
            dev_dependencies: [("missing-dev".to_string(), "file:../nope".to_string())].into(),
            ..Default::default()
        };

        let err = process_file_dependencies(&ws, &root.path().join("packages/a"), &mut m).unwrap_err();
        assert_eq!(err.code, "FileDependencyUnresolvable");

        // devDependencies is lenient.
        let mut m2 = Manifest {
            name: "pkg.a".into(),
            version: "1.0.0".into(),
            dev_dependencies: [("missing-dev".to_string(), "file:../nope".to_string())].into(),
            ..Default::default()
        };
        process_file_dependencies(&ws, &root.path().join("packages/a"), &mut m2).unwrap();
        assert_eq!(m2.dev_dependencies.get("missing-dev").unwrap(), "file:../nope");
    }
}
