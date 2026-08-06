//! The symlink phase, mirroring `lib/core/dependency/visitor/DepNodeSymlinker.js`,
//! `lib/core/dependency/util/deleteUselessLinks.js` and
//! `lib/tools/symlink/SymlinkDir.js`.
//!
//! Layout: every package's dependencies are symlinked as siblings into its own
//! `<saveRoot>/oh_modules/`; the top-level `oh_modules/` holds the direct deps;
//! `<projectRoot>/oh_modules/.ohpm/oh_modules/` is the phantom compatibility
//! layer. Targets are relative on unix (like the reference), created with
//! overwrite/reuse semantics.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::constants::{HSP_DIR, MY_MODULES, PM_DIR};
use crate::error::{OhpmError, Result};
use crate::install::graph::DependencyGraph;
use crate::install::spec::{is_local_dependency, OhpaType};
use crate::install::store::StoreContext;

/// `SymlinkDir.js` — create a directory symlink at `link` pointing at `real`
/// (relative on unix), with overwrite/reuse semantics.
pub fn symlink_dir(real: &Path, link: &Path) -> Result<()> {
    if real == link {
        return Err(OhpmError::new(
            "SymlinkPathError",
            format!("The symlink source and target are the same: {}", real.display()),
        ));
    }
    // Ensure the parent exists so `relative_target` can canonicalize it
    // (macOS /var -> /private/var would otherwise zero the common prefix).
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let target = relative_target(real, link);
    match std::os::unix::fs::symlink(&target, link) {
        Ok(()) => Ok(()),
        Err(e) => match e.kind() {
            std::io::ErrorKind::NotFound => {
                if let Some(parent) = link.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::os::unix::fs::symlink(&target, link)
                    .map_err(|e| OhpmError::file_symlink_dir_failed(link, real, &e.to_string()))
            }
            std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied => {
                // Overwrite: reuse when the target matches, else unlink+recreate.
                match std::fs::read_link(link) {
                    Ok(existing) if existing == target => Ok(()),
                    _ => {
                        remove_link_or_dir(link)?;
                        std::os::unix::fs::symlink(&target, link)
                            .map_err(|e| OhpmError::file_symlink_dir_failed(link, real, &e.to_string()))
                    }
                }
            }
            _ => Err(OhpmError::file_symlink_dir_failed(link, real, &e.to_string())),
        },
    }
}

/// `path.relative(dirname(link), real)` — forward slashes. Both sides are
/// canonicalized first (workspace member dirs are canonicalized during
/// discovery; on macOS `/var` is a symlink to `/private/var`, which would
/// otherwise zero out the common prefix).
fn relative_target(real: &Path, link: &Path) -> PathBuf {
    let real = real.canonicalize().unwrap_or_else(|_| real.to_path_buf());
    // The link itself does not exist yet — canonicalize its parent instead.
    let from = match link.parent() {
        Some(p) => p
            .canonicalize()
            .unwrap_or_else(|_| p.to_path_buf())
            .join(link.file_name().unwrap_or_default()),
        None => link.to_path_buf(),
    };
    let from = from.parent().unwrap_or_else(|| Path::new("."));
    let from_parts: Vec<_> = from.components().collect();
    let to_parts: Vec<_> = real.components().collect();
    let mut common = 0;
    while common < from_parts.len() && common < to_parts.len() && from_parts[common] == to_parts[common] {
        common += 1;
    }
    let mut parts: Vec<std::path::Component> = (0..from_parts.len() - common)
        .map(|_| std::path::Component::ParentDir)
        .collect();
    for c in &to_parts[common..] {
        parts.push(*c);
    }
    let mut out = PathBuf::new();
    for p in parts {
        out.push(p);
    }
    out
}

/// Remove an existing link (or a plain dir that occupies the link path).
fn remove_link_or_dir(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        std::fs::remove_file(path)?;
    } else if meta.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// `deleteUselessLinks.js` — remove entries of `dir` not kept by `keep`
