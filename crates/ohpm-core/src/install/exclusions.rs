//! Project-level exclusions, mirroring `lib/core/exclusions/ExclusionsManager.js`
//! — the `exclusions` field of `oh-package.json5`: `{ "foo": ["depA"],
//! "foo@1.0.0": ["depB"] }` removes the listed dependencies from the node's
//! `dependencies` and `dynamicDependencies` (never `devDependencies`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::error::{OhpmError, Result};
use crate::install::node::NodeData;
use crate::install::spec::{is_local_dependency, OhpaType};

/// The effective dependency maps of a node after the overrideDepMap mask and
/// exclusions — the requirements view used by the graph and the install record.
#[derive(Debug, Clone, Default)]
pub struct EffectiveDeps {
    pub dependencies: BTreeMap<String, String>,
    pub dynamic_dependencies: BTreeMap<String, String>,
}

impl EffectiveDeps {
    /// Build from the node's own maps or an overrideDepMap entry.
    pub fn from_node(node: &NodeData) -> EffectiveDeps {
        EffectiveDeps {
            dependencies: node.dependencies.clone(),
            dynamic_dependencies: node.dynamic_dependencies.clone(),
        }
    }

    /// Remove the excluded dep names (dependencies + dynamicDependencies).
    pub fn remove(&mut self, names: &[String]) {
        for name in names {
            self.dependencies.remove(name);
            self.dynamic_dependencies.remove(name);
        }
    }
}

/// The parsed exclusions map + the build-time accumulation for the install
/// record (`_exclusionsFinalMap`).
#[derive(Debug, Clone, Default)]
pub struct Exclusions {
    /// Resolved keys (`name` | `name@<spec>`) -> dep names.
    map: BTreeMap<String, Vec<String>>,
    /// The resolved keys set (`exclusionsKeys`).
    keys: BTreeSet<String>,
    /// The resolved overrideDependencyMap keys (key-choice + conflict check).
    override_dep_map_keys: BTreeSet<String>,
    /// `_exclusionsFinalMap` — `name` | `name@<versionKey>` -> the effective
    /// (post-exclusion) dependency maps, accumulated during the graph build.
    final_map: BTreeMap<String, EffectiveDeps>,
}

impl Exclusions {
    /// `init` — parse the `exclusions` field; `override_keys` are the resolved
    /// keys of the project's `overrideDependencyMap` (used by
    /// `addToFinalMap` and `validMultiModifiedOfExclusionsAndOvrdDepMap`).
    pub fn from_manifest_value(
        v: Option<&serde_json::Value>,
        project_root: &Path,
        override_keys: &BTreeSet<String>,
    ) -> Result<Exclusions> {
        let Some(v) = v else {
            return Ok(Exclusions::default());
        };
        let Some(obj) = v.as_object() else {
            return Err(OhpmError::new(
                "ExclusionsValueTypeError",
                "The value type of \"exclusions\" must be an object.",
            ));
        };
        let mut map = BTreeMap::new();
        let mut keys = BTreeSet::new();
        for (raw, value) in obj {
            let key = resolve_key(raw, project_root)?;
            keys.insert(key.clone());
            let Some(list) = value.as_array() else {
                return Err(OhpmError::new(
                    "ExclusionsValueNotArrayError",
                    format!("The exclusions value of \"{raw}\" must be an array of dependency names."),
                ));
            };
            let deps: Vec<String> = list
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            map.insert(key, deps);
        }
        let exclusions = Exclusions {
            map,
            keys,
            override_dep_map_keys: override_keys.clone(),
            final_map: BTreeMap::new(),
        };
        // `validMultiModifiedOfExclusionsAndOvrdDepMap` — the same dependency
        // modified by both maps with different explicit specs is an error.
        exclusions.valid_multi_modified()?;
        Ok(exclusions)
    }

