//! `ohpm list`, mirroring `lib/core/install/service/list.js` +
//! `listRecursive.js`, the `DepNodeSerializer` and `util/unicode-graph.js`.
//!
//! The graph is built with the `Installed` builder type (unmet nodes are kept
//! with their faults instead of aborting) and serialized in the reference's
//! unicode / json formats — byte-identical output, including the ANSI colors
//! (`blackBright` versions/paths, the `UNMET DEPENDENCY` prefix and the
//! `conflict versions:` suffix).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use crate::config::Config;
use crate::error::{OhpmError, Result};
use crate::install::graph::{build_graphs_installed, DependencyGraph};
use crate::install::node::Node;
use crate::install::resolver::Resolver;
use crate::registry::RegistryClient;

/// The `ohpm list` options.
#[derive(Debug, Clone, Default)]
pub struct ListOptions {
    /// `-d/--depth` — the max graph depth (default 0: the root + one level).
    pub depth: Option<u32>,
    /// `-j/--json`.
    pub json: bool,
    /// `[<@group>/]<pkg>[@<version>]` — show only this dependency (and its
    /// ancestors); without a depth the full chain is walked.
    pub pkg: Option<String>,
}

/// The rendered graph + the unmet-dependency problems.
#[derive(Debug)]
pub struct ListOutcome {
    /// The unicode or json rendering (the CLI prints it verbatim).
    pub output: String,
    /// The unmet-dep fault messages (`printListProblems`).
    pub problems: Vec<String>,
}

/// A walked tree node (`SyncDepthGraphWalker`'s `{node, depth, children}`).
#[derive(Debug, Clone)]
struct Tree {
    node: Arc<Node>,
    children: Vec<Tree>,
    displayed: bool,
}

/// The version sets per name (the graph's rough cache — `getVersionSet`).
type VersionSets = BTreeMap<String, Vec<String>>;

/// `list` — the graph of the package containing `prefix`.
pub async fn list(
    client: &RegistryClient,
    config: &Config,
    prefix: &Path,
    opts: &ListOptions,
) -> Result<ListOutcome> {
    if !prefix.join(crate::constants::MY_PACKAGE_JSON).is_file() {
        return Err(OhpmError::new(
            "PkgNotFound",
            "The package root does not exist or is inaccessible.",
        ));
    }
    let target_depth = depth_of(opts);
    let (graph, project_root) = build_list_graph(client, config, prefix).await?;
    Ok(render_graph(&graph, &project_root, prefix, opts, target_depth))
}

/// `listRecursive` — one graph per project module (`-r`).
pub async fn list_recursive(
    client: &RegistryClient,
    config: &Config,
    prefix: &Path,
    opts: &ListOptions,
) -> Result<ListOutcome> {
    let target_depth = depth_of(opts);
    let project = crate::config::find_project_root(prefix)
        .and_then(|p| crate::install::modules::ProjectBuildProfile::load(&p));
    let project_root = project
        .as_ref()
        .map(|p| p.project_root.clone())
        .unwrap_or_else(|| prefix.to_path_buf());
    let module_roots = crate::install::modules::module_roots(prefix, true, project.as_ref());
    let (resolver, parameter) =
        make_resolver(client, config, &project_root, &module_roots).await?;
    let max_concurrent = config.get_number(crate::config::default::types::MAX_CONCURRENT) as usize;
    let retry_times = config.get_number(crate::config::default::types::RETRY_TIMES) as u32;
    let retry_interval = config.get_number(crate::config::default::types::RETRY_INTERVAL) as u64;

    let mut graphs = Vec::new();
    for module_root in &module_roots {
        let root = crate::install::root::get_root_node(
            module_root,
            true,
            project.as_ref(),
            parameter.as_ref(),
        )?;
        let graph = build_graphs_installed(
            &resolver,
            vec![(module_root.clone(), Arc::new(root))],
            max_concurrent,
            retry_times,
            retry_interval,
            project.as_ref(),
        )
        .await?;
        graphs.push(graph);
    }
    // `sortGraphs` — the project-root graph first, the rest by root name.
    graphs.sort_by(|a, b| {
        let a_root = a.roots.first().map(|(_, r)| r.data.name.clone());
        let b_root = b.roots.first().map(|(_, r)| r.data.name.clone());
        match (a_root, b_root) {
            (Some(a), Some(b)) => {
                if a.is_empty() && !b.is_empty() {
                    std::cmp::Ordering::Less
                } else if b.is_empty() && !a.is_empty() {
                    std::cmp::Ordering::Greater
                } else {
                    a.cmp(&b)
                }
            }
            _ => std::cmp::Ordering::Equal,
        }
    });
    let mut problems = Vec::new();
    let mut rendered = String::new();
    if opts.json {
        rendered.push_str("[\n");
    }
    for (i, graph) in graphs.iter().enumerate() {
        let outcome = render_graph_with(
            graph,
            &project_root,
            prefix,
            opts,
            target_depth,
            true,
        );
        problems.extend(outcome.problems);
        if opts.json {
            if i > 0 {
                rendered.push_str(",\n");
            }
            rendered.push_str(&outcome.output);
        } else {
            rendered.push_str(&outcome.output);
            // Each recursive graph is printed with its own trailing newline
            // (the graph text ends with `\n`; the print adds a second one).
            rendered.push('\n');
        }
    }
    if opts.json {
        rendered.push_str("]\n");
    }
    Ok(ListOutcome {
        output: rendered,
        problems,
    })
}

