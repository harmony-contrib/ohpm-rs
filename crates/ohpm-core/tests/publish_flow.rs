//! End-to-end publish flow tests against a local mock registry.
//!
//! These verify the core requirement of this reimplementation: `publish` can
//! authenticate entirely from environment variables with no TUI input.

use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::{post, put};
use axum::{Json, Router};
use base64::Engine;
use ohpm_core::config::Config;
use ohpm_core::publish::{PublishRequest, publish};
use ohpm_core::registry::login::LoginContext;
use ohpm_core::registry::RegistryClient;
use serde_json::Value;

/// Requests the mock registry received.
#[derive(Debug, Default)]
struct Capture {
    login_pss: Vec<Value>,
    login_default: Vec<Value>,
    /// (authorization header, metadata JSON) for attachment (PUT) uploads.
    attachment: Vec<(String, Value)>,
    /// (authorization header, body) for stream (POST multipart) uploads.
    stream: Vec<(String, Bytes)>,
}

/// Env-var isolation: saves the variables this suite touches and restores them
/// on drop, so a panicking test can't leak state into the next one.
struct EnvGuard(Vec<(&'static str, Option<String>)>);

impl EnvGuard {
    fn new() -> Self {
        let keys = [
            "HOME",
            "OHPM_ACCESS_TOKEN",
            "OHPM_READ_ACCESS_TOKEN",
            "OHPM_PUBLISH_ID",
            "OHPM_KEY_PATH",
            "OHPM_KEY_CONTENT",
            "OHPM_KEY_PASSPHRASE",
            "OHPM_REGISTRY",
            "OHPM_PUBLISH_REGISTRY",
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

/// Build a minimal `.har` (gzip tar with a `package/` prefix) in `dir`.
fn build_har(dir: &Path, name: &str, version: &str) -> std::path::PathBuf {
    let manifest = format!("{{ name: \"{name}\", version: \"{version}\", main: \"index.ets\" }}\n");
    build_har_with_manifest(dir, name, version, &manifest)
}

fn build_har_with_manifest(
    dir: &Path,
    name: &str,
    version: &str,
    manifest: &str,
) -> std::path::PathBuf {
    let src = dir.join("src");
    std::fs::create_dir_all(src.join("package/entry")).unwrap();
    std::fs::write(src.join("package/oh-package.json5"), manifest).unwrap();
    std::fs::write(src.join("package/entry/index.ets"), "export {}\n").unwrap();

    // A scoped name contains `/`; use a flat filename on disk.
    let file_name = format!("{}-{}.har", name.replace('/', "-"), version);
    let har = dir.join(file_name);
    let f = std::fs::File::create(&har).unwrap();
    let enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
    let mut tar = tar::Builder::new(enc);
    tar.append_dir_all("package", src.join("package")).unwrap();
    let enc = tar.into_inner().unwrap();
    enc.finish().unwrap();
    har
}

/// Generate an encrypted PKCS#8 private key for the SSH-login test.
fn encrypted_key_pem(passphrase: &str) -> String {
    use rsa::pkcs8::EncodePrivateKey;
    use rsa::RsaPrivateKey;
    let mut rng = rand::thread_rng();
    let key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
    key.to_pkcs8_encrypted_pem(&mut rng, passphrase, rsa::pkcs8::LineEnding::LF)
        .unwrap()
        .to_string()
}

fn load_config(home: &Path) -> Config {
    // Isolate the user-level `.ohpmrc` so tests don't read the developer's.
    std::env::set_var("HOME", home);
    let mut cfg = Config::new();
    cfg.load(Path::new("."), None).unwrap();
    cfg
}

async fn spawn_mock() -> (String, Arc<Mutex<Capture>>) {
    let capture = Arc::new(Mutex::new(Capture::default()));

    let router = Router::new()
        .route("/ohpm/login_pss", post(login_pss_handler))
        .route("/ohpm/login", post(login_default_handler))
        .route("/ohpm/stream/:name", post(stream_handler))
        .route("/ohpm/:name", put(attachment_handler))
        .fallback(unmatched_handler)
        .with_state(capture.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}/ohpm/"), capture)
}

async fn login_pss_handler(
    State(cap): State<Arc<Mutex<Capture>>>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    cap.lock().unwrap_or_else(|e| e.into_inner()).login_pss.push(body);
    Json(serde_json::json!({ "token": "token-from-pss" }))
}

async fn login_default_handler(
    State(cap): State<Arc<Mutex<Capture>>>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    cap.lock().unwrap_or_else(|e| e.into_inner()).login_default.push(body);
    Json(serde_json::json!({ "token": "token-from-default" }))
}

async fn attachment_handler(
    State(cap): State<Arc<Mutex<Capture>>>,
    req: Request,
) -> impl IntoResponse {
    let auth = req
        .headers()
        .get("authorization")
        .map(|v| v.to_str().unwrap_or_default().to_string())
        .unwrap_or_default();
    let body = axum::body::to_bytes(req.into_body(), 16 * 1024 * 1024).await.unwrap();
    let parsed = serde_json::from_slice(&body).unwrap_or(Value::Null);
    cap.lock().unwrap_or_else(|e| e.into_inner()).attachment.push((auth, parsed));
    Json(serde_json::json!({ "ok": true }))
}

async fn stream_handler(State(cap): State<Arc<Mutex<Capture>>>, req: Request) -> impl IntoResponse {
    let auth = req
        .headers()
        .get("authorization")
        .map(|v| v.to_str().unwrap_or_default().to_string())
        .unwrap_or_default();
    let body = axum::body::to_bytes(req.into_body(), 16 * 1024 * 1024).await.unwrap();
    cap.lock().unwrap_or_else(|e| e.into_inner()).stream.push((auth, body));
    Json(serde_json::json!({ "ok": true }))
}

/// Log any unmatched request so we can see what is 404ing.
async fn unmatched_handler(uri: Uri, method: Method) -> impl IntoResponse {
    eprintln!("UNMATCHED: {method} {uri}");
    (StatusCode::NOT_FOUND, "not found")
}

/// Publish with `OHPM_ACCESS_TOKEN` -> attachment upload (PUT), token in header.
#[tokio::test]
async fn publish_with_env_access_token() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _env = EnvGuard::new();
    let (registry, capture) = spawn_mock().await;
    let dir = tempfile::TempDir::new().unwrap();
    let har = build_har(dir.path(), "com.example.ci", "1.0.0");

    std::env::set_var("OHPM_ACCESS_TOKEN", "env-secret-token");
    let config = load_config(dir.path());

    let req = PublishRequest {
        file: har.to_string_lossy().into_owned(),
        publish_registry: Some(registry.clone()),
        ..Default::default()
    };
    let client = RegistryClient::from_config(&config).unwrap();
    let outcome = publish(&client, &config, &req).await.expect("publish should succeed");
    assert_eq!(outcome.name, "com.example.ci");
    assert_eq!(outcome.version, "1.0.0");

    let cap = capture.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(cap.attachment.len(), 1, "expected an attachment (PUT) upload");
    let (auth, meta) = &cap.attachment[0];
    assert_eq!(auth, "env-secret-token", "token must be sent in the Authorization header");
    assert_eq!(meta["dist-tags"]["latest"], "1.0.0");
    assert!(meta["versions"]["1.0.0"]["dist"]["integrity"].as_str().unwrap().starts_with("sha1-"));
    assert!(meta["versions"]["1.0.0"]["dist"]["tarball"].as_str().unwrap().contains("com.example.ci"));
    // internal fields must be cleared before upload
    assert!(meta.get("pkg").is_none());
}

/// Publish with env token -> stream upload (POST multipart) when size exceeds
/// the (overridden) threshold.
#[tokio::test]
async fn publish_stream_upload_used_above_threshold() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _env = EnvGuard::new();
    let (registry, capture) = spawn_mock().await;
    let dir = tempfile::TempDir::new().unwrap();
    let har = build_har(dir.path(), "com.example.big", "1.0.0");

    std::env::set_var("OHPM_ACCESS_TOKEN", "env-secret-token");
    let mut config = load_config(dir.path());
    config.set_cli(ohpm_core::config::default::types::USE_STREAM_THRESHOLD_SIZE, "0");

    let req = PublishRequest {
        file: har.to_string_lossy().into_owned(),
        publish_registry: Some(registry.clone()),
        ..Default::default()
    };
    let client = RegistryClient::from_config(&config).unwrap();
    publish(&client, &config, &req).await.expect("publish should succeed");

    let cap = capture.lock().unwrap_or_else(|e| e.into_inner());
    assert!(cap.stream.len() >= 1, "expected a stream (POST) upload");
    let (auth, body) = &cap.stream[0];
    assert_eq!(auth, "env-secret-token");
    let body_text = String::from_utf8_lossy(body);
    assert!(body_text.contains("metadata"), "multipart must carry the metadata field");
    assert!(body_text.contains("pkg_stream"), "multipart must carry the pkg_stream field");
}

/// No token -> SSH-key login via env vars, then publish.
#[tokio::test]
async fn publish_logs_in_via_ssh_env() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _env = EnvGuard::new();
    let (registry, capture) = spawn_mock().await;
    let dir = tempfile::TempDir::new().unwrap();
    let har = build_har(dir.path(), "com.example.ssh", "1.0.0");

    let passphrase = "correct-horse-battery";
    let key_path = dir.path().join("key.pem");
    std::fs::write(&key_path, encrypted_key_pem(passphrase)).unwrap();

    std::env::set_var("OHPM_PUBLISH_ID", "publish-id-1");
    std::env::set_var("OHPM_KEY_PATH", key_path.to_string_lossy().into_owned());
    std::env::set_var("OHPM_KEY_PASSPHRASE", passphrase);
    let config = load_config(dir.path());

    let req = PublishRequest {
        file: har.to_string_lossy().into_owned(),
        publish_registry: Some(registry.clone()),
        ..Default::default()
    };
    let client = RegistryClient::from_config(&config).unwrap();
    publish(&client, &config, &req).await.expect("publish with ssh login should succeed");

    let cap = capture.lock().unwrap_or_else(|e| e.into_inner());
    assert!(
        !cap.login_pss.is_empty() || !cap.login_default.is_empty(),
        "expected a login request when no token is configured"
    );
    assert_eq!(cap.attachment.len(), 1, "the publish itself must still happen after login");
    // The login request must be signed: publishId/timestamp/nonce/signature/version.
    let login_body = cap.login_pss.first().or(cap.login_default.first()).unwrap();
    assert_eq!(login_body["publishId"], "publish-id-1");
    assert_eq!(login_body["version"], "v1");
    assert!(login_body["signature"].as_str().unwrap().len() > 20);
}

/// Missing passphrase must error, never hang or prompt.
#[tokio::test]
async fn publish_missing_passphrase_errors_not_prompts() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _env = EnvGuard::new();
    let (registry, capture) = spawn_mock().await;
    let dir = tempfile::TempDir::new().unwrap();
    let har = build_har(dir.path(), "com.example.nopass", "1.0.0");

    let key_path = dir.path().join("key.pem");
    std::fs::write(&key_path, encrypted_key_pem("secret")).unwrap();

    std::env::set_var("OHPM_PUBLISH_ID", "pid");
    std::env::set_var("OHPM_KEY_PATH", key_path.to_string_lossy().into_owned());
    // no OHPM_KEY_PASSPHRASE and no config passphrase

    let config = load_config(dir.path());
    let client = RegistryClient::from_config(&config).unwrap();
    let req = PublishRequest {
        file: har.to_string_lossy().into_owned(),
        publish_registry: Some(registry.clone()),
        ..Default::default()
    };
    let err = publish(&client, &config, &req).await.unwrap_err();
    assert_eq!(err.code, "KeyPassphraseMissing");
    assert!(err.message.contains("OHPM_KEY_PASSPHRASE"));
    // No login/publish request must have been sent.
    let cap = capture.lock().unwrap_or_else(|e| e.into_inner());
    assert!(cap.login_pss.is_empty());
    assert!(cap.attachment.is_empty());
}

/// Workspace mode: `file:` dependencies are rewritten to the target member's
/// version before upload.
#[tokio::test]
async fn publish_workspace_rewrites_file_dependencies() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _env = EnvGuard::new();
    let (registry, capture) = spawn_mock().await;
    let dir = tempfile::TempDir::new().unwrap();

    // workspace root + two members
    std::fs::write(dir.path().join("ohpm-workspace.yaml"), "packages:\n  - packages/*\n").unwrap();
    let member_a = dir.path().join("packages/a");
    let member_b = dir.path().join("packages/b");
    std::fs::create_dir_all(&member_a).unwrap();
    std::fs::create_dir_all(&member_b).unwrap();
    std::fs::write(
        member_b.join("oh-package.json5"),
        "{ name: \"@scope/b\", version: \"2.3.4\" }\n",
    )
    .unwrap();

    // Member `a` depends on `b` via a file: path.
    let har = build_har_with_manifest(
        &member_a,
        "@scope/a",
        "1.0.0",
        "{ name: \"@scope/a\", version: \"1.0.0\", \
           dependencies: { \"@scope/b\": \"file:../b\" } }\n",
    );

    std::env::set_var("OHPM_ACCESS_TOKEN", "ws-token");
    let config = load_config(dir.path());
    let req = PublishRequest {
        file: har.to_string_lossy().into_owned(),
        publish_registry: Some(registry.clone()),
        package_root: Some(member_a.clone()),
        ..Default::default()
    };
    let client = RegistryClient::from_config(&config).unwrap();
    publish(&client, &config, &req)
        .await
        .expect("workspace publish should succeed");

    let cap = capture.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(cap.attachment.len(), 1);
    let (_, meta) = &cap.attachment[0];
    let dep = &meta["versions"]["1.0.0"]["dependencies"]["@scope/b"];
    assert_eq!(dep.as_str().unwrap(), "2.3.4", "file: dep must be rewritten to the member version");
    assert!(
        !dep.as_str().unwrap().starts_with("file:"),
        "the published metadata must not contain file: paths"
    );
}

