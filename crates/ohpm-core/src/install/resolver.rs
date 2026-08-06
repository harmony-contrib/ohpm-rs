//! Node-data resolution, mirroring `lib/core/locker/PackageLockerManager.js`
//! (`getDepNodeData` / `buildNodeDataAsync` / `saveInTargetLockFile`),
//! `lib/core/dependency/dep-fetcher/InstallationMetadataFetcher.js` and the
//! metadata fetcher implementations (`RegistryMetaDataFetcherImpl`,
//! `LocalArtifactMetadataFetcherImpl`, `SourceCodeMetadataFetcherImpl`).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{Mutex, Notify};

use crate::config::Config;
use crate::error::{OhpmError, Result};
use crate::install::lockfile::{relative_spec, LockPkg, Locker};
use crate::constants::MY_MODULES;
use crate::install::node::{
    file_path_hash, pkg_store_dir_name, NodeData,
};
use crate::install::packument::{fetch_packument, Dist, Packument, PackumentCache, VersionMeta};
use crate::install::semver::{get_version_by_dist_tags, semver_max_satisfying};
use crate::install::spec::{
    is_local_dependency, is_standard_tag_dependency, parse_dependency, OhpaType, Spec,
};
use crate::package::Manifest;
use crate::registry::RegistryClient;

/// The shared resolver state (mirrors `PackageLockerManager` + the metadata
/// fetchers). One instance per install run.
pub struct Resolver {
    pub client: RegistryClient,
    pub config: Config,
    pub project_root: PathBuf,
    /// The enclosing workspace (None outside one); used by the workspace
    /// protocol.
    pub workspace: Option<crate::workspace::Workspace>,
    /// Local-artifact mtime/hash cache (M3).
    pub mtime_cache: Mutex<crate::install::mtime::MtimeCache>,
    /// Project-level overrides + overrideDependencyMap (M4).
    pub overrides: Option<crate::install::overrides::Overrides>,
    /// Project-level exclusions (M4); the final map accumulates during the
    /// graph build (`useExclusions`).
    pub exclusions: Mutex<Option<crate::install::exclusions::Exclusions>>,
    pub cli_input_names: std::sync::Mutex<Vec<String>>,
    pub packument_cache: PackumentCache,
    /// `needResolveConflict` — conflict resolution is on by default.
    pub resolve_conflict: bool,
    /// `resolve_conflict_strict` — the strict max-version strategy.
    pub strict_mode: bool,
    /// `resolveFailedDepNameSet` — strict-mode resolution failures.
    pub resolve_failed: std::sync::Arc<std::sync::Mutex<BTreeSet<String>>>,
    /// `versionsMap` — the pinned versions collected per name.
    pub versions: Mutex<HashMap<String, BTreeSet<String>>>,
    /// `fetchSpecMap` — the fetch specs (override-aware) collected per name.
    pub fetch_specs: Mutex<HashMap<String, BTreeSet<String>>>,
    /// `maxSatisfyingVersionCache` / `maxSatisfyingVersionLocalCache` — the
    /// max-satisfying node data per name (local deps win on lookup).
    pub max_satisfying: Mutex<HashMap<String, Arc<NodeData>>>,
    pub max_local: Mutex<HashMap<String, Arc<NodeData>>>,
    /// The lockfile file name (`getLockFileName` — target mode changes it).
    pub lock_name: String,
    lockers: Mutex<HashMap<PathBuf, Locker>>,
    de_dupe_cache: Mutex<HashMap<String, Arc<NodeData>>>,
    in_flight: Mutex<HashMap<String, Arc<InFlight>>>,
}

/// `nodeDataPromiseCache` — in-flight build dedup (the pacquet/pnpm pattern:
/// waiters block on a `Notify` instead of re-building).
struct InFlight {
    /// The first caller builds; `AtomicBool` must default to `true` (a
    /// `#[derive(Default)]` would make everyone a waiter and deadlock).
    first: AtomicBool,
    done: Mutex<Option<Result<Arc<NodeData>>>>,
    notify: Notify,
}

impl Default for InFlight {
    fn default() -> Self {
        InFlight {
            first: AtomicBool::new(true),
            done: Mutex::new(None),
            notify: Notify::new(),
        }
    }
}

impl Resolver {
    pub fn new(
        client: RegistryClient,
        config: Config,
        project_root: PathBuf,
        workspace: Option<crate::workspace::Workspace>,
        parameter: Option<&crate::install::parameter::Parameterization>,
        lock_name: &str,
    ) -> Result<Self> {
        let mtime_cache = Mutex::new(crate::install::mtime::MtimeCache::load(&project_root));
        // The manifest view — parameterized when a parameter file is
        // configured (`ParameterParsingChainManager` in the reference).
        let mut manifest_value = std::fs::read_to_string(project_root.join(crate::constants::MY_PACKAGE_JSON))
            .ok()
            .and_then(|t| json5::from_str::<serde_json::Value>(&t).ok())
            .unwrap_or_default();
        if let Some(p) = parameter {
            let name = manifest_value
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let _ = p.parse_value(&name, &mut manifest_value);
        }
        // `loadProjectLevelOverrides` — project-level overrides +
        // overrideDependencyMap (config errors propagate, like the reference).
        let overrides = crate::install::overrides::Overrides::from_manifest_value(
            Some(&manifest_value),
            &project_root,
        )?;
        let override_keys: std::collections::BTreeSet<String> = overrides
            .as_ref()
            .map(|o| o.override_dep_map.keys())
            .unwrap_or_default();
        // `ExclusionsManager.init` — parse the exclusions field (the conflict
        // check needs the resolved overrideDependencyMap keys).
        let exclusions = crate::install::exclusions::Exclusions::from_manifest_value(
            manifest_value.get("exclusions"),
            &project_root,
            &override_keys,
        )
        .map(Some)?;
        let exclusions = exclusions.filter(|e| !e.is_empty());
        let resolve_conflict = config.get_bool(crate::config::default::types::RESOLVE_CONFLICT)
            || config.get_bool(crate::config::default::types::RESOLVE_CONFLICT_STRICT);
        let strict_mode = config.get_bool(crate::config::default::types::RESOLVE_CONFLICT_STRICT);
        Ok(Resolver {
            client,
            config,
            project_root,
            workspace,
            mtime_cache,
            overrides,
            exclusions: Mutex::new(exclusions),
            cli_input_names: std::sync::Mutex::new(Vec::new()),
            packument_cache: PackumentCache::default(),
            resolve_conflict,
            strict_mode,
            resolve_failed: std::sync::Arc::new(std::sync::Mutex::new(BTreeSet::new())),
            versions: Mutex::new(HashMap::new()),
            fetch_specs: Mutex::new(HashMap::new()),
            max_satisfying: Mutex::new(HashMap::new()),
            max_local: Mutex::new(HashMap::new()),
            lock_name: lock_name.to_string(),
            lockers: Mutex::new(HashMap::new()),
            de_dupe_cache: Mutex::new(HashMap::new()),
            in_flight: Mutex::new(HashMap::new()),
        })
    }