/// `d && !r.depth ? Infinity : r.depth ? r.depth : 0` — a pkg filter without
/// `-d` walks the full chain; otherwise 0 (root + one level) by default.
fn depth_of(opts: &ListOptions) -> i32 {
    if opts.pkg.is_some() && opts.depth.is_none() {
        i32::MAX
    } else {
        opts.depth.unwrap_or(0) as i32
    }
}

/// The shared list setup: project + parameter + resolver (lockers eager).
async fn make_resolver(
    client: &RegistryClient,
    config: &Config,
    project_root: &Path,
    module_roots: &[std::path::PathBuf],
) -> Result<(Arc<Resolver>, Option<crate::install::parameter::Parameterization>)> {
    let parameter =
        crate::install::parameter::Parameterization::setup(config, project_root, None)?;
    let workspace = crate::workspace::Workspace::find(project_root)?;
    let lock_name = crate::install::lockfile::lock_file_name("");
    let module_root_set: std::collections::BTreeSet<_> =
        module_roots.iter().cloned().collect();
    let resolver = Arc::new(Resolver::new(
        client.clone(),
        config.clone(),
        project_root.to_path_buf(),
        workspace,
        parameter.as_ref(),
        &lock_name,
        false,
        module_root_set,
    )?);
    resolver.ensure_lockers(module_roots).await;
    Ok((resolver, parameter))
}

/// The `Installed`-mode graph of the prefix module.
async fn build_list_graph(
    client: &RegistryClient,
    config: &Config,
    prefix: &Path,
) -> Result<(DependencyGraph, std::path::PathBuf)> {
    let project = crate::config::find_project_root(prefix)
        .and_then(|p| crate::install::modules::ProjectBuildProfile::load(&p));
    let project_root = project
        .as_ref()
        .map(|p| p.project_root.clone())
        .unwrap_or_else(|| prefix.to_path_buf());
    let (resolver, parameter) = make_resolver(client, config, &project_root, &[prefix.to_path_buf()]).await?;
    let max_concurrent = config.get_number(crate::config::default::types::MAX_CONCURRENT) as usize;
    let retry_times = config.get_number(crate::config::default::types::RETRY_TIMES) as u32;
    let retry_interval = config.get_number(crate::config::default::types::RETRY_INTERVAL) as u64;
    let root = crate::install::root::get_root_node(prefix, true, project.as_ref(), parameter.as_ref())?;
    let graph = build_graphs_installed(
        &resolver,
        vec![(prefix.to_path_buf(), Arc::new(root))],
        max_concurrent,
        retry_times,
        retry_interval,
        project.as_ref(),
    )
    .await?;
    Ok((graph, project_root))
}

/// Walk the graph (`SyncDepthGraphWalker`) and render (`DepNodeSerializer`).
fn render_graph(
    graph: &DependencyGraph,
    project_root: &Path,
    prefix: &Path,
    opts: &ListOptions,
    target_depth: i32,
) -> ListOutcome {
    render_graph_with(graph, project_root, prefix, opts, target_depth, false)
}

