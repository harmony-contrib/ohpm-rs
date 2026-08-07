//! Store (`cache`) management tests against the mock registry: `cache add`
//! pre-fetch + reuse, `cache status` integrity checking, `cache path`
//! resolution, the `--cache` config wiring, and the pnpm-style
//! `cache_hardlink` zero-copy install mode (shared inodes across projects).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use base64::Engine;
use ohpm_core::cache;
use ohpm_core::config::default::types;
use ohpm_core::config::Config;
use ohpm_core::install::{install, InstallOptions};
use ohpm_core::registry::RegistryClient;

/// Env-var isolation: saves the variables this suite touches and restores them
/// on drop, so a panicking test can't leak state into the next one.
struct EnvGuard(Vec<(&'static str, Option<String>)>);

impl EnvGuard {
    fn new() -> Self {
        let keys = [
            "HOME",
            "OHPM_REGISTRY",
            "OHPM_CACHE",
            "OHPM_CACHE_HARDLINK",
            "OHPM_LOG_LEVEL",
        ];
        let saved = keys.iter().map(|k| (*k, std::env::var(k).ok())).collect();
        EnvGuard(saved)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }
}

/// Serialize tests in this binary (they mutate global env).
static ENV_LOCK: Mutex<()> = Mutex::new(());

// ---- mock registry (the install_flow.rs shape, trimmed) ----

#[derive(Clone)]
struct MockVersion {
    tarball: Bytes,
    deps: BTreeMap<String, String>,
}

#[derive(Clone, Default)]
struct MockPackage {
    versions: BTreeMap<String, MockVersion>,
    dist_tags: BTreeMap<String, String>,
}

#[derive(Clone, Default)]
struct MockRegistry {
    packages: BTreeMap<String, MockPackage>,
}

#[derive(Debug, Default)]
struct Capture {
    tarball_gets: Vec<String>,
}

fn build_har(dir: &Path, name: &str, version: &str, deps: &BTreeMap<String, String>) -> Bytes {
    let manifest = serde_json::json!({
        "name": name,
        "version": version,
        "main": "index.ets",
        "dependencies": deps,
    })
    .to_string();
    let src = dir.join("src");
    std::fs::create_dir_all(src.join("package/entry")).unwrap();
    std::fs::write(src.join("package/oh-package.json5"), manifest).unwrap();
    std::fs::write(src.join("package/entry/index.ets"), "export {}\n").unwrap();
    let mut out = Vec::new();
    {
        let enc = flate2::write::GzEncoder::new(&mut out, flate2::Compression::default());
        let mut tar = tar::Builder::new(enc);
        tar.append_dir_all("package", src.join("package")).unwrap();
        let enc = tar.into_inner().unwrap();
        enc.finish().unwrap();
    }
    Bytes::from(out)
}

fn sha512_b64(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha512::new();
    h.update(bytes);
    base64::engine::general_purpose::STANDARD.encode(h.finalize())
}

fn sha512_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha512::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn sha1_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut h = sha1::Sha1::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

async fn spawn_mock(registry: MockRegistry) -> (String, Arc<Mutex<Capture>>) {
    let capture = Arc::new(Mutex::new(Capture::default()));
    let registry = Arc::new(Mutex::new(registry));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = Router::new()
        .route("/ohpm/*rest", get(ohpm_handler))
        .with_state((registry.clone(), capture.clone(), addr.to_string()));
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}/ohpm/"), capture)
}

async fn ohpm_handler(
    State((registry, capture, addr)): State<(Arc<Mutex<MockRegistry>>, Arc<Mutex<Capture>>, String)>,
    AxumPath(rest): AxumPath<String>,
) -> impl IntoResponse {
    let registry = registry.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((name, file)) = rest.split_once("/-/") {
        return tarball_response(&registry, &capture, name, file);
    }
    packument_response(&registry, &capture, &addr, &rest)
}

fn packument_response(
    registry: &MockRegistry,
    capture: &Arc<Mutex<Capture>>,
    addr: &str,
    name: &str,
) -> axum::response::Response {
    let Some(pkg) = registry.packages.get(name) else {
        return (StatusCode::NOT_FOUND, "not found".to_string()).into_response();
    };
    let base = format!("http://{addr}/ohpm/{name}");
    let mut versions = serde_json::Map::new();
    for (version, mv) in &pkg.versions {
        let tarball_url = format!("{base}/-/{}-{version}.har", name.replace('/', "-"));
        let dist = serde_json::json!({
            "tarball": tarball_url,
            "shasum": sha1_hex(&mv.tarball),
            "integrity": format!("sha512-{}", sha512_b64(&mv.tarball)),
        });
        let mut entry = serde_json::json!({
            "name": name,
            "version": version,
            "_ohpmVersion": "1",
            "dist": dist,
        });
        if !mv.deps.is_empty() {
            entry["dependencies"] = serde_json::to_value(&mv.deps).unwrap();
        }
        versions.insert(version.clone(), entry);
    }
    serde_json::json!({
        "name": name,
        "dist-tags": pkg.dist_tags,
        "versions": versions,
    })
    .to_string()
    .into_response()
}

fn tarball_response(
    registry: &MockRegistry,
    capture: &Arc<Mutex<Capture>>,
    name: &str,
    file: &str,
) -> axum::response::Response {
    capture.lock().unwrap_or_else(|e| e.into_inner()).tarball_gets.push(format!("{name}/{file}"));
    let Some(pkg) = registry.packages.get(name) else {
        return (StatusCode::NOT_FOUND, "not found".to_string()).into_response();
    };
    let version = file
        .strip_suffix(".har")
        .and_then(|f| f.strip_prefix(&format!("{}-", name.replace('/', "-"))))
        .unwrap_or_default()
        .to_string();
    match pkg.versions.get(&version) {
        Some(mv) => mv.tarball.clone().into_response(),
        None => (StatusCode::NOT_FOUND, "not found".to_string()).into_response(),
    }
}

fn load_config(home: &Path, registry_url: &str, cache: &Path) -> Config {
    std::env::set_var("HOME", home);
    std::env::set_var("OHPM_REGISTRY", registry_url);
    std::env::set_var("OHPM_CACHE", cache);
    std::env::set_var("OHPM_LOG_LEVEL", "error");
    let mut cfg = Config::new();
    cfg.load(Path::new("."), None).unwrap();
    cfg
}

fn write_manifest(dir: &Path, deps: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("oh-package.json5"),
        format!(
            "{{ name: \"entry\", version: \"1.0.0\", main: \"index.ets\", dependencies: {deps} }}\n"
        ),
    )
    .unwrap();
}