    /// `collectDepVersions` / `collectFetchSpecs` / `updateMaxSatisfyingVersion`
    /// — record a built node in the global per-name sets; the max-satisfying
    /// node is chosen with the configured strategy (local deps get their own
    /// cache that wins on lookup).
    pub async fn collect_node_stats(&self, node: &NodeData) -> Result<()> {
        if node.is_root {
            return Ok(());
        }
        self.versions
            .lock()
            .await
            .entry(node.name.clone())
            .or_default()
            .insert(node.pinned_spec.clone());
        {
            let mut fs = self.fetch_specs.lock().await;
            let entry = fs.entry(node.name.clone()).or_default();
            entry.insert(
                self.overrides
                    .as_ref()
                    .and_then(|o| o.resolve_spec_with_overrides(&node.name))
                    .map(str::to_string)
                    .unwrap_or_else(|| node.fetch_spec.clone()),
            );
        }
        // Strict mode skips names that already failed resolution.
        if self.resolve_failed.lock().unwrap_or_else(|e| e.into_inner()).contains(&node.name) {
            return Ok(());
        }
        let strategy = if self.strict_mode {
            crate::install::version_conflict::Strategy::Strict
        } else {
            crate::install::version_conflict::Strategy::Max
        };
        let is_max = {
            // `getMaxSatisfyingVersionCache` — the merged view, local wins.
            let prev = {
                let local = self.max_local.lock().await.get(&node.name).cloned();
                if local.is_some() {
                    local
                } else {
                    self.max_satisfying.lock().await.get(&node.name).cloned()
                }
            };
            let fs = self.fetch_specs.lock().await.get(&node.name).cloned().unwrap_or_default();
            crate::install::version_conflict::is_max_satisfying(
                strategy,
                node,
                prev.as_deref(),
                &fs,
                &mut self.resolve_failed.lock().unwrap_or_else(|e| e.into_inner()),
            )?
        };
        if is_max {
            if node.ohpa_type == OhpaType::File {
                self.max_local
                    .lock()
                    .await
                    .insert(node.name.clone(), std::sync::Arc::new(node.clone()));
            } else {
                self.max_satisfying
                    .lock()
                    .await
                    .insert(node.name.clone(), std::sync::Arc::new(node.clone()));
            }
        }
        Ok(())
    }

    /// `getMaxSatisfyingVersionData` — the local cache wins on lookup.
    pub async fn max_satisfying_data(&self, name: &str) -> Option<Arc<NodeData>> {
        let local = self.max_local.lock().await.get(name).cloned();
        if local.is_some() {
            return local;
        }
        self.max_satisfying.lock().await.get(name).cloned()
    }

    /// `flushAllLockers` — flush every touched locker, then the mtime cache.
    pub async fn flush_lockers(&self) -> Result<()> {
        let flushed: Vec<Locker> = {
            let lockers = self.lockers.lock().await;
            lockers.values().cloned().collect()
        };
        for mut locker in flushed {
            locker.flush()?;
        }
        self.mtime_cache.lock().await.save()?;
        Ok(())
    }

    /// `PackageLockerManager.deleteSpecifier` — remove a specifier key of the
    /// module's locker (used by the update command).
    pub async fn delete_specifier(&self, root_dir: &Path, key: &str) {
        self.with_locker(root_dir, |locker| {
            locker.delete_specifier(key);
        })
        .await;
    }

    /// `PackageLockerManager.clearSpecifiers` — clear the module's specifiers
    /// (used by `update --all`).
    pub async fn clear_specifiers(&self, root_dir: &Path) {
        self.with_locker(root_dir, |locker| {
            locker.clear_specifiers();
        })
        .await;
    }

    /// `syncLoadLockers` — eagerly load the lockers for the module roots so an
    /// empty graph still flushes (update/uninstall with no remaining deps).
    pub async fn ensure_lockers(&self, roots: &[PathBuf]) {
        let mut lockers = self.lockers.lock().await;
        for root in roots {
            lockers.entry(root.clone()).or_insert_with(|| {
                Locker::load_with_lock_name(root, &self.lock_name)
            });
        }
    }

    /// Run `f` with the (mutable) locker for `root_dir`.
    pub async fn with_locker<T>(&self, root_dir: &Path, f: impl FnOnce(&mut Locker) -> T) -> T {
        let mut lockers = self.lockers.lock().await;
        let locker = lockers
            .entry(root_dir.to_path_buf())
            .or_insert_with(|| Locker::load_with_lock_name(root_dir, &self.lock_name));
        f(locker)
    }

    /// `PackageLockerManager.getDepNodeData` — parse, dedupe by `name@fetchSpec`
    /// (with in-flight dedup), build if needed, then write the lockfile
    /// entries (`saveInTargetLockFile`).
    pub async fn get_dep_node_data(
        &self,
        root_dir: &Path,
        name: &str,
        spec: &str,
        where_dir: &Path,
        is_link: bool,
        is_shared: bool,
    ) -> Result<Arc<NodeData>> {
        let parsed = parse_dependency(&format!("{name}@{spec}"), where_dir)?;
        let key = format!("{}@{}", parsed.name, parsed.fetch_spec);
        let root_dir = root_dir.to_path_buf();
        let name = name.to_string();
        let spec = spec.to_string();
        let where_dir = where_dir.to_path_buf();
        let parsed_for_build = parsed.clone();
        let root_dir_for_build = root_dir.clone();
        let node = self
            .build_or_wait(&key, async move {
                let mut node = self
                    .build_node_data(
                        &root_dir_for_build,
                        &name,
                        &spec,
                        &where_dir,
                        is_link,
                        is_shared,
                        &parsed_for_build,
                    )
                    .await?;
                self.apply_override_and_exclusions(&mut node).await;
                self.collect_node_stats(&node).await?;
                Ok(node)
            })
            .await?;
        self.save_in_target_lock_file(&root_dir, &parsed, &node).await;
        Ok(node)
    }

    /// `resolveVersionConflict2LockFile` — for every rough dep of the graph,
    /// rewrite the module locker's specifier and package entries so they point
    /// at the max-satisfying version of the name.
    pub async fn resolve_conflict_to_lockfile(
        &self,
        module_root: &Path,
        graph: &crate::install::graph::DependencyGraph,
    ) -> Result<()> {
        for node in graph.rough_nodes() {
            if node.data.is_root {
                continue;
            }
            // Workspace members never appear in the lockfile packages map.
            if node.data.registry_type == "workspace" {
                continue;
            }
            let max = self
                .max_satisfying_data(&node.data.name)
                .await
                .ok_or_else(|| {
                    OhpmError::new(
                        "NotFoundMaxVersionError",
                        format!(
                            "The max version of dependency \"{}@{}\" not found.",
                            node.data.name, node.data.pinned_spec
                        ),
                    )
                })?;
            // `l` — local deps key the specifier by fetch spec, others by save
            // spec; the lookup is relative to the module root.
            let spec = if node.data.registry_type == "local" {
                node.data.fetch_spec.clone()
            } else {
                node.data.save_spec.clone()
            };
            let rel_spec = relative_spec(module_root, &spec);
            let locked = self
                .with_locker(module_root, |locker| locker.get_lock_spec(&node.data.name, &rel_spec))
                .await;
            if let Some(old_value) = locked {
                let rel_max = relative_spec(module_root, &max.pinned_spec);
                self.with_locker(module_root, |locker| {
                    locker.delete_package(&old_value);
                    locker.update_lock_spec_named(
                        &node.data.name,
                        &max.name,
                        &rel_spec,
                        &rel_max,
                    );
                    locker.update_lock_pkg(&max.name, &rel_max, LockPkg::from_node_data(&max));
                })
                .await;
            }
        }
        Ok(())
    }