fn render_graph_with(
    graph: &DependencyGraph,
    project_root: &Path,
    prefix: &Path,
    opts: &ListOptions,
    target_depth: i32,
    recursive: bool,
) -> ListOutcome {
    let version_sets = version_sets(graph);
    let mut trees: Vec<Tree> = graph
        .roots
        .iter()
        .map(|(_, r)| walk(graph, r.clone(), -1, target_depth, &version_sets))
        .collect();
    let filter = opts.pkg.as_deref();
    for tree in &mut trees {
        mark_display(tree, filter);
    }
    // The serializer picks the root whose store dir matches the module dir.
    let root = trees
        .iter()
        .find(|t| {
            t.node.data.is_root
                && (t.node.data.pkg_store_dir == prefix.to_string_lossy()
                    || t.node.data.resolve_save_root(project_root) == prefix)
        })
        .or_else(|| trees.iter().next());
    let Some(root) = root else {
        return ListOutcome {
            output: String::new(),
            problems: Vec::new(),
        };
    };
    // `printListProblems` — the faults recorded at depth <= the requested one.
    let problems = graph
        .faults
        .iter()
        .filter(|(_, d)| *d <= target_depth)
        .map(|(m, _)| m.clone())
        .collect();
    let output = if opts.json {
        render_json(&root, project_root)
    } else {
        render_unicode(&root, &version_sets, filter.is_some(), empty_label(recursive))
    };
    ListOutcome { output, problems }
}

/// The placeholder node of an empty graph: the stream renders `"(empty)"`
/// with an empty label, the recursive renderer shows the text.
fn empty_label(recursive: bool) -> &'static str {
    if recursive { "(empty)" } else { "" }
}

/// The version sets per name (the graph's rough cache — `getVersionSet`).
fn version_sets(graph: &DependencyGraph) -> VersionSets {
    let mut sets: VersionSets = BTreeMap::new();
    for node in graph.rough_nodes() {
        if node.data.is_root {
            continue;
        }
        let set = sets.entry(node.data.name.clone()).or_default();
        let v = if node.data.pinned_spec.is_empty() {
            node.data.fetch_spec.clone()
        } else {
            node.data.pinned_spec.clone()
        };
        if !set.contains(&v) {
            set.push(v);
        }
    }
    sets
}

/// `SyncDepthGraphWalker` — children while `depth < targetDepth`.
fn walk(
    graph: &DependencyGraph,
    node: Arc<Node>,
    depth: i32,
    target_depth: i32,
    _sets: &VersionSets,
) -> Tree {
    let mut tree = Tree {
        node: node.clone(),
        children: Vec::new(),
        displayed: false,
    };
    if depth >= target_depth {
        return tree;
    }
    for (name, req) in &node.requirements {
        let Ok(child) = graph.pick_node(name, &req.spec, &node) else {
            continue;
        };
        tree.children
            .push(walk(graph, child, depth + 1, target_depth, _sets));
    }
    tree
}

/// `checkNeedDisplay` + the ancestor propagation: a matching node (name or
/// `name@version`) is displayed, and its ancestors inherit the display.
fn mark_display(tree: &mut Tree, filter: Option<&str>) -> bool {
    let mine = if let Some(filter) = filter {
        let has_version = filter.rfind('@').is_some_and(|i| i > 0);
        if has_version {
            filter == tree.node.node_key()
        } else {
            filter == tree.node.data.name
        }
    } else {
        true
    };
    let mut any_child = false;
    for child in &mut tree.children {
        any_child |= mark_display(child, filter);
    }
    let displayed = if filter.is_some() { mine || any_child } else { true };
    tree.displayed = displayed;
    displayed
}

/// `\x1b[90m` blackBright wrapping, reset with `\x1b[39m`.
fn dim(s: &str) -> String {
    format!("\x1b[90m{s}\x1b[39m")
}

/// The root label: `name <saveSpec> <realPath>` (the module dir).
fn root_label(root: &Tree) -> String {
    let name = if root.node.data.name.is_empty() {
        ".".to_string()
    } else {
        root.node.data.name.clone()
    };
    // `buildUnicodeArchyResult` — the realPath separator exists only when the
    // node has a name (`const i = e.node.name ? " " : ""`).
    let sep = if root.node.data.name.is_empty() { "" } else { " " };
    format!(
        "{name} {}{}{}",
        dim(&root.node.data.save_spec),
        sep,
        dim(&root.node.data.pkg_store_dir)
    )
}

