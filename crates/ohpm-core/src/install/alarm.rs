//! Install-time alarms, mirroring `lib/core/alarm/`: the strict version
//! conflict alarm (`strictConflictVersionAlarm.js`), the local
//! dependency-name inconsistency alarm
//! (`dependencyNameInconsistencyAlarm.js`, enforced by
//! `enforce_dependency_key` / the build-profile OHMUrl config) and the
//! registry name case-consistency alarm
//! (`registryDepNameConsistencyOnCaseAlarm.js`).
//!
//! The non-strict `conflictVersionAlarm` is deliberately silent: its message
//! print is unreachable in the reference (`installMultiModules` only calls
//! `alarmConflictMessage` when `resolveConflict && strict`).

use std::collections::{BTreeMap, BTreeSet};

use crate::constants::MY_PACKAGE_JSON;
use crate::error::{OhpmError, Result};
use crate::install::graph::DependencyGraph;
use crate::install::spec::OhpaType;

/// The conflict records of the strict alarm (per package name).
#[derive(Debug, Clone, Default)]
pub struct StrictConflictAlarm {
    /// `recordData` — name -> conflict record.
    pub records: BTreeMap<String, ConflictRecord>,
}

/// A conflict record (`{packageName, whichModules, versionSet, resolvedVersion}`).
#[derive(Debug, Clone)]
pub struct ConflictRecord {
    pub package_name: String,
    pub which_modules: BTreeSet<String>,
    pub version_set: BTreeSet<String>,
    pub resolved_version: String,
}

impl StrictConflictAlarm {
    pub fn new() -> StrictConflictAlarm {
        StrictConflictAlarm::default()
    }

    /// `recordConflictMessage` — for each rough dep with more than one pinned
    /// version across the run, record the conflict (first module wins the
    /// entry, later ones add to `whichModules`).
    pub fn record(&mut self, graph: &DependencyGraph, module_root: &str, versions: &std::collections::HashMap<String, BTreeSet<String>>, fetch_specs: &std::collections::HashMap<String, BTreeSet<String>>, max_versions: &BTreeMap<String, String>) {
        for node in graph.rough_nodes() {
            if node.data.is_root {
                continue;
            }
            let Some(set) = versions.get(&node.data.name) else {
                continue;
            };
            if set.len() <= 1 {
                continue;
            }
            if let Some(record) = self.records.get_mut(&node.data.name) {
                record.which_modules.insert(module_root.to_string());
            } else {
                let resolved_version = max_versions
                    .get(&node.data.name)
                    .cloned()
                    .unwrap_or_else(|| node.data.pinned_spec.clone());
                self.records.insert(
                    node.data.name.clone(),
                    ConflictRecord {
                        package_name: node.data.name.clone(),
                        which_modules: BTreeSet::from([module_root.to_string()]),
                        version_set: fetch_specs
                            .get(&node.data.name)
                            .cloned()
                            .unwrap_or_default(),
                        resolved_version,
                    },
                );
            }
        }
    }

    /// `alarmConflictMessage` — the strict mode prints the conflictVersionAlarm
    /// messages (verified against ohpm 6.0.1): the bold header (with the
    /// resolve-conflict suffix) and one warn per conflict with the affected
    /// modules. The reference's unicode-graph printing is unreachable there
    /// (an array `size` check that never fires).
    pub fn print(&self) {
        let header = "Found version conflict(s) in dependencies of project, and we have helped you resolve it automatically.";
        log::warn!("\x1b[93m\x1b[40m\x1b[1m{header}\x1b[22m\x1b[49m\x1b[39m");
        for record in self.records.values() {
            let versions: Vec<String> = record.version_set.iter().cloned().collect();
            let modules: Vec<String> = record.which_modules.iter().cloned().collect();
            log::warn!(
                "dependency \"{}\" has conflict versions: \"{}\", and has been resolved as \"{}\", the affected modules are as follows:\n\t - \"{}\"\n",
                record.package_name,
                versions.join("\", \""),
                record.resolved_version,
                modules.join("\"\n\t - \"")
            );
        }
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// The `ConflictResolveFailure` error (created but not thrown in the
    /// reference — kept for parity).
    pub fn conflict_resolve_failure(&self) -> OhpmError {
        OhpmError::new(
            "ConflictResolveFailure",
            "Encountered some dependency version conflicts that cannot be resolved. Please resolve the above conflict manually.",
        )
    }
}

/// `dependencyNameInconsistencyAlarm` — local (File/SourceCode) dependencies
/// whose declared key does not match the package's actual name.
pub struct NameInconsistencyAlarm {
    /// `enforce_dependency_key` — the inconsistencies become an error.
    enforce: bool,
    messages: Vec<String>,
}

impl NameInconsistencyAlarm {
    pub fn new(enforce: bool) -> NameInconsistencyAlarm {
        NameInconsistencyAlarm {
            enforce,
            messages: Vec::new(),
        }
    }

