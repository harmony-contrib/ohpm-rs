//! End-to-end tests for the protocol extensions: git specs, `ohpm:` aliases
//! and the `workspace:` protocol.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use ohpm_core::config::Config;
use ohpm_core::install::{install, InstallOptions};
use ohpm_core::registry::RegistryClient;

/// Env-var isolation (same pattern as install_flow.rs).
struct EnvGuard(Vec<(&'static str, Option<String>)>);

impl EnvGuard {
    fn new() -> Self {
        let keys = ["HOME", "OHPM_REGISTRY", "OHPM_CACHE", "OHPM_LOG_LEVEL"];
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

static ENV_LOCK: Mutex<()> = Mutex::new(());

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
        log::set_max_level(log::LevelFilter::Info);
    });
}

/// Build a minimal `.har` (gzip tar with a `package/` prefix).
fn build_har(dir: &Path, name: &str, version: &str, deps: &BTreeMap<String, String>) -> Vec<u8> {
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
    out
}

use axum::body::Bytes;
use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;

#[derive(Clone)]
struct MockVersion {
    tarball: Bytes,
    deps: BTreeMap<String, String>,
}

#[derive(Clone, Default)]
struct MockPackage {
    versions: BTreeMap<String, MockVersion>,
}

#[derive(Clone, Default)]
struct MockRegistry {
    packages: BTreeMap<String, MockPackage>,
}

#[derive(Debug, Default)]
struct Capture {
    packument_gets: Vec<String>,
    tarball_gets: Vec<String>,
}

async fn spawn_mock(registry: MockRegistry) -> (String, Arc<Mutex<MockRegistry>>, Arc<Mutex<Capture>>) {
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
    (format!("http://{addr}/ohpm/"), registry, capture)
}

async fn ohpm_handler(
    State((registry, capture, addr)): State<(Arc<Mutex<MockRegistry>>, Arc<Mutex<Capture>>, String)>,
    AxumPath(rest): AxumPath<String>,
) -> impl IntoResponse {
    let registry = registry.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((name, file)) = rest.split_once("/-/") {
        capture.lock().unwrap_or_else(|e| e.into_inner()).tarball_gets.push(format!("{name}/{file}"));
        let version = file
            .strip_suffix(".har")
            .and_then(|f| f.strip_prefix(&format!("{}-", name.replace('/', "-"))))
            .unwrap_or_default()
            .to_string();
        return match registry.packages.get(name).and_then(|p| p.versions.get(&version)) {
            Some(mv) => mv.tarball.clone().into_response(),
            None => (StatusCode::NOT_FOUND, "not found".to_string()).into_response(),
        };
    }
    capture.lock().unwrap_or_else(|e| e.into_inner()).packument_gets.push(rest.to_string());
    let Some(pkg) = registry.packages.get(&rest) else {
        return (StatusCode::NOT_FOUND, "not found".to_string()).into_response();
    };
    let mut versions = serde_json::Map::new();
    for (version, mv) in &pkg.versions {
        use base64::Engine;
        use sha2::Digest;
        let mut h = sha2::Sha512::new();
        h.update(&mv.tarball);
        let integrity = base64::engine::general_purpose::STANDARD.encode(h.finalize());
        let entry = serde_json::json!({
            "name": rest,
            "version": version,
            "_ohpmVersion": "1",
            "dependencies": mv.deps,
            "dist": {
                "tarball": format!("http://{addr}/ohpm/{rest}/-/{}-{version}.har", rest.replace('/', "-")),
                "integrity": format!("sha512-{integrity}"),
            },
        });
        versions.insert(version.clone(), entry);
    }
    serde_json::json!({ "name": rest, "dist-tags": {}, "versions": versions })
        .to_string()
        .into_response()
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

async fn run_install(
    cfg: &Config,
    prefix: &Path,
    args: &[String],
    opts: &InstallOptions,
) -> ohpm_core::Result<ohpm_core::install::InstallOutcome> {
    let client = RegistryClient::from_config(cfg)?;
    install(&client, cfg, prefix, args, opts).await
}

/// A git fixture repo created with the system git (tests may use git; the
/// product code never does).
struct GitFixture {
    _dir: tempfile::TempDir,
    pub url: String,
    repo: PathBuf,
}

impl GitFixture {
    fn new() -> GitFixture {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path().join("repo.git");
        std::fs::create_dir_all(&repo).unwrap();
        Self::run(&repo, &["init", "-q"]);
        GitFixture {
            _dir: dir,
            url: format!("file://{}", repo.display()),
            repo,
        }
    }

    fn run(repo: &Path, args: &[&str]) {
        let status = Command::new("git").args(args).current_dir(repo).status().unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn write(&self, rel: &str, content: &str) {
        let p = self.repo.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    fn commit(&self, msg: &str) {
        Self::run(&self.repo, &["add", "."]);
        Self::run(&self.repo, &["commit", "-q", "-m", msg]);
    }

    fn tag(&self, tag: &str) {
        Self::run(&self.repo, &["tag", tag]);
    }

    fn delete(&self) {
        let _ = std::fs::remove_dir_all(&self.repo);
    }
}

#[tokio::test]
async fn git_dependency_installs_and_reinstalls_offline() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();

    let fx = GitFixture::new();
    fx.write("oh-package.json5", "{ name: \"gitfoo\", version: \"2.1.0\", main: \"index.ets\" }\n");
    fx.write("src/a.ets", "export {}\n");
    fx.commit("init");
    fx.tag("v2.1.0");

    let prefix = work.path().join("entry");
    write_manifest(&prefix, &format!("{{ \"gitfoo\": \"{}#v2.1.0\" }}", fx.url), "{}");
    let cfg = load_config(home.path(), "http://127.0.0.1:1/ohpm/", cache.path());

    let outcome = run_install(&cfg, &prefix, &[], &InstallOptions::default()).await.unwrap();
    assert_eq!(outcome.installed, 1);

    // Lockfile: specifier pins the commit; packages entry is registryType git.
    let lock_text = std::fs::read_to_string(prefix.join("oh-package-lock.json5")).unwrap();
    assert!(
        lock_text.contains(&format!("\"gitfoo@{}#v2.1.0\": \"gitfoo@", fx.url)),
        "{lock_text}"
    );
    assert!(lock_text.contains("\"registryType\": \"git\""), "{lock_text}");
    assert!(lock_text.contains(&format!("\"resolved\": \"{}#", fx.url)), "{lock_text}");
    assert!(lock_text.contains("\"version\": \"2.1.0\""), "{lock_text}");

    // Store contains the committed files, no .git.
    let store = prefix.join("oh_modules/.ohpm");
    let store_dirs: Vec<String> = std::fs::read_dir(&store)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("gitfoo@"))
        .collect();
    assert_eq!(store_dirs.len(), 1, "{store_dirs:?}");
    let pkg = store.join(&store_dirs[0]).join("oh_modules/gitfoo");
    assert!(pkg.join("oh-package.json5").is_file());
    assert!(pkg.join("src/a.ets").is_file());
    assert!(!store.join(&store_dirs[0]).join(".git").exists());

    // Top-level link.
    let link = std::fs::read_link(prefix.join("oh_modules/gitfoo")).unwrap();
    assert_eq!(link, PathBuf::from(format!(".ohpm/{}/oh_modules/gitfoo", store_dirs[0])));

    // Delete the fixture repo: re-install still works (lockfile pins the
    // commit; the store is already materialized).
    fx.delete();
    run_install(&cfg, &prefix, &[], &InstallOptions::default()).await.unwrap();
}

#[tokio::test]
async fn git_dependency_semver_fragment_and_errors() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();

    let fx = GitFixture::new();
    fx.write("oh-package.json5", "{ name: \"gitfoo\", version: \"1.0.0\" }\n");
    fx.commit("v1");
    fx.tag("v1.0.0");
    fx.write("extra.ets", "export const x = 1\n");
    fx.commit("v2");
    fx.tag("v2.0.0");

    let prefix = work.path().join("entry");
    // `#semver:^1.0.0` picks v1.0.0 (not the max tag v2.0.0).
    write_manifest(&prefix, &format!("{{ \"gitfoo\": \"{}#semver:^1.0.0\" }}", fx.url), "{}");
    let cfg = load_config(home.path(), "http://127.0.0.1:1/ohpm/", cache.path());
    run_install(&cfg, &prefix, &[], &InstallOptions::default()).await.unwrap();
    // v1.0.0's tree must not contain extra.ets (it was added in v2).
    let store = prefix.join("oh_modules/.ohpm");
    let dirs: Vec<String> = std::fs::read_dir(&store).unwrap().flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    let git_dirs: Vec<&String> = dirs.iter().filter(|n| n.starts_with("gitfoo@")).collect();
    assert_eq!(git_dirs.len(), 1);
    assert!(!store.join(git_dirs[0]).join("oh_modules/gitfoo/extra.ets").exists());

    // Unknown ref -> GitRefNotFound.
    let prefix2 = work.path().join("entry2");
    write_manifest(&prefix2, &format!("{{ \"gitfoo\": \"{}#nope\" }}", fx.url), "{}");
    let err = run_install(&cfg, &prefix2, &[], &InstallOptions::default()).await.unwrap_err();
    assert_eq!(err.code, "GitRefNotFound");
}

#[tokio::test]
async fn alias_dependency() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();

    let bar_har = build_har(work.path(), "bar", "1.0.0", &BTreeMap::new());
    let registry = MockRegistry {
        packages: BTreeMap::from([(
            "bar".to_string(),
            MockPackage {
                versions: BTreeMap::from([("1.0.0".to_string(), MockVersion { tarball: Bytes::from(bar_har), deps: BTreeMap::new() })]),
            },
        )]),
    };
    let (addr, _registry, capture) = spawn_mock(registry).await;

    let prefix = work.path().join("entry");
    // `foo` aliases `bar` via the ohpm: protocol.
    write_manifest(&prefix, "{ \"foo\": \"ohpm:bar@^1.0.0\" }", "{}");
    let cfg = load_config(home.path(), &addr, cache.path());

    let outcome = run_install(&cfg, &prefix, &[], &InstallOptions::default()).await.unwrap();
    assert_eq!(outcome.installed, 1);

    // Lockfile: specifier key uses the alias, value/packages key the real name.
    let lock_text = std::fs::read_to_string(prefix.join("oh-package-lock.json5")).unwrap();
    assert!(lock_text.contains("\"foo@ohpm:bar@^1.0.0\": \"bar@1.0.0\""), "{lock_text}");
    assert!(lock_text.contains("\"bar@1.0.0\": {"), "{lock_text}");

    // Symlink named after the alias, pointing at the real package store dir.
    let link = std::fs::read_link(prefix.join("oh_modules/foo")).unwrap();
    assert_eq!(link, PathBuf::from(".ohpm/bar@1.0.0/oh_modules/bar"));
    assert!(prefix.join("oh_modules/.ohpm/bar@1.0.0/oh_modules/bar/oh-package.json5").is_file());

    // Re-install is lockfile-first: no new packument fetch.
    let before = capture.lock().unwrap().packument_gets.len();
    run_install(&cfg, &prefix, &[], &InstallOptions::default()).await.unwrap();
    assert_eq!(capture.lock().unwrap().packument_gets.len(), before);

    // Update moves the alias when the target gains a version in range.
    let bar_110 = build_har(work.path(), "bar", "1.1.0", &BTreeMap::new());
    _registry.lock().unwrap().packages.get_mut("bar").unwrap().versions.insert(
        "1.1.0".to_string(),
        MockVersion { tarball: Bytes::from(bar_110), deps: BTreeMap::new() },
    );
    let client = RegistryClient::from_config(&cfg).unwrap();
    ohpm_core::install::update(&client, &cfg, &prefix, &[], &InstallOptions::default()).await.unwrap();
    let lock_text = std::fs::read_to_string(prefix.join("oh-package-lock.json5")).unwrap();
    assert!(lock_text.contains("\"foo@ohpm:bar@^1.0.0\": \"bar@1.1.0\""), "{lock_text}");

    // Uninstall by the alias key prunes specifier and package entries.
    ohpm_core::install::uninstall(&client, &cfg, &prefix, &["foo".to_string()], &InstallOptions::default()).await.unwrap();
    let lock_text = std::fs::read_to_string(prefix.join("oh-package-lock.json5")).unwrap();
    assert!(!lock_text.contains("bar@"), "{lock_text}");
    assert!(!lock_text.contains("ohpm:bar"), "{lock_text}");
}

