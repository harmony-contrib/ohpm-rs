//! The `oh_modules/.ohpm/lock.json5` install record, mirroring
//! `lib/core/dependency/dep-lock/DependencyLockResolver.js`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::constants::{LOCK_JSON, MY_MODULES, PM_DIR};
use crate::error::Result;
use crate::install::graph::DependencyGraph;
use crate::install::node::{DepType, Node};
use crate::install::spec::{is_protocol_spec, is_valid_range};

/// `DependencyLockFileName`.
pub const DEPENDENCY_LOCK_FILE_NAME: &str = "lock.json5";

/// `lockVersion`.
pub const LOCK_VERSION: &str = "1.0";

/// The `settings` of the install record.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LockSettings {
    pub resolve_conflict: bool,
    pub resolve_conflict_strict: bool,
    pub install_all: bool,
}

/// A module entry's `{specifier, version}` pair.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecVer {
    pub specifier: String,
    pub version: String,
}

/// A module entry.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleEntry {
    pub name: String,
    pub dependencies: BTreeMap<String, SpecVer>,
    pub dev_dependencies: BTreeMap<String, SpecVer>,
    pub dynamic_dependencies: BTreeMap<String, SpecVer>,
    pub masked_by_override_dependency_map: bool,
}

/// A package entry.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageEntry {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integrity: Option<String>,
    pub store_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store_path_hsp: Option<String>,
    pub dependencies: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub dev_dependencies: BTreeMap<String, String>,
    pub dynamic_dependencies: BTreeMap<String, String>,
    pub dev: bool,
    pub dynamic: bool,
    pub masked_by_override_dependency_map: bool,
}

/// The install record document.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LockRecord {
    pub lock_version: String,
    pub settings: LockSettings,
    pub overrides: BTreeMap<String, String>,
    pub override_dependency_map: BTreeMap<String, serde_json::Value>,
    pub modules: BTreeMap<String, ModuleEntry>,
    pub packages: BTreeMap<String, PackageEntry>,
}

/// `needResolveConflict.js` — the default `resolve_conflict` config.
pub fn need_resolve_conflict(config: &crate::config::Config) -> bool {
    config.get_bool(crate::config::default::types::RESOLVE_CONFLICT)
}

