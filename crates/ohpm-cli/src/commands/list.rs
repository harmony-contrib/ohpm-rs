//! `ohpm list [-r/--recursive] [-j/--json]` — display the dependency graph.
//!
//! Default: the package containing the current directory. `-r` lists every
//! workspace member's graph; at the workspace root (no package in cwd) all
//! members are listed by default. Full dependency resolution is out of scope;
//! installed versions come from `oh_modules` when present.

use anyhow::{anyhow, Result};
use ohpm_core::config::find_local_prefix;
use ohpm_core::constants::MY_PACKAGE_JSON;
use ohpm_core::package::read_manifest_from_dir;
use ohpm_core::package::Manifest;
use ohpm_core::workspace::Workspace;

use super::{load_config, output};
use crate::cli::ListArgs;

pub async fn run(args: &ListArgs) -> Result<()> {
    let _config = load_config()?;
    let cwd = std::env::current_dir()?;

    let local = find_local_prefix(&cwd);
    let ws = Workspace::find(&cwd)?;

    let list_all = args.recursive || (local.is_none() && ws.is_some());
    if list_all {
        let ws = ws.ok_or_else(|| {
            anyhow!("-r/--recursive requires a workspace (no {} found).", ohpm_core::workspace::WORKSPACE_CONFIG)
        })?;
        return render_members(&ws, args.json);
    }

    let local = local.ok_or_else(|| anyhow!("No {} found in the current directory.", MY_PACKAGE_JSON))?;
    let manifest = read_manifest_from_dir(&local)?;
    if args.json {
        output::output(&serde_json::to_string_pretty(&member_json(&manifest, &local.join(ohpm_core::constants::MY_MODULES)))?);
    } else {
        for line in member_lines(&manifest, &local.join(ohpm_core::constants::MY_MODULES)) {
            output::output(&line);
        }
    }
    Ok(())
}

/// List every workspace member's dependency graph.
fn render_members(ws: &Workspace, json: bool) -> Result<()> {
    if json {
        let members: Vec<serde_json::Value> = ws
            .members
            .iter()
            .map(|m| member_json(&m.manifest, &m.dir.join(ohpm_core::constants::MY_MODULES)))
            .collect();
        output::output(&serde_json::to_string_pretty(&serde_json::json!({ "members": members }))?);
        return Ok(());
    }
    for member in &ws.members {
        output::output(&format!("== {}@{} ==", member.manifest.name, member.manifest.version));
        for line in member_lines(&member.manifest, &member.dir.join(ohpm_core::constants::MY_MODULES)) {
            output::output(&line);
        }
        output::output("");
    }
    Ok(())
}

/// Text rendering of one package's dependency graph.
fn member_lines(manifest: &Manifest, oh_modules: &std::path::Path) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!("{}@{}", manifest.name, manifest.version));
    let deps = manifest.dependencies.clone();
    if deps.is_empty() {
        lines.push("└── (no dependencies)".to_string());
        return lines;
    }
    let installed = installed_packages(oh_modules);
    for (i, (name, range)) in deps.iter().enumerate() {
        let is_last = i == deps.len() - 1;
        let branch = if is_last { "└──" } else { "├──" };
        let version = installed.get(name).cloned().unwrap_or_else(|| range.clone());
        lines.push(format!("{branch} {name}@{version}"));
        if let Some(children) = installed_children(oh_modules, name) {
            for (j, child) in children.iter().enumerate() {
                let cbranch = if is_last && j == children.len() - 1 {
                    "    └──"
                } else {
                    "    ├──"
                };
                lines.push(format!("{cbranch} {child}"));
            }
        }
    }
    lines
}

/// JSON rendering of one package.
fn member_json(manifest: &Manifest, oh_modules: &std::path::Path) -> serde_json::Value {
    let installed = installed_packages(oh_modules);
    let mut deps = serde_json::Map::new();
    for (name, range) in &manifest.dependencies {
        deps.insert(
            name.clone(),
            serde_json::json!(installed.get(name).cloned().unwrap_or_else(|| range.clone())),
        );
    }
    serde_json::json!({
        "name": manifest.name,
        "version": manifest.version,
        "dependencies": deps,
    })
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