#[tokio::test]
async fn workspace_protocol() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _env = EnvGuard::new();
    let home = tempfile::TempDir::new().unwrap();
    let work = tempfile::TempDir::new().unwrap();
    let cache = tempfile::TempDir::new().unwrap();

    // A workspace with members bar@1.0.0 and baz@2.0.0-beta.1.
    let ws_root = work.path().join("ws");
    std::fs::create_dir_all(ws_root.join("ohpm-workspace.yaml").parent().unwrap()).unwrap();
    std::fs::write(ws_root.join("ohpm-workspace.yaml"), "packages:\n  - \"packages/*\"\n").unwrap();
    for (name, version) in [("bar", "1.0.0"), ("baz", "2.0.0-beta.1")] {
        let dir = ws_root.join("packages").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("oh-package.json5"),
            format!("{{ name: \"{name}\", version: \"{version}\" }}\n"),
        )
        .unwrap();
    }

    let prefix = ws_root.join("entry");
    write_manifest(&prefix, "{ \"bar\": \"workspace:^1.0.0\", \"baz\": \"workspace:*\" }", "{}");
    let cfg = load_config(home.path(), "http://127.0.0.1:1/ohpm/", cache.path());

    let outcome = run_install(&cfg, &prefix, &[], &InstallOptions::default()).await.unwrap();
    assert_eq!(outcome.installed, 0, "workspace members are links, not installs");

    // Links point at the member dirs (two levels up from oh_modules/).
    let link = std::fs::read_link(prefix.join("oh_modules/bar")).unwrap();
    assert_eq!(link, PathBuf::from("../../packages/bar"));
    let link = std::fs::read_link(prefix.join("oh_modules/baz")).unwrap();
    assert_eq!(link, PathBuf::from("../../packages/baz"));

    // Workspace members never appear in packages; with no registry packages
    // the lockfile is not written (mirrors the reference flush condition).
    assert!(!prefix.join("oh-package-lock.json5").exists());

    // Missing member -> WorkspacePkgNotFound.
    let prefix2 = ws_root.join("entry2");
    write_manifest(&prefix2, "{ \"nope\": \"workspace:*\" }", "{}");
    let err = run_install(&cfg, &prefix2, &[], &InstallOptions::default()).await.unwrap_err();
    assert_eq!(err.code, "WorkspacePkgNotFound");

    // Range that matches nothing -> WorkspaceNoMatchingVersion.
    let prefix3 = ws_root.join("entry3");
    write_manifest(&prefix3, "{ \"bar\": \"workspace:^9.0.0\" }", "{}");
    let err = run_install(&cfg, &prefix3, &[], &InstallOptions::default()).await.unwrap_err();
    assert_eq!(err.code, "WorkspaceNoMatchingVersion");
}