/// `DependencyLockResolver.resolve` — build the record from the graphs.
pub fn resolve(
    graphs: &[DependencyGraph],
    project_root: &Path,
    settings: &LockSettings,
    overrides: Option<&crate::install::overrides::Overrides>,
    exclusions: Option<&crate::install::exclusions::Exclusions>,
) -> LockRecord {
    let mut modules = BTreeMap::new();
    let mut packages = BTreeMap::new();
    for graph in graphs {
        for (module_root, root_node) in &graph.roots {
            let key = relative_slash(project_root, module_root);
            let key = if key.is_empty() { ".".to_string() } else { key };
            modules.insert(key, gen_module(root_node, graph, project_root, module_root));
        }
        fill_packages(&mut packages, graph, project_root);
    }
    // `genOverrideDependencyMap` — the overrideDependencyMap entries, then
    // the build-time exclusions final map (exclusions win key collisions).
    let override_dep_map: BTreeMap<String, serde_json::Value> = overrides
        .map(|o| {
            o.override_dep_map
                .get_entries()
                .iter()
                .map(|(k, e)| {
                    let mut v = serde_json::Map::new();
                    if !e.dependencies.is_empty() {
                        v.insert("dependencies".into(), serde_json::to_value(&e.dependencies).unwrap());
                    }
                    if !e.dev_dependencies.is_empty() {
                        v.insert("devDependencies".into(), serde_json::to_value(&e.dev_dependencies).unwrap());
                    }
                    if !e.dynamic_dependencies.is_empty() {
                        v.insert("dynamicDependencies".into(), serde_json::to_value(&e.dynamic_dependencies).unwrap());
                    }
                    (k.clone(), serde_json::Value::Object(v))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut override_dependency_map = override_dep_map;
    if let Some(ex) = exclusions {
        for (key, eff) in ex.final_map() {
            let mut v = serde_json::Map::new();
            v.insert("dependencies".into(), serde_json::to_value(&eff.dependencies).unwrap());
            v.insert("dynamicDependencies".into(), serde_json::to_value(&eff.dynamic_dependencies).unwrap());
            override_dependency_map.insert(key.clone(), serde_json::Value::Object(v));
        }
    }
    LockRecord {
        lock_version: LOCK_VERSION.to_string(),
        settings: settings.clone(),
        overrides: overrides
            .map(|o| o.overrides_map.clone())
            .unwrap_or_default(),
        override_dependency_map,
        modules,
        packages,
    }
}

/// `genModule` — the module's dependency specifier/version maps.
fn gen_module(
    root_node: &Node,
    graph: &DependencyGraph,
    project_root: &Path,
    module_root: &Path,
) -> ModuleEntry {
    ModuleEntry {
        name: module_name(project_root, module_root),
        dependencies: string_map_to_specver(root_node, graph, project_root, module_root, root_node.dependencies()),
        dev_dependencies: string_map_to_specver(root_node, graph, project_root, module_root, root_node.dev_dependencies()),
        dynamic_dependencies: string_map_to_specver(root_node, graph, project_root, module_root, root_node.dynamic_dependencies()),
        masked_by_override_dependency_map: root_node.masked_by_override_dependency_map,
    }
}

/// The module name from the build profile ("" for the project root).
fn module_name(project_root: &Path, module_root: &Path) -> String {
    if project_root == module_root {
        return String::new();
    }
    if let Some(project) = crate::install::modules::ProjectBuildProfile::load(project_root) {
        return project.get_module_name(module_root);
    }
    String::new()
}

/// `stringMapToStringSpecifierVersion` — each dependency as
/// `{specifier, version}`.
fn string_map_to_specver(
    root_node: &Node,
    graph: &DependencyGraph,
    project_root: &Path,
    module_root: &Path,
    map: BTreeMap<String, String>,
) -> BTreeMap<String, SpecVer> {
    let mut out = BTreeMap::new();
    for (name, spec) in map {
        let child = match graph.pick_node(&name, &spec, root_node) {
            Ok(c) => c,
            Err(_) => continue,
        };
        out.insert(
            name,
            SpecVer {
                specifier: get_spec(project_root, module_root, &spec),
                version: get_actual_spec(&child.data.name, &child.data.pinned_spec, &child.data.version, &child.data.registry_type, project_root),
            },
        );
    }
    out
}

/// `getSpec` — the declared spec, or `file:<rel>` for local specs. Protocol
/// specs (workspace/alias/git) pass through verbatim.
fn get_spec(project_root: &Path, module_root: &Path, spec: &str) -> String {
    if is_protocol_spec(spec) {
        return spec.to_string();
    }
    if spec == "latest" || is_valid_range(spec) {
        return spec.to_string();
    }
    let stripped = spec.strip_prefix("file:").unwrap_or(spec);
    let resolved = crate::workspace::resolve_file_spec(module_root, &format!("file:{stripped}"));
    format!("file:{}", relative_slash(project_root, &resolved))
}

/// `getActualSpec` — the version, or `file:<rel>` for local deps; git deps
/// use the pinned commit as their version identifier.
fn get_actual_spec(name: &str, pinned_spec: &str, version: &str, registry_type: &str, project_root: &Path) -> String {
    let _ = name;
    if registry_type == "local" {
        format!("file:{}", relative_slash(project_root, Path::new(pinned_spec)))
    } else if registry_type == "git" {
        pinned_spec.to_string()
    } else {
        version.to_string()
    }
}

/// `fillPackages` — the package entries (roots excluded).
fn fill_packages(
    packages: &mut BTreeMap<String, PackageEntry>,
    graph: &DependencyGraph,
    project_root: &Path,
) {
    for node in graph.flat_graph() {
        if node.data.is_root || node.data.registry_type == "workspace" {
            // Workspace members never appear in the packages map (pnpm parity).
            continue;
        }
        let key = format!(
            "{}@{}",
            node.data.name,
            get_actual_spec(&node.data.name, &node.data.pinned_spec, &node.data.version, &node.data.registry_type, project_root)
        );
        if packages.contains_key(&key) {
            continue;
        }
        packages.insert(key, gen_package(&node, graph, project_root));
    }
}

/// `genPackage` — the package entry.
fn gen_package(node: &Node, graph: &DependencyGraph, project_root: &Path) -> PackageEntry {
    let save_root = node.data.resolve_save_root(project_root);
    PackageEntry {
        integrity: node.data.integrity.clone(),
        store_path: relative_slash(project_root, &save_root),
        store_path_hsp: None,
        dependencies: actual_dependency(node, graph, DepType::Prod, project_root),
        dev_dependencies: actual_dependency(node, graph, DepType::Dev, project_root),
        dynamic_dependencies: actual_dependency(node, graph, DepType::Dynamic, project_root),
        dev: node.dep_type == DepType::Dev,
        dynamic: node.dep_type == DepType::Dynamic,
        masked_by_override_dependency_map: node.masked_by_override_dependency_map,
    }
}

/// `getActualDependency` — the requirement map resolved to versions.
fn actual_dependency(
    node: &Node,
    graph: &DependencyGraph,
    dep_type: DepType,
    project_root: &Path,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (name, req) in &node.requirements {
        if req.dep_type != dep_type {
            continue;
        }
        let child = match graph.pick_node(name, &req.spec, node) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let value = if child.data.registry_type == "local" {
            relative_slash(project_root, Path::new(&child.data.pinned_spec))
        } else {
            child.data.version.clone()
        };
        out.insert(name.clone(), value);
    }
    out
}

/// `path.relative` with forward slashes.
fn relative_slash(from: &Path, to: &Path) -> String {
    let from_parts: Vec<_> = from.components().collect();
    let to_parts: Vec<_> = to.components().collect();
    let mut common = 0;
    while common < from_parts.len() && common < to_parts.len() && from_parts[common] == to_parts[common] {
        common += 1;
    }
    let mut parts: Vec<String> = (0..from_parts.len() - common).map(|_| "..".to_string()).collect();
    for c in &to_parts[common..] {
        parts.push(c.as_os_str().to_string_lossy().into_owned());
    }
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

/// Write the install record to `<projectRoot>/oh_modules/.ohpm/lock.json5`
/// when it has packages (mirrors `installModules`).
pub fn write_lock_record(project_root: &Path, record: &LockRecord) -> Result<bool> {
    if record.packages.is_empty() {
        return Ok(false);
    }
    let dir = project_root.join(MY_MODULES).join(PM_DIR);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(DEPENDENCY_LOCK_FILE_NAME);
    let json = serde_json::to_string_pretty(record)?;
    std::fs::write(&path, json)?;
    Ok(true)
}

/// The lockfile path for a module root (used by the orchestrator).
pub fn module_lock_path(module_root: &Path) -> PathBuf {
    module_root.join(LOCK_JSON)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::lockfile::LOCKFILE_ATTENTION;

    #[test]
    fn spec_and_version_forms() {
        let root = Path::new("/proj");
        let module = Path::new("/proj/entry");
        assert_eq!(get_spec(root, module, "^1.0.0"), "^1.0.0");
        assert_eq!(get_spec(root, module, "latest"), "latest");
        // Relative to the project root, like the reference.
        assert_eq!(get_spec(root, module, "file:../lib"), "file:lib");
        assert_eq!(get_spec(root, module, "../lib"), "file:lib");
        // `path.resolve("/proj/entry", "../../x")` = "/x".
        assert_eq!(get_spec(root, module, "file:../../x"), "file:../x");
        assert_eq!(
            get_actual_spec("foo", "/proj/../lib", "1.0.0", "local", root),
            "file:../lib"
        );
        assert_eq!(get_actual_spec("foo", "1.2.3", "1.2.3", "ohpm", root), "1.2.3");
    }

    #[test]
    fn record_serialization() {
        let record = LockRecord {
            lock_version: LOCK_VERSION.to_string(),
            settings: LockSettings {
                resolve_conflict: true,
                resolve_conflict_strict: false,
                install_all: false,
            },
            overrides: BTreeMap::new(),
            override_dependency_map: BTreeMap::new(),
            modules: BTreeMap::new(),
            packages: BTreeMap::new(),
        };
        let json = serde_json::to_string_pretty(&record).unwrap();
        assert!(json.starts_with("{\n  \"lockVersion\": \"1.0\","));
        assert!(json.contains("\"resolveConflict\": true"));
        assert!(json.contains("\"resolveConflictStrict\": false"));
        assert!(json.contains("\"installAll\": false"));
        let _ = LOCKFILE_ATTENTION;
    }
}
