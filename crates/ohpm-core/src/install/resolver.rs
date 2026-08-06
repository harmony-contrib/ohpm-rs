//! Node-data resolution, mirroring `lib/core/locker/PackageLockerManager.js`
//! (`getDepNodeData` / `buildNodeDataAsync` / `saveInTargetLockFile`),
//! `lib/core/dependency/dep-fetcher/InstallationMetadataFetcher.js` and the
//! metadata fetcher implementations (`RegistryMetaDataFetcherImpl`,
//! `LocalArtifactMetadataFetcherImpl`, `SourceCodeMetadataFetcherImpl`).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{Mutex, Notify};

use crate::config::Config;
use crate::error::{OhpmError, Result};
use crate::install::lockfile::{relative_spec, LockPkg, Locker};
use crate::install::node::{
    file_content_hash, file_path_hash, pkg_store_dir_name, NodeData,
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
    pub cli_input_names: std::sync::Mutex<Vec<String>>,
    pub packument_cache: PackumentCache,
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
    pub fn new(client: RegistryClient, config: Config, project_root: PathBuf) -> Self {
        Resolver {
            client,
            config,
            project_root,
            cli_input_names: std::sync::Mutex::new(Vec::new()),
            packument_cache: PackumentCache::default(),
            lockers: Mutex::new(HashMap::new()),
            de_dupe_cache: Mutex::new(HashMap::new()),
            in_flight: Mutex::new(HashMap::new()),
        }
    }

    /// `flushAllLockers` — flush every touched locker.
    pub async fn flush_lockers(&self) -> Result<()> {
        let flushed: Vec<Locker> = {
            let lockers = self.lockers.lock().await;
            lockers.values().cloned().collect()
        };
        for mut locker in flushed {
            locker.flush()?;
        }
        Ok(())
    }

    /// Run `f` with the (mutable) locker for `root_dir`.
    async fn with_locker<T>(&self, root_dir: &Path, f: impl FnOnce(&mut Locker) -> T) -> T {
        let mut lockers = self.lockers.lock().await;
        let locker = lockers
            .entry(root_dir.to_path_buf())
            .or_insert_with(|| Locker::load(root_dir));
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
                self.build_node_data(
                    &root_dir_for_build,
                    &name,
                    &spec,
                    &where_dir,
                    is_link,
                    is_shared,
                    &parsed_for_build,
                )
                .await
            })
            .await?;
        self.save_in_target_lock_file(&root_dir, &parsed, &node).await;
        Ok(node)
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
        // `InstallationMetadataFetcher.fetch` — lockfile-first.
        let mut parsed = original.clone();
        let lock_value = self
            .with_locker(root_dir, |locker| {
                locker.get_lock_spec(name, &relative_spec(root_dir, &original.fetch_spec))
            })
            .await;
        if let Some(value) = &lock_value {
            parsed = parse_dependency(value, where_dir)?;
            if let Some(mut pack) =
                self.try_lockfile_packument(root_dir, name, &original.fetch_spec, &parsed).await?
            {
                // `checkAndReassignResolvedField` — re-fetch when the locked
                // entry lacks resolved/integrity.
                if pack.versions.values().next().is_some_and(|v| v.resolved.is_empty() || v.integrity.is_none()) {
                    self.with_locker(root_dir, |locker| {
                        locker.delete_specifier(&format!(
                            "{name}@{}",
                            relative_spec(root_dir, &original.fetch_spec)
                        ));
                    })
                    .await;
                    let fresh = self.fetch_with_implementor(name, where_dir, &parsed).await?;
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
            .fetch_with_implementor(name, where_dir, &parsed)
            .await?;
        self.build_dep_node_data(name, original, &parsed, &packument, is_link, is_shared).await
    }

    /// `tryFetchFromLockFile` — build a lockfile-derived packument for
    /// registry types; File needs the (deferred) mtime cache and SourceCode is
    /// never lockfile-first. `Ok(None)` falls through to the implementor.
    async fn try_lockfile_packument(
        &self,
        root_dir: &Path,
        name: &str,
        fetch_spec: &str,
        parsed: &Spec,
    ) -> Result<Option<Packument>> {
        match parsed.ohpa_type {
            OhpaType::Range | OhpaType::Version | OhpaType::Tag => {
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
                }))
            }
            OhpaType::File | OhpaType::SourceCode => Ok(None),
        }
    }

    /// `fetchWithImplementor` — dispatch by spec type.
    async fn fetch_with_implementor(
        &self,
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
        let pinned = get_pinned_version(name, &parsed.fetch_spec, packument)?;
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
        let pinned_parse = parse_dependency(&format!("{name}@{pinned}"), &original.where_dir)?;
        let node_name = pinned_parse.name.clone();

        let (save_root_dir, pkg_store_dir) = match pinned_parse.ohpa_type {
            OhpaType::File => {
                let hash = file_content_hash(Path::new(&pinned_parse.fetch_spec))?;
                (
                    pkg_store_dir_name(&node_name, &pinned_parse.fetch_spec, &hash),
                    format!("oh_modules/{node_name}"),
                )
            }
            OhpaType::SourceCode => {
                if is_link {
                    (
                        pinned_parse.fetch_spec.clone(),
                        pinned_parse.fetch_spec.clone(),
                    )
                } else {
                    let hash = file_path_hash(&pinned_parse.fetch_spec);
                    (
                        pkg_store_dir_name(&node_name, &pinned_parse.fetch_spec, &hash),
                        format!("oh_modules/{node_name}"),
                    )
                }
            }
            _ => (
                pkg_store_dir_name(&node_name, &pinned, ""),
                format!("oh_modules/{node_name}"),
            ),
        };

        let version = if lock_pkg.version.is_empty() {
            "0.0.0".to_string()
        } else {
            lock_pkg.version.clone()
        };
        Ok(Arc::new(NodeData {
            name: node_name,
            version,
            actual_name: packument.actual_name.clone(),
            pinned_spec: pinned,
            save_spec,
            fetch_spec: original.fetch_spec.clone(),
            ohpa_type: pinned_parse.ohpa_type,
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
        let mut lockers = self.lockers.lock().await;
        let locker = lockers
            .entry(root_dir.to_path_buf())
            .or_insert_with(|| Locker::load(root_dir));
        locker.update_lock_spec(
            &node.name,
            &relative_spec(root_dir, &spec),
            &relative_spec(root_dir, &node.pinned_spec),
        );
        locker.update_lock_pkg(
            &node.name,
            &relative_spec(root_dir, &node.pinned_spec),
            LockPkg::from_node_data(node),
        );
    }
}

/// `getPinnedVersion.js` — the single-version shortcut, two-step
/// max-satisfying, then dist-tags, else `InstallNoMatch`.
pub fn get_pinned_version(name: &str, fetch_spec: &str, packument: &Packument) -> Result<String> {
    let keys: Vec<String> = packument.versions.keys().cloned().collect();
    if keys.len() == 1
        && (packument.is_from_lock_file
            || packument.registry_type == "local"
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

/// `getLockPkgFromNodeData.js` — the package entry written to the lockfile.
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