/// A child label: `[UNMET DEPENDENCY ]name <pinnedSpec>[ conflict versions: ...]`.
/// A displayed node of a filtered list is wrapped in `brightLabel`'s
/// `\x1b[40m\x1b[33m...\x1b[39m\x1b[49m`.
fn child_label(child: &Tree, conflict_suffix: &str, filtered: bool) -> String {
    let mut label = String::new();
    if child.node.data.unmet.is_some() {
        label.push_str("\x1b[40m\x1b[31mUNMET DEPENDENCY\x1b[39m\x1b[49m ");
    }
    let name = if child.node.data.name.is_empty() {
        ".".to_string()
    } else {
        child.node.data.name.clone()
    };
    let body = format!("{name} {}", dim(&child.node.data.pinned_spec));
    if filtered && child.displayed {
        label.push_str(&format!("\x1b[40m\x1b[33m{body}\x1b[39m\x1b[49m"));
    } else {
        label.push_str(&body);
    }
    if !conflict_suffix.is_empty() {
        label.push_str(&format!("\x1b[93m\x1b[40m{suffix}\x1b[39m\x1b[49m", suffix = conflict_suffix));
    }
    label
}

/// The `conflict versions: a, b` suffix for a child whose name resolves to
/// more than one version in the graph.
fn conflict_suffix(child: &Tree, sets: &VersionSets) -> String {
    let Some(versions) = sets.get(&child.node.data.name) else {
        return String::new();
    };
    if versions.len() <= 1 {
        return String::new();
    }
    format!("conflict versions: {}", versions.join(", "))
}

/// The unicode rendering (`unicodeGraphStream`), byte-identical to the
/// reference (including the ANSI colors and the empty `(empty)` label).
fn render_unicode(root: &Tree, sets: &VersionSets, filtered: bool, empty: &str) -> String {
    let mut out = String::new();
    out.push_str(&root_label(root));
    out.push('\n');
    if root.children.is_empty() {
        // The serializer's `"(empty)"` placeholder.
        out.push_str(&format!("└── {empty}\n"));
        return out;
    }
    render_children_unicode(root, "", sets, filtered, &mut out);
    out
}

fn render_children_unicode(
    node: &Tree,
    prefix: &str,
    sets: &VersionSets,
    filtered: bool,
    out: &mut String,
) {
    let displayed: Vec<&Tree> = node.children.iter().filter(|c| c.displayed).collect();
    for (i, child) in displayed.iter().enumerate() {
        let last = i == displayed.len() - 1;
        let junction = if last { '└' } else { '├' };
        // The tee follows the archy's DISPLAYED children.
        let tee = if child.children.iter().any(|c| c.displayed) { '┬' } else { '─' };
        out.push_str(prefix);
        out.push(junction);
        out.push('─');
        out.push(tee);
        out.push(' ');
        out.push_str(&child_label(child, &conflict_suffix(child, sets), filtered));
        out.push('\n');
        let next_prefix = format!("{prefix}{}", if last { "  " } else { "│ " });
        render_children_unicode(child, &next_prefix, sets, filtered, out);
    }
}

/// The json rendering (`buildJsonArchy` + `OhJsonPrinter` 2-space indent).
fn render_json(root: &Tree, project_root: &Path) -> String {
    let mut s = oh_json_print(&json_archy(root, project_root, true));
    s.push('\n');
    s
}

/// The serializer's json archy — the key order matches the reference; the
/// `dependencies` key exists on the root always, on children only when they
/// have displayed children.
fn json_archy(tree: &Tree, project_root: &Path, is_root: bool) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    // The root key order (name, version, faults, depType, realPath) differs
    // from the children's (version, unmet, realPath, depType, faults).
    let real_path = serde_json::Value::String(
        tree.node
            .data
            .resolve_pkg_store_dir(project_root)
            .to_string_lossy()
            .into_owned(),
    );
    if is_root {
        map.insert("name".to_string(), serde_json::Value::String(tree.node.data.name.clone()));
        map.insert(
            "version".to_string(),
            serde_json::Value::String(tree.node.data.save_spec.clone()),
        );
        map.insert("faults".to_string(), serde_json::Value::Array(Vec::new()));
        map.insert(
            "depType".to_string(),
            serde_json::Value::String(tree.node.dep_type.as_str().to_string()),
        );
        map.insert("realPath".to_string(), real_path);
    } else {
        if tree.node.data.unmet.is_none() {
            map.insert(
                "version".to_string(),
                serde_json::Value::String(tree.node.data.pinned_spec.clone()),
            );
        }
        if tree.node.data.unmet.is_some() {
            map.insert("unmet".to_string(), serde_json::json!({}));
        }
        map.insert("realPath".to_string(), real_path);
        map.insert(
            "depType".to_string(),
            serde_json::Value::String(tree.node.dep_type.as_str().to_string()),
        );
        if tree.node.data.unmet.is_some() {
            map.insert(
                "faults".to_string(),
                serde_json::json!([fault_message(tree)]),
            );
        }
    }
    let mut deps = serde_json::Map::new();
    for child in tree.children.iter().filter(|c| c.displayed) {
        deps.insert(
            child.node.data.name.clone(),
            json_archy(child, project_root, false),
        );
    }
    if is_root || !deps.is_empty() {
        map.insert("dependencies".to_string(), serde_json::Value::Object(deps));
    }
    serde_json::Value::Object(map)
}

