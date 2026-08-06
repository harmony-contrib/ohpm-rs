//! End-to-end install flow tests against a local mock registry.
//!
//! These verify the install pipeline: resolution → lockfile → store → symlinks
//! → install record, byte-compatible with the reference's artifacts.

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
use ohpm_core::config::Config;
use ohpm_core::install::{install, InstallOptions};
use ohpm_core::registry::RegistryClient;
use serde_json::Value;

/// Env-var isolation: saves the variables this suite touches and restores them
/// on drop, so a panicking test can't leak state into the next one.
struct EnvGuard(Vec<(&'static str, Option<String>)>);

impl EnvGuard {
    fn new() -> Self {
        let keys = [
            "HOME",
            "OHPM_REGISTRY",
            "OHPM_CACHE",
            "OHPM_READ_ACCESS_TOKEN",
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

/// Minimal log facade so the pipeline's `log::debug!` calls are visible.
fn init_test_logger() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        struct TestLogger;
        impl log::Log for TestLogger {
            fn enabled(&self, _m: &log::Metadata) -> bool {
                true
            }
            fn log(&self, r: &log::Record) {
                eprintln!("[{}] {}", r.level(), r.args());
            }
            fn flush(&self) {}
        }
        let _ = log::set_logger(&TestLogger);
        log::set_max_level(log::LevelFilter::Debug);
    });
}

/// A mock registry package: versions with tarball bytes + dependencies.
#[derive(Clone)]
struct MockVersion {
    tarball: Bytes,
    hsp: Option<Bytes>,
    hsp_type: Option<&'static str>,
    deps: BTreeMap<String, String>,
    dev_deps: BTreeMap<String, String>,
}

#[derive(Clone, Default)]
struct MockPackage {
    versions: BTreeMap<String, MockVersion>,
    dist_tags: BTreeMap<String, String>,
}

#[derive(Clone, Default)]
struct MockRegistry {
    packages: BTreeMap<String, MockPackage>,
    /// Whether to serve tarballs whose integrity does not match the packument.
    corrupt_tarballs: bool,
}

/// What the mock received.
#[derive(Debug, Default)]
struct Capture {
    packument_gets: Vec<String>,
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
    build_har_with_manifest(dir, name, version, &manifest)
}

fn build_har_with_manifest(dir: &Path, _name: &str, _version: &str, manifest: &str) -> Bytes {
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

fn sha1_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let mut h = sha1::Sha1::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

async fn spawn_mock(
    registry: MockRegistry,
) -> (String, Arc<Mutex<MockRegistry>>, Arc<Mutex<Capture>>) {
    let capture = Arc::new(Mutex::new(Capture::default()));
    let registry = Arc::new(Mutex::new(registry));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // `{*rest}`: scoped names contain "/" and are not a single path segment.
    let router = Router::new()
        .route("/ohpm/*rest", get(ohpm_handler))
        .with_state((registry.clone(), capture.clone(), addr.to_string()));
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}/ohpm/"), registry, capture)
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
    capture.lock().unwrap_or_else(|e| e.into_inner()).packument_gets.push(name.to_string());
    let Some(pkg) = registry.packages.get(name) else {
        return (StatusCode::NOT_FOUND, "not found".to_string()).into_response();
    };
    let base = format!("http://{addr}/ohpm/{name}");
    let mut versions = serde_json::Map::new();
    for (version, mv) in &pkg.versions {
        let tarball_url = format!("{base}/-/{}-{version}.har", name.replace('/', "-"));
        let mut dist = serde_json::json!({
            "tarball": tarball_url,
            "shasum": sha1_hex(&mv.tarball),
        });
        if let Some(hsp) = &mv.hsp {
            let hsp_url = format!("{base}/-/{}-{version}.hsp", name.replace('/', "-"));
            dist["resolved_hsp"] = serde_json::Value::String(hsp_url);
            dist["integrity_hsp"] =
                serde_json::Value::String(format!("sha512-{}", sha512_b64(hsp)));
        }
        if !registry.corrupt_tarballs {
            dist["integrity"] = serde_json::Value::String(format!("sha512-{}", sha512_b64(&mv.tarball)));
        } else {
            dist["integrity"] = serde_json::Value::String(format!("sha512-{}", sha512_b64(b"corrupted")));
        }
        let mut entry = serde_json::json!({
            "name": name,
            "version": version,
            "_ohpmVersion": "1",
            "dist": dist,
        });
        if let Some(hsp_type) = mv.hsp_type {
            entry["packageType"] = serde_json::Value::String("InterfaceHar".into());
            entry["hspType"] = serde_json::Value::String(hsp_type.into());
        }
        if !mv.deps.is_empty() {
            entry["dependencies"] = serde_json::to_value(&mv.deps).unwrap();
        }
        if !mv.dev_deps.is_empty() {
            entry["devDependencies"] = serde_json::to_value(&mv.dev_deps).unwrap();
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
    // `foo-1.2.3.har` / `foo-1.2.3.hsp` -> version `1.2.3`
    let version = file
        .strip_suffix(".har")
        .or_else(|| file.strip_suffix(".hsp"))
        .and_then(|f| f.strip_prefix(&format!("{}-", name.replace('/', "-"))))
        .unwrap_or_default()
        .to_string();
    match pkg.versions.get(&version) {
        Some(mv) if file.ends_with(".hsp") => match &mv.hsp {
            Some(hsp) => hsp.clone().into_response(),
            None => (StatusCode::NOT_FOUND, "not found".to_string()).into_response(),
        },
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

fn write_manifest(dir: &Path, deps: &str, dev: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("oh-package.json5"),
        format!(
            "{{ name: \"entry\", version: \"1.0.0\", main: \"index.ets\", dependencies: {deps}, devDependencies: {dev} }}\n"
        ),
    )
    .unwrap();
}

/// A sample registry: `@ohos/foo@1.2.3` -> `@ohos/bar@^1.0.0`,
/// `@ohos/bar` 1.0.0/2.0.0, `unittest@1.0.0`.
fn sample_registry(work: &Path) -> (MockRegistry, BTreeMap<String, Bytes>) {
    let mut registry = MockRegistry::default();
    let mut tarballs = BTreeMap::new();

    let bar_deps = BTreeMap::new();
    let bar_100 = build_har(work, "@ohos/bar", "1.0.0", &bar_deps);
    tarballs.insert("@ohos/bar@1.0.0".to_string(), bar_100.clone());
    let bar_200 = build_har(work, "@ohos/bar", "2.0.0", &bar_deps);
    tarballs.insert("@ohos/bar@2.0.0".to_string(), bar_200.clone());
    registry.packages.insert(
        "@ohos/bar".to_string(),
        MockPackage {
            versions: BTreeMap::from([
                ("1.0.0".to_string(), MockVersion { tarball: bar_100, hsp: None, hsp_type: None, deps: BTreeMap::new(), dev_deps: BTreeMap::new() }),
                ("2.0.0".to_string(), MockVersion { tarball: bar_200, hsp: None, hsp_type: None, deps: BTreeMap::new(), dev_deps: BTreeMap::new() }),
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
                MockVersion { tarball: foo_123, hsp: None, hsp_type: None, deps: foo_deps, dev_deps: BTreeMap::new() },
            )]),
            dist_tags: BTreeMap::from([("latest".to_string(), "1.2.3".to_string())]),
        },
    );

    let unittest_100 = build_har(work, "unittest", "1.0.0", &BTreeMap::new());
    tarballs.insert("unittest@1.0.0".to_string(), unittest_100.clone());
    registry.packages.insert(
        "unittest".to_string(),
        MockPackage {
            versions: BTreeMap::from([(
                "1.0.0".to_string(),
                MockVersion { tarball: unittest_100, hsp: None, hsp_type: None, deps: BTreeMap::new(), dev_deps: BTreeMap::new() },
            )]),
            dist_tags: BTreeMap::from([("latest".to_string(), "1.0.0".to_string())]),
        },
    );
    (registry, tarballs)
}

async fn run_install(
    cfg: &Config,
    prefix: &Path,
    args: &[String],
    opts: &InstallOptions,
) -> ohpm_core::Result<ohpm_core::install::InstallOutcome> {
    let client = RegistryClient::from_config(cfg)?;
    install(&client, cfg, prefix, args, opts).await
}

#[tokio::test]
async fn fresh_install_layout_and_lockfile() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let (registry, _tarballs) = sample_registry(work.path());
    let (addr, _registry, capture) = spawn_mock(registry).await;

    let prefix = work.path().join("entry");
    write_manifest(&prefix, "{ \"@ohos/foo\": \"^1.2.0\" }", "{ \"unittest\": \"1.0.0\" }");
    let cfg = load_config(home.path(), &addr, cache.path());

    let outcome = run_install(&cfg, &prefix, &[], &InstallOptions::default()).await.unwrap();
    assert_eq!(outcome.installed, 3);

    // 1. oh-package-lock.json5 — specifiers + packages.
    let lock_text = std::fs::read_to_string(prefix.join("oh-package-lock.json5")).unwrap();
    let lock: Value = json5::from_str(&lock_text).unwrap();
    assert_eq!(lock["lockfileVersion"], 3);
    assert_eq!(lock["meta"]["stableOrder"], true);
    assert_eq!(lock["meta"]["enableUnifiedLockfile"], false);
    let specifiers = lock["specifiers"].as_object().unwrap();
    assert_eq!(specifiers["@ohos/bar@^1.0.0"], "@ohos/bar@1.0.0");
    assert_eq!(specifiers["@ohos/foo@^1.2.0"], "@ohos/foo@1.2.3");
    assert_eq!(specifiers["unittest@1.0.0"], "unittest@1.0.0");
    let packages = lock["packages"].as_object().unwrap();
    assert_eq!(packages.len(), 3);
    let foo_pkg = &packages["@ohos/foo@1.2.3"];
    assert_eq!(foo_pkg["name"], "@ohos/foo");
    assert_eq!(foo_pkg["registryType"], "ohpm");
    // The lockfile records the metadata's declared spec.
    assert_eq!(foo_pkg["dependencies"]["@ohos/bar"], "^1.0.0");
    assert!(foo_pkg["integrity"].as_str().unwrap().starts_with("sha512-"));
    assert!(foo_pkg["resolved"].as_str().unwrap().ends_with("/ohpm/@ohos/foo/-/@ohos-foo-1.2.3.har"));
    assert_eq!(packages["@ohos/bar@1.0.0"]["version"], "1.0.0");

    // 2. oh_modules layout — symlinks into the store.
    let oh_modules = prefix.join("oh_modules");
    let foo_link = std::fs::read_link(oh_modules.join("@ohos/foo")).unwrap();
    assert_eq!(
        foo_link,
        PathBuf::from("../.ohpm/@ohos+foo@1.2.3/oh_modules/@ohos/foo")
    );
    let bar_link = std::fs::read_link(oh_modules.join(".ohpm/@ohos+foo@1.2.3/oh_modules/@ohos/bar")).unwrap();
    assert_eq!(
        bar_link,
        PathBuf::from("../../../@ohos+bar@1.0.0/oh_modules/@ohos/bar")
    );

    // 3. Store contents (extracted, strip=1, manifest renamed).
    let foo_store = oh_modules.join(".ohpm/@ohos+foo@1.2.3/oh_modules/@ohos/foo");
    assert!(foo_store.join("oh-package.json5").is_file());
    assert!(foo_store.join("entry/index.ets").is_file());

    // 4. Install record.
    let record: Value = json5::from_str(
        &std::fs::read_to_string(oh_modules.join(".ohpm/lock.json5")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["lockVersion"], "1.0");
    assert_eq!(record["settings"]["resolveConflict"], true);
    assert_eq!(record["settings"]["installAll"], true); // install_all config defaults to true
    let modules = record["modules"].as_object().unwrap();
    assert_eq!(modules["."]["dependencies"]["@ohos/foo"]["specifier"], "^1.2.0");
    assert_eq!(modules["."]["dependencies"]["@ohos/foo"]["version"], "1.2.3");
    assert_eq!(modules["."]["devDependencies"]["unittest"]["version"], "1.0.0");
    let rec_packages = record["packages"].as_object().unwrap();
    assert_eq!(rec_packages["@ohos/foo@1.2.3"]["storePath"], "oh_modules/.ohpm/@ohos+foo@1.2.3");
    assert_eq!(rec_packages["@ohos/foo@1.2.3"]["dependencies"]["@ohos/bar"], "1.0.0");
    assert_eq!(rec_packages["unittest@1.0.0"]["dev"], true);

    // 5. Request counts: 3 packuments + 3 tarballs.
    let cap = capture.lock().unwrap();
    assert_eq!(cap.packument_gets.len(), 3);
    assert_eq!(cap.tarball_gets.len(), 3);
    drop(cap);

    // 6. Re-install: lockfile-first — no new packument or tarball fetches.
    run_install(&cfg, &prefix, &[], &InstallOptions::default()).await.unwrap();
    let cap = capture.lock().unwrap();
    assert_eq!(cap.packument_gets.len(), 3, "lockfile-first re-install must not re-fetch metadata");
    assert_eq!(cap.tarball_gets.len(), 3, "content cache must serve the second run");
}

#[tokio::test]
async fn cli_input_saves_resolved_version() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let (registry, _tarballs) = sample_registry(work.path());
    let (addr, _registry, capture) = spawn_mock(registry).await;

    let prefix = work.path().join("entry");
    write_manifest(&prefix, "{}", "{}");
    let cfg = load_config(home.path(), &addr, cache.path());

    // `ohpm install @ohos/foo` — the manifest gets `"@ohos/foo": "^1.2.3"`.
    let opts = InstallOptions::default();
    run_install(&cfg, &prefix, &["@ohos/foo".to_string()], &opts).await.unwrap();

    let manifest_text = std::fs::read_to_string(prefix.join("oh-package.json5")).unwrap();
    assert!(manifest_text.contains("\"@ohos/foo\": \"^1.2.3\""), "{manifest_text}");

    let lock_text = std::fs::read_to_string(prefix.join("oh-package-lock.json5")).unwrap();
    assert!(lock_text.contains("\"@ohos/foo@^1.2.3\": \"@ohos/foo@1.2.3\""), "{lock_text}");
    assert!(!lock_text.contains("foo@latest"), "the bare specifier must be replaced");

    // `--no-save` leaves the manifest untouched; the specifier uses `latest`.
    let prefix2 = work.path().join("entry2");
    write_manifest(&prefix2, "{}", "{}");
    let before = std::fs::read_to_string(prefix2.join("oh-package.json5")).unwrap();
    let opts = InstallOptions {
        save: false,
        ..Default::default()
    };
    run_install(&cfg, &prefix2, &["unittest".to_string()], &opts).await.unwrap();
    let after = std::fs::read_to_string(prefix2.join("oh-package.json5")).unwrap();
    assert_eq!(before, after, "--no-save must not touch the manifest");
    let lock_text = std::fs::read_to_string(prefix2.join("oh-package-lock.json5")).unwrap();
    assert!(lock_text.contains("\"unittest@latest\": \"unittest@1.0.0\""), "{lock_text}");

    let cap = capture.lock().unwrap();
    assert!(cap.packument_gets.iter().any(|n| n == "@ohos/foo"));
    assert!(cap.packument_gets.iter().any(|n| n == "unittest"));
}

#[tokio::test]
async fn integrity_mismatch_fails() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let (mut registry, _tarballs) = sample_registry(work.path());
    registry.corrupt_tarballs = true;
    let (addr, _registry, _capture) = spawn_mock(registry).await;

    let prefix = work.path().join("entry");
    write_manifest(&prefix, "{ \"@ohos/foo\": \"^1.2.0\" }", "{}");
    let cfg = load_config(home.path(), &addr, cache.path());

    let err = run_install(&cfg, &prefix, &[], &InstallOptions::default()).await.unwrap_err();
    assert_eq!(err.code, "CacheInvalidCachePackage");
}

#[tokio::test]
async fn missing_package_and_unresolvable_spec() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let (registry, _tarballs) = sample_registry(work.path());
    let (addr, _registry, _capture) = spawn_mock(registry).await;

    // Unknown package -> FetcherRegistryFetchPkgInfoFailed.
    let prefix = work.path().join("entry");
    write_manifest(&prefix, "{ \"nope\": \"^1.0.0\" }", "{}");
    let cfg = load_config(home.path(), &addr, cache.path());
    let err = run_install(&cfg, &prefix, &[], &InstallOptions::default()).await.unwrap_err();
    assert_eq!(err.code, "FetcherRegistryFetchPkgInfoFailed");

    // Known package whose spec does not resolve in any registry -> the
    // reference also surfaces FetcherRegistryFetchPkgInfoFailed (the
    // per-registry check happens before pinned selection).
    let prefix2 = work.path().join("entry2");
    write_manifest(&prefix2, "{ \"@ohos/foo\": \"^9.0.0\" }", "{}");
    let err = run_install(&cfg, &prefix2, &[], &InstallOptions::default()).await.unwrap_err();
    assert_eq!(err.code, "FetcherRegistryFetchPkgInfoFailed");
}

#[tokio::test]
async fn local_file_dependency() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let (registry, _tarballs) = sample_registry(work.path());
    let (addr, _registry, _capture) = spawn_mock(registry).await;

    // A local .har dependency next to the project.
    let lib = work.path().join("lib");
    let har_path = lib.join("mylib-1.0.0.har");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(&har_path, build_har(&lib, "mylib", "1.0.0", &BTreeMap::new())).unwrap();

    let prefix = work.path().join("entry");
    write_manifest(&prefix, "{ \"mylib\": \"file:../lib/mylib-1.0.0.har\" }", "{}");
    let cfg = load_config(home.path(), &addr, cache.path());

    let outcome = run_install(&cfg, &prefix, &[], &InstallOptions::default()).await.unwrap();
    assert_eq!(outcome.installed, 1);

    // The local artifact store dir is name@<content-hash>.
    let oh_modules = prefix.join("oh_modules");
    let entries = std::fs::read_dir(oh_modules.join(".ohpm")).unwrap();
    let store_dirs: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("mylib@"))
        .collect();
    assert_eq!(store_dirs.len(), 1, "local artifact store dirs: {store_dirs:?}");
    let store = oh_modules.join(".ohpm").join(&store_dirs[0]).join("oh_modules/mylib");
    assert!(store.join("oh-package.json5").is_file());

    let link = std::fs::read_link(oh_modules.join("mylib")).unwrap();
    assert_eq!(link, PathBuf::from(format!(".ohpm/{}/oh_modules/mylib", store_dirs[0])));

    // The lockfile records the file dependency with a relative path.
    let lock_text = std::fs::read_to_string(prefix.join("oh-package-lock.json5")).unwrap();
    assert!(
        lock_text.contains("mylib@../lib/mylib-1.0.0.har"),
        "the lockfile must record the local file specifier: {lock_text}"
    );
}

