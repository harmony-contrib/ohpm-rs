//! Dependency graph build, mirroring
//! `lib/core/dependency/struct/dependencyGraph.js`,
//! `lib/core/dependency/graph-builder/AsyncGraphBuilder.js` and
//! `lib/concurrent/ConcurrentExecutor.js`.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Mutex;

use crate::error::{OhpmError, Result};
use crate::install::node::{dep_node_version_compare, DepType, Node, NodeData};
use crate::install::spec::is_local_dependency;
use crate::install::resolver::{with_network_retry, Resolver};
use crate::install::spec::OhpaType;

/// The dependency graph (`DependencyGraph`).
#[derive(Debug, Clone)]
pub struct DependencyGraph {
    pub project_root: PathBuf,
    pub roots: Vec<(PathBuf, Arc<Node>)>,
    /// name -> fetchSpec -> node (`_roughDepCache`).
    rough: HashMap<String, HashMap<String, Arc<Node>>>,
    /// The max-satisfying node per name (`_finalDepCache` / MaxVersionStrategy).
    final_dep_cache: HashMap<String, Arc<Node>>,
    max_satisfying_cache: HashMap<String, Arc<NodeData>>,
}

impl DependencyGraph {
    pub fn new(project_root: PathBuf, roots: Vec<(PathBuf, Arc<Node>)>) -> Self {
        DependencyGraph {
            project_root,
            roots,
            rough: HashMap::new(),
            final_dep_cache: HashMap::new(),
            max_satisfying_cache: HashMap::new(),
        }
    }

    /// `findNode` — the repeat-node dedupe check.
    pub fn find_node(&self, name: &str, fetch_spec: &str) -> Option<Arc<Node>> {
        self.rough.get(name).and_then(|m| m.get(fetch_spec)).cloned()
    }

    /// `registerNode` — add to the rough cache and update the max-satisfying
    /// cache (`VersionConflictManager.isMaxSatisfying` with the default
    /// `MaxVersionStrategy`).
    pub fn register_node(&mut self, node: Arc<Node>) -> Result<()> {
        let name = node.data.name.clone();
        let fetch_spec = node.data.fetch_spec.clone();
        self.rough.entry(name.clone()).or_default().insert(fetch_spec, node.clone());
        if node.data.is_root {
            return Ok(());
        }
        let is_max = match self.max_satisfying_cache.get(&name).cloned() {
            None => true,
            Some(prev) => {
                if prev.unmet.is_some() {
                    true
                } else if node.data.unmet.is_some() {
                    false
                } else {
                    // `MaxVersionStrategy.isMaxSatisfying` — registry nodes
                    // must carry valid pinned/version semver.
                    if (!is_local_dependency(&node.data.pinned_spec)
                        && node_semver::Version::parse(&node.data.pinned_spec).is_err())
                        || node_semver::Version::parse(&node.data.version).is_err()
                    {
                        return Err(OhpmError::dep_builder_invalid_dep_version(
                            &node.data.version,
                            &format!("{}@{}", node.data.name, node.data.pinned_spec),
                        ));
                    }
                    dep_node_version_compare(&node.data, &prev) == std::cmp::Ordering::Greater
                }
            }
        };
        if is_max {
            self.final_dep_cache.insert(name.clone(), node.clone());
            self.max_satisfying_cache.insert(name, node.data.clone());
        }
        Ok(())
    }

    /// `whereToFindChildNode` — the base directory for a child's relative
    /// paths: the parent's pkg store dir for local deps of non-linked parents,
    /// the parent's source dir for linked source code, the parent's save root
    /// for registry deps.
    pub fn where_to_find_child_node(&self, spec: &str, parent: &Node) -> PathBuf {
        if is_local_dependency(spec) {
            if parent.data.is_link || parent.data.ohpa_type != OhpaType::SourceCode {
                parent.data.resolve_pkg_store_dir(&self.project_root)
            } else {
                PathBuf::from(&parent.data.fetch_spec)
            }
        } else {
            parent.data.resolve_save_root(&self.project_root)
        }
    }

    /// `flatGraph` — all registered nodes (deduped by `name@fetchSpec`).
    pub fn flat_graph(&self) -> Vec<Arc<Node>> {
        self.rough.values().flat_map(|m| m.values()).cloned().collect()
    }

    /// `pickMaxVersion` — the max-satisfying node for a name.
    pub fn pick_max_version(&self, name: &str) -> Result<Arc<Node>> {
        self.final_dep_cache
            .get(name)
            .cloned()
            .ok_or_else(|| OhpmError::new("DepNodeMaxVersionNotFound", format!("The max version of dependency \"{name}\" not found in the dependency final graph")))
    }

    /// The names in the max-satisfying cache.
    pub fn max_satisfying_names(&self) -> Vec<String> {
        self.max_satisfying_cache.keys().cloned().collect()
    }

