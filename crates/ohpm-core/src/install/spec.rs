//! Dependency spec parsing, mirroring `lib/tools/ohpa/Ohpa.js`,
//! `lib/tools/ohpa/ValidatePkg.js` and `lib/core/dependency/util/parseDependency.js`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::constants::{LATEST, TAG_PREFIX};
use crate::error::{OhpmError, Result};
use crate::workspace::resolve_file_spec;

/// The ohpa dependency types (see `OhpaType.js`).
///
/// `Git`, `Alias` and `Workspace` are ohpm-rs extensions beyond the reference
/// (which rejects git specs and has no alias/workspace protocols), mirroring
/// pnpm/pacquet semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OhpaType {
    /// An exact valid version, e.g. `1.2.3`.
    Version,
    /// A semver range, e.g. `^1.2.3`.
    #[default]
    Range,
    /// A dist-tag, e.g. `tag:beta`.
    Tag,
    /// A local artifact (.tgz / .tar.gz / .tar / .har).
    File,
    /// A local source-code directory.
    SourceCode,
    /// A git repository (`git+https://...`, scp-style, `https://...git`),
    /// pinned by commit.
    Git,
    /// A package alias (`ohpm:<real-package>@<spec>`); the dependency key is
    /// the local name, the target is the real package.
    Alias,
    /// The workspace protocol (`workspace:*|^|~|range`), resolved against
    /// `ohpm-workspace.yaml` members.
    Workspace,
}

/// A parsed dependency spec (mirrors `OhpaResult.js`).
#[derive(Debug, Clone)]
pub struct Spec {
    /// The original CLI/manifest string.
    pub raw: String,
    /// The spec portion as typed ("" when absent).
    pub raw_spec: String,
    /// The package name ("" for unnamed local inputs).
    pub name: String,
    /// The name with the first "/" escaped ("@ohos%2ffoo").
    pub escaped_name: String,
    /// The scope ("@ohos") or "" for unscoped names.
    pub scope: String,
    /// The directory relative paths resolve against.
    pub where_dir: PathBuf,
    pub ohpa_type: OhpaType,
    /// The specifier as written to the manifest ("file:..." for local deps).
    pub save_spec: String,
    /// The specifier used for resolution ("latest" | raw spec | absolute path).
    pub fetch_spec: String,
    /// `OhpaType::Alias` only: the real package name behind `ohpm:` (e.g.
    /// "bar" for `foo@ohpm:bar@^1.0.0`). Resolution, store dirs and lockfile
    /// package keys use the target; the specifier key and symlink use `name`.
    pub alias_target: Option<String>,
}

impl Spec {
    pub fn is_local(&self) -> bool {
        matches!(self.ohpa_type, OhpaType::File | OhpaType::SourceCode)
    }
}

/// `Ohpa.isURL` — `git+<protocol>:` URLs (accepted as git specs in ohpm-rs).
pub fn is_git_url(raw: &str) -> bool {
    raw.to_ascii_lowercase().starts_with("git+") && raw[4..].contains(':')
}

/// `Ohpa.isGit` — scp-like `user@host:path` strings (accepted as git specs).
pub fn is_git_scp(raw: &str) -> bool {
    // /^[^@]+@[^:.]+\.[^:]+:.+$/
    let Some(rest) = raw.split_once('@') else {
        return false;
    };
    if rest.0.is_empty() || rest.0.contains('@') {
        return false;
    }
    let Some((host, path)) = rest.1.split_once(':') else {
        return false;
    };
    host.contains('.') && !host.contains(':') && !path.is_empty()
}

/// A git spec: `git+<protocol>:` URLs, scp-style `user@host:path`, or an
/// http(s) URL whose path ends in `.git` (pnpm's plain-URL rule).
pub fn is_git_spec(raw: &str) -> bool {
    is_git_url(raw)
        || is_git_scp(raw)
        || url::Url::parse(raw)
            .map(|u| u.path().ends_with(".git") || u.path().ends_with(".git/"))
            .unwrap_or(false)
}

/// A protocol spec that must never be treated as a local path or a registry
/// range: `workspace:`, `ohpm:`, git URLs and pinned git commits.
pub fn is_protocol_spec(spec: &str) -> bool {
    spec.starts_with(crate::constants::WORKSPACE_PREFIX)
        || spec.starts_with(crate::constants::ALIAS_PREFIX)
        || is_git_spec(spec)
        || is_git_pinned(spec)
}