#[tokio::test]
async fn update_moves_to_new_version() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let (registry, _tarballs) = sample_registry(work.path());
    let (addr, registry_handle, _capture) = spawn_mock(registry).await;

    let prefix = work.path().join("entry");
    write_manifest(&prefix, "{ \"@ohos/foo\": \"^1.2.0\" }", "{}");
    let cfg = load_config(home.path(), &addr, cache.path());

    // Install pins 1.2.3.
    run_install(&cfg, &prefix, &[], &InstallOptions::default()).await.unwrap();
    let lock_text = std::fs::read_to_string(prefix.join("oh-package-lock.json5")).unwrap();
    assert!(lock_text.contains("\"@ohos/foo@^1.2.0\": \"@ohos/foo@1.2.3\""), "{lock_text}");

    // The registry gains 1.3.0.
    let new_har = build_har(work.path(), "@ohos/foo", "1.3.0", &BTreeMap::new());
    let mut r = registry_handle.lock().unwrap();
    r.packages
        .get_mut("@ohos/foo")
        .unwrap()
        .versions
        .insert(
            "1.3.0".to_string(),
            MockVersion {
                tarball: new_har,
                hsp: None,
                hsp_type: None,
                deps: BTreeMap::new(),
                dev_deps: BTreeMap::new(),
            },
        );
    drop(r);

    // `ohpm update` (no args) clears the specifiers and re-resolves.
    let client = RegistryClient::from_config(&cfg).unwrap();
    ohpm_core::install::update(&client, &cfg, &prefix, &[], &InstallOptions::default())
        .await
        .unwrap();
    let lock_text = std::fs::read_to_string(prefix.join("oh-package-lock.json5")).unwrap();
    assert!(lock_text.contains("\"@ohos/foo@^1.2.0\": \"@ohos/foo@1.3.0\""), "{lock_text}");
    assert!(prefix.join("oh_modules/.ohpm/@ohos+foo@1.3.0/oh_modules/@ohos/foo/oh-package.json5").is_file());
    let _ = registry;
}