    /// `AsyncGraphBuilder.createChildNode` — the overrideDependencyMap mask
    /// (`getConfig`) + `ExclusionsManager.useExclusions`, applied once per
    /// built node: the override entry replaces the node's own maps
    /// (`maskedDeps`), exclusions remove the listed deps from the effective
    /// maps (mutating the node's own maps when there is no override, so the
    /// lockfile packages reflect them) and accumulate the install-record
    /// final map; `maskedByOverrideDependencyMap` is set on the node data.
    async fn apply_override_and_exclusions(&self, node: &mut Arc<NodeData>) {
        let Some(data) = Arc::get_mut(node) else {
            return;
        };
        if data.is_root {
            return;
        }
        // `getVersionJudgedWithTagAndLocal` — slashed local for exclusions,
        // unslashed for the overrideDependencyMap lookup.
        let version_key = crate::install::spec::version_judged_with_tag_and_local(
            &data.fetch_spec,
            &data.pinned_spec,
            true,
        );
        let lookup_key = crate::install::spec::version_judged_with_tag_and_local(
            &data.fetch_spec,
            &data.pinned_spec,
            false,
        );
        let override_entry = self
            .overrides
            .as_ref()
            .and_then(|o| o.override_dep_map.get(&data.name, &lookup_key))
            .cloned();
        let mut masked = override_entry.is_some();
        let mut effective = match &override_entry {
            Some(e) => crate::install::exclusions::EffectiveDeps {
                dependencies: e.dependencies.clone(),
                dynamic_dependencies: e.dynamic_dependencies.clone(),
            },
            None => crate::install::exclusions::EffectiveDeps::from_node(data),
        };
        let project_root = self.project_root.clone();
        let mut exclusions = self.exclusions.lock().await;
        if let Some(ex) = exclusions.as_mut() {
            if ex.use_exclusions(data, &version_key, &mut effective, &project_root) {
                masked = true;
            }
            // `!s.name || r` — a SourceCode node masked by an override still
            // records its config in the final map.
            if data.ohpa_type == OhpaType::SourceCode && override_entry.is_some() {
                ex.add_to_final_map(&data.name, &version_key, &effective, &project_root);
            }
        }
        data.masked_by_override_dependency_map = masked;
        match override_entry {
            Some(entry) => {
                // The mask replaces the node's own maps (post-exclusion).
                data.masked_deps = Some(crate::install::node::MaskedDeps {
                    dependencies: effective.dependencies,
                    dev_dependencies: entry.dev_dependencies,
                    dynamic_dependencies: effective.dynamic_dependencies,
                });
            }
            None => {
                // The reference mutates the shared nodeData maps in place.
                data.dependencies = effective.dependencies;
                data.dynamic_dependencies = effective.dynamic_dependencies;
            }
        }
    }

    /// In-flight + completed dedup keyed `name@fetchSpec`.
    async fn build_or_wait(
        &self,
        key: &str,
        build: impl std::future::Future<Output = Result<Arc<NodeData>>>,
    ) -> Result<Arc<NodeData>> {
        if let Some(node) = self.de_dupe_cache.lock().await.get(key) {
            return Ok(node.clone());
        }
        let cell = self
            .in_flight
            .lock()
            .await
            .entry(key.to_string())
            .or_default()
            .clone();
        if cell.first.swap(false, Ordering::SeqCst) {
            let result = build.await;
            if let Ok(node) = &result {
                self.de_dupe_cache
                    .lock()
                    .await
                    .insert(key.to_string(), node.clone());
            }
            *cell.done.lock().await = Some(result);
            cell.notify.notify_waiters();
            self.in_flight.lock().await.remove(key);
            return match cell.done.lock().await.as_ref() {
                Some(Ok(node)) => Ok(node.clone()),
                Some(Err(e)) => Err(OhpmError::new(e.code, e.message.clone())),
                None => unreachable!("first builder stores its result"),
            };
        }
        loop {
            let done = cell.done.lock().await;
            if let Some(result) = done.as_ref() {
                return match result {
                    Ok(node) => Ok(node.clone()),
                    Err(e) => Err(OhpmError::new(e.code, e.message.clone())),
                };
            }
            drop(done);
            cell.notify.notified().await;
        }
    }

    /// `AsyncDepNodeDataBuilder.build` + the metadata fetchers — the resolution
    /// chain. Resolution failures become unmet node data (the graph build then
    /// rethrows them), matching the reference's `unMetDepNodeData`.
    async fn build_node_data(
        &self,
        root_dir: &Path,
        name: &str,
        spec: &str,
        where_dir: &Path,
        is_link: bool,
        is_shared: bool,
        original: &Spec,
    ) -> Result<Arc<NodeData>> {
        let result = self
            .build_node_data_inner(root_dir, name, spec, where_dir, is_link, is_shared, original)
            .await;
        match result {
            Ok(node) => Ok(node),
            Err(e) => Ok(Arc::new(NodeData {
                name: name.to_string(),
                declared_name: String::new(),
                version: String::new(),
                actual_name: String::new(),
                pinned_spec: String::new(),
                save_spec: String::new(),
                fetch_spec: original.fetch_spec.clone(),
                ohpa_type: original.ohpa_type,
                registry_type: String::new(),
                package_type: None,
                is_root: false,
                is_link,
                is_shared,
                save_root_dir: String::new(),
                pkg_store_dir: String::new(),
                integrity: None,
                shasum: None,
                resolved: String::new(),
                dependencies: BTreeMap::new(),
                dev_dependencies: BTreeMap::new(),
                dynamic_dependencies: BTreeMap::new(),
                unmet: Some(e),
                masked_by_override_dependency_map: false,
                masked_deps: None,
            })),
        }
    }

    async fn build_node_data_inner(
        &self,
        root_dir: &Path,
        name: &str,
        _spec: &str,
        where_dir: &Path,
        is_link: bool,
        is_shared: bool,
        original: &Spec,
    ) -> Result<Arc<NodeData>> {
        // `InstallationMetadataFetcher.fetch` — lockfile-first. The resolution
        // spec is the override when one matches (`resolveSpecWithOverrides`).
        let mut parsed = original.clone();
        if let Some(override_spec) = self
            .overrides
            .as_ref()
            .and_then(|o| o.resolve_spec_with_overrides(name))
        {
            parsed = parse_dependency(&format!("{name}@{override_spec}"), where_dir)?;
        }
        let lock_value = self
            .with_locker(root_dir, |locker| {
                locker.get_lock_spec(name, &relative_spec(root_dir, &original.fetch_spec))
            })
            .await;
        if let Some(value) = &lock_value {
            // Protocol specs resolve via their own fetch arms: the lockfile
            // value re-parse is only meaningful for registry deps (a pinned
            // git SHA re-parses as a Tag or SourceCode and would mis-dispatch
            // the fall-through).
            if !matches!(
                original.ohpa_type,
                OhpaType::Git | OhpaType::Workspace | OhpaType::File | OhpaType::SourceCode
            ) {
                parsed = parse_dependency(value, where_dir)?;
            }
            if let Some(mut pack) = self
                .try_lockfile_packument(root_dir, name, &original.fetch_spec, original, &parsed)
                .await?
            {
                // `checkAndReassignResolvedField` — re-fetch when the locked
                // entry lacks resolved/integrity. Registry deps only: git
                // entries carry no integrity (they are pinned by commit) and
                // workspace entries resolve to member dirs.
                let needs_reassign = matches!(
                    original.ohpa_type,
                    OhpaType::Range | OhpaType::Version | OhpaType::Tag | OhpaType::Alias
                ) && pack
                    .versions
                    .values()
                    .next()
                    .is_some_and(|v| v.resolved.is_empty() || v.integrity.is_none());
                if needs_reassign {
                    self.with_locker(root_dir, |locker| {
                        locker.delete_specifier(&format!(
                            "{name}@{}",
                            relative_spec(root_dir, &original.fetch_spec)
                        ));
                    })
                    .await;
                    let fresh = self
                        .fetch_with_implementor(root_dir, name, where_dir, &parsed)
                        .await?;
                    let first = pack.versions.keys().next().cloned().unwrap_or_default();
                    if let Some(meta) = pack.versions.get_mut(&first) {
                        if let Some(fresh_meta) = fresh.versions.get(&first) {
                            if let Some(dist) = &fresh_meta.dist {
                                meta.resolved = dist.tarball.clone();
                                meta.integrity = dist.integrity.clone();
                            }
                        }
                    }
                    pack.actual_name = fresh.actual_name.clone();
                }
                return self
                    .build_dep_node_data(name, original, &parsed, &pack, is_link, is_shared)
                    .await;
            }
        }
        let packument = self
            .fetch_with_implementor(root_dir, name, where_dir, &parsed)
            .await?;
        self.build_dep_node_data(name, original, &parsed, &packument, is_link, is_shared).await
    }