/// A git *pinned* store key: a 40-hex commit, optionally with a `&path:` dir.
pub fn is_git_pinned(spec: &str) -> bool {
    let core = spec.split("&path:").next().unwrap_or(spec);
    core.len() == 40 && core.chars().all(|c| c.is_ascii_hexdigit())
}

/// `Ohpa.isLocalDependency` — the parse-time variant used to decide whether a
/// spec is a local (file/source-code) dependency.
pub fn is_local_spec(spec: &str) -> bool {
    REGEX_LOCAL().is_match(spec)
        || spec.contains('/')
        || REGEX_IS_FILE().is_match(spec)
        || REGEX_IS_DIR().is_match(spec)
}

/// `Ohpa.isLocalFile` — ends with `.tgz`/`.tar.gz`/`.tar`/`.har`.
pub fn is_local_file(spec: &str) -> bool {
    REGEX_IS_FILE().is_match(spec)
}

/// `Ohpa.isInnerPkgDependency` — not `latest`, not a valid range, starts with a
/// letter. (V1: no inner/native package resolution; kept for spec typing.)
pub fn is_inner_pkg_dependency(spec: &str) -> bool {
    spec != LATEST
        && !is_valid_range(spec)
        && spec.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
}

/// `util/isLocalDependency.js` — the store-dir-naming variant (differs from the
/// parse-time one: it also accepts `file:`-protocol strings and rejects plain
/// URLs like `https://...`).
pub fn is_local_dependency(spec: &str) -> bool {
    // Protocol specs (`workspace:^1.0.0`, `git+...`, pinned git commits) are
    // never local paths — without this they would be misclassified by the
    // path/URL checks below and corrupt store-dir names and lockfile keys.
    if is_protocol_spec(spec) {
        return false;
    }
    spec != LATEST
        && !is_valid_range(spec)
        && (!is_valid_url(spec)
            || (!spec.is_empty() && (REGEX_LOCAL_PATH_START().is_match(spec) || spec.starts_with("file:"))))
}

/// `util/isValidUrl.js` — `new URL(s)` succeeds.
pub fn is_valid_url(spec: &str) -> bool {
    url::Url::parse(spec).is_ok()
}

/// `util/isTagDependency.js` — `^tag:` (case-insensitive).
pub fn is_tag_dependency(spec: &str) -> bool {
    REGEX_TAG_PREFIX().is_match(spec)
}

/// `util/isStandardTagDependency.js` — `tag:<standard-tag>`.
pub fn is_standard_tag_dependency(spec: &str) -> bool {
    if !is_tag_dependency(spec) {
        return false;
    }
    let tag = &spec[TAG_PREFIX.len()..];
    is_standard_tag(tag)
}

/// `dist-tags/isStandardTag.js` — regex + `!= latest`.
pub fn is_standard_tag(tag: &str) -> bool {
    REGEX_TAG().is_match(tag) && tag != LATEST
}

/// `util/getVersionJudgedWithTagAndLocal.js` — the version key used by the
/// overrideDependencyMap and exclusions lookups: `tag:xxx`/`latest` stay as
/// the fetch spec, local deps resolve to their absolute path (slash-normalized
/// when `slash_local`), everything else is the pinned spec.
pub fn version_judged_with_tag_and_local(
    fetch_spec: &str,
    pinned_spec: &str,
    slash_local: bool,
) -> String {
    let mut key = pinned_spec;
    if is_standard_tag_dependency(fetch_spec) || fetch_spec == LATEST {
        key = fetch_spec;
    }
    if is_local_dependency(key) {
        let p = std::path::Path::new(key.trim_start_matches("file:"));
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(p)
        };
        let mut s = abs.to_string_lossy().into_owned();
        if slash_local {
            s = s.replace('\\', "/");
        }
        return s;
    }
    key.to_string()
}

/// `semver.validRange(spec, {loose: true, includePrerelease: true})` — the
/// crate's `Range::parse` is strict, but accepts partials and a `v` prefix, so
/// it covers the practical loose cases.
pub fn is_valid_range(spec: &str) -> bool {
    node_semver::Range::parse(spec).is_ok()
}