#[tokio::test]
async fn uninstall_removes_package() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let (registry, _tarballs) = sample_registry(work.path());
    let (addr, _registry_handle, _capture) = spawn_mock(registry).await;

    let prefix = work.path().join("entry");
    write_manifest(&prefix, "{ \"@ohos/foo\": \"^1.2.0\" }", "{}");
    let cfg = load_config(home.path(), &addr, cache.path());

    run_install(&cfg, &prefix, &[], &InstallOptions::default()).await.unwrap();
    assert!(prefix.join("oh_modules/@ohos/foo").exists());

    // `ohpm uninstall @ohos/foo` — removed from the manifest, the lockfile and
    // the top-level links.
    let client = RegistryClient::from_config(&cfg).unwrap();
    ohpm_core::install::uninstall(&client, &cfg, &prefix, &["@ohos/foo".to_string()], &InstallOptions::default())
        .await
        .unwrap();

    let manifest_text = std::fs::read_to_string(prefix.join("oh-package.json5")).unwrap();
    assert!(!manifest_text.contains("@ohos/foo"), "{manifest_text}");
    let lock_text = std::fs::read_to_string(prefix.join("oh-package-lock.json5")).unwrap();
    assert!(!lock_text.contains("@ohos/foo"), "{lock_text}");
    assert!(!prefix.join("oh_modules/@ohos/foo").exists(), "top-level link removed");
    // The install record is not rewritten when the graph is empty (mirrors
    // `installModules`' `packages.length === 0` early return) — the previous
    // file stays, like the reference.
    assert!(prefix.join("oh_modules/.ohpm/lock.json5").is_file());

    // Versions in arguments are rejected.
    let err = ohpm_core::install::uninstall(
        &client,
        &cfg,
        &prefix,
        &["@ohos/foo@1.0.0".to_string()],
        &InstallOptions::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, "UninstallHasVersion");
    let err = ohpm_core::install::update(
        &client,
        &cfg,
        &prefix,
        &["@ohos/foo@1.0.0".to_string()],
        &InstallOptions::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, "UpdateHasVersion");
}