    /// `recordInconsistencyMessage` — per graph: each root requirement with a
    /// File/SourceCode target whose declared key differs from the actual name.
    pub fn record(&mut self, graph: &DependencyGraph) {
        for (module_root, root) in &graph.roots {
            for (name, req) in &root.requirements {
                let Ok(child) = graph.pick_node(name, &req.spec, root) else {
                    continue;
                };
                if !matches!(child.data.ohpa_type, OhpaType::File | OhpaType::SourceCode) {
                    continue;
                }
                if child.data.actual_name.is_empty() || name == &child.data.actual_name {
                    continue;
                }
                self.messages.push(format!(
                    "local dependency \"{name}\" found in \"{}\" does not match the actual name \"{}\" of its oh-package.json5",
                    module_root.join(MY_PACKAGE_JSON).display(),
                    child.data.actual_name
                ));
            }
        }
    }

    /// `alarmInconsistencyMessage` — throw when enforced, warn otherwise.
    pub fn print(&self) -> Result<()> {
        if self.messages.is_empty() {
            return Ok(());
        }
        if self.enforce {
            let mut error = OhpmError::new(
                "InconsistentDepNames",
                "There are some dependency names that are inconsistent with the actual package names.",
            );
            error = error.with_detail(&self.messages.join("\n"));
            return Err(error);
        }
        for m in &self.messages {
            log::warn!("{}", m);
        }
        Ok(())
    }
}

/// `registryDepNameConsistencyOnCaseAlarm` — registry dependencies whose
/// declared key differs from the actual name only by case.
pub struct CaseInconsistencyAlarm {
    messages: Vec<String>,
}

impl CaseInconsistencyAlarm {
    pub fn new() -> CaseInconsistencyAlarm {
        CaseInconsistencyAlarm {
            messages: Vec::new(),
        }
    }

    /// `recordInconsistencyMessage` — per graph: each root requirement whose
    /// registry target's actual name differs from the declared key by case.
    pub fn record(&mut self, graph: &DependencyGraph) {
        // actualName -> seen declared keys (the reference's `r` map).
        let mut seen: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (module_root, root) in &graph.roots {
            for (name, req) in &root.requirements {
                let Ok(child) = graph.pick_node(name, &req.spec, root) else {
                    continue;
                };
                // `isRegistryDependency` — Version | Range | Tag only.
                if !matches!(
                    child.data.ohpa_type,
                    OhpaType::Range | OhpaType::Version | OhpaType::Tag
                ) {
                    continue;
                }
                let actual = child.data.actual_name.clone();
                if actual.is_empty() {
                    continue;
                }
                // Seeded with the actual name itself (`new Set([actualName])`).
                let keys = seen
                    .entry(actual.clone())
                    .or_insert_with(|| BTreeSet::from([actual.clone()]));
                let lower = name.to_lowercase();
                if !keys.contains(name) && keys.contains(&lower) {
                    self.messages.push(format!(
                        "dependency \"{name}\" found in \"{}\" has a case sensitivity issue and does not match the actual name \"{}\" of its oh-package.json5",
                        module_root.join(MY_PACKAGE_JSON).display(),
                        actual
                    ));
                }
                keys.insert(name.clone());
            }
        }
    }