/// `util/parseDependency.js` — parse a raw CLI/manifest string into a `Spec`.
/// Errors are wrapped as `Common.ParseDependencyFailed`.
pub fn parse_dependency(raw: &str, where_dir: &Path) -> Result<Spec> {
    parse_inner(raw, where_dir).map_err(|_| OhpmError::parse_dependency_failed(raw))
}

/// `Ohpa.parse` — the unwrapped parse (mirrors the exact error taxonomy).
///
/// Protocol specs (git / `ohpm:` alias / `workspace:`) are detected before
/// the name@spec split: an unnamed git spec (`git+https://host/repo`) is the
/// whole raw string, while named forms (`foo@git+ssh://user@host/repo`,
/// `foo@ohpm:bar@^1.0.0`) split on the first `@` as usual.
pub fn parse_inner(raw: &str, where_dir: &Path) -> Result<Spec> {
    let mut name = String::new();
    let mut raw_spec = String::new();
    if is_git_spec(raw) {
        // Unnamed git dependency: the whole string is the spec.
        raw_spec = raw.to_string();
        return resolve(name, raw_spec, where_dir, raw);
    }
    // The name@spec separator: the second "@" for scoped names (a scoped name
    // itself starts with "@"), the first otherwise. The separator index in
    // `raw` is `i`; the name is `raw[..i]` and the spec is `raw[i + 1..]`
    // (the reference computes `i` as `slice(1).indexOf("@") + 1` for scoped
    // names and `indexOf("@")` otherwise, then slices `(0, i)` / `(i + 1)`).
    let sep = if raw.starts_with('@') {
        raw[1..].find('@').map(|i| i + 1)
    } else {
        raw.find('@')
    };
    let sep = sep.unwrap_or(0);
    let name_part = if sep > 0 { &raw[..sep] } else { raw };
    if !name_part.starts_with('@') && sep == 0 && is_local_spec(name_part) {
        // Unnamed local dependency: the whole string is the spec.
        raw_spec = raw.to_string();
    } else {
        validate_name(name_part)?;
        name = name_part.to_string();
        if sep > 0 {
            raw_spec = raw[sep + 1..].to_string();
        }
    }
    resolve(name, raw_spec, where_dir, raw)
}

fn resolve(name: String, raw_spec: String, where_dir: &Path, raw: &str) -> Result<Spec> {
    let mut spec = Spec {
        raw: raw.to_string(),
        raw_spec,
        name,
        escaped_name: String::new(),
        scope: String::new(),
        where_dir: where_dir.to_path_buf(),
        ohpa_type: OhpaType::Range,
        save_spec: String::new(),
        fetch_spec: String::new(),
        alias_target: None,
    };
    if !spec.name.is_empty() {
        spec.escaped_name = spec.name.replacen('/', "%2f", 1);
        spec.scope = if spec.name.starts_with('@') {
            spec.name
                .find('/')
                .map(|i| spec.name[..i].to_string())
                .unwrap_or_default()
        } else {
            String::new()
        };
    }
    if spec.raw_spec.is_empty() {
        return from_registry(&mut spec).map(|_| spec);
    }
    // Protocol specs are detected before the local/registry split (in
    // particular `ohpm:...` would otherwise be grabbed by the inner-pkg
    // branch of `from_file`).
    if spec.raw_spec.starts_with(crate::constants::WORKSPACE_PREFIX) {
        from_workspace(&mut spec);
    } else if spec.raw_spec.starts_with(crate::constants::ALIAS_PREFIX) {
        from_alias(&mut spec)?;
    } else if is_git_spec(&spec.raw_spec) {
        from_git(&mut spec);
    } else if !is_tag_dependency(&spec.raw_spec)
        && (is_local_spec(&spec.raw_spec) || is_inner_pkg_dependency(&spec.raw_spec))
    {
        from_file(&mut spec);
    } else {
        from_registry(&mut spec)?;
    }
    Ok(spec)
}