#[tokio::test]
async fn override_dependency_map_masks_node_deps() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let (registry, _tarballs) = sample_registry(work.path());
    let (addr, _registry, capture) = spawn_mock(registry).await;

    let prefix = work.path().join("entry");
    std::fs::create_dir_all(&prefix).unwrap();
    std::fs::write(
        prefix.join("oh-package.json5"),
        "{\n  name: \"entry\", version: \"1.0.0\",\n  dependencies: { \"@ohos/foo\": \"^1.2.0\" },\n  overrideDependencyMap: { \"@ohos/foo\": \"./foo-override.json5\" },\n}\n",
    )
    .unwrap();
    // The override entry REPLACES foo's own maps: bar is dropped, unittest is
    // injected instead.
    std::fs::write(
        prefix.join("foo-override.json5"),
        "{ dependencies: { \"unittest\": \"1.0.0\" } }\n",
    )
    .unwrap();
    let cfg = load_config(home.path(), &addr, cache.path());

    let outcome = run_install(&cfg, &prefix, &[], &InstallOptions::default()).await.unwrap();
    assert_eq!(outcome.installed, 2, "foo + unittest; bar must not be installed");

    // The masked node's requirements come from the override entry — bar is
    // never resolved.
    let cap = capture.lock().unwrap();
    assert!(!cap.packument_gets.iter().any(|n| n == "@ohos/bar"), "bar not fetched: {:?}", cap.packument_gets);
    drop(cap);

    // Lockfile: the package keeps its ORIGINAL deps (bar) and carries the
    // masked tag (`addOrRemoveOverrideDependencyMapTag`).
    let lock: Value = json5::from_str(
        &std::fs::read_to_string(prefix.join("oh-package-lock.json5")).unwrap(),
    )
    .unwrap();
    let foo_pkg = &lock["packages"]["@ohos/foo@1.2.3"];
    assert_eq!(foo_pkg["dependencies"]["@ohos/bar"], "^1.0.0");
    assert_eq!(foo_pkg["maskedByOverrideDependencyMap"], true);
    assert!(lock["packages"]["@ohos/bar@1.0.0"].is_null(), "no bar in the lockfile");

    // Install record: the masked package shows the override deps and the tag;
    // the record's overrideDependencyMap carries the override entry.
    let record: Value = json5::from_str(
        &std::fs::read_to_string(prefix.join("oh_modules/.ohpm/lock.json5")).unwrap(),
    )
    .unwrap();
    let rec_foo = &record["packages"]["@ohos/foo@1.2.3"];
    assert_eq!(rec_foo["maskedByOverrideDependencyMap"], true);
    assert_eq!(rec_foo["dependencies"]["unittest"], "1.0.0");
    assert!(rec_foo["dependencies"]["@ohos/bar"].is_null());
    assert_eq!(
        record["overrideDependencyMap"]["@ohos/foo"]["dependencies"]["unittest"],
        "1.0.0"
    );
    // The project module entry itself is not masked (roots never are).
    assert_eq!(record["modules"]["."]["maskedByOverrideDependencyMap"], false);
}

