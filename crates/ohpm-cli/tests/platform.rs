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

    fn member(&self, name: &str, manifest: serde_json::Value) -> PathBuf {
        let dir = self.root.join("packages").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("oh-package.json5"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        dir
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
fn workspace_install_includes_unreferenced_private_members_and_dev_dependencies() {
    let fixture = WorkspaceFixture::new();
    let private = fixture.member(
        "private",
        json!({
            "name": "@fixture/private", "version": "1.0.0", "publish": false,
            "devDependencies": {"@fixture/library": "workspace:*"}
        }),
    );
    let excluded = fixture.member(
        "excluded",
        json!({
            "name": "@fixture/excluded", "version": "1.0.0",
            "dependencies": {"@fixture/missing": "workspace:*"}
        }),
    );
    fs::write(
        fixture.root.join("ohpm-workspace.yaml"),
        "packages:\n  - 'packages/*'\nexclude:\n  - 'packages/excluded'\n",
    )
    .unwrap();
    assert!(!fixture.root.join("oh-package.json5").exists());
    fixture.succeed(&fixture.root, &["install", "--workspace"]);
    for dir in [&fixture.app, &private] {
        assert_eq!(
            dir.join("oh_modules/@fixture/library")
                .canonicalize()
                .unwrap(),
            fixture.library.canonicalize().unwrap()
        );
    }
    assert!(!excluded.join("oh_modules").exists());
    let first_lock = fs::read(private.join("oh-package-lock.json5")).unwrap();
    fixture.succeed(&fixture.app, &["install", "--workspace"]);
    assert_eq!(
        fs::read(private.join("oh-package-lock.json5")).unwrap(),
        first_lock
    );
}

#[test]
fn workspace_install_filter_and_prefix_select_members_from_another_directory() {
    let fixture = WorkspaceFixture::new();
    let broken = fixture.member(
        "broken",
        json!({
            "name": "@fixture/broken", "version": "1.0.0",
            "dependencies": {"@fixture/missing": "workspace:*"}
        }),
    );
    let outside = fixture.root.parent().unwrap();
    fixture.succeed(
        outside,
        &[
            "install",
            "--prefix",
            "workspace space 库",
            "--filter",
            "@fixture/app,@fixture/library,@fixture/app",
        ],
    );
    assert_eq!(
        fixture
            .app
            .join("oh_modules/@fixture/library")
            .canonicalize()
            .unwrap(),
        fixture.library.canonicalize().unwrap()
    );
    assert!(!broken.join("oh_modules").exists());
    assert!(!broken.join("oh-package-lock.json5").exists());
    fixture.succeed(
        &fixture.app,
        &["install", "--workspace", "--filter", "@fixture/app"],
    );
}

#[test]
fn workspace_install_validates_scope_and_conflicting_arguments_before_installing() {
    let fixture = WorkspaceFixture::new();
    let missing_workspace =
        fixture.run(fixture.root.parent().unwrap(), &["install", "--workspace"]);
    assert!(!missing_workspace.status.success());
    assert!(String::from_utf8_lossy(&missing_workspace.stderr).contains("ohpm-workspace.yaml"));
    let unknown = fixture.run(
        &fixture.root,
        &[
            "install",
            "--workspace",
            "--filter",
            "@fixture/app,@fixture/unknown",
        ],
    );
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("@fixture/unknown"));

    for args in [
        vec!["install", "--workspace", "@fixture/library"],
        vec!["install", "--filter", "@fixture/app", "@fixture/library"],
        vec!["install", "--workspace", "--target_path", "."],
        vec!["install", "--filter", "@fixture/app", "--target_path", "."],
        vec!["install", "--workspace", "--prefix", "missing directory"],
    ] {
        let result = fixture.run(&fixture.root, &args);
        assert!(!result.status.success(), "{args:?}");
    }
    assert!(!fixture.app.join("oh_modules").exists());
    assert!(!fixture.app.join("oh-package-lock.json5").exists());
}

#[test]
fn workspace_install_all_includes_build_profile_modules() {
    let fixture = WorkspaceFixture::new();
    fs::write(fixture.root.join(".ohpmrc"), "install_all=false\n").unwrap();
    let project = fixture.member(
        "project",
        json!({"name": "@fixture/project", "version": "1.0.0", "publish": false}),
    );
    let entry = project.join("entry");
    fs::create_dir_all(&entry).unwrap();
    fixture.manifest(
        &entry,
        "@fixture/entry",
        json!({"@fixture/library": "workspace:*"}),
    );
    fs::write(
        project.join("build-profile.json5"),
        serde_json::to_vec(&json!({"modules": [{"name": "entry", "srcPath": "./entry"}]})).unwrap(),
    )
    .unwrap();
    fixture.succeed(&fixture.root, &["install", "--workspace"]);
    assert!(!entry.join("oh_modules").exists());
    fixture.succeed(&fixture.root, &["install", "--workspace", "--all"]);
    assert_eq!(
        entry
            .join("oh_modules/@fixture/library")
            .canonicalize()
            .unwrap(),
        fixture.library.canonicalize().unwrap()
    );
}

#[test]
fn workspace_install_reports_a_member_failure() {
    let fixture = WorkspaceFixture::new();
    fixture.member(
        "z-broken",
        json!({
            "name": "@fixture/broken", "version": "1.0.0",
            "dependencies": {"@fixture/missing": "workspace:*"}
        }),
    );
    let result = fixture.run(&fixture.root, &["install", "--workspace"]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("@fixture/missing"));
    assert!(fixture
        .app
        .join("oh_modules/@fixture/library/oh-package.json5")
        .is_file());
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