/// (skipping `.ohpm`/`.hsp`; scoped names iterate `@scope/name` children).
pub fn delete_useless_links(dir: &Path, keep: &dyn Fn(&str) -> bool) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == PM_DIR || name == HSP_DIR {
            continue;
        }
        let full = entry.path();
        if !name.starts_with('@') {
            if !keep(&name) {
                let _ = std::fs::remove_dir_all(&full);
                log::info!("remove useless folder succeed: {}", full.display());
            }
            continue;
        }
        let children: Vec<String> = match std::fs::read_dir(&full) {
            Ok(c) => c
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => continue,
        };
        let mut remaining = children.len();
        for child in children {
            if !keep(&format!("{name}/{child}")) {
                let _ = std::fs::remove_dir_all(full.join(&child));
                remaining -= 1;
            }
        }
        if remaining == 0 {
            let _ = std::fs::remove_dir(&full);
        }
    }
    Ok(())
}

/// The symlink-phase state (mirrors `DepNodeSymlinker`'s caches).
#[derive(Default)]
struct SymlinkerState {
    symlink_visited: HashSet<PathBuf>,
    symlink_cache: HashSet<PathBuf>,
    phantom_cache: HashSet<PathBuf>,
}

/// `deleteUselessLinks` of the phantom layer — after all graphs are symlinked,
/// `<projectRoot>/oh_modules/.ohpm/oh_modules/` keeps only names present in the
/// max-satisfying caches (mirrors `installMultiModules`).
pub fn clean_phantom_links(
    project_root: &Path,
    keep_names: &std::collections::BTreeSet<String>,
) -> Result<()> {
    let dir = project_root.join(MY_MODULES).join(PM_DIR).join(MY_MODULES);
    delete_useless_links(&dir, &|name| keep_names.contains(name))
}

/// `DepNodeSymlinker.run` — preWalk (direct-dep cleanup), the per-node walk,
/// then the phantom compatibility layer.
pub async fn symlink_phase(ctx: &StoreContext, graph: &DependencyGraph) -> Result<()> {
    let state = Mutex::new(SymlinkerState::default());
    // preWalk: `deleteUselessDirectDepLinks` per module root.
    for (module_root, root) in &graph.roots {
        let keep: Vec<String> = root.requirements.keys().cloned().collect();
        let dir = module_root.join(MY_MODULES);
        delete_useless_links(&dir, &|name| keep.iter().any(|k| k == name))?;
    }
    // walk over the flat graph.
    let nodes = graph.flat_graph();
    for node in nodes {
        symlink_node(ctx, graph, &state, &node)?;
    }
    // postWalk: `symlinkCompatibleModules`.
    let project_root = &ctx.project_root;
    for (module_root, root) in &graph.roots {
        let phantom_root = root
            .data
            .is_shared
            .then(|| project_root.clone())
            .unwrap_or_else(|| module_root.clone())
            .join(MY_MODULES)
            .join(PM_DIR)
            .join(MY_MODULES);
        for name in graph.max_satisfying_names() {
            let Some(node) = graph.pick_max_version(&name).ok() else {
                continue;
            };
            if node.data.is_root || is_local_dependency(&node.data.pinned_spec) {
                continue;
            }
            let link = phantom_root.join(&node.data.name);
            let mut st = state.lock().unwrap();
            if st.phantom_cache.contains(&link) {
                continue;
            }
            st.phantom_cache.insert(link.clone());
            drop(st);
            let real = node
                .data
                .resolve_save_root(module_root)
                .join(MY_MODULES)
                .join(&node.data.name);
            if !real.exists() {
                continue;
            }
            symlink_dir(&real, &link)?;
        }
    }
    Ok(())
}