/// `@ohos/foo@1.2.3` -> `@ohos/bar@^1.0.0`; `@ohos/bar` 1.0.0/2.0.0
/// (latest = 2.0.0).
fn sample_registry(work: &Path) -> (MockRegistry, BTreeMap<String, Bytes>) {
    let mut registry = MockRegistry::default();
    let mut tarballs = BTreeMap::new();

    let no_deps = BTreeMap::new();
    let bar_100 = build_har(work, "@ohos/bar", "1.0.0", &no_deps);
    tarballs.insert("@ohos/bar@1.0.0".to_string(), bar_100.clone());
    let bar_200 = build_har(work, "@ohos/bar", "2.0.0", &no_deps);
    tarballs.insert("@ohos/bar@2.0.0".to_string(), bar_200.clone());
    registry.packages.insert(
        "@ohos/bar".to_string(),
        MockPackage {
            versions: BTreeMap::from([
                ("1.0.0".to_string(), MockVersion { tarball: bar_100, deps: BTreeMap::new() }),
                ("2.0.0".to_string(), MockVersion { tarball: bar_200, deps: BTreeMap::new() }),
            ]),
            dist_tags: BTreeMap::from([("latest".to_string(), "2.0.0".to_string())]),
        },
    );

    let mut foo_deps = BTreeMap::new();
    foo_deps.insert("@ohos/bar".to_string(), "^1.0.0".to_string());
    let foo_123 = build_har(work, "@ohos/foo", "1.2.3", &foo_deps);
    tarballs.insert("@ohos/foo@1.2.3".to_string(), foo_123.clone());
    registry.packages.insert(
        "@ohos/foo".to_string(),
        MockPackage {
            versions: BTreeMap::from([(
                "1.2.3".to_string(),
                MockVersion { tarball: foo_123, deps: foo_deps },
            )]),
            dist_tags: BTreeMap::from([("latest".to_string(), "1.2.3".to_string())]),
        },
    );
    (registry, tarballs)
}