#[tokio::test]
async fn exclusions_remove_deps_from_node() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let (registry, _tarballs) = sample_registry(work.path());
    let (addr, _registry, capture) = spawn_mock(registry).await;

    let prefix = work.path().join("entry");
    std::fs::create_dir_all(&prefix).unwrap();
    std::fs::write(
        prefix.join("oh-package.json5"),
        "{\n  name: \"entry\", version: \"1.0.0\",\n  dependencies: { \"@ohos/foo\": \"^1.2.0\" },\n  exclusions: { \"@ohos/foo\": [\"@ohos/bar\"] },\n}\n",
    )
    .unwrap();
    let cfg = load_config(home.path(), &addr, cache.path());

    let outcome = run_install(&cfg, &prefix, &[], &InstallOptions::default()).await.unwrap();
    assert_eq!(outcome.installed, 1, "foo only — bar excluded");

    let cap = capture.lock().unwrap();
    assert!(!cap.packument_gets.iter().any(|n| n == "@ohos/bar"), "bar not fetched: {:?}", cap.packument_gets);
    drop(cap);

    // Lockfile: the exclusion mutates the node's own maps, so the package's
    // deps are the post-exclusion view; the masked tag is written.
    let lock: Value = json5::from_str(
        &std::fs::read_to_string(prefix.join("oh-package-lock.json5")).unwrap(),
    )
    .unwrap();
    let foo_pkg = &lock["packages"]["@ohos/foo@1.2.3"];
    assert!(foo_pkg["dependencies"]["@ohos/bar"].is_null());
    assert_eq!(foo_pkg["maskedByOverrideDependencyMap"], true);

    // Install record: the final map records foo's remaining (empty) deps.
    let record: Value = json5::from_str(
        &std::fs::read_to_string(prefix.join("oh_modules/.ohpm/lock.json5")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["packages"]["@ohos/foo@1.2.3"]["maskedByOverrideDependencyMap"], true);
    assert!(
        record["packages"]["@ohos/foo@1.2.3"]["dependencies"]
            .as_object()
            .is_some_and(|d| d.is_empty()),
        "foo's record deps must be the post-exclusion (empty) view"
    );
    let ex_final = &record["overrideDependencyMap"]["@ohos/foo"];
    assert!(ex_final["dependencies"].is_object() && ex_final["dependencies"].as_object().unwrap().is_empty());
}