/// `publish: false` in the manifest forbids publishing.
#[tokio::test]
async fn publish_refuses_publish_false_package() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _env = EnvGuard::new();
    let dir = tempfile::TempDir::new().unwrap();
    let har = build_har_with_manifest(
        dir.path(),
        "com.example.priv",
        "1.0.0",
        "{ name: \"com.example.priv\", version: \"1.0.0\", publish: false }\n",
    );

    std::env::set_var("OHPM_ACCESS_TOKEN", "t");
    let config = load_config(dir.path());
    let req = PublishRequest {
        file: har.to_string_lossy().into_owned(),
        publish_registry: Some("http://127.0.0.1:1/ohpm/".into()), // never reached
        ..Default::default()
    };
    let client = RegistryClient::from_config(&config).unwrap();
    let err = publish(&client, &config, &req).await.unwrap_err();
    assert_eq!(err.code, "PublishForbidden");
    assert!(err.message.contains("publish: false"));
}

/// Publish accepts a source directory and packs it into a har first.
#[tokio::test]
async fn publish_from_source_directory() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _env = EnvGuard::new();
    let (registry, capture) = spawn_mock().await;
    let dir = tempfile::TempDir::new().unwrap();

    // A source package directory with the usual structure.
    std::fs::create_dir_all(dir.path().join("src/main/ets")).unwrap();
    std::fs::write(
        dir.path().join("oh-package.json5"),
        "{ name: \"com.example.dirpkg\", version: \"1.0.0\", main: \"index.ets\" }\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("index.ets"), "export {}\n").unwrap();
    std::fs::write(dir.path().join("src/main/ets/a.ets"), "// a\n").unwrap();
    std::fs::create_dir_all(dir.path().join("oh_modules/x")).unwrap();
    std::fs::write(dir.path().join("oh_modules/x/index.ets"), "// excluded\n").unwrap();

    std::env::set_var("OHPM_ACCESS_TOKEN", "dir-token");
    let config = load_config(dir.path());
    let req = PublishRequest {
        file: dir.path().to_string_lossy().into_owned(), // a directory!
        publish_registry: Some(registry.clone()),
        package_root: Some(dir.path().to_path_buf()),
        ..Default::default()
    };
    let client = RegistryClient::from_config(&config).unwrap();
    publish(&client, &config, &req).await.expect("directory publish should succeed");

    let cap = capture.lock().unwrap_or_else(|e| e.into_inner());
    assert_eq!(cap.attachment.len(), 1);
    let (_, meta) = &cap.attachment[0];
    assert_eq!(meta["name"], "com.example.dirpkg");

    // Decode the packed har attachment and verify its contents.
    let data = meta["_attachments"]["com.example.dirpkg-1.0.0.har"]["data"]
        .as_str()
        .unwrap();
    let bytes = base64::engine::general_purpose::STANDARD.decode(data).unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(&bytes[..]));
    let paths: Vec<String> = archive
        .entries()
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(paths.iter().any(|p| p == "package/oh-package.json5"));
    assert!(paths.iter().any(|p| p == "package/src/main/ets/a.ets"));
    assert!(!paths.iter().any(|p| p.contains("oh_modules")), "pack excludes oh_modules");
}

