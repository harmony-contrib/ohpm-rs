//! Dependency spec parsing, mirroring `lib/tools/ohpa/Ohpa.js`,
//! `lib/tools/ohpa/ValidatePkg.js` and `lib/core/dependency/util/parseDependency.js`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::constants::{LATEST, TAG_PREFIX};
use crate::error::{OhpmError, Result};
use crate::workspace::resolve_file_spec;

/// The ohpa dependency types (see `OhpaType.js`).
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
}

impl Spec {
    pub fn is_local(&self) -> bool {
        matches!(self.ohpa_type, OhpaType::File | OhpaType::SourceCode)
    }
}

/// `Ohpa.isURL` — `git+<protocol>:` URLs are rejected.
fn is_git_url(raw: &str) -> bool {
    raw.to_ascii_lowercase().starts_with("git+") && raw[4..].contains(':')
}

/// `Ohpa.isGit` — scp-like `user@host:path` strings are rejected.
fn is_git_scp(raw: &str) -> bool {
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
pub fn parse_inner(raw: &str, where_dir: &Path) -> Result<Spec> {
    if is_git_url(raw) || is_git_scp(raw) {
        return Err(OhpmError::ohpa_pkg_invalid());
    }
    let mut name = String::new();
    let mut raw_spec = String::new();
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
    if !spec.raw_spec.is_empty()
        && !is_tag_dependency(&spec.raw_spec)
        && (is_local_spec(&spec.raw_spec) || is_inner_pkg_dependency(&spec.raw_spec))
    {
        from_file(&mut spec);
    } else {
        from_registry(&mut spec)?;
    }
    Ok(spec)
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
        assert!(parse_inner("git+https://x/y", Path::new("/proj")).is_err());
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

        // Wrapped by parse_dependency.
        let err = parse_dependency("git+https://x/y", Path::new("/proj")).unwrap_err();
        assert_eq!(err.code, "ParseDependencyFailed");
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