#[tokio::test]
async fn version_conflict_resolves_to_max() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();

    // foo requires bar@^1.0.0 (pins 1.0.0), conflictee requires bar@^2.0.0
    // (pins 2.0.0) — a version conflict, resolved to the max (2.0.0).
    let mut registry = MockRegistry::default();
    let bar_100 = build_har(work.path(), "@ohos/bar", "1.0.0", &BTreeMap::new());
    let bar_200 = build_har(work.path(), "@ohos/bar", "2.0.0", &BTreeMap::new());
    registry.packages.insert(
        "@ohos/bar".to_string(),
        MockPackage {
            versions: BTreeMap::from([
                ("1.0.0".to_string(), MockVersion { tarball: bar_100, hsp: None, hsp_type: None, deps: BTreeMap::new(), dev_deps: BTreeMap::new() }),
                ("2.0.0".to_string(), MockVersion { tarball: bar_200, hsp: None, hsp_type: None, deps: BTreeMap::new(), dev_deps: BTreeMap::new() }),
            ]),
            dist_tags: BTreeMap::from([("latest".to_string(), "2.0.0".to_string())]),
        },
    );
    let mut foo_deps = BTreeMap::new();
    foo_deps.insert("@ohos/bar".to_string(), "^1.0.0".to_string());
    let foo_123 = build_har(work.path(), "@ohos/foo", "1.2.3", &foo_deps);
    registry.packages.insert(
        "@ohos/foo".to_string(),
        MockPackage {
            versions: BTreeMap::from([("1.2.3".to_string(), MockVersion { tarball: foo_123, hsp: None, hsp_type: None, deps: foo_deps, dev_deps: BTreeMap::new() })]),
            dist_tags: BTreeMap::from([("latest".to_string(), "1.2.3".to_string())]),
        },
    );
    let mut conflict_deps = BTreeMap::new();
    conflict_deps.insert("@ohos/bar".to_string(), "^2.0.0".to_string());
    let conflict_100 = build_har(work.path(), "conflictee", "1.0.0", &conflict_deps);
    registry.packages.insert(
        "conflictee".to_string(),
        MockPackage {
            versions: BTreeMap::from([("1.0.0".to_string(), MockVersion { tarball: conflict_100, hsp: None, hsp_type: None, deps: conflict_deps, dev_deps: BTreeMap::new() })]),
            dist_tags: BTreeMap::from([("latest".to_string(), "1.0.0".to_string())]),
        },
    );
    let (addr, _registry, _capture) = spawn_mock(registry).await;

    let prefix = work.path().join("entry");
    write_manifest(&prefix, "{ \"@ohos/foo\": \"^1.2.0\", \"conflictee\": \"^1.0.0\" }", "{}");
    let cfg = load_config(home.path(), &addr, cache.path());

    let outcome = run_install(&cfg, &prefix, &[], &InstallOptions::default()).await.unwrap();
    assert_eq!(outcome.installed, 3, "foo + conflictee + bar@2.0.0 only");

    // Lockfile: both bar specifiers point at 2.0.0; the 1.0.0 package entry
    // was deleted by `resolveVersionConflict2LockFile`.
    let lock: Value = json5::from_str(
        &std::fs::read_to_string(prefix.join("oh-package-lock.json5")).unwrap(),
    )
    .unwrap();
    let specifiers = lock["specifiers"].as_object().unwrap();
    assert_eq!(specifiers["@ohos/bar@^1.0.0"], "@ohos/bar@2.0.0");
    assert_eq!(specifiers["@ohos/bar@^2.0.0"], "@ohos/bar@2.0.0");
    let packages = lock["packages"].as_object().unwrap();
    assert!(packages.contains_key("@ohos/bar@2.0.0"));
    assert!(!packages.contains_key("@ohos/bar@1.0.0"), "old version pruned: {:?}", packages.keys());

    // Symlinks: every bar edge resolves to the max version.
    let bar_link = std::fs::read_link(
        prefix.join("oh_modules/.ohpm/@ohos+foo@1.2.3/oh_modules/@ohos/bar"),
    )
    .unwrap();
    assert_eq!(
        bar_link,
        PathBuf::from("../../../@ohos+bar@2.0.0/oh_modules/@ohos/bar"),
        "foo's bar edge must resolve to 2.0.0"
    );

    // Install record: foo's dependencies show the resolved version.
    let record: Value = json5::from_str(
        &std::fs::read_to_string(prefix.join("oh_modules/.ohpm/lock.json5")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["packages"]["@ohos/foo@1.2.3"]["dependencies"]["@ohos/bar"], "2.0.0");
    assert!(!record["packages"].as_object().unwrap().contains_key("@ohos/bar@1.0.0"));
}

