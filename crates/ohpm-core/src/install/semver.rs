//! Version-range matching, mirroring `lib/core/dependency/util/semverMaxSatisfying.js`,
//! `lib/util/semverRangesConversion.js` and
//! `lib/core/dependency/util/getVersionByDistTags.js`.
//!
//! The underlying matcher is the `node-semver` crate (the same choice as
//! pnpm's Rust engine). The ohpm-specific deviations live here:
//!
//! * `...` hyphen shorthand → ` - `;
//! * `^0.x.y` → `>=0.x.y <1.0.0` (ohpm's `^0` semantics differ from semver's);
//! * two-step `maxSatisfying`: strict first (a prerelease result is discarded),
//!   then an `includePrerelease` pass.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use node_semver::{Range, Version};

use crate::constants::TAG_PREFIX;
use crate::install::spec::is_tag_dependency;

/// `semverRangesConversion.js` — transform the range strings where ohpm
/// deviates from plain semver.
///
/// A range that contains `^0` but does not match the whole-string `^0...` form
/// is dropped from the output, mirroring the reference (which then behaves like
/// an empty range, i.e. matches anything).
pub fn convert_versions(ranges: &[String]) -> Vec<String> {
    let converted: Vec<String> = ranges
        .iter()
        .map(|r| r.replace("...", " - "))
        .collect();
    if !converted.iter().any(|r| r.contains("^0")) {
        return converted;
    }
    let mut out = Vec::new();
    for range in converted {
        if !range.contains("^0") {
            out.push(range);
            continue;
        }
        if let Some(m) = REGEX_0_RANGE().captures(&range) {
            let whole = m.get(0).expect("whole match").as_str();
            if whole.starts_with("^0") {
                let converted = convert_0_range(&whole[1..]);
                out.push(range.replacen(whole, &converted, 1));
            }
        }
        // else: dropped, mirroring the JS behavior
    }
    out
}

/// The inner `convert0Range`: `0.x.y[-pre]` → `>=0.x.y[-pre] <1.0.0`, with
/// trailing `x`/`X`/`*` replaced by `0` and a `semver.coerce` fallback.
fn convert_0_range(range: &str) -> String {
    let (numeric, pre) = match range.split_once('-') {
        Some((n, p)) => (n, format!("-{p}")),
        None => (range, String::new()),
    };
    // `o.replace(/[xX\*]$/g, "0")` — the trailing x/X/* (if any) becomes 0.
    let mut s = numeric.to_string();
    if matches!(s.chars().last(), Some('x' | 'X' | '*')) {
        s.pop();
        s.push('0');
    }
    let u = Version::parse(&s).ok().or_else(|| coerce(&s));
    let lower = match &u {
        Some(v) => format!("{}.{}.{}{}", v.major, v.minor, v.patch, pre),
        None => format!("{s}{pre}"),
    };
    let major = u.map(|v| v.major).unwrap_or(0);
    format!(">={lower} <{}.0.0", major + 1)
}

/// `semver.coerce` approximation: the first `x[.y[.z]]` numeric run.
fn coerce(s: &str) -> Option<Version> {
    let m = REGEX_COERCE().captures(s)?;
    let major: u64 = m.get(1)?.as_str().parse().ok()?;
    let minor: u64 = m.get(2).map(|g| g.as_str().parse().unwrap_or(0)).unwrap_or(0);
    let patch: u64 = m.get(3).map(|g| g.as_str().parse().unwrap_or(0)).unwrap_or(0);
    Some(Version::new(major, minor, patch))
}

/// `semverMaxSatisfying.js` — the two-step max-satisfying selection.
///
/// Step 1: plain `maxSatisfying` (the crate's `Range::satisfies` implements
/// node-semver's prerelease gate); a prerelease result is discarded. Step 2:
/// `includePrerelease` bounds-only matching.
pub fn semver_max_satisfying(versions: &[String], fetch_spec: &str) -> Option<String> {
    // `semver.valid(fetchSpec) && versions.includes(fetchSpec)` — exact match.
    if Version::parse(fetch_spec).is_ok() && versions.iter().any(|v| v == fetch_spec) {
        return Some(fetch_spec.to_string());
    }
    let raw = if fetch_spec == "latest" {
        "*".to_string()
    } else {
        fetch_spec.to_string()
    };
    let converted = convert_versions(&[raw]);
    // An empty conversion (dropped `^0` range) behaves like `*`.
    let range_str = if converted.is_empty() {
        "*".to_string()
    } else {
        converted.join(" ")
    };
    let range = Range::parse(&range_str).ok()?;
    first_match(versions, &range, false).or_else(|| first_match(versions, &range, true))
}

