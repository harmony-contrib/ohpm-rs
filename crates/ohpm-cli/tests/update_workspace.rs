//! Workspace updates through the real CLI, without a remote registry.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

struct WorkspaceFixture {
    temp: tempfile::TempDir,
    root: PathBuf,
    app: PathBuf,
    library: PathBuf,
}

impl WorkspaceFixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace space 库");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("ohpm-workspace.yaml"),
            "packages:\n  - 'packages/*'\nexclude:\n  - 'packages/excluded'\n",
        )
        .unwrap();
        let mut fixture = Self {
            temp,
            root,
            app: PathBuf::new(),
            library: PathBuf::new(),
        };
        fixture.library = fixture.member("library", "1.0.0", json!({}));
        fixture.app = fixture.member(
            "app",
            "1.0.0",
            json!({"dependencies": {"@fixture/library": "workspace:*"}}),
        );
        fixture
    }

    fn member(&self, name: &str, version: &str, fields: Value) -> PathBuf {
        let dir = self.root.join("packages").join(name);
        fs::create_dir_all(&dir).unwrap();
        let mut manifest = json!({
            "name": format!("@fixture/{name}"),
            "version": version,
            "main": "index.ets",
        });
        manifest
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        Self::write_json(&dir.join("oh-package.json5"), &manifest);
        fs::write(dir.join("index.ets"), "export {};\n").unwrap();
        dir
    }

    fn write_json(path: &Path, value: &Value) {
        fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }

    fn read_json(path: &Path) -> Value {
        json5::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn lock_version(&self, member: &Path, package: &str) -> String {
        let lock = Self::read_json(&member.join("oh-package-lock.json5"));
        lock["packages"]
            .as_object()
            .unwrap()
            .values()
            .find(|value| value["name"] == package)
            .unwrap_or_else(|| panic!("No {package} in {lock}"))["version"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_ohpm-rs"))
            .args(args)
            .current_dir(cwd)
            .env("HOME", self.temp.path())
            .env("USERPROFILE", self.temp.path())
            .env("OHPM_CACHE", self.root.join("cache"))
            .env("OHPM_REGISTRY", "http://127.0.0.1:9/")
            .env("OHPM_LOG_LEVEL", "error")
            .output()
            .unwrap()
    }

    fn succeed(&self, cwd: &Path, args: &[&str]) {
        let output = self.run(cwd, args);
        assert!(
            output.status.success(),
            "{args:?}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn assert_untouched(&self, dir: &Path) {
        assert!(!dir.join("oh_modules").exists(), "{}", dir.display());
        assert!(
            !dir.join("oh-package-lock.json5").exists(),
            "{}",
            dir.display()
        );
    }
}

#[test]
fn workspace_update_refreshes_private_members_and_respects_excludes() {
    let fixture = WorkspaceFixture::new();
    let private = fixture.member(
        "private",
        "1.0.0",
        json!({
            "publish": false,
            "devDependencies": {"@fixture/library": "workspace:*"}
        }),
    );
    let excluded = fixture.member(
        "excluded",
        "1.0.0",
        json!({"dependencies": {"@fixture/missing": "workspace:*"}}),
    );
    assert!(!fixture.root.join("oh-package.json5").exists());
    fixture.succeed(&fixture.root, &["update", "--workspace"]);
    assert_eq!(
        fixture.lock_version(&fixture.app, "@fixture/library"),
        "1.0.0"
    );
    assert_eq!(fixture.lock_version(&private, "@fixture/library"), "1.0.0");
    fixture.member("library", "1.1.0", json!({}));
    fixture.succeed(&fixture.app, &["update", "--workspace"]);
    assert_eq!(
        fixture.lock_version(&fixture.app, "@fixture/library"),
        "1.1.0"
    );
    assert_eq!(fixture.lock_version(&private, "@fixture/library"), "1.1.0");
    assert_eq!(
        WorkspaceFixture::read_json(&private.join("oh-package.json5"))["devDependencies"]
            ["@fixture/library"],
        "workspace:*"
    );
    fixture.assert_untouched(&excluded);
}

#[test]
fn filter_implies_workspace_and_prefix_locates_it_outside_the_root() {
    let fixture = WorkspaceFixture::new();
    let broken = fixture.member(
        "broken",
        "1.0.0",
        json!({"dependencies": {"@fixture/missing": "workspace:*"}}),
    );
    fixture.succeed(
        fixture.root.parent().unwrap(),
        &[
            "up",
            "--prefix",
            "workspace space 库",
            "--filter",
            "@fixture/app,@fixture/app",
            "@fixture/library",
        ],
    );
    assert_eq!(
        fixture.lock_version(&fixture.app, "@fixture/library"),
        "1.0.0"
    );
    fixture.assert_untouched(&fixture.library);
    fixture.assert_untouched(&broken);
}

#[test]
fn workspace_update_accepts_multiple_dependency_names_without_rewriting_ranges() {
    let fixture = WorkspaceFixture::new();
    fixture.member("second", "1.0.0", json!({}));
    fixture.member(
        "app",
        "1.0.0",
        json!({
            "dependencies": {"@fixture/library": "workspace:^1.0.0"},
            "devDependencies": {"@fixture/second": "workspace:~1.0.0"}
        }),
    );
    fixture.succeed(&fixture.root, &["update", "--filter", "@fixture/app"]);
    fixture.member("library", "1.2.0", json!({}));
    fixture.member("second", "1.0.1", json!({}));
    fixture.succeed(
        &fixture.root,
        &[
            "update",
            "@fixture/library",
            "@fixture/second",
            "--workspace",
            "--filter",
            "@fixture/app",
        ],
    );
    assert_eq!(
        fixture.lock_version(&fixture.app, "@fixture/library"),
        "1.2.0"
    );
    assert_eq!(
        fixture.lock_version(&fixture.app, "@fixture/second"),
        "1.0.1"
    );
    let manifest = WorkspaceFixture::read_json(&fixture.app.join("oh-package.json5"));
    assert_eq!(
        manifest["dependencies"]["@fixture/library"],
        "workspace:^1.0.0"
    );
    assert_eq!(
        manifest["devDependencies"]["@fixture/second"],
        "workspace:~1.0.0"
    );
}

#[test]
fn workspace_scope_does_not_expand_to_build_profile_modules_or_shared_lockfiles() {
    let fixture = WorkspaceFixture::new();
    let broken = fixture.member(
        "broken",
        "1.0.0",
        json!({"dependencies": {"@fixture/missing": "workspace:*"}}),
    );
    WorkspaceFixture::write_json(
        &fixture.root.join("oh-package.json5"),
        &json!({"name": "@fixture/root", "version": "1.0.0"}),
    );
    WorkspaceFixture::write_json(
        &fixture.root.join("build-profile.json5"),
        &json!({"modules": [
            {"name": "app", "srcPath": "./packages/app"},
            {"name": "broken", "srcPath": "./packages/broken"}
        ]}),
    );
    fs::write(
        fixture.root.join(".ohpmrc"),
        "install_all=true\nenable_unified_lockfile=true\n",
    )
    .unwrap();
    let root_lock = fixture.root.join("oh-package-lock.json5");
    fs::write(&root_lock, "untouched root lock\n").unwrap();
    fixture.succeed(&fixture.root, &["update", "--filter", "@fixture/app"]);
    assert_eq!(
        fixture.lock_version(&fixture.app, "@fixture/library"),
        "1.0.0"
    );
    assert_eq!(
        fs::read_to_string(root_lock).unwrap(),
        "untouched root lock\n"
    );
    assert!(!fixture.root.join("oh_modules").exists());
    fixture.assert_untouched(&broken);
}

#[test]
fn unmatched_filter_and_invalid_arguments_fail_before_writing() {
    let fixture = WorkspaceFixture::new();
    let original_manifest = fs::read(fixture.app.join("oh-package.json5")).unwrap();
    for args in [
        vec!["update", "--filter", "@fixture/app,@fixture/unknown"],
        vec!["update", "--filter", "@fixture/app", "--all"],
        vec!["update", "--filter", "@fixture/app", "--all-modules"],
        vec!["update", "--workspace", "--all"],
        vec!["update", "--workspace", "--all-modules"],
        vec!["update", "--workspace", "--prefix", "missing directory"],
        vec!["update", "--workspace", "@fixture/library@1.2.0"],
        vec!["update", "--workspace", "invalid name"],
        vec!["update", "--workspace", "--registry", "invalid-registry"],
        vec!["update", "--workspace", "--fetch_timeout", "1"],
    ] {
        let output = fixture.run(&fixture.root, &args);
        assert!(!output.status.success(), "{args:?}");
        fixture.assert_untouched(&fixture.app);
        fixture.assert_untouched(&fixture.library);
        assert_eq!(
            fs::read(fixture.app.join("oh-package.json5")).unwrap(),
            original_manifest,
            "{args:?}"
        );
    }
    let no_workspace = fixture.run(fixture.root.parent().unwrap(), &["update", "--workspace"]);
    assert!(!no_workspace.status.success());
    assert!(String::from_utf8_lossy(&no_workspace.stderr).contains("ohpm-workspace.yaml"));
}

#[cfg(unix)]
#[test]
fn workspace_members_are_sorted_and_updated_once_after_full_validation() {
    let fixture = WorkspaceFixture::new();
    for name in ["z-last", "a-first"] {
        fixture.member(
            name,
            "1.0.0",
            json!({"hooks": {"preInstall": format!("echo {name} >> ../../visits.txt")}}),
        );
    }
    let failed = fixture.run(
        &fixture.root,
        &["update", "--filter", "@fixture/a-first,@fixture/unknown"],
    );
    assert!(!failed.status.success());
    assert!(!fixture.root.join("visits.txt").exists());
    let failed = fixture.run(
        &fixture.root,
        &[
            "update",
            "--filter",
            "@fixture/a-first",
            "@fixture/library@2",
        ],
    );
    assert!(!failed.status.success());
    assert!(!fixture.root.join("visits.txt").exists());
    fixture.succeed(
        &fixture.root,
        &[
            "update",
            "--filter",
            "@fixture/z-last,@fixture/a-first,@fixture/z-last",
        ],
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("visits.txt")).unwrap(),
        "a-first\nz-last\n"
    );
}

#[test]
fn workspace_update_stops_after_the_first_failed_member() {
    let fixture = WorkspaceFixture::new();
    let first = fixture.member(
        "a-first",
        "1.0.0",
        json!({"dependencies": {"@fixture/library": "workspace:*"}}),
    );
    let broken = fixture.member(
        "b-broken",
        "1.0.0",
        json!({"dependencies": {"@fixture/missing": "workspace:*"}}),
    );
    let last = fixture.member(
        "c-last",
        "1.0.0",
        json!({"dependencies": {"@fixture/library": "workspace:*"}}),
    );
    let output = fixture.run(
        &fixture.root,
        &[
            "update",
            "--filter",
            "@fixture/c-last,@fixture/b-broken,@fixture/a-first",
        ],
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("b-broken"));
    assert!(first.join("oh-package-lock.json5").exists());
    assert!(!broken.join("oh-package-lock.json5").exists());
    fixture.assert_untouched(&last);
}