/// The reference's `fault` message (`missing: ...`), stored on the unmet node.
fn fault_message(tree: &Tree) -> String {
    tree.node
        .data
        .unmet
        .as_ref()
        .map(|e| e.message.clone())
        .unwrap_or_default()
}

/// `OhJsonPrinter` — the reference's pretty printer: 2-space indent, raw
/// (unescaped, slash-normalized) strings, and empty containers expanded to
/// `[\n  ]` / `{\n  }`.
fn oh_json_print(value: &serde_json::Value) -> String {
    let mut out = String::new();
    print_json(value, 0, &mut out);
    out
}

fn print_json(value: &serde_json::Value, level: usize, out: &mut String) {
    match value {
        serde_json::Value::Object(map) => {
            out.push_str("{\n");
            let keys: Vec<&String> = map.keys().collect();
            for (i, key) in keys.iter().enumerate() {
                out.push_str(&"  ".repeat(level + 1));
                out.push_str(&format!("\"{key}\": "));
                let child = &map[*key];
                if child.is_object() || child.is_array() {
                    print_json(child, level + 1, out);
                } else {
                    out.push_str(&json_scalar(child));
                }
                if i + 1 < keys.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&"  ".repeat(level));
            out.push('}');
        }
        serde_json::Value::Array(items) => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                out.push_str(&"  ".repeat(level + 1));
                if item.is_object() || item.is_array() {
                    print_json(item, level + 1, out);
                } else {
                    out.push_str(&json_scalar(item));
                }
                if i + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&"  ".repeat(level));
            out.push(']');
        }
        other => out.push_str(&json_scalar(other)),
    }
}

/// `FsUtil.slash(String(o))` wrapped in quotes — raw, unescaped.
fn json_scalar(value: &serde_json::Value) -> String {
    let raw = match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Null => "null".to_string(),
        _ => String::new(),
    };
    if value.is_string() {
        format!("\"{}\"", raw.replace('\\', "/"))
    } else {
        raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::node::DepType;

    fn node(name: &str, pinned: &str) -> Arc<Node> {
        Arc::new(Node {
            data: Arc::new(crate::install::node::NodeData {
                name: name.to_string(),
                actual_name: name.to_string(),
                pinned_spec: pinned.to_string(),
                fetch_spec: pinned.to_string(),
                save_spec: pinned.to_string(),
                ..Default::default()
            }),
            dep_type: DepType::Prod,
            requirements: BTreeMap::new(),
            masked_by_override_dependency_map: false,
        })
    }

    #[test]
    fn root_label_format() {
        let mut root = node("entry", "1.0.0");
        Arc::get_mut(&mut root).unwrap().data = Arc::new(crate::install::node::NodeData {
            save_spec: "1.0.0".to_string(),
            pkg_store_dir: "/proj/entry".to_string(),
            ..(*root.data).clone()
        });
        let tree = Tree {
            node: root,
            children: Vec::new(),
            displayed: true,
        };
        let out = render_unicode(&tree, &VersionSets::new(), false, "");
        assert_eq!(
            out,
            "entry \u{1b}[90m1.0.0\u{1b}[39m \u{1b}[90m/proj/entry\u{1b}[39m\n└── \n"
        );
    }

    #[test]
    fn conflict_suffix_built() {
        let child = Tree {
            node: node("foo", "1.0.0"),
            children: Vec::new(),
            displayed: true,
        };
        let mut sets = VersionSets::new();
        sets.insert("foo".to_string(), vec!["1.0.0".to_string(), "2.0.0".to_string()]);
        assert_eq!(conflict_suffix(&child, &sets), "conflict versions: 1.0.0, 2.0.0");
        sets.insert("foo".to_string(), vec!["1.0.0".to_string()]);
        assert_eq!(conflict_suffix(&child, &sets), "");
    }
}