#[tokio::test]
async fn publish_workspace_replacement() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    init_test_logger();
    let _env = EnvGuard::new();
    let work = tempfile::TempDir::new().unwrap();

    let ws_root = work.path().join("ws");
    std::fs::create_dir_all(&ws_root).unwrap();
    std::fs::write(ws_root.join("ohpm-workspace.yaml"), "packages:\n  - \"packages/*\"\n").unwrap();
    for (name, version) in [("bar", "1.5.0"), ("baz", "2.0.0")] {
        let dir = ws_root.join("packages").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("oh-package.json5"),
            format!("{{ name: \"{name}\", version: \"{version}\" }}\n"),
        )
        .unwrap();
    }
    let ws = ohpm_core::workspace::Workspace::find(&ws_root).unwrap().unwrap();

    // Replace table.
    for (declared, expected, rewritten) in [
        ("workspace:*", "1.5.0", 1),
        ("workspace:", "1.5.0", 1),
        ("workspace:^", "^1.5.0", 1),
        ("workspace:~", "~1.5.0", 1),
        ("workspace:^1.5.0", "workspace:^1.5.0", 0), // explicit range kept
    ] {
        let mut manifest = ohpm_core::package::Manifest::default();
        manifest.dependencies.insert("bar".to_string(), declared.to_string());
        let n = ohpm_core::workspace::process_workspace_dependencies(&ws, &ws_root, &mut manifest).unwrap();
        assert_eq!(manifest.dependencies["bar"], expected, "{declared}");
        assert_eq!(n, rewritten, "{declared}");
    }

    // Alias form -> ohpm:<member>@<version>.
    let mut manifest = ohpm_core::package::Manifest::default();
    manifest.dependencies.insert("b".to_string(), "workspace:bar@*".to_string());
    ohpm_core::workspace::process_workspace_dependencies(&ws, &ws_root, &mut manifest).unwrap();
    assert_eq!(manifest.dependencies["b"], "ohpm:bar@1.5.0");

    // Missing member -> WorkspacePkgNotFound.
    let mut manifest = ohpm_core::package::Manifest::default();
    manifest.dependencies.insert("x".to_string(), "workspace:*".to_string());
    let err = ohpm_core::workspace::process_workspace_dependencies(&ws, &ws_root, &mut manifest).unwrap_err();
    assert_eq!(err.code, "WorkspacePkgNotFound");
}