#[tokio::test]
async fn parameterized_install_substitutes_deps() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let (registry, _tarballs) = sample_registry(work.path());
    let (addr, _registry, _capture) = spawn_mock(registry).await;

    let prefix = work.path().join("entry");
    std::fs::create_dir_all(&prefix).unwrap();
    // The manifest's dependency spec is parameterized via `parameterFile`.
    std::fs::write(
        prefix.join("oh-package.json5"),
        "{\n  name: \"entry\", version: \"1.0.0\",\n  dependencies: { \"@ohos/foo\": \"@param:deps.foo\" },\n  parameterFile: \"./params.json5\",\n}\n",
    )
    .unwrap();
    std::fs::write(
        prefix.join("params.json5"),
        "{ deps: { foo: \"^1.2.0\" } }\n",
    )
    .unwrap();
    let cfg = load_config(home.path(), &addr, cache.path());

    let outcome = run_install(&cfg, &prefix, &[], &InstallOptions::default()).await.unwrap();
    assert_eq!(outcome.installed, 2, "foo + bar (via ^1.2.0 -> 1.2.3)");

    // The lockfile specifier reflects the substituted spec.
    let lock: Value = json5::from_str(
        &std::fs::read_to_string(prefix.join("oh-package-lock.json5")).unwrap(),
    )
    .unwrap();
    let specifiers = lock["specifiers"].as_object().unwrap();
    assert_eq!(specifiers["@ohos/foo@^1.2.0"], "@ohos/foo@1.2.3");

    // `ohpm install <pkg>` with a parameterized project is forbidden.
    let err = run_install(&cfg, &prefix, &["@ohos/bar".to_string()], &InstallOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.code, "ParameterizationForbiddenInstallError");

    // The CLI `--parameter-file` option overrides the manifest field.
    let prefix2 = work.path().join("entry2");
    std::fs::create_dir_all(&prefix2).unwrap();
    std::fs::write(
        prefix2.join("oh-package.json5"),
        "{\n  name: \"entry2\", version: \"1.0.0\",\n  dependencies: { \"@ohos/foo\": \"@param:deps.foo\" },\n}\n",
    )
    .unwrap();
    std::fs::write(
        prefix2.join("cli-params.json5"),
        "{ deps: { foo: \"^1.2.0\" } }\n",
    )
    .unwrap();
    let opts = InstallOptions {
        parameter_file: Some(prefix2.join("cli-params.json5")),
        ..Default::default()
    };
    let outcome = run_install(&cfg, &prefix2, &[], &opts).await.unwrap();
    assert_eq!(outcome.installed, 2);
    // A missing CLI parameter file errors.
    let opts = InstallOptions {
        parameter_file: Some(prefix2.join("missing.json5")),
        ..Default::default()
    };
    let err = run_install(&cfg, &prefix2, &[], &opts).await.unwrap_err();
    assert_eq!(err.code, "CliInputParameterFileNotExist");
}

#[tokio::test]
async fn target_install_uses_dependency_map() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let (registry, _tarballs) = sample_registry(work.path());
    let (addr, _registry, _capture) = spawn_mock(registry).await;

    // A project with a build-profile declaring the module.
    let prefix = work.path().join("proj");
    std::fs::create_dir_all(prefix.join("entry")).unwrap();
    std::fs::write(
        prefix.join("build-profile.json5"),
        "{ app: { products: [] }, modules: [{ name: \"entry\", srcPath: \"./entry\" }] }\n",
    )
    .unwrap();
    std::fs::write(
        prefix.join("entry/oh-package.json5"),
        "{ name: \"entry\", version: \"1.0.0\", dependencies: { \"@ohos/foo\": \"^1.2.0\" } }\n",
    )
    .unwrap();

    // The target directory with a dependencyMap (targetName "debug").
    let target = work.path().join("target-debug");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(
        target.join("dependencyMap.json5"),
        "{\n  targetName: \"debug\",\n  basePath: \"..\",\n  dependencyMap: { entry: \"../proj/entry/oh-package.json5\" },\n  modules: [{ name: \"entry\", srcPath: \"./proj/entry\" }],\n}\n",
    )
    .unwrap();
    let cfg = load_config(home.path(), &addr, cache.path());
    let opts = InstallOptions {
        target_path: Some(target.clone()),
        ..Default::default()
    };

    let outcome = run_install(&cfg, &prefix, &[], &opts).await.unwrap();
    assert_eq!(outcome.installed, 2, "foo + bar");
    assert_eq!(outcome.module_roots, vec![prefix.join("entry")], "module roots from the dependencyMap");

    // The lockfile uses the target name.
    let lock_path = prefix.join("entry/oh-package-debug-lock.json5");
    assert!(lock_path.is_file(), "target lockfile name");
    let lock: Value = json5::from_str(&std::fs::read_to_string(&lock_path).unwrap()).unwrap();
    assert_eq!(lock["specifiers"]["@ohos/foo@^1.2.0"], "@ohos/foo@1.2.3");
    assert!(!prefix.join("entry/oh-package-lock.json5").exists());

    // The resolve-conflict snapshot manifest records the resolved versions.
    let snapshot = std::fs::read_to_string(
        target.join("resolve-conflict/entry/oh-package.json5"),
    )
    .unwrap();
    let snapshot: Value = json5::from_str(&snapshot).unwrap();
    assert_eq!(snapshot["dependencies"]["@ohos/foo"], "1.2.3");
}