    /// `ohpm:` alias — resolve the TARGET package; the alias key is handled by
    /// `build_dep_node_data` (declared_name) and the lockfile writer.
    async fn fetch_alias_dep(&self, parsed: &Spec) -> Result<Packument> {
        let target = parsed
            .alias_target
            .as_deref()
            .ok_or_else(|| OhpmError::alias_pkg_invalid(&parsed.raw))?;
        let inner = &parsed.fetch_spec[crate::constants::ALIAS_PREFIX.len()..];
        let sep = if inner.starts_with('@') {
            inner[1..].find('@').map(|i| i + 1)
        } else {
            inner.find('@')
        }
        .unwrap_or(0);
        let inner_spec = if sep > 0 { &inner[sep + 1..] } else { "" };
        fetch_packument(
            &self.client,
            &self.config,
            target,
            inner_spec,
            &self.packument_cache,
        )
        .await
    }

    /// `workspace:` — resolve against the `ohpm-workspace.yaml` members.
    fn fetch_workspace_dep(&self, name: &str, parsed: &Spec) -> Result<Packument> {
        let spec = parsed.fetch_spec.clone();
        let query = crate::workspace::WorkspaceQuery::parse(&spec);
        let ws = self
            .workspace
            .as_ref()
            .ok_or_else(|| OhpmError::workspace_pkg_not_found(name, &spec))?;
        // Path forms resolve by directory; everything else by member name.
        let member = match &query {
            crate::workspace::WorkspaceQuery::Path(p) => {
                let resolved = crate::workspace::resolve_file_spec(
                    &self.project_root,
                    &format!("file:{p}"),
                );
                ws.members
                    .iter()
                    .find(|m| m.dir == resolved)
                    .ok_or_else(|| OhpmError::workspace_pkg_not_found(name, &spec))?
            }
            crate::workspace::WorkspaceQuery::Alias { member, .. } => ws
                .members_by_name()
                .get(member.as_str())
                .copied()
                .ok_or_else(|| OhpmError::workspace_pkg_not_found(member, &spec))?,
            _ => ws
                .members_by_name()
                .get(name)
                .copied()
                .ok_or_else(|| OhpmError::workspace_pkg_not_found(name, &spec))?,
        };
        let version = crate::workspace::resolve_workspace_version(&query, &member.manifest.version, name, &spec)?;
        let meta = VersionMeta {
            name: member.manifest.name.clone(),
            version: version.clone(),
            package_type: member.manifest.package_type.clone(),
            dependencies: member.manifest.dependencies.clone(),
            dev_dependencies: member.manifest.dev_dependencies.clone(),
            dynamic_dependencies: member.manifest.dynamic_dependencies.clone(),
            resolved: member.dir.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let mut versions = BTreeMap::new();
        versions.insert(version.clone(), meta);
        Ok(Packument {
            name: name.to_string(),
            actual_name: member.manifest.name.clone(),
            dist_tags: BTreeMap::new(),
            versions,
            registry_type: "workspace".to_string(),
            is_from_lock_file: false,
            package_type: member.manifest.package_type.clone(),
            package_type_header: None,
        })
    }

    /// Git spec — clone (or reuse the locked commit), materialize into the
    /// store, and read the manifest from the checkout.
    async fn fetch_git_dep(&self, root_dir: &Path, name: &str, parsed: &Spec) -> Result<Packument> {
        let spec = parsed.fetch_spec.clone();
        let query = crate::install::git::parse_git_spec(&spec)?;
        let url = spec.split('#').next().unwrap_or(&spec).to_string();

        // The lockfile may already pin the commit (re-install, no ls-remote).
        let locked = self
            .with_locker(root_dir, |locker| {
                locker.get_lock_spec(name, &relative_spec(root_dir, &spec))
            })
            .await;
        let (commit, manifest) = match locked {
            Some(value) => {
                let (_, commit) = crate::install::lockfile::parse_spec_key(&value)
                    .map_err(|_| OhpmError::locker_invalid_specifier(&value))?;
                let store_dir = self
                    .project_root
                    .join(MY_MODULES)
                    .join(".ohpm")
                    .join(pkg_store_dir_name(name, &commit, ""));
                let pkg_store = store_dir.join("oh_modules").join(name);
                if !pkg_store.join(crate::constants::MY_PACKAGE_JSON).is_file() {
                    // Store missing (crash between phases) — re-materialize
                    // the pinned commit.
                    let sub = crate::install::git::sub_dir_of(&query);
                    let tmp = self.project_root.join(MY_MODULES).join(".tmp").join(format!("git-{}", uuid::Uuid::new_v4().simple()));
                    std::fs::create_dir_all(tmp.parent().unwrap())?;
                    let repo = crate::install::git::fetch_repo(&url, &tmp)?;
                    crate::install::git::materialize_commit_in(&repo, &commit, sub, &pkg_store)?;
                    let _ = std::fs::remove_dir_all(&tmp);
                }
                let manifest = read_manifest_from_dir(&pkg_store)?;
                (commit, manifest)
            }
            None => {
                // Fresh resolution: clone once, resolve the ref locally, and
                // materialize the pinned commit's tree into the store.
                let tmp = self.project_root.join(MY_MODULES).join(".tmp").join(format!("git-{}", uuid::Uuid::new_v4().simple()));
                std::fs::create_dir_all(tmp.parent().unwrap())?;
                let repo = crate::install::git::fetch_repo(&url, &tmp)?;
                let commit = crate::install::git::resolve_commit_in(&repo, &query)?;
                let sub = crate::install::git::sub_dir_of(&query);
                let commit_key = match sub {
                    Some(s) => format!("{commit}&path:{s}"),
                    None => commit.clone(),
                };
                let store_dir = self
                    .project_root
                    .join(MY_MODULES)
                    .join(".ohpm")
                    .join(pkg_store_dir_name(name, &commit_key, ""));
                let pkg_store = store_dir.join("oh_modules").join(name);
                crate::install::git::materialize_commit_in(&repo, &commit, sub, &pkg_store)?;
                let _ = std::fs::remove_dir_all(&tmp);
                let manifest = read_manifest_from_dir(&pkg_store)?;
                (commit_key, manifest)
            }
        };
        let meta = VersionMeta {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            package_type: manifest.package_type.clone(),
            dependencies: manifest.dependencies.clone(),
            dev_dependencies: manifest.dev_dependencies.clone(),
            dynamic_dependencies: manifest.dynamic_dependencies.clone(),
            resolved: format!("{url}#{commit}"),
            ..Default::default()
        };
        let mut versions = BTreeMap::new();
        versions.insert(commit.clone(), meta);
        Ok(Packument {
            name: name.to_string(),
            actual_name: manifest.name,
            dist_tags: BTreeMap::new(),
            versions,
            registry_type: "git".to_string(),
            is_from_lock_file: false,
            package_type: manifest.package_type.clone(),
            package_type_header: None,
        })
    }

    /// `tryFetchFromLockFile` — build a lockfile-derived packument. Dispatch
    /// on the ORIGINAL spec type (a git value `foo@<sha>` re-parses as a Tag
    /// and a workspace value `foo@1.2.3` as a Version — both would otherwise
    /// fall into the registry branch and miss the lock package).
    async fn try_lockfile_packument(
        &self,
        root_dir: &Path,
        name: &str,
        fetch_spec: &str,
        original: &Spec,
        _parsed: &Spec,
    ) -> Result<Option<Packument>> {
        match original.ohpa_type {
            OhpaType::Range | OhpaType::Version | OhpaType::Tag | OhpaType::Alias => {
                let Some((value, mut lock_pkg)) = self
                    .with_locker(root_dir, |locker| {
                        locker.get_lock_pkg(name, &relative_spec(root_dir, fetch_spec))
                    })
                    .await
                else {
                    return Ok(None);
                };
                let (_, version) = crate::install::lockfile::parse_spec_key(&value)
                    .map_err(|_| OhpmError::locker_invalid_specifier(&value))?;
                if version.is_empty() {
                    return Err(OhpmError::locker_invalid_specifier(&value));
                }
                if lock_pkg.name.is_empty() {
                    lock_pkg.name = name.to_string();
                }
                if lock_pkg.version.is_empty() {
                    lock_pkg.version = version.clone();
                }
                let meta = VersionMeta::from_lock_pkg(&lock_pkg);
                let mut versions = BTreeMap::new();
                versions.insert(version.clone(), meta);
                Ok(Some(Packument {
                    name: name.to_string(),
                    actual_name: lock_pkg.name,
                    dist_tags: BTreeMap::new(),
                    versions,
                    registry_type: lock_pkg.registry_type,
                    is_from_lock_file: true,
                    package_type: lock_pkg.package_type.clone(),
                    package_type_header: None,
                }))
            }
            OhpaType::Git => {
                let Some((value, _lock_pkg)) = self
                    .with_locker(root_dir, |locker| {
                        locker.get_lock_pkg(name, &relative_spec(root_dir, fetch_spec))
                    })
                    .await
                else {
                    return Ok(None);
                };
                let (_, commit) = crate::install::lockfile::parse_spec_key(&value)
                    .map_err(|_| OhpmError::locker_invalid_specifier(&value))?;
                // The store dir holds the materialized checkout; read the
                // manifest from it (no network on re-install).
                let store_root = self
                    .project_root
                    .join(MY_MODULES)
                    .join(".ohpm");
                let store_dir = store_root.join(pkg_store_dir_name(name, &commit, ""));
                let pkg_store = store_dir.join("oh_modules").join(name);
                if !pkg_store.join(crate::constants::MY_PACKAGE_JSON).is_file() {
                    return Ok(None); // store missing — re-materialize below
                }
                let manifest = read_manifest_from_dir(&pkg_store)?;
                let meta = VersionMeta {
                    name: manifest.name.clone(),
                    version: manifest.version.clone(),
                    package_type: manifest.package_type.clone(),
                    dependencies: manifest.dependencies.clone(),
                    dev_dependencies: manifest.dev_dependencies.clone(),
                    dynamic_dependencies: manifest.dynamic_dependencies.clone(),
                    resolved: format!("{}#{}", fetch_spec.split('#').next().unwrap_or(fetch_spec), commit),
                    ..Default::default()
                };
                let mut versions = BTreeMap::new();
                versions.insert(commit.clone(), meta);
                Ok(Some(Packument {
                    name: name.to_string(),
                    actual_name: manifest.name,
                    dist_tags: BTreeMap::new(),
                    versions,
                    registry_type: "git".to_string(),
                    is_from_lock_file: true,
                    package_type: manifest.package_type.clone(),
                    package_type_header: None,
                }))
            }
            OhpaType::File => {
                // `tryFetchFileResultFromLockPkg` — the lockfile entry is
                // usable only when the artifact's mtime still matches the
                // cached value (the content hash is then trusted).
                let Some((value, lock_pkg)) = self
                    .with_locker(root_dir, |locker| {
                        locker.get_lock_pkg(name, &relative_spec(root_dir, fetch_spec))
                    })
                    .await
                else {
                    return Ok(None);
                };
                let (_, version) = crate::install::lockfile::parse_spec_key(&value)
                    .map_err(|_| OhpmError::locker_invalid_specifier(&value))?;
                let path = Path::new(&version);
                let cache = self.mtime_cache.lock().await;
                let mtime_ok = cache.get_mtime(path) == Some(crate::install::mtime::read_modify_time(path).as_str());
                let hash_ok = cache.get_hash(path).is_some();
                if !(mtime_ok && hash_ok && !lock_pkg.name.is_empty() && !lock_pkg.version.is_empty()) {
                    return Ok(None);
                }
                let meta = VersionMeta::from_lock_pkg(&lock_pkg);
                let mut versions = BTreeMap::new();
                versions.insert(version.clone(), meta);
                Ok(Some(Packument {
                    name: name.to_string(),
                    actual_name: lock_pkg.name,
                    dist_tags: BTreeMap::new(),
                    versions,
                    registry_type: "local".to_string(),
                    is_from_lock_file: true,
                    package_type: lock_pkg.package_type.clone(),
                    package_type_header: None,
                }))
            }
            OhpaType::SourceCode | OhpaType::Workspace => Ok(None),
        }
    }

    /// `fetchWithImplementor` — dispatch by spec type.
    async fn fetch_with_implementor(
        &self,
        root_dir: &Path,
        name: &str,
        where_dir: &Path,
        parsed: &Spec,
    ) -> Result<Packument> {
        match parsed.ohpa_type {
            OhpaType::Version | OhpaType::Range | OhpaType::Tag => {
                fetch_packument(
                    &self.client,
                    &self.config,
                    name,
                    &parsed.fetch_spec,
                    &self.packument_cache,
                )
                .await
            }
            OhpaType::File => self.fetch_local_artifact(name, parsed).await,
            OhpaType::SourceCode => {
                self.fetch_source_code(name, parsed, where_dir).await
            }
            OhpaType::Git => self.fetch_git_dep(root_dir, name, parsed).await,
            OhpaType::Alias => self.fetch_alias_dep(parsed).await,
            OhpaType::Workspace => self.fetch_workspace_dep(name, parsed),
        }
    }

    /// `LocalArtifactMetadataFetcherImpl.fetch` — read the manifest from a
    /// local .har/.tgz.
    async fn fetch_local_artifact(&self, name: &str, parsed: &Spec) -> Result<Packument> {
        let path = PathBuf::from(&parsed.fetch_spec);
        if !path.exists() {
            return Err(OhpmError::fetcher_local_artifact_fetch_local_package_failed(&path));
        }
        let manifest = read_manifest_from_tar(&path)?;
        if manifest.name.is_empty() {
            return Err(OhpmError::install_field_is_empty("name"));
        }
        // The local metadata carries `resolved` as a plain field (no `dist`).
        let meta = VersionMeta {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            package_type: manifest.package_type.clone(),
            dependencies: manifest.dependencies.clone(),
            dev_dependencies: manifest.dev_dependencies.clone(),
            dynamic_dependencies: manifest.dynamic_dependencies.clone(),
            resolved: parsed.fetch_spec.clone(),
            ..Default::default()
        };
        let mut versions = BTreeMap::new();
        versions.insert(parsed.fetch_spec.clone(), meta);
        Ok(Packument {
            name: name.to_string(),
            actual_name: manifest.name,
            dist_tags: BTreeMap::new(),
            versions,
            registry_type: "local".to_string(),
            is_from_lock_file: false,
            package_type: manifest.package_type.clone(),
            package_type_header: None,
        })
    }

    /// `SourceCodeMetadataFetcherImpl.fetch` — read the manifest from a local
    /// directory.
    async fn fetch_source_code(&self, name: &str, parsed: &Spec, where_dir: &Path) -> Result<Packument> {
        let path = crate::workspace::resolve_file_spec(where_dir, &format!("file:{}", parsed.fetch_spec));
        if !path.exists() {
            return Err(OhpmError::fetcher_source_code_fetch_failed(&path));
        }
        let manifest = read_manifest_from_dir(&path)?;
        if manifest.name.is_empty() {
            return Err(OhpmError::install_field_is_empty("name"));
        }
        // The source-code metadata carries `dist: {tarball: fetchSpec}`.
        let meta = VersionMeta {
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            package_type: manifest.package_type.clone(),
            dependencies: manifest.dependencies.clone(),
            dev_dependencies: manifest.dev_dependencies.clone(),
            dynamic_dependencies: manifest.dynamic_dependencies.clone(),
            resolved: parsed.fetch_spec.clone(),
            dist: Some(Dist {
                tarball: parsed.fetch_spec.clone(),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut versions = BTreeMap::new();
        versions.insert(parsed.fetch_spec.clone(), meta);
        Ok(Packument {
            name: name.to_string(),
            actual_name: manifest.name,
            dist_tags: BTreeMap::new(),
            versions,
            registry_type: "local".to_string(),
            is_from_lock_file: true,
            package_type: manifest.package_type.clone(),
            package_type_header: None,
        })
    }

    /// `getPinnedVersion` + `resolveSaveSpec` + `getLockPkgFromMetaData` +
    /// the artifact/source-code dep builders.
    async fn build_dep_node_data(
        &self,
        name: &str,
        original: &Spec,
        parsed: &Spec,
        packument: &Packument,
        is_link: bool,
        is_shared: bool,
    ) -> Result<Arc<NodeData>> {
        let _ = parsed; // the declared spec is re-derived per type below
        // Alias: resolution uses the TARGET name and inner spec; the declared
        // name stays on the node for specifier keys and symlinks.
        let (resolve_name, resolve_spec) = match original.ohpa_type {
            OhpaType::Alias => {
                let target = original.alias_target.clone().unwrap_or_else(|| name.to_string());
                let inner = &original.fetch_spec[crate::constants::ALIAS_PREFIX.len()..];
                let sep = if inner.starts_with('@') {
                    inner[1..].find('@').map(|i| i + 1)
                } else {
                    inner.find('@')
                }
                .unwrap_or(0);
                let inner_spec = if sep > 0 { &inner[sep + 1..] } else { "" };
                (target, inner_spec.to_string())
            }
            _ => (name.to_string(), parsed.fetch_spec.clone()),
        };
        let pinned = get_pinned_version(&resolve_name, &resolve_spec, packument)?;
        // `resolveSaveSpec` — "latest" spec without "latest" in the original
        // pkg string becomes the pinned version.
        let mut save_spec = original.save_spec.clone();
        if parsed.fetch_spec.to_lowercase().contains("latest")
            && !original.raw.to_lowercase().contains("latest")
        {
            save_spec = pinned.clone();
        }
        // `applySpecChangedIfNeeded` — a CLI input without a version gets
        // `^<pinned>` as its save spec (the manifest write uses it).
        if original.raw_spec.is_empty()
            && !is_local_dependency(&pinned)
            && self.cli_input_names.lock().unwrap().contains(&name.to_string())
        {
            save_spec = format!("^{pinned}");
        }
        let meta = packument
            .versions
            .get(&pinned)
            .cloned()
            .unwrap_or_default();
        let lock_pkg = LockPkg::from_meta(&meta, &packument.registry_type);
        // Git pinned commits (`<sha>&path:...`) would fail the tag URI check
        // in a registry re-parse — skip it; alias/workspace use the declared
        // name as the node name for symlinks and lockfile keys.
        let (node_name, pinned_parse) = match original.ohpa_type {
            OhpaType::Git => (name.to_string(), None),
            OhpaType::Workspace => {
                (name.to_string(), None)
            }
            _ => {
                let p = parse_dependency(&format!("{resolve_name}@{pinned}"), &original.where_dir)?;
                (p.name.clone(), Some(p))
            }
        };

        let (save_root_dir, pkg_store_dir) = match original.ohpa_type {
            OhpaType::Workspace => {
                // Link to the member directory (absolute paths replace the
                // base in `resolve_save_root`/`resolve_pkg_store_dir`).
                let dir = meta.resolved.clone();
                (dir.clone(), dir)
            }
            OhpaType::Git => (
                pkg_store_dir_name(&node_name, &pinned, ""),
                format!("oh_modules/{node_name}"),
            ),
            _ => match pinned_parse.as_ref().map(|p| p.ohpa_type).unwrap_or(OhpaType::Range) {
                OhpaType::File => {
                    let fetch = &pinned_parse.as_ref().unwrap().fetch_spec;
                    let store_name = self
                        .mtime_cache
                        .lock()
                        .await
                        .get_file_store_dir_name(&node_name, Path::new(fetch))?;
                    (store_name, format!("oh_modules/{node_name}"))
                }
                OhpaType::SourceCode => {
                    let fetch = &pinned_parse.as_ref().unwrap().fetch_spec;
                    if is_link {
                        (fetch.clone(), fetch.clone())
                    } else {
                        let hash = file_path_hash(fetch);
                        (
                            pkg_store_dir_name(&node_name, fetch, &hash),
                            format!("oh_modules/{node_name}"),
                        )
                    }
                }
                _ => (
                    pkg_store_dir_name(&node_name, &pinned, ""),
                    format!("oh_modules/{node_name}"),
                ),
            },
        };

        let version = if lock_pkg.version.is_empty() {
            "0.0.0".to_string()
        } else {
            lock_pkg.version.clone()
        };
        Ok(Arc::new(NodeData {
            name: node_name,
            declared_name: original.name.clone(),
            version,
            actual_name: packument.actual_name.clone(),
            pinned_spec: pinned,
            save_spec,
            fetch_spec: original.fetch_spec.clone(),
            ohpa_type: pinned_parse
                .as_ref()
                .map(|p| p.ohpa_type)
                .unwrap_or(original.ohpa_type),
            registry_type: packument.registry_type.clone(),
            package_type: lock_pkg.package_type.clone(),
            is_root: false,
            is_link,
            is_shared,
            save_root_dir,
            pkg_store_dir,
            integrity: lock_pkg.integrity.clone(),
            shasum: lock_pkg.shasum.clone(),
            resolved: lock_pkg.resolved.clone(),
            dependencies: lock_pkg.dependencies.clone(),
            dev_dependencies: lock_pkg.dev_dependencies.clone(),
            dynamic_dependencies: lock_pkg.dynamic_dependencies.clone(),
            unmet: None,
            masked_by_override_dependency_map: false,
            masked_deps: None,
        }))
    }

    /// `saveInTargetLockFile` + `applySpecChangedIfNeeded` — update the
    /// module's specifier and package entries.
    async fn save_in_target_lock_file(&self, root_dir: &Path, parsed: &Spec, node: &NodeData) {
        if node.unmet.is_some() {
            return;
        }
        // `applySpecChangedIfNeeded` (specifier half) — a CLI input without a
        // version deletes the bare `name@latest` specifier; the `^<pinned>`
        // save-spec rewrite happens in `build_dep_node_data`.
        if parsed.raw_spec.is_empty()
            && !is_local_dependency(&node.pinned_spec)
            && self.cli_input_names.lock().unwrap().contains(&node.name)
        {
            self.with_locker(root_dir, |locker| {
                locker.delete_specifier(&format!(
                    "{}@{}",
                    parsed.name,
                    relative_spec(root_dir, &parsed.fetch_spec)
                ));
            })
            .await;
        }
        // `getVersionByRegistryType` — local deps use the fetch spec.
        let spec = if node.registry_type == "local" {
            node.fetch_spec.clone()
        } else {
            node.save_spec.clone()
        };
        // The specifier key uses the DECLARED name (aliases write
        // `foo@ohpm:bar@^1.0.0`), the package key the real name (`bar@1.2.3`)
        // so the flush-prune keeps the right packages entry.
        let key_name = if node.declared_name.is_empty() {
            &node.name
        } else {
            &node.declared_name
        };
        // Overrides: an exact-version override pins the specifier; any other
        // override deletes the stale one (re-resolved next run).
        if let Some(override_spec) = self
            .overrides
            .as_ref()
            .and_then(|o| o.resolve_spec_with_overrides(&node.name))
        {
            self.with_locker(root_dir, |locker| {
                locker.delete_specifier(&format!(
                    "{key_name}@{}",
                    relative_spec(root_dir, &node.fetch_spec)
                ));
            })
            .await;
            if node_semver::Version::parse(override_spec).is_ok() {
                let mut lockers = self.lockers.lock().await;
                let locker = lockers
                    .entry(root_dir.to_path_buf())
                    .or_insert_with(|| Locker::load_with_lock_name(root_dir, &self.lock_name));
                locker.update_lock_spec_named(
                    key_name,
                    &node.name,
                    &relative_spec(root_dir, override_spec),
                    override_spec,
                );
                if node.registry_type != "workspace" {
                    locker.update_lock_pkg(
                        &node.name,
                        &relative_spec(root_dir, override_spec),
                        LockPkg::from_node_data(node),
                    );
                }
                return;
            }
        }
        let mut lockers = self.lockers.lock().await;
        let locker = lockers
            .entry(root_dir.to_path_buf())
            .or_insert_with(|| Locker::load_with_lock_name(root_dir, &self.lock_name));
        locker.update_lock_spec_named(
            key_name,
            &node.name,
            &relative_spec(root_dir, &spec),
            &relative_spec(root_dir, &node.pinned_spec),
        );
        // Workspace members never appear in the packages map (pnpm parity).
        if node.registry_type != "workspace" {
            locker.update_lock_pkg(
                &node.name,
                &relative_spec(root_dir, &node.pinned_spec),
                LockPkg::from_node_data(node),
            );
        }
    }
}

/// `getPinnedVersion.js` — the single-version shortcut, two-step
/// max-satisfying, then dist-tags, else `InstallNoMatch`.
pub fn get_pinned_version(name: &str, fetch_spec: &str, packument: &Packument) -> Result<String> {
    let keys: Vec<String> = packument.versions.keys().cloned().collect();
    if keys.len() == 1
        && (packument.is_from_lock_file
            || packument.registry_type == "local"
            || packument.registry_type == "git"
            || packument.registry_type == "workspace"
            || is_standard_tag_dependency(fetch_spec))
    {
        return Ok(keys[0].clone());
    }
    let tags: std::collections::BTreeSet<String> = keys.iter().cloned().collect();
    let pinned = semver_max_satisfying(&keys, fetch_spec)
        .or_else(|| get_version_by_dist_tags(fetch_spec, &packument.dist_tags, &tags));
    match pinned {
        Some(p) => Ok(p),
        None => Err(OhpmError::install_no_match(name, fetch_spec)),
    }
}

/// `getLockPkgFromMetaData.js` / `getLockPkgFromNodeData.js` — the lockfile
/// package-entry builders.
impl LockPkg {
    pub fn from_meta(meta: &VersionMeta, registry_type: &str) -> LockPkg {
        match &meta.dist {
            Some(dist) => LockPkg {
                name: meta.name.clone(),
                version: meta.version.clone(),
                integrity: dist.integrity.clone(),
                resolved: dist.tarball.clone(),
                shasum: dist.shasum.clone(),
                registry_type: registry_type.to_string(),
                dependencies: meta.dependencies.clone(),
                dev_dependencies: meta.dev_dependencies.clone(),
                dynamic_dependencies: meta.dynamic_dependencies.clone(),
                package_type: meta.package_type.clone(),
                ..Default::default()
            },
            // No `dist` (local artifacts): the metadata is used as-is; the
            // local fetchers carry `resolved` in the flattened fields.
            None => LockPkg {
                name: meta.name.clone(),
                version: meta.version.clone(),
                resolved: meta.resolved.clone(),
                integrity: meta.integrity.clone(),
                shasum: meta.shasum.clone(),
                registry_type: registry_type.to_string(),
                dependencies: meta.dependencies.clone(),
                dev_dependencies: meta.dev_dependencies.clone(),
                dynamic_dependencies: meta.dynamic_dependencies.clone(),
                package_type: meta.package_type.clone(),
                ..Default::default()
            },
        }
    }
}

/// `getLockPkgFromNodeData.js` — the package entry written to the lockfile
/// (`addOrRemoveOverrideDependencyMapTag` adds the masked tag when the node's
/// deps were modified by `exclusions` / `overrideDependencyMap`).
impl LockPkg {
    pub fn from_node_data(node: &NodeData) -> LockPkg {
        LockPkg {
            name: node.actual_name.clone(),
            version: node.version.clone(),
            integrity: node.integrity.clone(),
            resolved: node.resolved.clone(),
            shasum: node.shasum.clone(),
            registry_type: node.registry_type.clone(),
            dependencies: node.dependencies.clone(),
            dynamic_dependencies: node.dynamic_dependencies.clone(),
            package_type: node.package_type.clone(),
            masked_by_override_dependency_map: node
                .masked_by_override_dependency_map
                .then_some(true),
            ..Default::default()
        }
    }
}

/// The lockfile-derived `versions` entry (a `LockPkg` flattened into the
/// packument `VersionMeta` shape).
impl VersionMeta {
    pub fn from_lock_pkg(lock_pkg: &LockPkg) -> VersionMeta {
        VersionMeta {
            name: lock_pkg.name.clone(),
            version: lock_pkg.version.clone(),
            package_type: lock_pkg.package_type.clone(),
            dependencies: lock_pkg.dependencies.clone(),
            dev_dependencies: lock_pkg.dev_dependencies.clone(),
            dynamic_dependencies: lock_pkg.dynamic_dependencies.clone(),
            resolved: lock_pkg.resolved.clone(),
            integrity: lock_pkg.integrity.clone(),
            shasum: lock_pkg.shasum.clone(),
            ..Default::default()
        }
    }
}

/// Read the manifest from a local .har/.tgz without extracting the whole
/// archive (mirrors `readPkgJsonFromTarball`).
pub fn read_manifest_from_tar(path: &Path) -> Result<Manifest> {
    let entry = crate::archive::find_manifest_entry(path).map_err(|_| {
        OhpmError::fetcher_local_artifact_fetch_metadata_failed(&path.to_string_lossy(), path)
    })?;
    let bytes = crate::archive::read_entry_content(path, &entry)?;
    let text = String::from_utf8_lossy(&bytes);
    Manifest::from_json5(&text).map_err(|_| {
        OhpmError::fetcher_local_artifact_fetch_metadata_failed(&path.to_string_lossy(), path)
    })
}

/// Read the manifest from a local directory.
pub fn read_manifest_from_dir(dir: &Path) -> Result<Manifest> {
    crate::package::read_manifest_from_dir(dir)
}

/// Retry a network-bound future on `RequestFailed` errors, mirroring the
/// `ConcurrentExecutor` retry list (errno-based in the reference; reqwest
/// errors map to `RequestFailed`).
pub async fn with_network_retry<T, F, Fut>(retry_times: u32, retry_interval_ms: u64, mut f: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut attempts = 0;
    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) if e.code == "RequestFailed" && attempts < retry_times => {
                attempts += 1;
                tokio::time::sleep(std::time::Duration::from_millis(retry_interval_ms)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::packument::Dist;

    fn meta(name: &str, version: &str, tarball: &str) -> VersionMeta {
        VersionMeta {
            name: name.to_string(),
            version: version.to_string(),
            package_type: None,
            dependencies: BTreeMap::new(),
            dev_dependencies: BTreeMap::new(),
            dynamic_dependencies: BTreeMap::new(),
            dist: Some(Dist {
                tarball: tarball.to_string(),
                integrity: Some("sha512-X".to_string()),
                shasum: Some("abc".to_string()),
                resolved_hsp: None,
                integrity_hsp: None,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn pinned_version_shortcuts() {
        let mut p = Packument {
            name: "foo".into(),
            actual_name: "foo".into(),
            dist_tags: BTreeMap::new(),
            versions: BTreeMap::new(),
            registry_type: "ohpm".into(),
            is_from_lock_file: false,
            package_type: None,
            package_type_header: None,
        };
        p.versions.insert("1.2.3".to_string(), meta("foo", "1.2.3", "t"));
        // Single version without lock/local/tag -> max-satisfying path.
        assert_eq!(get_pinned_version("foo", "^1.0.0", &p).unwrap(), "1.2.3");
        assert_eq!(get_pinned_version("foo", "latest", &p).unwrap(), "1.2.3");
        p.is_from_lock_file = true;
        assert_eq!(get_pinned_version("foo", "^9.9.9", &p).unwrap(), "1.2.3");
        p.is_from_lock_file = false;
        p.registry_type = "local".into();
        assert_eq!(get_pinned_version("foo", "../lib", &p).unwrap(), "1.2.3");
        p.registry_type = "ohpm".into();
        p.dist_tags.insert("beta".to_string(), "1.2.3".to_string());
        assert_eq!(get_pinned_version("foo", "tag:beta", &p).unwrap(), "1.2.3");
        p.versions.clear();
        let err = get_pinned_version("foo", "^1.0.0", &p).unwrap_err();
        assert_eq!(err.code, "InstallNoMatch");
    }

    #[test]
    fn lock_pkg_from_meta_and_node() {
        let m = meta("@ohos/foo", "1.2.3", "https://r/t.tgz");
        let lp = LockPkg::from_meta(&m, "ohpm");
        assert_eq!(lp.name, "@ohos/foo");
        assert_eq!(lp.resolved, "https://r/t.tgz");
        assert_eq!(lp.integrity.as_deref(), Some("sha512-X"));
        assert_eq!(lp.registry_type, "ohpm");

        let mut node = NodeData {
            name: "@ohos/foo".into(),
            declared_name: String::new(),
            version: "1.2.3".into(),
            actual_name: "@ohos/foo".into(),
            pinned_spec: "1.2.3".into(),
            save_spec: "^1.2.3".into(),
            fetch_spec: "latest".into(),
            ohpa_type: OhpaType::Range,
            registry_type: "ohpm".into(),
            package_type: None,
            is_root: false,
            is_link: false,
            is_shared: true,
            save_root_dir: "x".into(),
            pkg_store_dir: "y".into(),
            integrity: Some("sha512-Z".into()),
            shasum: Some("d".into()),
            resolved: "https://r/t.tgz".into(),
            dependencies: BTreeMap::new(),
            dev_dependencies: BTreeMap::new(),
            dynamic_dependencies: BTreeMap::new(),
            unmet: None,
            masked_by_override_dependency_map: false,
            masked_deps: None,
        };
        node.dependencies.insert("bar".to_string(), "1.0.0".to_string());
        let lp = LockPkg::from_node_data(&node);
        assert_eq!(lp.name, "@ohos/foo");
        assert_eq!(lp.dependencies["bar"], "1.0.0");
        assert_eq!(lp.shasum.as_deref(), Some("d"));
    }

    #[test]
    fn network_retry() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
        rt.block_on(async {
            let calls = std::sync::Arc::new(std::sync::Mutex::new(0u32));
            let result = {
                let calls = calls.clone();
                with_network_retry(2, 1, move || {
                    let calls = calls.clone();
                    async move {
                        let mut c = calls.lock().unwrap();
                        *c += 1;
                        if *c < 3 {
                            Err(OhpmError::request_failed("network"))
                        } else {
                            Ok(42u32)
                        }
                    }
                })
                .await
            };
            assert_eq!(result.unwrap(), 42);
            assert_eq!(*calls.lock().unwrap(), 3);

            // Non-network errors are not retried.
            let calls = std::sync::Arc::new(std::sync::Mutex::new(0u32));
            let result: Result<u32> = {
                let calls = calls.clone();
                with_network_retry(2, 1, move || {
                    let calls = calls.clone();
                    async move {
                        *calls.lock().unwrap() += 1;
                        Err(OhpmError::install_no_match("x", "y"))
                    }
                })
                .await
            };
            assert!(result.is_err());
            assert_eq!(*calls.lock().unwrap(), 1);
        });
    }
}