/// The private key can be supplied as inline PEM content (no key file).
#[tokio::test]
async fn publish_with_key_content_env() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _env = EnvGuard::new();
    let (registry, capture) = spawn_mock().await;
    let dir = tempfile::TempDir::new().unwrap();
    let har = build_har(dir.path(), "com.example.kc", "1.0.0");

    let passphrase = "secret";
    let key_pem = encrypted_key_pem(passphrase);
    // No key file anywhere — the PEM travels via OHPM_KEY_CONTENT.
    std::env::set_var("OHPM_PUBLISH_ID", "kc-pid");
    std::env::set_var("OHPM_KEY_CONTENT", &key_pem);
    std::env::set_var("OHPM_KEY_PASSPHRASE", passphrase);
    let config = load_config(dir.path());

    let req = PublishRequest {
        file: har.to_string_lossy().into_owned(),
        publish_registry: Some(registry.clone()),
        ..Default::default()
    };
    let client = RegistryClient::from_config(&config).unwrap();
    publish(&client, &config, &req)
        .await
        .expect("publish with inline key content should succeed");

    let cap = capture.lock().unwrap_or_else(|e| e.into_inner());
    assert!(!cap.login_pss.is_empty(), "login happened with the inline key");
    assert_eq!(cap.attachment.len(), 1);
}

/// The SSH login context can be built from config too (no env).
#[test]
fn login_context_from_config_only() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let _env = EnvGuard::new();
    let dir = tempfile::TempDir::new().unwrap();
    let key_path = dir.path().join("key.pem");
    std::fs::write(&key_path, encrypted_key_pem("pw")).unwrap();

    let mut cfg = Config::new();
    cfg.set(ohpm_core::config::default::types::PUBLISH_ID, "pid");
    cfg.set(ohpm_core::config::default::types::KEY_PATH, &key_path.to_string_lossy());
    cfg.set(ohpm_core::config::default::types::KEY_PASSPHRASE, "pw");
    let ctx = LoginContext::resolve(&cfg, &ohpm_core::registry::login::LoginOverrides::default()).unwrap();
    assert_eq!(ctx.publish_id, "pid");
    assert!(ctx.key_path.as_ref().unwrap().exists());
    assert_eq!(ctx.passphrase, "pw");
}
