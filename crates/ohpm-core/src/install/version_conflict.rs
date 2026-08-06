//! Version-conflict handling, mirroring `lib/core/version-conflict/`
//! (`VersionConflictManager` + `MaxVersionStrategy` /
//! `VersionStrictModeStrategy`) — the max-satisfying node decision that both
//! the dependency graph and the lockfile conflict resolution run on.

use std::collections::BTreeSet;

use crate::error::{OhpmError, Result};
use crate::install::node::{dep_node_version_compare, NodeData};
use crate::install::spec::is_local_dependency;

/// The max-version strategy (`MaxVersionStrategy` | `VersionStrictModeStrategy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    Max,
    Strict,
}

/// `VersionConflictManager.isMaxSatisfying` — whether `node` replaces the
/// current max-satisfying node of its name (`prev`, from the resolved cache).
/// The `fetch_specs` are the declared fetch specs collected for the name
/// (`fetchSpecMap`); strict-mode resolution failures are recorded in
/// `resolve_failed` (`addResolveFailedDepName`).
pub fn is_max_satisfying(
    strategy: Strategy,
    node: &NodeData,
    prev: Option<&NodeData>,
    fetch_specs: &BTreeSet<String>,
    resolve_failed: &mut BTreeSet<String>,
) -> Result<bool> {
    match strategy {
        Strategy::Max => max_is_max_satisfying(node, prev),
        Strategy::Strict => strict_is_max_satisfying(node, prev, fetch_specs, resolve_failed),
    }
}

/// `MaxVersionStrategy.isMaxSatisfying` — the default: a resolved node with
/// no unmet dependency stays; otherwise the higher version wins (registry
/// nodes must carry valid semver).
fn max_is_max_satisfying(node: &NodeData, prev: Option<&NodeData>) -> Result<bool> {
    match prev {
        None => Ok(true),
        Some(prev) if prev.unmet.is_some() => Ok(true),
        Some(_) if node.unmet.is_some() => Ok(false),
        Some(prev) => {
            if (!is_local_dependency(&node.pinned_spec)
                && node_semver::Version::parse(&node.pinned_spec).is_err())
                || node_semver::Version::parse(&node.version).is_err()
            {
                return Err(OhpmError::dep_builder_invalid_dep_version(
                    &node.version,
                    &format!("{}@{}", node.name, node.pinned_spec),
                ));
            }
            Ok(dep_node_version_compare(node, prev) == std::cmp::Ordering::Greater)
        }
    }
}

/// `VersionStrictModeStrategy.isMaxSatisfying` — strict mode: non-fixed
/// pinned versions are rejected, and the target must be satisfiable by the
/// resolved node's constraints (and vice versa), otherwise the name is
/// recorded as resolve-failed.
fn strict_is_max_satisfying(
    node: &NodeData,
    prev: Option<&NodeData>,
    fetch_specs: &BTreeSet<String>,
    resolve_failed: &mut BTreeSet<String>,
) -> Result<bool> {
    if node.is_root {
        return Ok(true);
    }
    if !is_local_dependency(&node.pinned_spec) && !is_fixed_version(&node.pinned_spec) {
        return Err(OhpmError::dep_builder_invalid_dep_version(
            &node.version,
            &format!("{}@{}", node.name, node.pinned_spec),
        ));
    }
    let Some(d) = prev else {
        return Ok(true);
    };
    if d.unmet.is_some() {
        return Ok(true);
    }
    if node.unmet.is_some() {
        return Ok(false);
    }
    if node.pinned_spec != d.pinned_spec
        && (is_local_dependency(&node.pinned_spec) || is_local_dependency(&d.pinned_spec))
    {
        // A local dependency conflict can never be resolved.
        resolve_failed.insert(node.name.clone());
        return Ok(false);
    }
    strict_decision(node, d, fetch_specs, resolve_failed)
}

/// `strictDecisionMakingMode` — the three cases by fetch-spec fixedness of
/// the target and the resolved node.
fn strict_decision(
    node: &NodeData,
    resolved: &NodeData,
    fetch_specs: &BTreeSet<String>,
    resolve_failed: &mut BTreeSet<String>,
) -> Result<bool> {
    if is_fixed_version(&resolved.fetch_spec) {
        strict_resolved_fixed(node, resolved, resolve_failed)
    } else if is_fixed_version(&node.fetch_spec) {
        strict_target_fixed(node, resolved, fetch_specs, resolve_failed)
    } else {
        strict_both_ranges(node, resolved, fetch_specs, resolve_failed)
    }
}

