//! `ohpm list` — display the dependency graph (basic).
//!
//! Reads the project manifest and walks `oh_modules` when present. Full
//! dependency resolution is out of scope for this reimplementation.

use anyhow::{anyhow, Result};
use ohpm_core::config::find_local_prefix;
use ohpm_core::package::read_manifest_from_dir;
use ohpm_core::constants::MY_PACKAGE_JSON;

use super::{load_config, output};
use crate::cli::ListArgs;

pub async fn run(args: &ListArgs) -> Result<()> {
    let _config = load_config()?;
    let cwd = std::env::current_dir()?;
    let local = find_local_prefix(&cwd)
        .ok_or_else(|| anyhow!("No {} found in the current directory.", MY_PACKAGE_JSON))?;
    let manifest = read_manifest_from_dir(&local)?;

    let root = format!("{}@{}", manifest.name, manifest.version);
    let deps = manifest.dependencies.clone();

    if args.json {
        let mut tree = serde_json::Map::new();
        tree.insert("name".into(), serde_json::json!(manifest.name));
        tree.insert("version".into(), serde_json::json!(manifest.version));
        tree.insert("dependencies".into(), serde_json::json!(deps));
        output::output(&serde_json::to_string_pretty(&serde_json::Value::Object(tree))?);
        return Ok(());
    }

    output::output(&root);
    if deps.is_empty() {
        output::output("└── (no dependencies)");
        return Ok(());
    }
    let oh_modules = local.join(ohpm_core::constants::MY_MODULES);
    let installed = installed_packages(&oh_modules);
    for (i, (name, range)) in deps.iter().enumerate() {
        let is_last = i == deps.len() - 1;
        let branch = if is_last { "└──" } else { "├──" };
        let version = installed.get(name).cloned().unwrap_or_else(|| range.clone());
        output::output(&format!("{branch} {name}@{version}"));
        if let Some(children) = installed_children(&oh_modules, name) {
            for (j, child) in children.iter().enumerate() {
                let cbranch = if is_last && j == children.len() - 1 { "    └──" } else { "    ├──" };
                output::output(&format!("{cbranch} {child}"));
            }
        }
    }
    let _ = args;
    Ok(())
}

/// Map of installed `name -> version` directly under `oh_modules`.
fn installed_packages(oh_modules: &std::path::Path) -> std::collections::BTreeMap<String, String> {
    let mut map = std::collections::BTreeMap::new();
    if !oh_modules.is_dir() {
        return map;
    }
    for entry in std::fs::read_dir(oh_modules).ok().into_iter().flatten() {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let manifest_path = path.join(MY_PACKAGE_JSON);
        if manifest_path.exists() {
            if let Ok(text) = std::fs::read_to_string(&manifest_path) {
                if let Ok(m) = ohpm_core::package::Manifest::from_json5(&text) {
                    map.insert(m.name.clone(), m.version.clone());
                }
            }
        }
    }
    map
}

/// One-level children of a scoped dependency folder, e.g. `@ohos/foo/oh_modules/*`.
fn installed_children(oh_modules: &std::path::Path, name: &str) -> Option<Vec<String>> {
    let dir = if let Some(local) = name.strip_prefix('@') {
        let (scope, rest) = local.split_once('/')?;
        oh_modules.join(scope).join(rest)
    } else {
        oh_modules.join(name)
    };
    let map = installed_packages(&dir.join(ohpm_core::constants::MY_MODULES));
    if map.is_empty() {
        None
    } else {
        Some(map.into_iter().map(|(n, v)| format!("{n}@{v}")).collect())
    }
}