    /// `validMultiModifiedOfExclusionsAndOvrdDepMap` — a key collision on the
    /// same name where BOTH maps carry explicit DIFFERENT specs is an error.
    fn valid_multi_modified(&self) -> Result<()> {
        let mut seen: BTreeMap<&str, (&str, &str)> = BTreeMap::new();
        let mut conflicts = Vec::new();
        for (name, spec) in self
            .override_dep_map_keys
            .iter()
            .map(|k| split_key(k))
        {
            seen.insert(name, (spec, ""));
        }
        for key in &self.keys {
            let (name, spec) = split_key(key);
            if let Some((ov_spec, _)) = seen.get(name).copied() {
                if !spec.is_empty() && !ov_spec.is_empty() && spec != ov_spec {
                    conflicts.push(key.clone());
                }
            }
        }
        if conflicts.is_empty() {
            return Ok(());
        }
        Err(OhpmError::exclusions_conflict(&conflicts.join(", ")))
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// The resolved keys set (`exclusionsKeys`).
    pub fn keys(&self) -> &BTreeSet<String> {
        &self.keys
    }

    /// `_exclusionsFinalMap` — accumulated during the graph build.
    pub fn final_map(&self) -> &BTreeMap<String, EffectiveDeps> {
        &self.final_map
    }

    /// `hasExclusions` — the resolved keys contain `name@version` or `name`.
    pub fn has_exclusions_for(&self, name: &str, version_key: &str) -> bool {
        self.keys.contains(&format!("{name}@{version_key}")) || self.keys.contains(name)
    }

    /// `getValueFromMapByNameAndVersion` — try `name@version` then `name`.
    pub fn deps_for(&self, name: &str, version_key: &str) -> Option<&Vec<String>> {
        self.map
            .get(&format!("{name}@{version_key}"))
            .or_else(|| self.map.get(name))
    }

    /// `useExclusions` — for a SourceCode node, warn (when configured) and
    /// skip; otherwise delete the excluded deps from the effective maps and
    /// record the final map entry. Returns `true` when deps were removed.
    pub fn use_exclusions(
        &mut self,
        node: &NodeData,
        version_key: &str,
        effective: &mut EffectiveDeps,
        project_root: &Path,
    ) -> bool {
        // `checkSourceCodeConfigInExclusionsAndWarning` — SourceCode deps can
        // never be excluded.
        if node.ohpa_type == OhpaType::SourceCode {
            if self.has_exclusions_for(&node.name, version_key) {
                log::warn!(
                    "Dependency of source code \"{}@{}\" cannot be excluded by \"exclusions\".",
                    node.name,
                    version_key
                );
            }
            return false;
        }
        let Some(deps) = self.deps_for(&node.name, version_key).cloned() else {
            return false;
        };
        if deps.is_empty() {
            return false;
        }
        effective.remove(&deps);
        self.add_to_final_map(&node.name, version_key, effective, project_root);
        true
    }

    /// `addToFinalMap` — the record key is `name@<versionKey>` when the
    /// configured key (or an overrideDependencyMap key) matches it exactly,
    /// local version keys relative to the project root; otherwise `name`.
    pub fn add_to_final_map(
        &mut self,
        name: &str,
        version_key: &str,
        effective: &EffectiveDeps,
        project_root: &Path,
    ) {
        let key = if self.keys.contains(&format!("{name}@{version_key}"))
            || self.override_dep_map_keys.contains(&format!("{name}@{version_key}"))
        {
            if is_local_dependency(version_key) {
                let rel = relative_slash(project_root, Path::new(version_key));
                format!("{name}@{rel}")
            } else {
                format!("{name}@{version_key}")
            }
        } else {
            name.to_string()
        };
        self.final_map.insert(key, effective.clone());
    }
}

/// `resolveKey` — `name` or `name@<exactVersion|standardTag|latest|localPath>`,
/// local specs resolved against the project root with forward slashes.
fn resolve_key(raw: &str, project_root: &Path) -> Result<String> {
    if raw.trim().is_empty() {
        return Err(OhpmError::new(
            "ExclusionsKeyEmptyError",
            "The \"exclusions\" key cannot be empty.",
        ));
    }
    let (name, spec) = split_key(raw);
    if spec.is_empty() {
        return Ok(name.to_string());
    }
    if is_local_dependency(spec) {
        let stripped = spec.strip_prefix("file:").unwrap_or(spec);
        let p = Path::new(stripped);
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            project_root.join(p)
        };
        return Ok(format!("{name}@{}", abs.to_string_lossy().replace('\\', "/")));
    }
    // `validSpec` — exact version, standard tag or latest only.
    if node_semver::Version::parse(spec).is_err()
        && !crate::install::spec::is_standard_tag_dependency(spec)
        && spec != crate::constants::LATEST
    {
        return Err(OhpmError::new(
            "ExclusionsInvalidSpecError",
            format!("Invalid specification: \"{spec}\" in \"exclusions\"."),
        ));
    }
    Ok(raw.to_string())
}

