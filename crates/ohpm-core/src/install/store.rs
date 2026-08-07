//! Content cache + package installation into the store, mirroring
//! `lib/core/cache/index.js`, `lib/core/dependency/dep-install/*` and
//! `lib/core/dependency/visitor/DepNodeInstaller.js`.
//!
//! Layout (empirically matched to the real ohpm): the global content cache is
//! `<cache>/content-v1/<alg>/<h[0:2]>/<h[2:4]>/<h[4:]>` keyed by the sha512 hex
//! of `dist.integrity` (or the sha1 hex of `dist.shasum`), with the algorithm
//! directory (`sha512`/`sha1`) — the reference's `getPkgCachePath` nests under
//! `<alg>` and real ohpm shares this layout. With `cache_hardlink` enabled
//! (an ohpm-rs extension, pnpm-store aligned), the same tree is extracted
//! once into `<cache>/extracted-v1/<alg>/<h[0:2]>/<h[2:4]>/<h[4:]>` and
//! hard-linked (copy fallback) into each project.
//!
//! Packages are extracted (strip=1, `.CodeSignature` skipped) into
//! `<projectRoot>/oh_modules/.ohpm/<name@version>/oh_modules/<name>` via a
//! temp dir + atomic rename (the default concurrently-safe mode).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::{Mutex, Notify, Semaphore};

use crate::archive;
use crate::config::{default::types, Config};
use crate::constants::{MAX_PACK_SIZE_B, MY_MODULES, NODE_MODULES, SIGN_FOLDER_NAME, TMP_DIR_NAME};
use crate::error::{OhpmError, Result};
use crate::install::graph::DependencyGraph;
use crate::install::node::{ensure_oh_package_json5, Node};
use crate::install::packument::get_with_redirect;
use crate::registry::RegistryClient;

/// In-flight download / install dedup cell (the pacquet/pnpm pattern).
pub(crate) struct InFlightCell<T> {
    /// The first caller runs the build; `AtomicBool` must default to `true`
    /// (a `#[derive(Default)]` would make everyone a waiter and deadlock).
    first: AtomicBool,
    done: Mutex<Option<Result<T>>>,
    notify: Notify,
}

impl<T> Default for InFlightCell<T> {
    fn default() -> Self {
        InFlightCell {
            first: AtomicBool::new(true),
            done: Mutex::new(None),
            notify: Notify::new(),
        }
    }
}

impl<T: Clone> InFlightCell<T> {
    pub async fn run(&self, build: impl std::future::Future<Output = Result<T>>) -> Result<T> {
        if self.first.swap(false, Ordering::SeqCst) {
            let result = build.await;
            *self.done.lock().await = Some(result);
            self.notify.notify_waiters();
            return match self.done.lock().await.as_ref() {
                Some(Ok(v)) => Ok(v.clone()),
                Some(Err(e)) => Err(OhpmError::new(e.code, e.message.clone())),
                None => unreachable!("first runner stores its result"),
            };
        }
        loop {
            let done = self.done.lock().await;
            if let Some(result) = done.as_ref() {
                return match result {
                    Ok(v) => Ok(v.clone()),
                    Err(e) => Err(OhpmError::new(e.code, e.message.clone())),
                };
            }
            drop(done);
            self.notify.notified().await;
        }
    }
}

/// Shared install state (mirrors `DepNodeInstaller`'s caches + the
/// `fetchPackageCacheMap` + `InstallationPromiseCache`).
pub struct StoreContext {
    pub client: RegistryClient,
    pub config: Config,
    pub project_root: PathBuf,
    /// Bounds the blocking extraction work (`spawn_blocking` + semaphore).
    pub max_concurrent: usize,
    /// `CONCURRENTLY_SAFE_INSTALL` — the tmp+rename placement (default).
    pub concurrent_safe: bool,
    downloads: Mutex<HashMap<String, Arc<InFlightCell<Arc<Vec<u8>>>>>>,
    install_promises: Mutex<HashMap<PathBuf, Arc<InFlightCell<()>>>>,
    visited_save_roots: Mutex<std::collections::HashSet<PathBuf>>,
    installed: std::sync::atomic::AtomicUsize,
}