#[tokio::test]
async fn unified_lockfile_merges_modules() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();
    let (registry, _tarballs) = sample_registry(work.path());
    let (addr, _registry, _capture) = spawn_mock(registry).await;

    // A project with two modules: entry (foo, prod) and lib1 (foo, dev —
    // the propagated type must be prod) plus a module-root dep entry.
    let prefix = work.path().join("proj");
    std::fs::create_dir_all(prefix.join("entry")).unwrap();
    std::fs::create_dir_all(prefix.join("lib1")).unwrap();
    std::fs::write(
        prefix.join("build-profile.json5"),
        "{ app: { products: [] }, modules: [{ name: \"entry\", srcPath: \"./entry\" }, { name: \"lib1\", srcPath: \"./lib1\" }] }\n",
    )
    .unwrap();
    std::fs::write(
        prefix.join("entry/oh-package.json5"),
        "{ name: \"entry\", version: \"1.0.0\", dependencies: { \"@ohos/foo\": \"^1.2.0\" } }\n",
    )
    .unwrap();
    std::fs::write(
        prefix.join("oh-package.json5"),
        "{ name: \"proj\", version: \"1.0.0\" }\n",
    )
    .unwrap();
    std::fs::write(
        prefix.join("lib1/oh-package.json5"),
        "{ name: \"lib1\", version: \"1.0.0\", devDependencies: { \"@ohos/foo\": \"^1.2.0\" }, dependencies: { \"entry\": \"../entry\" } }\n",
    )
    .unwrap();
    // Enable the unified lockfile via the project .ohpmrc.
    std::fs::write(prefix.join(".ohpmrc"), "enable_unified_lockfile=true\n").unwrap();

    // Load the config with the prefix as cwd so the project .ohpmrc applies.
    std::env::set_var("HOME", home.path());
    std::env::set_var("OHPM_REGISTRY", &addr);
    std::env::set_var("OHPM_CACHE", cache.path());
    std::env::set_var("OHPM_LOG_LEVEL", "error");
    let mut cfg = Config::new();
    cfg.load(&prefix, None).unwrap();
    let client = RegistryClient::from_config(&cfg).unwrap();

    let opts = InstallOptions { all: true, ..Default::default() };
    let outcome = install(&client, &cfg, &prefix, &[], &opts).await.unwrap();
    assert_eq!(outcome.module_roots.len(), 3, "project root + entry + lib1");

    // ONE lockfile at the project root with the unified meta flag; the module
    // directories carry none.
    let lock: Value = json5::from_str(
        &std::fs::read_to_string(prefix.join("oh-package-lock.json5")).unwrap(),
    )
    .unwrap();
    assert_eq!(lock["meta"]["enableUnifiedLockfile"], true);
    assert!(!prefix.join("entry/oh-package-lock.json5").exists());
    assert!(!prefix.join("lib1/oh-package-lock.json5").exists());
    let specifiers = lock["specifiers"].as_object().unwrap();
    assert_eq!(specifiers["@ohos/foo@^1.2.0"], "@ohos/foo@1.2.3");
    // Unified mode relativizes local deps against the PROJECT root.
    assert!(specifiers.contains_key("entry@entry"), "module-root dep key: {:?}", specifiers.keys());

    // The record: lib1's foo lands in `dependencies` (the propagated prod
    // type beats the dev declaration), and the depended entry root appears in
    // packages with its own directory as the store path.
    let record: Value = json5::from_str(
        &std::fs::read_to_string(prefix.join("oh_modules/.ohpm/lock.json5")).unwrap(),
    )
    .unwrap();
    let lib1 = &record["modules"]["lib1"];
    assert_eq!(lib1["dependencies"]["@ohos/foo"]["version"], "1.2.3");
    assert!(lib1["devDependencies"].as_object().is_none_or(|m| !m.contains_key("@ohos/foo")));
    let entry_pkg = &record["packages"]["entry@file:entry"];
    assert!(entry_pkg.is_object(), "depended module root recorded: {:?}", record["packages"]);
    assert_eq!(entry_pkg["storePath"], "entry");
}

#[tokio::test]
async fn hsp_bundle_app_places_hsp_file() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();

    // A bundle-app HSP package: the registry serves the main tarball and a
    // separate .hsp file (dist.resolved_hsp / integrity_hsp).
    let mut registry = MockRegistry::default();
    let hsp_bytes: Bytes = Bytes::from_static(b"fake-hsp-content");
    let main_har = build_har(work.path(), "myhsp", "1.0.0", &BTreeMap::new());
    registry.packages.insert(
        "myhsp".to_string(),
        MockPackage {
            versions: BTreeMap::from([(
                "1.0.0".to_string(),
                MockVersion {
                    tarball: main_har,
                    hsp: Some(hsp_bytes),
                    hsp_type: Some("bundle_app"),
                    deps: BTreeMap::new(),
                    dev_deps: BTreeMap::new(),
                },
            )]),
            dist_tags: BTreeMap::from([("latest".to_string(), "1.0.0".to_string())]),
        },
    );
    let (addr, _registry, _capture) = spawn_mock(registry).await;

    let prefix = work.path().join("entry");
    write_manifest(&prefix, "{ \"myhsp\": \"^1.0.0\" }", "{}");
    let cfg = load_config(home.path(), &addr, cache.path());

    let outcome = run_install(&cfg, &prefix, &[], &InstallOptions::default()).await.unwrap();
    assert_eq!(outcome.installed, 1);

    // The .hsp file lands in oh_modules/.hsp/<storeDir>/myhsp.hsp next to the
    // package manifest.
    let hsp_dir = prefix.join("oh_modules/.hsp/myhsp@1.0.0");
    let hsp_file = hsp_dir.join("myhsp.hsp");
    assert_eq!(std::fs::read(&hsp_file).unwrap(), b"fake-hsp-content");
    assert!(hsp_dir.join("oh-package.json5").is_file());

    // The install record carries storePathHsp.
    let record: Value = json5::from_str(
        &std::fs::read_to_string(prefix.join("oh_modules/.ohpm/lock.json5")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        record["packages"]["myhsp@1.0.0"]["storePathHsp"],
        "oh_modules/.hsp/myhsp@1.0.0"
    );

    // A bundle-app HSP without a .hsp file fails for runtime deps.
    let mut registry = MockRegistry::default();
    let no_hsp_har = build_har(work.path(), "nohsp", "1.0.0", &BTreeMap::new());
    registry.packages.insert(
        "nohsp".to_string(),
        MockPackage {
            versions: BTreeMap::from([(
                "1.0.0".to_string(),
                MockVersion {
                    tarball: no_hsp_har,
                    hsp: None,
                    hsp_type: Some("bundle_app"),
                    deps: BTreeMap::new(),
                    dev_deps: BTreeMap::new(),
                },
            )]),
            dist_tags: BTreeMap::from([("latest".to_string(), "1.0.0".to_string())]),
        },
    );
    let (addr, _registry, _capture) = spawn_mock(registry).await;
    let prefix2 = work.path().join("entry2");
    write_manifest(&prefix2, "{ \"nohsp\": \"^1.0.0\" }", "{}");
    let cfg = load_config(home.path(), &addr, cache.path());
    let err = run_install(&cfg, &prefix2, &[], &InstallOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.code, "NotFoundHspFileByRegistryTgz");
}