    /// `alarmInconsistencyMessage` — warn every inconsistency.
    pub fn print(&self) {
        for m in &self.messages {
            log::warn!("{}", m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::node::{DepType, Node, NodeData, Requirement};
    use std::sync::Arc;

    fn node(name: &str, actual: &str, ohpa: OhpaType, pinned: &str) -> Arc<Node> {
        Arc::new(Node {
            data: Arc::new(NodeData {
                name: name.to_string(),
                actual_name: actual.to_string(),
                ohpa_type: ohpa,
                pinned_spec: pinned.to_string(),
                fetch_spec: pinned.to_string(),
                ..Default::default()
            }),
            dep_type: DepType::Prod,
            requirements: BTreeMap::new(),
            masked_by_override_dependency_map: false,
        })
    }

    /// A graph whose root requires `child` (a node registered in the rough
    /// cache under its fetch spec).
    fn graph_with(child: &Arc<Node>, spec: &str) -> DependencyGraph {
        let mut root = node("entry", "entry", OhpaType::SourceCode, "/src");
        Arc::get_mut(&mut root).unwrap().requirements.insert(
            child.data.name.clone(),
            Requirement {
                spec: spec.to_string(),
                dep_type: DepType::Prod,
            },
        );
        let mut g = DependencyGraph::new(
            "/proj".into(),
            vec![("/proj/entry".into(), root)],
            false,
            false,
            Arc::new(std::sync::Mutex::new(BTreeSet::new())),
        );
        g.register_node(child.clone()).unwrap();
        g
    }

    #[test]
    fn name_inconsistency_records_and_prints() {
        // A registry dep is not recorded by the local-name alarm.
        let child = node("foo", "foo", OhpaType::Range, "1.0.0");
        let graph = graph_with(&child, "^1.0.0");
        let mut alarm = NameInconsistencyAlarm::new(false);
        alarm.record(&graph);
        assert!(alarm.messages.is_empty());

        // A File dep whose declared key differs from the actual name: recorded.
        let child = node("foo", "actual-lib", OhpaType::File, "/lib/foo.har");
        let graph = graph_with(&child, "/lib/foo.har");
        let mut alarm = NameInconsistencyAlarm::new(true);
        alarm.record(&graph);
        assert_eq!(alarm.messages.len(), 1);
        assert!(alarm.messages[0].contains("does not match the actual name \"actual-lib\""));
        let err = alarm.print().unwrap_err();
        assert_eq!(err.code, "InconsistentDepNames");
    }

    #[test]
    fn case_inconsistency_records() {
        // The declared key "Foo" is a case-variant of the actual name "foo":
        // the seen-set is seeded with the actual name, so the mismatch fires.
        let child = node("Foo", "foo", OhpaType::Range, "1.0.0");
        let mut child = child;
        // The rough cache is keyed by the DECLARED fetch spec.
        Arc::get_mut(&mut child).unwrap().data = Arc::new(NodeData {
            fetch_spec: "^1.0.0".to_string(),
            ..(*child.data).clone()
        });
        let graph = graph_with(&child, "^1.0.0");
        let mut alarm = CaseInconsistencyAlarm::new();
        alarm.record(&graph);
        assert_eq!(alarm.messages.len(), 1);
        assert!(alarm.messages[0].contains("case sensitivity"));
    }

    #[test]
    fn strict_conflict_records() {
        let mut alarm = StrictConflictAlarm::new();
        let mut versions: std::collections::HashMap<String, BTreeSet<String>> = std::collections::HashMap::new();
        versions.insert("foo".to_string(), BTreeSet::from(["1.0.0".to_string(), "2.0.0".to_string()]));
        let mut fetch: std::collections::HashMap<String, BTreeSet<String>> = std::collections::HashMap::new();
        fetch.insert("foo".to_string(), BTreeSet::from(["^1.0.0".to_string(), "^2.0.0".to_string()]));
        let mut maxes: BTreeMap<String, String> = BTreeMap::new();
        maxes.insert("foo".to_string(), "2.0.0".to_string());
        let child = node("foo", "foo", OhpaType::Range, "2.0.0");
        alarm.record(&graph_with(&child, "^2.0.0"), "/proj/entry", &versions, &fetch, &maxes);
        assert_eq!(alarm.records.len(), 1);
        assert_eq!(alarm.records["foo"].resolved_version, "2.0.0");
        assert_eq!(alarm.records["foo"].version_set.len(), 2);
        assert!(!alarm.is_empty());
    }
}