/// `DepNodeSymlinker.visit` — per-node: clean the save-root `oh_modules`, then
/// symlink every requirement's resolved node into it.
fn symlink_node(
    ctx: &StoreContext,
    graph: &DependencyGraph,
    state: &Mutex<SymlinkerState>,
    node: &crate::install::node::Node,
) -> Result<()> {
    let project_root = &ctx.project_root;
    let mut save_root_oh_modules = node.data.resolve_save_root(project_root).join(MY_MODULES);
    if node.data.is_root {
        for (module_root, root) in &graph.roots {
            if root.node_key() == node.node_key() {
                save_root_oh_modules = module_root.join(MY_MODULES);
            }
        }
    }
    let mut st = state.lock().unwrap();
    if st.symlink_visited.contains(&save_root_oh_modules) {
        return Ok(());
    }
    st.symlink_visited.insert(save_root_oh_modules.clone());
    drop(st);
    // `deleteUselessLinks` — keep: the node's own name, raw requirement names,
    // everything under a source-code parent.
    let keep: Vec<String> = std::iter::once(node.data.name.clone())
        .chain(node.requirements.keys().cloned())
        .collect();
    delete_useless_links(
        &save_root_oh_modules,
        &|name| {
            node.data.ohpa_type == OhpaType::SourceCode || keep.iter().any(|k| k == name)
        },
    )?;
    if node.requirements.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(&save_root_oh_modules)?;
    for (child_name, req) in &node.requirements {
        let child = graph.pick_node(child_name, &req.spec, node)?;
        let link = save_root_oh_modules.join(child_name);
        let mut st = state.lock().unwrap();
        if st.symlink_cache.contains(&link) {
            continue;
        }
        st.symlink_cache.insert(link.clone());
        drop(st);
        let real = child.data.resolve_pkg_store_dir(project_root);
        symlink_dir(&real, &link)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_targets() {
        // <proj>/oh_modules/@ohos/foo -> <proj>/oh_modules/.ohpm/@ohos+foo@1.2.3/oh_modules/@ohos/foo
        let real = Path::new("/proj/oh_modules/.ohpm/@ohos+foo@1.2.3/oh_modules/@ohos/foo");
        let link = Path::new("/proj/oh_modules/@ohos/foo");
        assert_eq!(
            relative_target(real, link),
            PathBuf::from("../.ohpm/@ohos+foo@1.2.3/oh_modules/@ohos/foo")
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_creates_and_reuses() {
        let dir = tempfile::TempDir::new().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = dir.path().join("link");
        symlink_dir(&real, &link).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_link(&link).unwrap(), PathBuf::from("real"));
        // Reuse when identical.
        symlink_dir(&real, &link).unwrap();
        // Overwrite when the target changed.
        let real2 = dir.path().join("real2");
        std::fs::create_dir_all(&real2).unwrap();
        symlink_dir(&real2, &link).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), PathBuf::from("real2"));
    }

    #[cfg(unix)]
    #[test]
    fn delete_useless_links_behavior() {
        let dir = tempfile::TempDir::new().unwrap();
        let oh_modules = dir.path().join("oh_modules");
        std::fs::create_dir_all(oh_modules.join(".ohpm")).unwrap();
        std::fs::create_dir_all(oh_modules.join(".hsp")).unwrap();
        std::fs::create_dir_all(oh_modules.join("@ohos")).unwrap();
        std::fs::create_dir_all(oh_modules.join("@ohos/keep")).unwrap();
        std::fs::create_dir_all(oh_modules.join("@ohos/stale")).unwrap();
        std::fs::create_dir_all(oh_modules.join("stale2")).unwrap();
        delete_useless_links(&oh_modules, &|name| {
            name == "@ohos/keep" || name == "stale2"
        })
        .unwrap();
        assert!(oh_modules.join(".ohpm").exists());
        assert!(oh_modules.join(".hsp").exists());
        assert!(oh_modules.join("@ohos/keep").exists());
        assert!(!oh_modules.join("@ohos/stale").exists());
        assert!(oh_modules.join("@ohos").exists()); // non-empty scope kept
        assert!(oh_modules.join("stale2").exists());
        // Removing the last child drops the scope dir.
        delete_useless_links(&oh_modules, &|name| name == "stale2").unwrap();
        assert!(!oh_modules.join("@ohos").exists());
    }
}