/// `ohpm:` alias — `ohpm:<real-package>[@<spec>]`. The dependency key (`name`)
/// is the local alias; `alias_target` carries the real package name.
fn from_alias(spec: &mut Spec) -> Result<()> {
    let inner = &spec.raw_spec[crate::constants::ALIAS_PREFIX.len()..];
    let sep = if inner.starts_with('@') {
        inner[1..].find('@').map(|i| i + 1)
    } else {
        inner.find('@')
    }
    .unwrap_or(0);
    let target = if sep > 0 { &inner[..sep] } else { inner };
    let target_spec = if sep > 0 { &inner[sep + 1..] } else { "" };
    if target.is_empty() || !is_registry_spec(target_spec) {
        return Err(OhpmError::alias_pkg_invalid(&spec.raw));
    }
    validate_name(target)?;
    spec.ohpa_type = OhpaType::Alias;
    spec.alias_target = Some(target.to_string());
    spec.save_spec = spec.raw_spec.clone();
    spec.fetch_spec = spec.raw_spec.clone();
    Ok(())
}

/// `workspace:` protocol — `workspace:*|^|~|<range>|./path|<member>@*`.
fn from_workspace(spec: &mut Spec) {
    spec.ohpa_type = OhpaType::Workspace;
    spec.save_spec = spec.raw_spec.clone();
    spec.fetch_spec = spec.raw_spec.clone();
}

/// A git spec — `git+<protocol>:...`, scp-style, or `https://...git`.
fn from_git(spec: &mut Spec) {
    spec.ohpa_type = OhpaType::Git;
    spec.save_spec = spec.raw_spec.clone();
    spec.fetch_spec = spec.raw_spec.clone();
}

/// Whether a string is a registry spec (version/range/tag) as required by the
/// `ohpm:` alias inner spec.
fn is_registry_spec(spec: &str) -> bool {
    if spec.is_empty() {
        return false;
    }
    node_semver::Version::parse(spec).is_ok()
        || node_semver::Range::parse(spec).is_ok()
        || is_tag_dependency(spec)
}

/// `Ohpa.fromFile` — local artifact / source-code specs. The fetch spec is the
/// resolved absolute path; the save spec is `file:<rel>` (or `file:~...`).
fn from_file(spec: &mut Spec) {
    spec.ohpa_type = if is_local_file(&spec.raw_spec) {
        OhpaType::File
    } else {
        OhpaType::SourceCode
    };
    let stripped = REGEX_LOCAL().replace(&spec.raw_spec, "").trim().to_string();
    // `p.resolve(where, spec)` — absolute stays, relative joins and normalizes.
    let resolved = resolve_file_spec(&spec.where_dir, &format!("file:{stripped}"));
    spec.save_spec = save_spec(&spec.where_dir, &stripped, &resolved);
    spec.fetch_spec = resolved.to_string_lossy().into_owned();
}

/// `Ohpa.resolveSaveSpec` — `file:~...` for `~`-forms, otherwise
/// `file:<rel>` with `rel` relative to the where dir.
fn save_spec(where_dir: &Path, stripped: &str, resolved: &Path) -> String {
    if REGEX_PATH_LINUX().is_match(stripped) {
        let idx = stripped.find('~').unwrap_or(0);
        format!("file:{}", &stripped[idx..])
    } else if Path::new(stripped).is_absolute() {
        format!("file:{}", pathdiff_relative(where_dir, resolved))
    } else {
        format!("file:{}", pathdiff_relative(where_dir, resolved))
    }
}

/// `Ohpa.fromRegistry` — version/range/tag typing.
fn from_registry(spec: &mut Spec) -> Result<()> {
    let fetch_spec = if spec.raw_spec.is_empty() {
        LATEST.to_string()
    } else {
        spec.raw_spec.trim().to_string()
    };
    spec.save_spec = fetch_spec.clone();
    spec.fetch_spec = fetch_spec.clone();
    if node_semver::Version::parse(&fetch_spec).is_ok() {
        spec.ohpa_type = OhpaType::Version;
        return Ok(());
    }
    if node_semver::Range::parse(&fetch_spec).is_ok() {
        spec.ohpa_type = OhpaType::Range;
        return Ok(());
    }
    let tag = if is_tag_dependency(&fetch_spec) {
        let t = &fetch_spec[TAG_PREFIX.len()..];
        if !is_standard_tag(t) {
            return Err(OhpmError::tag_pkg_invalid(t, &spec.raw));
        }
        t.to_string()
    } else {
        fetch_spec.clone()
    };
    if encode_uri_component(&tag) != tag {
        return Err(OhpmError::ohpa_spec_uri(&fetch_spec, &spec.raw));
    }
    spec.ohpa_type = OhpaType::Tag;
    Ok(())
}