/// `getVersionByDistTags.js` — `tag:<name>` → the tagged version, resolved via
/// the packument's `dist-tags` (see `packument.rs` for the packument-bound
/// variant). Kept here as a `Map`-independent helper.
pub fn get_version_by_dist_tags(
    spec: &str,
    dist_tags: &std::collections::BTreeMap<String, String>,
    versions: &BTreeSet<String>,
) -> Option<String> {
    if !is_tag_dependency(spec) {
        return None;
    }
    let tag = &spec[TAG_PREFIX.len()..];
    let v = dist_tags.get(tag)?;
    versions.contains(v).then(|| v.clone())
}

/// Sorted descending (node-semver ordering).
fn sorted_desc(versions: &[String]) -> Vec<Version> {
    let mut vs: Vec<Version> = versions
        .iter()
        .filter_map(|v| Version::parse(v).ok())
        .collect();
    vs.sort_by(|a, b| b.cmp(a));
    vs
}

/// Step-1 / step-2 first match over candidates in descending order.
fn first_match(versions: &[String], range: &Range, prerelease_ok: bool) -> Option<String> {
    sorted_desc(versions).into_iter().find(|v| {
        if v.is_prerelease() {
            // Bounds-only membership (includePrerelease): the exact version is
            // a point range; `allows_any` compares bounds in node-semver
            // ordering, which is exactly the JS bounds test for prereleases.
            prerelease_ok && bounds_only_matches(range, v)
        } else {
            range.satisfies(v)
        }
    }).map(|v| v.to_string())
}

/// `Range::satisfies` with the prerelease gate disabled — true iff `v` falls
/// within the range's bounds in node-semver ordering.
fn bounds_only_matches(range: &Range, v: &Version) -> bool {
    let exact = Range::parse(v.to_string()).ok();
    match exact {
        Some(exact) => range.allows_any(&exact),
        None => range.satisfies(v),
    }
}

// ---- regexes ---------------------------------------------------------------

macro_rules! static_regex {
    ($name:ident, $pat:expr) => {
        #[allow(non_snake_case)]
        fn $name() -> &'static regex::Regex {
            static RE: OnceLock<regex::Regex> = OnceLock::new();
            RE.get_or_init(|| regex::Regex::new($pat).expect("static regex"))
        }
    };
}

