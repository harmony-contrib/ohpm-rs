//! Run the real CLI without a registry, in paths containing spaces and Unicode.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::json;

struct WorkspaceFixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    app: PathBuf,
    library: PathBuf,
}

impl WorkspaceFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace space 库");
        let app = root.join("packages/app");
        let library = root.join("packages/library");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&library).unwrap();
        fs::write(
            root.join("ohpm-workspace.yaml"),
            "packages:\n  - 'packages/*'\nversion:\n  mode: unified\n",
        )
        .unwrap();
        let fixture = Self {
            _temp: temp,
            root,
            app,
            library,
        };
        fixture.manifest(&fixture.library, "@fixture/library", json!({}));
        fixture.manifest(
            &fixture.app,
            "@fixture/app",
            json!({"@fixture/library": "workspace:*"}),
        );
        fs::write(
            fixture.library.join("index.ets"),
            "export const value = 'library payload';\n",
        )
        .unwrap();
        fs::write(fixture.app.join("index.ets"), "export {};\n").unwrap();
        fixture
    }

    fn manifest(&self, dir: &Path, name: &str, dependencies: serde_json::Value) {
        fs::write(
            dir.join("oh-package.json5"),
            serde_json::to_vec_pretty(&json!({
                "name": name,
                "version": "1.0.0-beta.3",
                "main": "index.ets",
                "dependencies": dependencies,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_ohpm-rs"))
            .args(args)
            .current_dir(cwd)
            .env("OHPM_CACHE", self.root.join("cache"))
            .env("OHPM_REGISTRY", "http://127.0.0.1:9/")
            .env("OHPM_LOG_LEVEL", "error")
            .output()
            .unwrap()
    }

    fn succeed(&self, cwd: &Path, args: &[&str]) -> Output {
        let output = self.run(cwd, args);
        assert!(
            output.status.success(),
            "{args:?}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
}

#[test]
fn workspace_install_reinstall_retarget_and_clean_preserve_sources() {
    let fixture = WorkspaceFixture::new();
    fixture.succeed(
        &fixture.root,
        &[
            "install",
            "--all",
            "--prefix",
            fixture.app.to_str().unwrap(),
        ],
    );
    let link = fixture.app.join("oh_modules/@fixture/library");
    let lock = fixture.app.join("oh-package-lock.json5");
    assert_eq!(
        link.canonicalize().unwrap(),
        fixture.library.canonicalize().unwrap()
    );
    let first_lock = fs::read(&lock).unwrap();
    fixture.succeed(&fixture.app, &["install"]);
    assert_eq!(fs::read(&lock).unwrap(), first_lock);

    let moved = fixture.root.join("packages/library moved");
    fs::rename(&fixture.library, &moved).unwrap();
    fixture.succeed(&fixture.app, &["install"]);
    assert_eq!(link.canonicalize().unwrap(), moved.canonicalize().unwrap());
    assert!(fs::read_to_string(link.join("index.ets"))
        .unwrap()
        .contains("library payload"));

    fixture.succeed(&fixture.root, &["clean", "--workspace"]);
    assert!(!fixture.app.join("oh_modules").exists());
    assert!(!lock.exists());
    assert!(
        moved.join("index.ets").is_file(),
        "clean must not delete a linked workspace member"
    );
}

#[test]
fn pack_prepublish_local_har_install_and_unified_version() {
    let fixture = WorkspaceFixture::new();
    let packed = fixture.root.join("packed output");
    fixture.succeed(
        &fixture.root,
        &["pack", "--workspace", "--output", packed.to_str().unwrap()],
    );
    let archives: Vec<_> = fs::read_dir(&packed).unwrap().collect();
    assert_eq!(archives.len(), 2);
    fixture.succeed(&fixture.root, &["prepublish", "--workspace"]);

    fixture.manifest(
        &fixture.app,
        "@fixture/app",
        json!({
            "@fixture/library": "file:../../packed output/fixture-library-1.0.0-beta.3.har"
        }),
    );
    fixture.succeed(&fixture.app, &["install"]);
    assert!(
        fs::read_to_string(fixture.app.join("oh_modules/@fixture/library/index.ets"))
            .unwrap()
            .contains("library payload")
    );

    fixture.succeed(&fixture.root, &["version", "1.0.0-beta.4", "--workspace"]);
    for dir in [&fixture.app, &fixture.library] {
        let manifest: serde_json::Value =
            json5::from_str(&fs::read_to_string(dir.join("oh-package.json5")).unwrap()).unwrap();
        assert_eq!(manifest["version"], "1.0.0-beta.4");
    }
}