// ---- tests ----

#[tokio::test]
async fn cache_add_prefetches_and_reuses() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let (registry, tarballs) = sample_registry(work.path());
    let (addr, capture) = spawn_mock(registry).await;

    let cfg = load_config(home.path(), &addr, cache.path());
    let client = RegistryClient::from_config(&cfg).unwrap();

    // `pnpm store add` — pre-fetch `@ohos/foo@1.2.3` into content-v1.
    let fetched = cache::add(&client, &cfg, &["@ohos/foo@1.2.3".to_string()])
        .await
        .unwrap();
    assert_eq!(fetched, 1);
    assert_eq!(capture.lock().unwrap().tarball_gets.len(), 1);

    // The archive lands at `<cache>/content-v1/sha512/<h[0:2]>/<h[2:4]>/<h[4:]>`.
    let digest = sha512_hex(&tarballs["@ohos/foo@1.2.3"]);
    let file = ohpm_core::install::store::cache_file_path(cache.path(), "sha512", &digest);
    assert!(file.is_file(), "{}", file.display());

    // A bare name resolves the `latest` dist-tag (bar latest = 2.0.0).
    let fetched = cache::add(&client, &cfg, &["@ohos/bar".to_string()]).await.unwrap();
    assert_eq!(fetched, 1);
    let digest = sha512_hex(&tarballs["@ohos/bar@2.0.0"]);
    assert!(ohpm_core::install::store::cache_file_path(cache.path(), "sha512", &digest).is_file());

    // Re-add: the content cache serves it — no new tarball fetch.
    let fetched = cache::add(&client, &cfg, &["@ohos/foo@1.2.3".to_string()])
        .await
        .unwrap();
    assert_eq!(fetched, 1);
    assert_eq!(
        capture.lock().unwrap().tarball_gets.len(),
        2,
        "content cache must serve the re-add"
    );
}

#[tokio::test]
async fn cache_add_rejects_unknown_and_non_registry_specs() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let (registry, _tarballs) = sample_registry(work.path());
    let (addr, _capture) = spawn_mock(registry).await;

    let cfg = load_config(home.path(), &addr, cache.path());
    let client = RegistryClient::from_config(&cfg).unwrap();

    // An unknown package fails with the registry fetch error.
    let err = cache::add(&client, &cfg, &["@ohos/nope@1.0.0".to_string()])
        .await
        .unwrap_err();
    assert!(err.message.contains("@ohos/nope"), "{}", err.message);

    // A local source dir is not pre-fetchable.
    let local = work.path().join("local-pkg").to_string_lossy().into_owned();
    let err = cache::add(&client, &cfg, &[local]).await.unwrap_err();
    assert_eq!(err.code, "CacheAddNotSupport");
}

#[tokio::test]
async fn cache_status_detects_corruption() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let (registry, tarballs) = sample_registry(work.path());
    let (addr, _capture) = spawn_mock(registry).await;

    let cfg = load_config(home.path(), &addr, cache.path());
    let client = RegistryClient::from_config(&cfg).unwrap();
    cache::add(&client, &cfg, &["@ohos/foo@1.2.3".to_string()])
        .await
        .unwrap();

    // A clean store passes.
    assert!(cache::status(&cfg).unwrap().is_empty());

    // Corrupt the cached archive → `pnpm store status`-style report.
    let digest = sha512_hex(&tarballs["@ohos/foo@1.2.3"]);
    let file = ohpm_core::install::store::cache_file_path(cache.path(), "sha512", &digest);
    std::fs::write(&file, "garbage").unwrap();
    let corrupted = cache::status(&cfg).unwrap();
    assert_eq!(corrupted, vec![file]);
}

#[test]
fn cache_path_follows_config() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let cfg = load_config(home.path(), "https://example.invalid/", cache.path());
    assert_eq!(cache::store_path(&cfg), cache.path().to_path_buf());
}