/// `lib/tools/ohpa/ValidatePkg.js` — `validatePkg.validate`.
pub fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(OhpmError::ohpa_name_empty());
    }
    if name.chars().count() > MAX_LEN_NAME {
        return Err(OhpmError::ohpa_name_too_long());
    }
    if name.trim() != name {
        return Err(OhpmError::ohpa_name_blank());
    }
    if encode_uri_component(name) != name {
        // Mirrors the reference: the check runs only when the name matches the
        // `@scope/name` shape (at most one "/", and any "/" must be preceded by
        // an "@scope"). For the unscoped shape `encodeURIComponent(undefined)`
        // differs from `undefined`, so any escapable character throws.
        match split_scoped(name) {
            Some((scope, pkg)) => {
                if encode_uri_component(scope) != scope || encode_uri_component(pkg) != pkg {
                    return Err(OhpmError::ohpa_name_uri());
                }
            }
            None => {
                if !name.contains('/') {
                    return Err(OhpmError::ohpa_name_uri());
                }
            }
        }
    }
    if name.starts_with('.') || name.starts_with('_') {
        return Err(OhpmError::ohpa_name_invalid_start());
    }
    if let Some(last) = name.rsplit('/').next() {
        if REGEX_INVALID_CHARACTERS().is_match(last) {
            return Err(OhpmError::ohpa_name_special());
        }
    }
    if matches!(name.to_ascii_lowercase().as_str(), "oh_modules" | "node_modules") {
        return Err(OhpmError::ohpa_name_blacklist(name));
    }
    Ok(())
}