    /// `pickNode` — locate a requirement's node in the rough cache (the
    /// symlink phase resolves each requirement edge back to its node).
    pub fn pick_node(&self, name: &str, spec: &str, parent: &Node) -> Result<Arc<Node>> {
        let where_dir = self.where_to_find_child_node(spec, parent);
        let parsed = crate::install::spec::parse_dependency(&format!("{name}@{spec}"), &where_dir)
            .map_err(|_| OhpmError::dep_node_not_found(name, spec))?;
        self.rough
            .get(name)
            .and_then(|m| m.get(&parsed.fetch_spec))
            .cloned()
            .ok_or_else(|| OhpmError::dep_node_not_found(name, spec))
    }
}

/// A queued build task (a requirement edge of a node).
struct Task {
    module_root_dir: PathBuf,
    root_node: Arc<Node>,
    child_name: String,
    spec: String,
    dep_type: DepType,
    cur_node: Arc<Node>,
    cur_depth: usize,
}

/// `AsyncGraphBuilder.build` + `ConcurrentExecutor.runWithErrorHandle` —
/// build the graph with `max_concurrent` workers over a shared queue, early
/// stop on the first error.
pub async fn build_graphs(
    resolver: &Arc<Resolver>,
    roots: Vec<(PathBuf, Arc<Node>)>,
    max_concurrent: usize,
    retry_times: u32,
    retry_interval_ms: u64,
    project: Option<&crate::install::modules::ProjectBuildProfile>,
) -> Result<DependencyGraph> {
    let project_root = resolver.project_root.clone();
    let graph = Arc::new(Mutex::new(DependencyGraph::new(project_root, roots.clone())));
    let queue: Arc<Mutex<VecDeque<Task>>> = Arc::new(Mutex::new(VecDeque::new()));
    let error_flag = Arc::new(AtomicBool::new(false));
    let first_error: Arc<Mutex<Option<OhpmError>>> = Arc::new(Mutex::new(None));

    // Seed: register the roots and enqueue their requirements.
    {
        let mut g = graph.lock().await;
        for (module_root_dir, root) in &roots {
            g.register_node(root.clone())?;
            for (child_name, req) in &root.requirements {
                queue.lock().await.push_back(Task {
                    module_root_dir: module_root_dir.clone(),
                    root_node: root.clone(),
                    child_name: child_name.clone(),
                    spec: req.spec.clone(),
                    dep_type: req.dep_type,
                    cur_node: root.clone(),
                    cur_depth: 0,
                });
            }
        }
    }

    let mut workers = tokio::task::JoinSet::new();
    for _ in 0..max_concurrent.max(1) {
        let graph = graph.clone();
        let queue = queue.clone();
        let error_flag = error_flag.clone();
        let first_error = first_error.clone();
        let resolver = Arc::clone(resolver);
        let project = project.cloned();
        workers.spawn(async move {
            loop {
                if error_flag.load(Ordering::SeqCst) {
                    return;
                }
                let task = queue.lock().await.pop_front();
                let Some(task) = task else { return };
                if let Err(e) = run_task(&graph, &resolver, &queue, task, retry_times, retry_interval_ms, project.as_ref()).await {
                    error_flag.store(true, Ordering::SeqCst);
                    let mut guard = first_error.lock().await;
                    if guard.is_none() {
                        *guard = Some(e);
                    }
                    return;
                }
            }
        });
    }
    while let Some(res) = workers.join_next().await {
        let _ = res;
    }

    let error = first_error.lock().await.take();
    if let Some(e) = error {
        return Err(e);
    }
    let g = graph.lock().await;
    Ok((*g).clone())
}

/// One worker task: `createChildNode` + the dfs continuation.
async fn run_task(
    graph: &Arc<Mutex<DependencyGraph>>,
    resolver: &Arc<Resolver>,
    queue: &Arc<Mutex<VecDeque<Task>>>,
    task: Task,
    retry_times: u32,
    retry_interval_ms: u64,
    project: Option<&crate::install::modules::ProjectBuildProfile>,
) -> Result<()> {
    let where_dir = {
        let g = graph.lock().await;
        g.where_to_find_child_node(&task.spec, &task.cur_node)
    };
    let resolver = Arc::clone(resolver);
    let module_root_dir = task.module_root_dir.clone();
    let child_name = task.child_name.clone();
    let spec = task.spec.clone();
    let where_dir = where_dir.clone();
    let is_link = task.cur_node.data.is_link;
    let is_shared = task.cur_node.data.is_shared;
    let node_data = with_network_retry(retry_times, retry_interval_ms, move || {
        let resolver = Arc::clone(&resolver);
        let module_root_dir = module_root_dir.clone();
        let child_name = child_name.clone();
        let spec = spec.clone();
        let where_dir = where_dir.clone();
        async move {
            resolver
                .get_dep_node_data(&module_root_dir, &child_name, &spec, &where_dir, is_link, is_shared)
                .await
        }
    })
    .await?;
    // `new DependencyNode(nodeData, depType)` — requirements from the node
    // data's own dependency maps.
    let node = Node::with_requirements(
        node_data.clone(),
        task.dep_type,
        Some(node_data.dev_dependencies.clone()),
        node_data.dynamic_dependencies.clone(),
        node_data.dependencies.clone(),
        project,
    )?;
    dfs(graph, queue, &task, Arc::new(node), task.cur_depth + 1).await
}