#[tokio::test]
async fn cli_cache_flag_selects_store_dir() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let flag_dir = tempfile::TempDir::new().unwrap();
    let (registry, _tarballs) = sample_registry(work.path());
    let (addr, _capture) = spawn_mock(registry).await;

    let mut cfg = load_config(home.path(), &addr, cache.path());
    // `--cache <dir>` feeds the cli config layer (apply_cli_options).
    cfg.set_cli(types::CACHE, flag_dir.path().to_str().unwrap());

    let prefix = work.path().join("entry");
    write_manifest(&prefix, "{ \"@ohos/foo\": \"^1.2.3\" }");
    let client = RegistryClient::from_config(&cfg).unwrap();
    install(&client, &cfg, &prefix, &[], &InstallOptions::default())
        .await
        .unwrap();

    assert!(flag_dir.path().join("content-v1").is_dir());
    assert!(!cache.path().join("content-v1").exists());
}

#[tokio::test]
async fn hardlink_mode_shares_inodes_across_projects() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let (registry, _tarballs) = sample_registry(work.path());
    let (addr, _capture) = spawn_mock(registry).await;

    let mut cfg = load_config(home.path(), &addr, cache.path());
    cfg.set_cli(types::CACHE_HARDLINK, "true");

    let prefix1 = work.path().join("p1");
    let prefix2 = work.path().join("p2");
    write_manifest(&prefix1, "{ \"@ohos/foo\": \"^1.2.3\" }");
    write_manifest(&prefix2, "{ \"@ohos/foo\": \"^1.2.3\" }");
    let client = RegistryClient::from_config(&cfg).unwrap();
    install(&client, &cfg, &prefix1, &[], &InstallOptions::default())
        .await
        .unwrap();
    install(&client, &cfg, &prefix2, &[], &InstallOptions::default())
        .await
        .unwrap();

    // Both projects' store files are hard links into the same inode.
    use std::os::unix::fs::MetadataExt;
    let rel = "oh_modules/.ohpm/@ohos+foo@1.2.3/oh_modules/@ohos/foo/oh-package.json5";
    let f1 = prefix1.join(rel);
    let f2 = prefix2.join(rel);
    assert!(f1.is_file() && f2.is_file(), "{f1:?} {f2:?}");
    assert_eq!(
        std::fs::metadata(&f1).unwrap().ino(),
        std::fs::metadata(&f2).unwrap().ino(),
        "hard-link mode must share inodes"
    );

    // The shared extracted layer materialized once.
    let extracted = cache.path().join("extracted-v1");
    assert!(extracted.is_dir());

    // `cache clean` removes both the content and the extracted layers.
    cache::clean_all(&cfg).unwrap();
    assert!(!cache.path().join("content-v1").exists());
    assert!(!extracted.exists());
}

#[tokio::test]
async fn default_mode_installs_independent_copies() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let (registry, _tarballs) = sample_registry(work.path());
    let (addr, _capture) = spawn_mock(registry).await;

    let cfg = load_config(home.path(), &addr, cache.path());

    let prefix1 = work.path().join("p1");
    let prefix2 = work.path().join("p2");
    write_manifest(&prefix1, "{ \"@ohos/foo\": \"^1.2.3\" }");
    write_manifest(&prefix2, "{ \"@ohos/foo\": \"^1.2.3\" }");
    let client = RegistryClient::from_config(&cfg).unwrap();
    install(&client, &cfg, &prefix1, &[], &InstallOptions::default())
        .await
        .unwrap();
    install(&client, &cfg, &prefix2, &[], &InstallOptions::default())
        .await
        .unwrap();

    // Default (off): each project extracts its own copy — distinct inodes,
    // and no shared extracted layer.
    use std::os::unix::fs::MetadataExt;
    let rel = "oh_modules/.ohpm/@ohos+foo@1.2.3/oh_modules/@ohos/foo/oh-package.json5";
    assert_ne!(
        std::fs::metadata(prefix1.join(&rel)).unwrap().ino(),
        std::fs::metadata(prefix2.join(&rel)).unwrap().ino()
    );
    assert!(!cache.path().join("extracted-v1").exists());
}