/// `splitKeyToNameAndSpec` — split at the last `@` (`lastIndexOf`): no `@` or
/// a leading `@` (scoped name) yields no spec; a trailing `@` is stripped.
fn split_key(raw: &str) -> (&str, &str) {
    match raw.rfind('@') {
        Some(i) if i > 0 && i == raw.len() - 1 => (&raw[..i], ""),
        Some(i) if i > 0 => (&raw[..i], &raw[i + 1..]),
        _ => (raw, ""),
    }
}

/// `path.relative` with forward slashes.
fn relative_slash(from: &Path, to: &Path) -> String {
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

/// Whether a node's own type is excludable (SourceCode never is).
pub fn is_excludable(node: &NodeData) -> bool {
    node.ohpa_type != OhpaType::SourceCode
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, ohpa_type: OhpaType) -> NodeData {
        NodeData {
            name: name.to_string(),
            ohpa_type,
            ..Default::default()
        }
    }

    #[test]
    fn excludes_from_both_maps() {
        let mut ex = Exclusions::default();
        ex.map.insert("foo".to_string(), vec!["depA".to_string()]);
        ex.keys.insert("foo".to_string());
        let mut eff = EffectiveDeps::default();
        eff.dependencies.insert("depA".to_string(), "^1.0.0".to_string());
        eff.dependencies.insert("depB".to_string(), "^2.0.0".to_string());
        eff.dynamic_dependencies.insert("depA".to_string(), "^1.0.0".to_string());
        let masked = ex.use_exclusions(&node("foo", OhpaType::Range), "1.2.3", &mut eff, Path::new("/proj"));
        assert!(masked);
        assert!(!eff.dependencies.contains_key("depA"));
        assert!(eff.dependencies.contains_key("depB"));
        assert!(!eff.dynamic_dependencies.contains_key("depA"));
        // Record key is the bare name when the configured key has no spec.
        assert!(ex.final_map().contains_key("foo"));
        assert!(!ex.final_map().contains_key("foo@1.2.3"));
    }

    #[test]
    fn versioned_key_uses_version_in_record() {
        let mut ex = Exclusions {
            map: BTreeMap::new(),
            keys: BTreeSet::from(["foo@1.2.3".to_string()]),
            override_dep_map_keys: BTreeSet::new(),
            final_map: BTreeMap::new(),
        };
        ex.map
            .insert("foo@1.2.3".to_string(), vec!["depA".to_string()]);
        let mut eff = EffectiveDeps::default();
        eff.dependencies.insert("depA".to_string(), "^1.0.0".to_string());
        ex.use_exclusions(&node("foo", OhpaType::Range), "1.2.3", &mut eff, Path::new("/proj"));
        assert!(ex.final_map().contains_key("foo@1.2.3"));
    }

    #[test]
    fn source_code_never_excluded() {
        let mut ex = Exclusions {
            map: BTreeMap::new(),
            keys: BTreeSet::from(["foo".to_string()]),
            override_dep_map_keys: BTreeSet::new(),
            final_map: BTreeMap::new(),
        };
        ex.map.insert("foo".to_string(), vec!["depA".to_string()]);
        let mut eff = EffectiveDeps::default();
        eff.dependencies.insert("depA".to_string(), "^1.0.0".to_string());
        let masked = ex.use_exclusions(&node("foo", OhpaType::SourceCode), "/src/foo", &mut eff, Path::new("/proj"));
        assert!(!masked);
        assert!(eff.dependencies.contains_key("depA"));
        assert!(ex.final_map().is_empty());
    }

    #[test]
    fn conflicting_specs_are_an_error() {
        let err = Exclusions::from_manifest_value(
            Some(&serde_json::json!({"foo@1.0.0": ["a"]})),
            Path::new("/proj"),
            &BTreeSet::from(["foo@2.0.0".to_string()]),
        )
        .unwrap_err();
        assert_eq!(err.code, "ExclusionsConflict");
    }
}