/// The `dfs` continuation: invalid-dependency check, unmet rethrow, depth
/// limit, repeat-node dedupe, registration and child enqueue.
async fn dfs(
    graph: &Arc<Mutex<DependencyGraph>>,
    queue: &Arc<Mutex<VecDeque<Task>>>,
    task: &Task,
    child: Arc<Node>,
    depth: usize,
) -> Result<()> {
    if !child.data.is_root && task.root_node.data.name == child.data.name {
        return Err(OhpmError::dep_builder_invalid_dependency(
            &child.data.name,
            &child.data.pinned_spec,
            &task.root_node.data.name,
            &task.root_node.data.version,
        ));
    }
    if child.data.unmet.is_some() {
        return Err(child
            .data
            .unmet
            .clone()
            .unwrap_or_else(OhpmError::dep_builder_build_dependency_node_failed));
    }
    let mut g = graph.lock().await;
    if g.find_node(&child.data.name, &child.data.fetch_spec).is_some() {
        // Repeat node — ignore (the first registration wins).
        return Ok(());
    }
    g.register_node(child.clone())?;
    for (child_name, req) in &child.requirements {
        // `const p = curNode.isRoot ? requirements[childName].depType : curNode.depType`
        let dep_type = if child.data.is_root {
            req.dep_type
        } else {
            child.dep_type
        };
        queue.lock().await.push_back(Task {
            module_root_dir: task.module_root_dir.clone(),
            root_node: task.root_node.clone(),
            child_name: child_name.clone(),
            spec: req.spec.clone(),
            dep_type,
            cur_node: child.clone(),
            cur_depth: depth,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::node::{pkg_store_dir_name, NodeData};
    use crate::install::spec::OhpaType;
    use std::collections::BTreeMap;

    fn node_data(name: &str, version: &str, pinned: &str) -> Arc<NodeData> {
        Arc::new(NodeData {
            name: name.to_string(),
            version: version.to_string(),
            actual_name: name.to_string(),
            pinned_spec: pinned.to_string(),
            save_spec: "^1.0.0".to_string(),
            fetch_spec: if pinned == "latest" { "latest".to_string() } else { pinned.to_string() },
            ohpa_type: OhpaType::Range,
            registry_type: "ohpm".to_string(),
            package_type: None,
            is_root: false,
            is_link: false,
            is_shared: true,
            save_root_dir: pkg_store_dir_name(name, pinned, ""),
            pkg_store_dir: format!("oh_modules/{name}"),
            integrity: None,
            shasum: None,
            resolved: String::new(),
            dependencies: BTreeMap::new(),
            dev_dependencies: BTreeMap::new(),
            dynamic_dependencies: BTreeMap::new(),
            unmet: None,
        })
    }

    fn node(name: &str, version: &str, pinned: &str) -> Arc<Node> {
        Arc::new(Node {
            data: node_data(name, version, pinned),
            dep_type: DepType::Prod,
            requirements: BTreeMap::new(),
        })
    }

    #[test]
    fn register_and_max_version() {
        let mut g = DependencyGraph::new("/proj".into(), Vec::new());
        let low = node("foo", "1.0.0", "1.0.0");
        let high = node("foo", "2.0.0", "2.0.0");
        g.register_node(low.clone()).unwrap();
        g.register_node(high.clone()).unwrap();
        assert_eq!(g.flat_graph().len(), 2); // both in the rough cache
        assert_eq!(g.pick_max_version("foo").unwrap().data.version, "2.0.0");
        // Repeat registration of the same node is deduped by find_node.
        assert!(g.find_node("foo", "1.0.0").is_some());
        assert!(g.find_node("foo", "3.0.0").is_none());
    }

    #[test]
    fn invalid_dep_version_rejected() {
        let mut g = DependencyGraph::new("/proj".into(), Vec::new());
        // The validation only runs when a node of the same name exists.
        g.register_node(node("foo", "1.0.0", "1.0.0")).unwrap();
        let bad = node("foo", "not-a-version", "latest");
        let err = g.register_node(bad).unwrap_err();
        assert_eq!(err.code, "DepBuilderInvalidDepVersion");
    }

    #[test]
    fn where_to_find_child() {
        let root = Node {
            data: Arc::new(NodeData {
                is_root: true,
                is_link: true,
                fetch_spec: "/src".to_string(),
                pkg_store_dir: "/src".to_string(),
                ..(*node_data("root", "1.0.0", "latest")).clone()
            }),
            dep_type: DepType::Prod,
            requirements: BTreeMap::new(),
        };
        let g = DependencyGraph::new("/proj".into(), vec![("/proj".into(), Arc::new(root.clone()))]);
        // Local child of a linked root -> the source dir.
        assert_eq!(
            g.where_to_find_child_node("../lib", &root),
            PathBuf::from("/src")
        );
        // Registry child -> the root's save root (the project root for roots).
        assert_eq!(
            g.where_to_find_child_node("^1.0.0", &root),
            PathBuf::from("/proj")
        );
    }
}