/// `handleWhileResolvedNodeIsFixedVersion` — the resolved node is a fixed
/// version: the target must match it exactly (fixed) or include it (range).
fn strict_resolved_fixed(
    node: &NodeData,
    resolved: &NodeData,
    resolve_failed: &mut BTreeSet<String>,
) -> Result<bool> {
    if is_fixed_version(&node.fetch_spec) {
        if resolved.pinned_spec == node.pinned_spec {
            return Ok(true);
        }
        resolve_failed.insert(node.name.clone());
        return Ok(false);
    }
    if is_version_in_multi_ranges(&node.pinned_spec, &[resolved.fetch_spec.clone()]) {
        return Ok(true);
    }
    resolve_failed.insert(node.name.clone());
    Ok(false)
}

/// `handleWhileTargetNodeIsFixedVersion` — the target is a fixed version: it
/// must be in the resolved node's range, or be a raw version of the name.
fn strict_target_fixed(
    node: &NodeData,
    resolved: &NodeData,
    fetch_specs: &BTreeSet<String>,
    resolve_failed: &mut BTreeSet<String>,
) -> Result<bool> {
    if node.pinned_spec == resolved.pinned_spec {
        return Ok(true);
    }
    if is_version_in_multi_ranges(&node.pinned_spec, &[resolved.fetch_spec.clone()]) {
        let raw = raw_versions(fetch_specs);
        let (_, ranges) = fixed_and_ranges(&raw);
        if is_version_in_multi_ranges(&node.pinned_spec, &ranges) {
            return Ok(true);
        }
    }
    resolve_failed.insert(node.name.clone());
    Ok(false)
}

/// `handleWhileTargetAndResolvedNodeIsRangeVersion` — both are ranges: the
/// lower-pinned node must satisfy the other's constraint.
fn strict_both_ranges(
    node: &NodeData,
    resolved: &NodeData,
    fetch_specs: &BTreeSet<String>,
    resolve_failed: &mut BTreeSet<String>,
) -> Result<bool> {
    let lower_pinned = node_semver::Version::parse(&node.pinned_spec);
    let higher_pinned = node_semver::Version::parse(&resolved.pinned_spec);
    if let (Ok(a), Ok(b)) = (&lower_pinned, &higher_pinned) {
        if a < b {
            let raw = raw_versions(fetch_specs);
            let (_, ranges) = fixed_and_ranges(&raw);
            if is_version_in_multi_ranges(&node.pinned_spec, &ranges) {
                return Ok(true);
            }
            resolve_failed.insert(node.name.clone());
            return Ok(false);
        }
    }
    if is_version_in_multi_ranges(&node.pinned_spec, &[resolved.fetch_spec.clone()])
        || is_version_in_multi_ranges(&resolved.pinned_spec, &[node.fetch_spec.clone()])
    {
        return Ok(true);
    }
    resolve_failed.insert(node.name.clone());
    Ok(false)
}

/// `semver.valid(spec, {loose: true})` — an exact version.
fn is_fixed_version(spec: &str) -> bool {
    node_semver::Version::parse(spec).is_ok()
}

/// `validAndGetRawVersions` — the collected fetch specs of a name.
fn raw_versions(fetch_specs: &BTreeSet<String>) -> BTreeSet<String> {
    fetch_specs.clone()
}

/// `getFixedAndRanges` — split raw versions into fixed and range forms.
fn fixed_and_ranges(versions: &BTreeSet<String>) -> (Vec<String>, Vec<String>) {
    let mut fixed = Vec::new();
    let mut ranges = Vec::new();
    for v in versions {
        if is_local_dependency(v) {
            continue;
        }
        if is_fixed_version(v) {
            fixed.push(v.clone());
        } else {
            ranges.push(v.clone());
        }
    }
    (fixed, ranges)
}

/// `isVersionInMultiRanges` — the version must satisfy EVERY converted range.
pub fn is_version_in_multi_ranges(version: &str, ranges: &[String]) -> bool {
    if ranges.is_empty() {
        return true;
    }
    let converted = convert_versions(ranges);
    converted.iter().all(|r| satisfies(version, r))
}

/// `semver.satisfies(v, range, {loose: true, includePrerelease: true})`.
fn satisfies(version: &str, range: &str) -> bool {
    match (node_semver::Version::parse(version), node_semver::Range::parse(range)) {
        (Ok(v), Ok(r)) => r.satisfies(&v),
        _ => false,
    }
}