static_regex!(REGEX_0_RANGE, r"^\^0(\.(?:\d+|x|X|\*)){0,2}(?:-[a-zA-Z0-9.-_]+)*$");
static_regex!(REGEX_COERCE, r"(\d+)(?:\.(\d+))?(?:\.(\d+))?");

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(versions: &[&str], spec: &str) -> Option<String> {
        let vs: Vec<String> = versions.iter().map(|s| s.to_string()).collect();
        semver_max_satisfying(&vs, spec)
    }

    #[test]
    fn convert_0_ranges() {
        assert_eq!(convert_versions(&["^0.1.2".into()]), vec![">=0.1.2 <1.0.0"]);
        assert_eq!(convert_versions(&["^0.0.3".into()]), vec![">=0.0.3 <1.0.0"]);
        assert_eq!(
            convert_versions(&["^0.1.2-beta.1".into()]),
            vec![">=0.1.2-beta.1 <1.0.0"]
        );
        assert_eq!(convert_versions(&["^0.1.x".into()]), vec![">=0.1.0 <1.0.0"]);
        assert_eq!(convert_versions(&["^0.x.x".into()]), vec![">=0.0.0 <1.0.0"]);
        assert_eq!(convert_versions(&["^0".into()]), vec![">=0.0.0 <1.0.0"]);
        // Untouched ranges pass through.
        assert_eq!(convert_versions(&["^1.2.3".into()]), vec!["^1.2.3"]);
        assert_eq!(convert_versions(&["~0.1.2".into()]), vec!["~0.1.2"]);
        // "..." → " - ".
        assert_eq!(convert_versions(&["1...2".into()]), vec!["1 - 2"]);
        // A "^0" range that is not the whole string is dropped.
        assert!(convert_versions(&["^0.1.2 || ^1.0.0".into()]).is_empty());
    }

    #[test]
    fn max_satisfying_basics() {
        assert_eq!(pick(&["1.2.0", "1.2.3", "1.2.1"], "^1.2.0"), Some("1.2.3".into()));
        assert_eq!(pick(&["1.2.3"], "1.2.3"), Some("1.2.3".into()));
        assert_eq!(pick(&["1.0.0", "1.2.3"], "1.2.3"), Some("1.2.3".into()));
        assert_eq!(pick(&["1.2.3", "2.0.0"], "latest"), Some("2.0.0".into()));
        assert_eq!(pick(&["1.2.3"], "tag:beta"), None);
        assert_eq!(pick(&["1.2.3"], ">=2.0.0"), None);
        assert_eq!(pick(&["1.0.0", "1.5.0"], "^0.9.0"), None);
        // ^0 conversion applies: `^0.1.0` -> `>=0.1.0 <1.0.0` (reference widens
        // `^0` to <1.0.0, unlike plain semver).
        assert_eq!(pick(&["0.1.0", "0.5.0", "1.0.0"], "^0.1.0"), Some("0.5.0".into()));
        assert_eq!(pick(&["0.9.9", "1.0.0"], "^0.1.0"), Some("0.9.9".into()));
        assert_eq!(pick(&["0.1.0"], "^0.1.0"), Some("0.1.0".into()));
        // Hyphen ranges.
        assert_eq!(pick(&["1.0.0", "2.5.0", "3.0.0"], "1...2"), Some("2.5.0".into()));
        // x-ranges.
        assert_eq!(pick(&["1.2.3", "1.9.9"], "1.x"), Some("1.9.9".into()));
        assert_eq!(pick(&["1.2.3", "2.0.0"], "*"), Some("2.0.0".into()));
    }

    #[test]
    fn max_satisfying_prerelease() {
        // Prereleases are excluded in step 1.
        assert_eq!(
            pick(&["1.0.1-alpha", "1.0.0"], "^1.0.0"),
            Some("1.0.0".into())
        );
        // Prerelease-only registry: step 2 picks it.
        assert_eq!(pick(&["1.0.1-beta.2"], "^1.0.0"), Some("1.0.1-beta.2".into()));
        // Exact prerelease present wins immediately.
        assert_eq!(pick(&["1.0.1-beta.2"], "1.0.1-beta.2"), Some("1.0.1-beta.2".into()));
        // Range with prerelease comparator: the release version wins in step 1.
        assert_eq!(
            pick(&["1.2.3-beta.1", "1.2.3"], "^1.2.3-beta.1"),
            Some("1.2.3".into())
        );
        assert_eq!(pick(&["1.2.3-beta.1"], "^1.2.3-beta.1"), Some("1.2.3-beta.1".into()));
        // includePrerelease bounds: a next-major prerelease does NOT match
        // `^1.0.0` — the reference's upper bound is `<2.0.0-0`, and
        // `2.0.0-beta.1` sorts above `2.0.0-0`.
        assert_eq!(pick(&["2.0.0-beta.1"], "^1.0.0"), None);
        // A prerelease below an inclusive lower bound does not match.
        assert_eq!(pick(&["1.2.5-beta.1"], ">=1.2.5"), None);
        assert_eq!(pick(&["1.2.5-beta.1"], "<=1.2.5"), Some("1.2.5-beta.1".into()));
        // Highest matching prerelease wins in step 2.
        assert_eq!(
            pick(&["1.0.1-beta.1", "1.0.1-alpha.9"], "^1.0.0"),
            Some("1.0.1-beta.1".into())
        );
    }

    #[test]
    fn dist_tags() {
        let dist_tags = std::collections::BTreeMap::from([("beta".to_string(), "1.2.3-beta.1".to_string())]);
        let versions = BTreeSet::from(["1.2.3-beta.1".to_string()]);
        assert_eq!(
            get_version_by_dist_tags("tag:beta", &dist_tags, &versions),
            Some("1.2.3-beta.1".to_string())
        );
        assert!(get_version_by_dist_tags("1.2.3", &dist_tags, &versions).is_none());
        assert!(get_version_by_dist_tags("tag:nope", &dist_tags, &versions).is_none());
        // Tagged version not in versions → None.
        let missing = BTreeSet::new();
        assert!(get_version_by_dist_tags("tag:beta", &dist_tags, &missing).is_none());
    }
}