impl StoreContext {
    pub fn new(
        client: RegistryClient,
        config: Config,
        project_root: PathBuf,
        max_concurrent: usize,
        concurrent_safe: bool,
    ) -> Self {
        StoreContext {
            client,
            config,
            project_root,
            max_concurrent: max_concurrent.max(1),
            concurrent_safe,
            downloads: Mutex::new(HashMap::new()),
            install_promises: Mutex::new(HashMap::new()),
            visited_save_roots: Mutex::new(std::collections::HashSet::new()),
            installed: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// The number of store dirs materialized this run.
    pub async fn installed_count(&self) -> usize {
        self.installed.load(Ordering::SeqCst)
    }

    fn cache_dir(&self) -> PathBuf {
        self.config.cache_dir()
    }
}

/// `ssri.parse`-style integrity parsing — `sha512-<base64>` (or `sha1-<base64>`)
/// to `(algorithm, hex_digest)`.
pub fn parse_ssri(integrity: &str) -> Result<(String, String)> {
    let (algo, b64) = integrity
        .split_once('-')
        .ok_or_else(|| OhpmError::cache_invalid_package("", ""))?;
    if algo != "sha512" && algo != "sha1" {
        return Err(OhpmError::cache_invalid_package("", ""));
    }
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|_| OhpmError::cache_invalid_package("", ""))?;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok((algo.to_string(), hex))
}

/// `getPkgFilePathInCacheDir` — `<cache>/content-v1/<alg>/<h[0:2]>/<h[2:4]>/<h[4:]>`
/// (the `<alg>` directory matches the reference layout).
pub fn cache_file_path(cache_dir: &Path, algo: &str, hex: &str) -> PathBuf {
    cache_dir
        .join("content-v1")
        .join(algo)
        .join(&hex[0..2])
        .join(&hex[2..4])
        .join(&hex[4..])
}

/// The hard-link mode's shared extracted layer — the `content-v1` sibling:
/// `<cache>/extracted-v1/<alg>/<h[0:2]>/<h[2:4]>/<h[4:]>`.
fn extracted_path_of(cache_path: &Path) -> PathBuf {
    let mut comps: Vec<std::ffi::OsString> = cache_path
        .components()
        .map(|c| c.as_os_str().to_os_string())
        .collect();
    for c in comps.iter_mut().rev() {
        if c == "content-v1" {
            *c = "extracted-v1".into();
            break;
        }
    }
    let mut out = PathBuf::new();
    for c in comps {
        if c.is_empty() {
            // The root component (""): push("/") resets to the filesystem root.
            out.push("/");
        } else {
            out.push(c);
        }
    }
    out
}

/// Hash a byte slice with the given algorithm and hex-encode.
pub(crate) fn hex_digest(algo: &str, bytes: &[u8]) -> String {
    use sha2::Digest;
    match algo {
        "sha1" => {
            let mut h = sha1::Sha1::new();
            h.update(bytes);
            hex(&h.finalize())
        }
        _ => {
            let mut h = sha2::Sha512::new();
            h.update(bytes);
            hex(&h.finalize())
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `getPkgCachePath` — the content-cache path for a registry node; downloads
/// and verifies when missing (with in-flight dedup by tarball URL).
pub async fn get_pkg_cache_path(ctx: &StoreContext, node: &Node) -> Result<PathBuf> {
    let (algo, digest) = match &node.data.integrity {
        Some(integrity) => parse_ssri(integrity)?,
        None => {
            let shasum = node
                .data
                .shasum
                .clone()
                .ok_or_else(|| OhpmError::cache_invalid_package(&node.data.name, &node.data.pinned_spec))?;
            ("sha1".to_string(), shasum)
        }
    };
    let path = cache_file_path(&ctx.cache_dir(), &algo, &digest);
    if path.exists() {
        // Cache hit: re-hash and compare before reuse.
        let bytes = std::fs::read(&path)?;
        if hex_digest(&algo, &bytes) == digest {
            log::debug!(
                "found package {}@{} from cache file {}",
                node.data.name,
                node.data.pinned_spec,
                path.display()
            );
            return Ok(path);
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let url = node.data.resolved.clone();
    let token = read_token_for_url(&ctx.config, &url);
    let timeout = ctx.config.get_number(types::FETCH_TIMEOUT) as u64;
    let bytes = {
        let cell = ctx
            .downloads
            .lock()
            .await
            .entry(url.clone())
            .or_default()
            .clone();
        cell.run(async {
            let response = get_with_redirect(&ctx.client, &ctx.config, &url, &token, timeout).await?;
            if !response.status().is_success() {
                return Err(OhpmError::response_status(
                    response.status().as_u16(),
                    "",
                ));
            }
            Ok(Arc::new(response.bytes().await.map_err(OhpmError::from)?.to_vec()))
        })
        .await?
    };
    if hex_digest(&algo, &bytes) != digest {
        return Err(OhpmError::cache_invalid_package(&node.data.name, &node.data.pinned_spec));
    }
    let tmp = path.parent().unwrap().join(format!(
        "{}+{}@{}",
        node.data.name.replace('/', "+"),
        node.data.pinned_spec,
        std::process::id()
    ));
    std::fs::write(&tmp, &bytes[..])?;
    rename_if_not_exist(&tmp, &path)?;
    Ok(path)
}

/// `getPkgCachePath` for the `.hsp` file of a bundle-app HSP package — the
/// `resolved_hsp` tarball is fetched into the content cache (verified with
/// `integrity_hsp`) and returned as the file to copy into `oh_modules/.hsp`.
/// `None` when the package carries no `.hsp` URL.
pub async fn get_hsp_cache_path(ctx: &StoreContext, node: &Node) -> Result<Option<PathBuf>> {
    let Some(resolved_hsp) = node.data.resolved_hsp.clone() else {
        return Ok(None);
    };
    let (algo, digest) = match &node.data.integrity_hsp {
        Some(integrity) => parse_ssri(integrity)?,
        None => {
            let shasum = node
                .data
                .shasum
                .clone()
                .ok_or_else(|| OhpmError::cache_invalid_package(&node.data.name, &node.data.pinned_spec))?;
            ("sha1".to_string(), shasum)
        }
    };
    let path = cache_file_path(&ctx.cache_dir(), &algo, &digest);
    if path.exists() {
        let bytes = std::fs::read(&path)?;
        if hex_digest(&algo, &bytes) == digest {
            return Ok(Some(path));
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let token = read_token_for_url(&ctx.config, &resolved_hsp);
    let timeout = ctx.config.get_number(types::FETCH_TIMEOUT) as u64;
    let bytes = {
        let cell = ctx
            .downloads
            .lock()
            .await
            .entry(resolved_hsp.clone())
            .or_default()
            .clone();
        cell.run(async {
            let response =
                get_with_redirect(&ctx.client, &ctx.config, &resolved_hsp, &token, timeout).await?;
            if !response.status().is_success() {
                return Err(OhpmError::response_status(
                    response.status().as_u16(),
                    "",
                ));
            }
            Ok(Arc::new(response.bytes().await.map_err(OhpmError::from)?.to_vec()))
        })
        .await?
    };
    if hex_digest(&algo, &bytes) != digest {
        return Err(OhpmError::cache_invalid_package(&node.data.name, &node.data.pinned_spec));
    }
    let tmp = path.parent().unwrap().join(format!(
        "{}+{}@{}",
        node.data.name.replace('/', "+"),
        node.data.pinned_spec,
        std::process::id()
    ));
    std::fs::write(&tmp, &bytes[..])?;
    rename_if_not_exist(&tmp, &path)?;
    Ok(Some(path))
}

/// The `.hsp` placement shared by the registry and local installers: copy the
/// `.hsp` file + the package manifest into `oh_modules/.hsp/<storeDir>`.
fn place_hsp_file(ctx: &StoreContext, node: &Node, hsp_bytes: &[u8]) -> Result<()> {
    let hsp_dir = node.data.resolve_hsp_store_dir(&ctx.project_root);
    std::fs::create_dir_all(&hsp_dir)?;
    let hsp_path = hsp_dir.join(&node.data.hsp_name);
    std::fs::write(&hsp_path, hsp_bytes)?;
    // The manifest alongside, like the reference.
    let manifest = node
        .data
        .resolve_pkg_store_dir(&ctx.project_root)
        .join(crate::constants::MY_PACKAGE_JSON);
    if manifest.is_file() {
        std::fs::copy(&manifest, hsp_dir.join(crate::constants::MY_PACKAGE_JSON))?;
    }
    Ok(())
}

/// The reference's `getAvailableAuthByUrl` — a config key `//host/path/:_read_auth`
/// applies when the URL starts with `//host/path/`.
fn read_token_for_url(config: &Config, url: &str) -> String {
    let url = url.to_string();
    for key in config.effective_keys() {
        if let Some(prefix) = key.strip_suffix(":_read_auth") {
            if url.starts_with(prefix) {
                let token = config.get_string(&key);
                if !token.is_empty() {
                    return token;
                }
            }
        }
    }
    String::new()
}

/// `renameIfNotExist` — atomic rename only when the target is absent.
pub fn rename_if_not_exist(from: &Path, to: &Path) -> Result<()> {
    if to.exists() {
        let _ = std::fs::remove_file(from);
        return Ok(());
    }
    std::fs::rename(from, to).map_err(|e| OhpmError::fs_rename_file_error(from, to).with_detail(&e.to_string()))
}

/// `handleInstallException` — extraction/placement failures surface as
/// `InstallPkgToLocalFailed`.
fn handle_install_exception(node: &Node, e: &OhpmError) -> OhpmError {
    log::warn!(
        "install {}@{} to {} fail, detail: {}",
        node.data.name,
        node.data.pinned_spec,
        node.data.resolve_pkg_store_dir(Path::new("/")).display(),
        e
    );
    if e.message.contains("TAR_BAD_ARCHIVE") {
        OhpmError::new("TARBADARCHIVE", e.message.clone())
    } else {
        OhpmError::install_pkg_to_local_failed()
    }
}

/// `installRegistryArtifactDep` — cache download + size/traversal checks +
/// extract (tmp + atomic rename) + `ensureOhPackageJson5`. With
/// `cache_hardlink` enabled (ohpm-rs extension, pnpm-store aligned) the
/// archive is extracted once into the shared `extracted-v1` layer and each
/// project's store dir is hard-linked (copy fallback) from it instead.
async fn install_registry_node(ctx: &StoreContext, node: &Node) -> Result<()> {
    let cache = get_pkg_cache_path(ctx, node).await?;
    let store_dir = node.data.resolve_pkg_store_dir(&ctx.project_root);
    if ctx.config.get_bool(types::CACHE_HARDLINK) {
        let extracted = materialize_extracted(ctx, node, &cache).await?;
        tokio::task::spawn_blocking(move || link_extracted_to_store(&extracted, &store_dir))
            .await
            .map_err(|e| OhpmError::install_pkg_to_local_failed().with_detail(&e.to_string()))?
            .map_err(|e| handle_install_exception(node, &e))?;
    } else {
        let bytes = std::fs::read(&cache)?;
        let sem = Arc::new(Semaphore::new(ctx.max_concurrent));
        let result = extract_to_store(
            &cache,
            &store_dir,
            bytes.len() as u64,
            &sem,
            ctx.concurrent_safe,
        )
        .await;
        result.map_err(|e| handle_install_exception(node, &e))?;
    }
    // `installRegistryArtifactDep` — the bundle-app HSP `.hsp` file.
    if node.data.hsp_type.as_deref() == Some(crate::constants::HSP_TYPE_BUNDLE_APP) {
        install_hsp_file(ctx, node).await?;
    }
    Ok(())
}

/// Extract the cached archive once into the shared `extracted-v1` layer (the
/// same strip=1 / `.CodeSignature`-skip / `ensureOhPackageJson5` rules as a
/// project extraction). Returns the extracted dir.
async fn materialize_extracted(ctx: &StoreContext, node: &Node, archive: &Path) -> Result<PathBuf> {
    let dir = extracted_path_of(archive);
    if dir.join(crate::constants::MY_PACKAGE_JSON).is_file() {
        return Ok(dir);
    }
    let bytes = std::fs::read(archive)?;
    let sem = Arc::new(Semaphore::new(ctx.max_concurrent));
    let result = extract_to_store(archive, &dir, bytes.len() as u64, &sem, ctx.concurrent_safe).await;
    result.map_err(|e| handle_install_exception(node, &e))?;
    Ok(dir)
}

/// The pnpm-style placement: hard-link the shared extracted tree into the
/// project store dir. An existing store dir is left alone (`renameIfNotExist`
/// semantics); cross-device link failures fall back to copying (pnpm's
/// behavior).
fn link_extracted_to_store(extracted: &Path, store_dir: &Path) -> Result<()> {
    if store_dir.join(crate::constants::MY_PACKAGE_JSON).is_file() {
        return Ok(());
    }
    std::fs::create_dir_all(store_dir)?;
    link_tree(extracted, store_dir)
}

/// Recursively hard-link `src`'s entries into `dst` (real directories, file
/// hard links).
fn link_tree(src: &Path, dst: &Path) -> Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            std::fs::create_dir_all(&to)?;
            link_tree(&from, &to)?;
        } else {
            link_or_copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Hard-link `src` to `dst`, falling back to a full copy when the link fails
/// (cross-device `EXDEV`, permissions, ... — pnpm falls back the same way).
fn link_or_copy(src: &Path, dst: &Path) -> Result<()> {
    match std::fs::hard_link(src, dst) {
        Ok(()) => Ok(()),
        Err(_) => std::fs::copy(src, dst).map(|_| ()).map_err(OhpmError::from),
    }
}

/// The `.hsp` file placement: the cached `resolved_hsp` tarball is copied into
/// `oh_modules/.hsp/<storeDir>` (dev deps skip the copy, like the reference).
async fn install_hsp_file(ctx: &StoreContext, node: &Node) -> Result<()> {
    let hsp_cache = get_hsp_cache_path(ctx, node).await?;
    // `NotFoundHspFileByRegistryTgz` — a bundle-app HSP without its `.hsp`
    // file cannot be installed as a runtime dependency.
    let Some(hsp_cache) = hsp_cache else {
        if node.dep_type != crate::install::node::DepType::Dev {
            return Err(OhpmError::not_found_hsp_file_by_registry_tgz(
                &node.data.name,
                &node.data.pinned_spec,
            ));
        }
        return Ok(());
    };
    if node.data.is_debug_hsp {
        log::warn!(
            "The installed HSP package \"{}@{}\" was compiled in debugging mode which may cause asset leakage.",
            node.data.name,
            node.data.pinned_spec
        );
    }
    let bytes = std::fs::read(&hsp_cache)?;
    place_hsp_file(ctx, node, &bytes)
}

/// `installLocalArtifact` — extract a local .har/.tgz into the store; a
/// bundle-app HSP tgz also places its embedded `.hsp` file.
async fn install_local_artifact_node(ctx: &StoreContext, node: &Node) -> Result<()> {
    let path = PathBuf::from(&node.data.pinned_spec);
    let bytes = std::fs::read(&path).map_err(|_| {
        OhpmError::fetcher_local_artifact_fetch_local_package_failed(&path)
    })?;
    let store_dir = node.data.resolve_pkg_store_dir(&ctx.project_root);
    let sem = Arc::new(Semaphore::new(ctx.max_concurrent));
    let result = extract_to_store(
        &path,
        &store_dir,
        bytes.len() as u64,
        &sem,
        ctx.concurrent_safe,
    )
    .await;
    result.map_err(|e| handle_install_exception(node, &e))?;
    // `installLocalArtifact` — the bundle-app HSP tgz: merge the embedded
    // interfaceHar (a nested archive, strip 1, sign folder ignored) into the
    // store, drop the .har/.hsp entries, and place the `.hsp` file.
    if node.data.hsp_type.as_deref() == Some(crate::constants::HSP_TYPE_BUNDLE_APP) {
        if let Some((har_entry, hsp_entry)) = crate::archive::hsp_detect(&path).ok().flatten() {
            if node.data.is_debug_hsp {
                log::warn!(
                    "The installed HSP package \"{}@{}\" was compiled in debugging mode which may cause asset leakage.",
                    node.data.name,
                    node.data.pinned_spec
                );
            }
            if node.dep_type != crate::install::node::DepType::Dev {
                if let Ok(hsp_bytes) = crate::archive::read_entry_content(&path, &hsp_entry) {
                    place_hsp_file(ctx, node, &hsp_bytes)?;
                }
            }
            // `extractAndIgnoreTargetFolder(har, store, SignFolderName, 1)`
            // — the interfaceHar content merges into the store.
            let har_rel = relative_entry(&har_entry);
            let har_in_store = store_dir.join(&har_rel);
            if har_in_store.is_file() {
                crate::archive::extract_ignore_dir(
                    &har_in_store,
                    &store_dir,
                    1,
                    SIGN_FOLDER_NAME,
                )
                .map_err(|e| OhpmError::install_pkg_to_local_failed().with_detail(&e.message))?;
                let _ = std::fs::remove_file(&har_in_store);
            }
            let hsp_rel = relative_entry(&hsp_entry);
            let _ = std::fs::remove_file(store_dir.join(&hsp_rel));
        }
    }
    Ok(())
}

/// The entry path after the strip-1 extraction (drop the leading component).
fn relative_entry(entry: &str) -> String {
    let mut parts = entry.split('/');
    parts.next();
    parts.collect::<Vec<_>>().join("/")
}

/// The shared extract path: size check, tmp dir, extract with `.CodeSignature`
/// skipped and strip=1, `ensureOhPackageJson5`, atomic rename.
async fn extract_to_store(
    archive_path: &Path,
    store_dir: &Path,
    size: u64,
    sem: &Arc<Semaphore>,
    concurrent_safe: bool,
) -> Result<()> {
    if size > MAX_PACK_SIZE_B {
        return Err(OhpmError::dep_install_package_size_exceed());
    }
    let archive_path = archive_path.to_path_buf();
    let store_dir = store_dir.to_path_buf();
    let _permit = sem.acquire().await.map_err(|_| OhpmError::install_pkg_to_local_failed())?;
    tokio::task::spawn_blocking(move || {
        if concurrent_safe {
            // The tmp dir + atomic rename (the default concurrently-safe
            // placement, `h()`).
            let tmp = store_dir.with_extension(format!("{}.{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
            std::fs::create_dir_all(&tmp)?;
            archive::extract_ignore_dir(&archive_path, &tmp, 1, SIGN_FOLDER_NAME)
                .map_err(|e| OhpmError::install_pkg_to_local_failed().with_detail(&e.message))?;
            ensure_oh_package_json5(&tmp)?;
            rename_if_not_exist(&tmp, &store_dir)?;
        } else {
            // `g()` — extract directly into the store dir.
            std::fs::create_dir_all(&store_dir)?;
            archive::extract_ignore_dir(&archive_path, &store_dir, 1, SIGN_FOLDER_NAME)
                .map_err(|e| OhpmError::install_pkg_to_local_failed().with_detail(&e.message))?;
            ensure_oh_package_json5(&store_dir)?;
        }
        Ok::<(), OhpmError>(())
    })
    .await
    .map_err(|e| OhpmError::install_pkg_to_local_failed().with_detail(&e.to_string()))?
}

/// Git deps: the resolution phase already materialized the checkout into the
/// store dir — this is an idempotent no-op (a crash between phases re-runs
/// the materialization).
async fn install_git_node(ctx: &StoreContext, node: &Node) -> Result<()> {
    let store_dir = node.data.resolve_pkg_store_dir(&ctx.project_root);
    if store_dir.join(crate::constants::MY_PACKAGE_JSON).is_file() {
        return Ok(());
    }
    // Store missing — re-materialize the pinned commit via the clone path.
    let url = node.data.resolved.split('#').next().unwrap_or(&node.data.resolved).to_string();
    let commit = node.data.pinned_spec.clone();
    let tmp = ctx
        .project_root
        .join(MY_MODULES)
        .join(TMP_DIR_NAME)
        .join(format!("git-{}", uuid::Uuid::new_v4().simple()));
    let sem = Arc::new(Semaphore::new(ctx.max_concurrent));
    let _permit = sem.acquire().await.map_err(|_| OhpmError::install_pkg_to_local_failed())?;
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(tmp.parent().unwrap())?;
        let repo = crate::install::git::fetch_repo(&url, &tmp)?;
        crate::install::git::materialize_commit_in(&repo, &commit, None, &store_dir)?;
        let _ = std::fs::remove_dir_all(&tmp);
        Ok::<(), OhpmError>(())
    })
    .await
    .map_err(|e| OhpmError::install_pkg_to_local_failed().with_detail(&e.to_string()))?
}

/// `installSourceCodeArtifactDep` — linked source code is a no-op; otherwise
/// copy the source dir (excluding node_modules/oh_modules/build).
async fn install_source_code_node(ctx: &StoreContext, node: &Node) -> Result<()> {
    if node.data.is_link {
        return Ok(());
    }
    let src = PathBuf::from(&node.data.pinned_spec);
    let store_dir = node.data.resolve_pkg_store_dir(&ctx.project_root);
    let _ = std::fs::remove_dir_all(&store_dir);
    let tmp = store_dir.with_extension(format!("{}.{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
    let sem = Arc::new(Semaphore::new(ctx.max_concurrent));
    let _permit = sem.acquire().await.map_err(|_| OhpmError::install_pkg_to_local_failed())?;
    tokio::task::spawn_blocking(move || {
        copy_dir_excluding(&src, &tmp, &[NODE_MODULES, MY_MODULES, "build"])?;
        rename_if_not_exist(&tmp, &store_dir)?;
        Ok::<(), OhpmError>(())
    })
    .await
    .map_err(|e| OhpmError::install_pkg_to_local_failed().with_detail(&e.to_string()))?
}

/// Copy a directory tree, skipping top-level entries in `exclude`.
fn copy_dir_excluding(src: &Path, dest: &Path, exclude: &[&str]) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if exclude.contains(&name.as_str()) {
            continue;
        }
        let from = entry.path();
        let to = dest.join(&name);
        if entry.file_type()?.is_dir() {
            copy_dir_excluding(&from, &to, exclude)?;
        } else {
            std::fs::create_dir_all(to.parent().unwrap())?;
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// `installDependency` — dispatch by spec type with per-store-dir dedup.
async fn install_dependency(ctx: &StoreContext, node: &Node) -> Result<()> {
    let store_dir = node.data.resolve_pkg_store_dir(&ctx.project_root);
    let cell = ctx
        .install_promises
        .lock()
        .await
        .entry(store_dir.clone())
        .or_default()
        .clone();
    let result = cell
        .run(async {
            log::debug!("start to install depNode: {}", node.node_key());
            match node.data.ohpa_type {
                crate::install::spec::OhpaType::SourceCode => {
                    install_source_code_node(ctx, node).await
                }
                crate::install::spec::OhpaType::File => {
                    install_local_artifact_node(ctx, node).await
                }
                crate::install::spec::OhpaType::Git => install_git_node(ctx, node).await,
                crate::install::spec::OhpaType::Workspace => Ok(()), // link only
                _ => install_registry_node(ctx, node).await,
            }
        })
        .await;
    ctx.install_promises.lock().await.remove(&store_dir);
    if result.is_ok() && node.data.registry_type != "workspace" {
        ctx.installed.fetch_add(1, Ordering::SeqCst);
        log::info!(
            "place package done: {}@{} to {}",
            node.data.name,
            node.data.pinned_spec,
            store_dir.display()
        );
    }
    result
}

/// `DepNodeInstaller.run` — the install phase over the flat graph.
pub async fn install_phase(ctx: &Arc<StoreContext>, graph: &DependencyGraph) -> Result<()> {
    // preWalk: per-module `.ohpm` dirs of non-project-root modules are stale.
    for (module_root, root) in &graph.roots {
        if root.data.is_shared && module_root != &ctx.project_root {
            let dir = module_root.join(MY_MODULES).join(".ohpm");
            if dir.exists() {
                std::fs::remove_dir_all(&dir).map_err(OhpmError::from)?;
            }
        }
    }
    let nodes = graph.flat_graph();
    let sem = Arc::new(Semaphore::new(ctx.max_concurrent));
    let mut tasks = tokio::task::JoinSet::new();
    for node in nodes {
        // `isVisited` — the save root already installed this run.
        let save_root = node.data.resolve_save_root(&ctx.project_root);
        let visited = ctx.visited_save_roots.lock().await.contains(&save_root);
        if node.data.is_root || visited {
            continue;
        }
        ctx.visited_save_roots.lock().await.insert(save_root);
        let ctx = ctx.clone();
        let sem = sem.clone();
        tasks.spawn(async move {
            let _permit = sem.acquire().await.map_err(|_| OhpmError::install_pkg_to_local_failed())?;
            install_dependency(&ctx, &node).await
        });
    }
    while let Some(res) = tasks.join_next().await {
        res.map_err(|e| OhpmError::install_pkg_to_local_failed().with_detail(&e.to_string()))??;
    }
    Ok(())
}

/// `deleteEmptyOhModulesDir` — remove an empty oh_modules dir.
pub fn delete_empty_oh_modules_dir(module_root: &Path) -> Result<()> {
    let dir = module_root.join(MY_MODULES);
    if dir.exists() && dir.read_dir().map(|mut d| d.next().is_none()).unwrap_or(false) {
        std::fs::remove_dir(&dir)?;
    }
    Ok(())
}

/// `deleteUselessTmpDir` — remove `oh_modules/.tmp`.
pub fn delete_useless_tmp_dir(project_root: &Path) -> Result<()> {
    let dir = project_root.join(MY_MODULES).join(TMP_DIR_NAME);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::node::{pkg_store_dir_name, NodeData};
    use crate::install::spec::OhpaType;

    fn node_data(name: &str, version: &str, pinned: &str) -> NodeData {
        NodeData {
            name: name.to_string(),
            declared_name: String::new(),
            version: version.to_string(),
            actual_name: name.to_string(),
            pinned_spec: pinned.to_string(),
            save_spec: "^1.0.0".to_string(),
            fetch_spec: "latest".to_string(),
            ohpa_type: OhpaType::Range,
            registry_type: "ohpm".to_string(),
            package_type: None,
            is_root: false,
            is_link: false,
            is_shared: true,
            save_root_dir: pkg_store_dir_name(name, pinned, ""),
            pkg_store_dir: format!("{MY_MODULES}/{name}"),
            integrity: None,
            shasum: None,
            resolved: String::new(),
            dependencies: Default::default(),
            dev_dependencies: Default::default(),
            dynamic_dependencies: Default::default(),
            unmet: None,
            masked_by_override_dependency_map: false,
            masked_deps: None,
            hsp_store_dir: String::new(),
            hsp_name: String::new(),
            hsp_type: None,
            is_debug_hsp: false,
            resolved_hsp: None,
            integrity_hsp: None,
        }
    }

    #[test]
    fn ssri_parse() {
        // base64 of sha512("hello") — the ssri digest.
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(
            hex_decode("9b71d224bd62f3785d96d46ad3ea3d73319bfbc2890caadae2dff72519673ca72323c3d99ba5c11d7c7acc6e14b8c5da0c4663475c2e5c3adef46f73bcdec043"),
        );
        let (algo, hex) = parse_ssri(&format!("sha512-{b64}")).unwrap();
        assert_eq!(algo, "sha512");
        assert_eq!(hex.len(), 128);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(parse_ssri("md5-x").is_err());
        assert!(parse_ssri("sha512-not-base64!!").is_err());
    }

    fn hex_decode(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn cache_layout() {
        let dir = tempfile::TempDir::new().unwrap();
        let hex = "aabbccddeeff";
        let p = cache_file_path(dir.path(), "sha512", hex);
        assert_eq!(p, dir.path().join("content-v1/sha512/aa/bb/ccddeeff"));
        let p = cache_file_path(dir.path(), "sha1", hex);
        assert_eq!(p, dir.path().join("content-v1/sha1/aa/bb/ccddeeff"));
    }

    #[test]
    fn extracted_path_is_content_sibling() {
        let p = extracted_path_of(Path::new("/c/content-v1/sha512/aa/bb/cc"));
        assert_eq!(p, Path::new("/c/extracted-v1/sha512/aa/bb/cc"));
        // The last "content-v1" component wins (nested names never occur,
        // but the replacement must not hit an unrelated directory).
        let p = extracted_path_of(Path::new("/c/content-v1/sha512/aa/content-v1/bb"));
        assert_eq!(p, Path::new("/c/content-v1/sha512/aa/extracted-v1/bb"));
    }

    #[test]
    fn link_or_copy_falls_back_when_link_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = dir.path().join("src");
        let dst = dir.path().join("dst");
        std::fs::write(&src, "content").unwrap();
        // A pre-existing dst makes `hard_link` fail (EEXIST) → copy fallback.
        std::fs::write(&dst, "stale").unwrap();
        link_or_copy(&src, &dst).unwrap();
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "content");
    }

    #[test]
    fn digest_matches_openssl() {
        // sha512("hello") hex
        let expected = "9b71d224bd62f3785d96d46ad3ea3d73319bfbc2890caadae2dff72519673ca72323c3d99ba5c11d7c7acc6e14b8c5da0c4663475c2e5c3adef46f73bcdec043";
        assert_eq!(hex_digest("sha512", b"hello"), expected);
        // sha1("hello")
        let expected_sha1 = "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d";
        assert_eq!(hex_digest("sha1", b"hello"), expected_sha1);
    }

    #[test]
    fn copy_excluding() {
        let src = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(src.path().join("node_modules")).unwrap();
        std::fs::create_dir_all(src.path().join("oh_modules")).unwrap();
        std::fs::create_dir_all(src.path().join("build")).unwrap();
        std::fs::create_dir_all(src.path().join("src")).unwrap();
        std::fs::write(src.path().join("oh-package.json5"), "{}").unwrap();
        std::fs::write(src.path().join("node_modules/x"), "x").unwrap();
        std::fs::write(src.path().join("src/a.ets"), "a").unwrap();
        let dest = tempfile::TempDir::new().unwrap();
        copy_dir_excluding(src.path(), dest.path(), &[NODE_MODULES, MY_MODULES, "build"]).unwrap();
        assert!(dest.path().join("oh-package.json5").exists());
        assert!(dest.path().join("src/a.ets").exists());
        assert!(!dest.path().join("node_modules").exists());
        assert!(!dest.path().join("oh_modules").exists());
        assert!(!dest.path().join("build").exists());
    }

    #[test]
    fn extract_ignores_code_signature() {
        let dir = tempfile::TempDir::new().unwrap();
        let har = dir.path().join("p.har");
        let f = std::fs::File::create(&har).unwrap();
        let enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
        let mut tar = tar::Builder::new(enc);
        let mut h = tar::Header::new_gnu();
        h.set_size(4);
        h.set_mode(0o644);
        tar.append_data(&mut h, "package/real.txt", &b"data"[..]).unwrap();
        let mut h2 = tar::Header::new_gnu();
        h2.set_size(3);
        h2.set_mode(0o644);
        tar.append_data(&mut h2, "package/.CodeSignature/code", &b"bad"[..]).unwrap();
        let enc = tar.into_inner().unwrap();
        enc.finish().unwrap();

        let dest = dir.path().join("out");
        archive::extract_ignore_dir(&har, &dest, 1, SIGN_FOLDER_NAME).unwrap();
        assert!(dest.join("real.txt").exists());
        assert!(!dest.join(".CodeSignature").exists());
    }

    #[test]
    fn extract_rejects_traversal() {
        // The tar::Builder refuses `..` paths, so craft the archive manually.
        let dir = tempfile::TempDir::new().unwrap();
        let har = dir.path().join("evil.har");
        let bytes = raw_tar_with_path("package/../evil.txt", b"bad");
        let f = std::fs::File::create(&har).unwrap();
        let mut enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
        std::io::Write::write_all(&mut enc, &bytes).unwrap();
        enc.finish().unwrap();

        let dest = dir.path().join("out");
        let err = archive::extract_ignore_dir(&har, &dest, 1, SIGN_FOLDER_NAME).unwrap_err();
        assert_eq!(err.code, "DepInstallDirectoryTraversal");
        assert!(!dest.join("evil.txt").exists());
    }

    /// Hand-craft a single-file tar with an arbitrary (unvalidated) entry path.
    fn raw_tar_with_path(name: &str, content: &[u8]) -> Vec<u8> {
        let mut header = [0u8; 512];
        let name_bytes = name.as_bytes();
        header[..name_bytes.len()].copy_from_slice(name_bytes);
        let size = format!("{:011o}", content.len());
        header[124..135].copy_from_slice(size.as_bytes());
        header[136..147].copy_from_slice(b"00000000000");
        header[156] = b'0'; // regular file
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        // checksum: sum of header bytes with the field treated as spaces.
        for b in &mut header[148..156] {
            *b = b' ';
        }
        let sum: u32 = header.iter().map(|b| *b as u32).sum();
        let chksum = format!("{:06o}\0 ", sum);
        header[148..156].copy_from_slice(chksum.as_bytes());
        let mut out = Vec::new();
        out.extend_from_slice(&header);
        out.extend_from_slice(content);
        let pad = (512 - content.len() % 512) % 512;
        out.extend(std::iter::repeat(0u8).take(pad));
        out.extend_from_slice(&[0u8; 1024]); // two zero blocks
        out
    }

    #[test]
    fn rename_if_not_exist_semantics() {
        let dir = tempfile::TempDir::new().unwrap();
        let from = dir.path().join("from");
        let to = dir.path().join("to");
        std::fs::write(&from, "a").unwrap();
        rename_if_not_exist(&from, &to).unwrap();
        assert!(to.exists());
        // Target exists → no rename (source removed).
        std::fs::write(&from, "b").unwrap();
        std::fs::write(&to, "keep").unwrap();
        rename_if_not_exist(&from, &to).unwrap();
        assert_eq!(std::fs::read_to_string(&to).unwrap(), "keep");
        assert!(!from.exists());
    }

    #[test]
    fn node_store_paths() {
        let n = NodeData {
            name: "@ohos/foo".to_string(),
            ..node_data("@ohos/foo", "1.2.3", "1.2.3")
        };
        assert_eq!(
            n.resolve_pkg_store_dir(Path::new("/proj")),
            Path::new("/proj/oh_modules/.ohpm/@ohos+foo@1.2.3/oh_modules/@ohos/foo")
        );
    }
}