/// `semverRangesConversion.convertVersions` — `"1.0.0 - 2.0.0"`-style (`...`)
/// ranges and `^0.x` caret forms rewritten to `>=x <y` compound ranges.
pub fn convert_versions(ranges: &[String]) -> Vec<String> {
    let mut any_zero = false;
    let out: Vec<String> = ranges
        .iter()
        .map(|r| r.replace("...", " - "))
        .collect();
    for r in &out {
        if r.contains("^0") {
            any_zero = true;
            break;
        }
    }
    if !any_zero {
        return out;
    }
    let caret_zero = regex::Regex::new(r"^\^0(\.(?:\d+|x|X|\*)){0,2}(?:-[a-zA-Z0-9.-_]+)*$").unwrap();
    let mut converted = Vec::new();
    for r in out {
        if let Some(captures) = caret_zero.captures(&r) {
            let m = captures.get(0).unwrap().as_str();
            if m.starts_with("^0") {
                let replaced = m.replace('^', "");
                let (version_part, prerelease) = match replaced.split_once('-') {
                    Some((v, p)) => (v.to_string(), Some(p.to_string())),
                    None => (replaced, None),
                };
                // `1.x`/`1.*`/`1` -> `1.0.0`-ish base via the fixed parts.
                let cleaned = version_part
                    .replace('x', "0")
                    .replace('X', "0")
                    .replace('*', "0");
                // Pad to three parts (`semver.coerce`): "0" -> "0.0.0",
                // "0.1" -> "0.1.0".
                let padded = match cleaned.matches('.').count() {
                    0 => format!("{cleaned}.0.0"),
                    1 => format!("{cleaned}.0"),
                    _ => cleaned,
                };
                let parsed = node_semver::Version::parse(&padded).ok();
                if let Some(v) = parsed {
                    let major = v.major;
                    let lo = format!(
                        ">={}.{}.{}{}",
                        v.major,
                        v.minor,
                        v.patch,
                        prerelease.as_ref().map(|p| format!("-{p}")).unwrap_or_default()
                    );
                    let hi = format!("<{}.0.0", major + 1);
                    converted.push(r.replace(&m, &format!("{lo} {hi}")));
                    continue;
                }
            }
        }
        converted.push(r);
    }
    converted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, fetch: &str, pinned: &str) -> NodeData {
        NodeData {
            name: name.to_string(),
            fetch_spec: fetch.to_string(),
            pinned_spec: pinned.to_string(),
            version: pinned.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn max_strategy_basics() {
        let mut failed = BTreeSet::new();
        let n1 = node("foo", "^1.0.0", "1.2.0");
        let n2 = node("foo", "^1.0.0", "1.5.0");
        assert!(is_max_satisfying(Strategy::Max, &n1, None, &BTreeSet::new(), &mut failed).unwrap());
        assert!(is_max_satisfying(Strategy::Max, &n2, Some(&n1), &BTreeSet::new(), &mut failed).unwrap());
        assert!(!is_max_satisfying(Strategy::Max, &n1, Some(&n2), &BTreeSet::new(), &mut failed).unwrap());
    }

    #[test]
    fn strict_rejects_non_fixed_pinned() {
        let mut failed = BTreeSet::new();
        // A range pinned spec (registry dep) is rejected before any comparison.
        let n = node("foo", "^1.0.0", "^1.0.0");
        let err = is_max_satisfying(Strategy::Strict, &n, None, &BTreeSet::new(), &mut failed).unwrap_err();
        assert_eq!(err.code, "DepBuilderInvalidDepVersion");
    }

    #[test]
    fn strict_range_target_in_resolved_range() {
        let mut failed = BTreeSet::new();
        // resolved is fixed 1.5.0; target range ^1.0.0 pins 1.5.0 -> ok.
        let resolved = node("foo", "1.5.0", "1.5.0");
        let target = node("foo", "^1.0.0", "1.5.0");
        assert!(is_max_satisfying(Strategy::Strict, &target, Some(&resolved), &BTreeSet::new(), &mut failed).unwrap());
        // target pins 1.2.0, out of the resolved fixed version -> fail.
        let target = node("foo", "^1.0.0", "1.2.0");
        assert!(!is_max_satisfying(Strategy::Strict, &target, Some(&resolved), &BTreeSet::new(), &mut failed).unwrap());
        assert!(failed.contains("foo"));
    }

    #[test]
    fn convert_zero_carets() {
        assert_eq!(convert_versions(&["^0.1.2".to_string()]), vec![">=0.1.2 <1.0.0"]);
        assert_eq!(convert_versions(&["^0.x".to_string()]), vec![">=0.0.0 <1.0.0"]);
        assert_eq!(convert_versions(&["^1.2.3".to_string()]), vec!["^1.2.3"]);
        // The `^0` regex is fully anchored — hyphen ranges stay untouched.
        assert_eq!(
            convert_versions(&["^0.1.0 - 0.1.9".to_string()]),
            vec!["^0.1.0 - 0.1.9"]
        );
    }
}