/// `encodeURIComponent` — percent-encode every UTF-8 byte outside
/// `A-Za-z0-9 - _ . ! ~ * ' ( )`.
pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')');
        if keep {
            out.push(*b as char);
        } else {
            use std::fmt::Write;
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// Split an `@scope/name` into (scope-without-@, name); `None` when not the
/// two-part scoped shape.
fn split_scoped(name: &str) -> Option<(&str, &str)> {
    let (scope, rest) = name.split_once('/')?;
    if !scope.starts_with('@') || rest.contains('/') {
        return None;
    }
    Some((&scope[1..], rest))
}

/// `path.relative(from, to)` with both sides already absolute/normalized:
/// the `..`-relative path from `from` to `to`, forward slashes, "." when equal.
fn pathdiff_relative(from: &Path, to: &Path) -> String {
    let from_parts: Vec<_> = from.components().collect();
    let to_parts: Vec<_> = to.components().collect();
    let mut common = 0;
    while common < from_parts.len()
        && common < to_parts.len()
        && from_parts[common] == to_parts[common]
    {
        common += 1;
    }
    let mut parts: Vec<String> = (0..from_parts.len() - common)
        .map(|_| "..".to_string())
        .collect();
    for c in &to_parts[common..] {
        parts.push(c.as_os_str().to_string_lossy().into_owned());
    }
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

// ---- regexes (mirror `Ohpa.js` / `Regex.js` / `ValidatePkg.js`) -------------

const MAX_LEN_NAME: usize = 214;

macro_rules! static_regex {
    ($name:ident, $pat:expr) => {
        #[allow(non_snake_case)]
        fn $name() -> &'static regex::Regex {
            static RE: OnceLock<regex::Regex> = OnceLock::new();
            RE.get_or_init(|| regex::Regex::new($pat).expect("static regex"))
        }
    };
}

static_regex!(REGEX_LOCAL, r"^file:(\s+)?");
static_regex!(REGEX_IS_FILE, r"[.](?:tgz|tar.gz|tar|har)$");
static_regex!(REGEX_IS_DIR, r"^(?:[.]|~/|/|[a-zA-Z]:)");
static_regex!(REGEX_PATH_LINUX, r"^/?~(/|$)");
static_regex!(REGEX_TAG_PREFIX, r"^tag:");
static_regex!(REGEX_TAG, r"^[A-Za-z0-9][A-Za-z0-9._-]{0,59}$");
static_regex!(REGEX_LOCAL_PATH_START, r"^(?:[.]|~/|/|[a-zA-Z]:)");
static_regex!(REGEX_INVALID_CHARACTERS, r"[~'!()*]");

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str, where_dir: &str) -> Spec {
        parse_inner(raw, Path::new(where_dir)).expect("parse")
    }

    #[test]
    fn registry_specs() {
        let s = parse("@ohos/foo", "/proj");
        assert_eq!(s.name, "@ohos/foo");
        assert_eq!(s.scope, "@ohos");
        assert_eq!(s.escaped_name, "@ohos%2ffoo");
        assert_eq!(s.raw_spec, "");
        assert_eq!(s.fetch_spec, "latest");
        assert_eq!(s.save_spec, "latest");
        // Empty spec -> "latest": not a version/range -> Tag typing (reference).
        assert_eq!(s.ohpa_type, OhpaType::Tag);

        let s = parse("@ohos/foo@1.2.3", "/proj");
        assert_eq!(s.name, "@ohos/foo");
        assert_eq!(s.raw_spec, "1.2.3");
        assert_eq!(s.fetch_spec, "1.2.3");
        assert_eq!(s.ohpa_type, OhpaType::Version);

        let s = parse("foo@^1.0.0", "/proj");
        assert_eq!(s.name, "foo");
        assert_eq!(s.ohpa_type, OhpaType::Range);
        assert_eq!(s.fetch_spec, "^1.0.0");

        let s = parse("foo@tag:beta", "/proj");
        assert_eq!(s.name, "foo");
        assert_eq!(s.ohpa_type, OhpaType::Tag);
        assert_eq!(s.fetch_spec, "tag:beta");

        // Bare name: fetchSpec "latest".
        let s = parse("foo", "/proj");
        assert_eq!(s.name, "foo");
        assert_eq!(s.fetch_spec, "latest");
        assert!(!s.is_local());
    }

    #[test]
    fn local_specs() {
        let s = parse("../lib", "/proj/entry");
        assert_eq!(s.name, "");
        assert_eq!(s.ohpa_type, OhpaType::SourceCode);
        assert_eq!(s.fetch_spec, "/proj/lib");
        assert_eq!(s.save_spec, "file:../lib");
        assert!(s.is_local());

        let s = parse("./lib", "/proj/entry");
        assert_eq!(s.fetch_spec, "/proj/entry/lib");
        assert_eq!(s.save_spec, "file:lib"); // relative from where dir
        assert_eq!(s.ohpa_type, OhpaType::SourceCode);

        let s = parse("file:../lib", "/proj/entry");
        assert_eq!(s.ohpa_type, OhpaType::SourceCode);
        assert_eq!(s.fetch_spec, "/proj/lib");
        assert_eq!(s.save_spec, "file:../lib");

        let s = parse("~/dev/lib", "/proj/entry");
        assert_eq!(s.ohpa_type, OhpaType::SourceCode);
        assert!(s.save_spec.starts_with("file:~"));
        let home = dirs::home_dir().unwrap();
        assert!(s.fetch_spec.starts_with(home.to_str().unwrap()));

        let s = parse("pkg.har", "/proj/entry");
        assert_eq!(s.ohpa_type, OhpaType::File);
        assert_eq!(s.fetch_spec, "/proj/entry/pkg.har");

        let s = parse("pkg.tgz", "/proj/entry");
        assert_eq!(s.ohpa_type, OhpaType::File);
        let s = parse("pkg.tar.gz", "/proj/entry");
        assert_eq!(s.ohpa_type, OhpaType::File);

        // Absolute path input.
        let s = parse("/abs/pkg.har", "/proj/entry");
        assert_eq!(s.ohpa_type, OhpaType::File);
        assert_eq!(s.fetch_spec, "/abs/pkg.har");
        assert_eq!(s.save_spec, "file:../../abs/pkg.har");
    }

    #[test]
    fn invalid_inputs() {
        // "foo@" parses as foo@latest in the reference (empty spec).
        let s = parse_inner("foo@", Path::new("/proj")).unwrap();
        assert_eq!(s.name, "foo");
        assert_eq!(s.fetch_spec, "latest");

        let err = parse_inner("foo@tag:latest", Path::new("/proj")).unwrap_err();
        assert_eq!(err.code, "TagPkgInvalid");

        let err = parse_inner("foo@tag:not a tag", Path::new("/proj")).unwrap_err();
        assert_eq!(err.code, "TagPkgInvalid");

        let err = parse_inner("oh_modules@1.0.0", Path::new("/proj")).unwrap_err();
        assert_eq!(err.code, "OhpaNameInBlacklist");

        let err = parse_inner(".foo@1.0.0", Path::new("/proj")).unwrap_err();
        assert_eq!(err.code, "OhpaNameInvalidStart");

        let err = parse_inner("foo bar@1.0.0", Path::new("/proj")).unwrap_err();
        assert_eq!(err.code, "OhpaNameUri"); // escapable char, unscoped shape

        let err = parse_inner(" foo@1.0.0", Path::new("/proj")).unwrap_err();
        assert_eq!(err.code, "OhpaNameWithBlank"); // leading space

        // "sp ace" starts with a letter and is not a valid range -> inner-pkg
        // dependency -> SourceCode (mirrors the reference).
        let s = parse_inner("foo@sp ace", Path::new("/proj")).unwrap();
        assert_eq!(s.ohpa_type, OhpaType::SourceCode);

        // A digit-leading spec that is neither local nor a range reaches the
        // URI check (the crate rejects `&`, where the reference rejects
        // trailing junk after whitespace — a documented leniency).
        let err = parse_inner("foo@1.2.3&x", Path::new("/proj")).unwrap_err();
        assert_eq!(err.code, "OhpaSpecUri");

    }

    #[test]
    fn helper_predicates() {
        assert!(is_local_spec("../lib"));
        assert!(is_local_spec("file:a"));
        assert!(is_local_spec("pkg.har"));
        assert!(is_local_spec("/abs"));
        assert!(!is_local_spec("foo"));
        assert!(!is_local_spec("^1.0.0"));

        assert!(is_local_file("a.tgz"));
        assert!(is_local_file("a.tar.gz"));
        assert!(is_local_file("a.har"));
        assert!(!is_local_file("a.hsp"));

        assert!(is_tag_dependency("tag:beta"));
        assert!(!is_tag_dependency("tagbeta"));
        assert!(is_standard_tag_dependency("tag:beta"));
        assert!(!is_standard_tag_dependency("tag:latest"));

        assert!(is_standard_tag("beta"));
        assert!(is_standard_tag("b1.2_x-y"));
        assert!(!is_standard_tag("latest"));
        assert!(!is_standard_tag(""));

        assert!(is_valid_url("https://x.y"));
        assert!(is_valid_url("file:../lib"));
        assert!(!is_valid_url("../lib"));

        // store-dir-naming variant
        assert!(is_local_dependency("file:../lib"));
        assert!(is_local_dependency("../lib"));
        assert!(!is_local_dependency("https://x.y/foo"));
        assert!(!is_local_dependency("^1.0.0"));
        assert!(!is_local_dependency("latest"));
    }

    #[test]
    fn encode_uri() {
        assert_eq!(encode_uri_component("foo-bar_1.2"), "foo-bar_1.2");
        assert_eq!(encode_uri_component("a b"), "a%20b");
        assert_eq!(encode_uri_component("@ohos"), "%40ohos");
        assert_eq!(encode_uri_component("a/b"), "a%2Fb");
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::*;

    fn parse(raw: &str) -> Spec {
        parse_inner(raw, Path::new("/proj")).expect("parse")
    }

    #[test]
    fn git_specs() {
        // Unnamed git specs: the whole string is the spec.
        let s = parse("git+https://host/repo.git");
        assert_eq!(s.name, "");
        assert_eq!(s.ohpa_type, OhpaType::Git);
        assert_eq!(s.fetch_spec, "git+https://host/repo.git");

        let s = parse("git+ssh://user@host/repo.git#main");
        assert_eq!(s.ohpa_type, OhpaType::Git);
        assert_eq!(s.fetch_spec, "git+ssh://user@host/repo.git#main");

        let s = parse("git+file:///abs/repo.git#v1.0.0");
        assert_eq!(s.ohpa_type, OhpaType::Git);

        // Named forms split on the first @.
        let s = parse("foo@git+ssh://user@host/repo.git");
        assert_eq!(s.name, "foo");
        assert_eq!(s.ohpa_type, OhpaType::Git);
        assert_eq!(s.fetch_spec, "git+ssh://user@host/repo.git");

        let s = parse("@ohos/foo@git+https://host/repo.git#semver:^1.0.0");
        assert_eq!(s.name, "@ohos/foo");
        assert_eq!(s.scope, "@ohos");
        assert_eq!(s.ohpa_type, OhpaType::Git);

        // Plain https URL ending in .git.
        let s = parse("https://github.com/user/repo.git");
        assert_eq!(s.ohpa_type, OhpaType::Git);

        // scp-style.
        let s = parse("git@github.com:user/repo.git");
        assert_eq!(s.ohpa_type, OhpaType::Git);

        // Plain URL without .git is not a git spec (falls into the existing
        // local-path logic, like the reference).
        let s = parse("foo@https://host/repo");
        assert_eq!(s.ohpa_type, OhpaType::SourceCode);
    }

    #[test]
    fn alias_specs() {
        let s = parse("foo@ohpm:bar@^1.0.0");
        assert_eq!(s.name, "foo");
        assert_eq!(s.ohpa_type, OhpaType::Alias);
        assert_eq!(s.alias_target.as_deref(), Some("bar"));
        assert_eq!(s.fetch_spec, "ohpm:bar@^1.0.0");
        assert_eq!(s.save_spec, "ohpm:bar@^1.0.0");

        let s = parse("foo@ohpm:@ohos/bar@1.2.3");
        assert_eq!(s.alias_target.as_deref(), Some("@ohos/bar"));
        assert_eq!(s.ohpa_type, OhpaType::Alias);

        let s = parse("foo@ohpm:bar@tag:beta");
        assert_eq!(s.ohpa_type, OhpaType::Alias);

        // Bare alias without a spec is invalid.
        let err = parse_inner("foo@ohpm:bar", Path::new("/proj")).unwrap_err();
        assert_eq!(err.code, "AliasPkgInvalid");

        // Nested protocols are rejected.
        let err = parse_inner("foo@ohpm:git+https://x/y", Path::new("/proj")).unwrap_err();
        assert_eq!(err.code, "AliasPkgInvalid");
        let err = parse_inner("foo@ohpm:workspace:*", Path::new("/proj")).unwrap_err();
        assert_eq!(err.code, "AliasPkgInvalid");
    }

    #[test]
    fn workspace_specs() {
        for spec in [
            "workspace:*",
            "workspace:",
            "workspace:^",
            "workspace:~",
            "workspace:^1.5.0",
            "workspace:1.5.0",
            "workspace:./foo",
            "workspace:../foo",
            "workspace:foo@*",
        ] {
            let s = parse(&format!("bar@{spec}"));
            assert_eq!(s.name, "bar", "{spec}");
            assert_eq!(s.ohpa_type, OhpaType::Workspace, "{spec}");
            assert_eq!(s.fetch_spec, spec);
            assert_eq!(s.save_spec, spec);
        }
    }

    #[test]
    fn predicates() {
        assert!(is_git_spec("git+https://x/y"));
        assert!(is_git_spec("git@github.com:user/repo.git"));
        assert!(is_git_spec("https://host/repo.git"));
        assert!(!is_git_spec("https://host/repo"));
        assert!(!is_git_spec("^1.0.0"));

        assert!(is_protocol_spec("workspace:^1.0.0"));
        assert!(is_protocol_spec("ohpm:bar@^1.0.0"));
        assert!(is_protocol_spec("git+https://x/y"));
        assert!(is_protocol_spec("0123456789abcdef0123456789abcdef01234567"));
        assert!(!is_protocol_spec("^1.0.0"));
        assert!(!is_protocol_spec("../lib"));

        assert!(is_git_pinned("0123456789abcdef0123456789abcdef01234567"));
        assert!(is_git_pinned("0123456789abcdef0123456789abcdef01234567&path:sub"));
        assert!(!is_git_pinned("1.2.3"));

        // Protocol specs are never "local" for store-dir naming.
        assert!(!is_local_dependency("workspace:^1.0.0"));
        assert!(!is_local_dependency("git+https://x/y"));
        assert!(!is_local_dependency("0123456789abcdef0123456789abcdef01234567"));
        assert!(!is_local_dependency("ohpm:bar@^1.0.0"));
    }
}
