//! Real CLI uninstall coverage with local workspace dependencies and no registry.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{json, Value};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace space 卸载");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("ohpm-workspace.yaml"),
            "packages:\n  - 'packages/*'\nexclude:\n  - 'packages/excluded'\n",
        )
        .unwrap();
        let fixture = Self { _temp: temp, root };
        for name in ["runtime", "development", "dynamic"] {
            fixture.write_manifest(name, json!({"name": name, "version": "1.0.0"}));
        }
        for name in ["app", "private", "excluded"] {
            fixture.write_manifest(
                name,
                json!({
                    "name": name, "version": "1.0.0", "publish": name != "private",
                    "dependencies": {"runtime": "workspace:*"},
                    "devDependencies": {"development": "workspace:*"},
                    "dynamicDependencies": {"dynamic": "workspace:*"},
                }),
            );
        }
        fixture
    }

    fn dir(&self, member: &str) -> PathBuf {
        self.root.join("packages").join(member)
    }

    fn write_manifest(&self, member: &str, value: Value) {
        fs::create_dir_all(self.dir(member)).unwrap();
        fs::write(
            self.dir(member).join("oh-package.json5"),
            serde_json::to_vec_pretty(&value).unwrap(),
        )
        .unwrap();
    }

    fn manifest(&self, member: &str) -> Value {
        json5::from_str(&fs::read_to_string(self.dir(member).join("oh-package.json5")).unwrap())
            .unwrap()
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_ohpm-rs"))
            .args(args)
            .current_dir(cwd)
            .env("OHPM_CACHE", self.root.join("cache"))
            .env("OHPM_REGISTRY", "http://127.0.0.1:9/")
            .env("OHPM_LOG_LEVEL", "error")
            .env("OHPM_INSTALL_ALL", "true")
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

    fn install(&self) {
        self.succeed(&self.root, &["install", "--filter", "app,private"]);
    }

    fn removed(&self, member: &str, dependency: &str, field: &str) {
        assert!(self.manifest(member)[field].get(dependency).is_none());
        assert!(!self
            .dir(member)
            .join("oh_modules")
            .join(dependency)
            .exists());
    }
}

#[test]
fn workspace_uninstall_removes_multiple_dependency_kinds_including_private_members() {
    let fixture = Fixture::new();
    fixture.install();
    let excluded = fs::read(fixture.dir("excluded").join("oh-package.json5")).unwrap();
    assert!(!fixture.root.join("oh-package.json5").exists());
    fixture.succeed(
        fixture.root.parent().unwrap(),
        &[
            "uninstall",
            "runtime",
            "development",
            "dynamic",
            "--workspace",
            "--prefix",
            "workspace space 卸载",
        ],
    );
    for name in ["app", "private"] {
        fixture.removed(name, "runtime", "dependencies");
        fixture.removed(name, "development", "devDependencies");
        fixture.removed(name, "dynamic", "dynamicDependencies");
    }
    assert_eq!(
        fs::read(fixture.dir("excluded").join("oh-package.json5")).unwrap(),
        excluded
    );
    assert!(!fixture.dir("excluded").join("oh_modules").exists());
}

#[test]
fn filter_is_isolated_from_build_profile_and_install_all_configuration() {
    let fixture = Fixture::new();
    fixture.install();
    let private_manifest = fs::read(fixture.dir("private").join("oh-package.json5")).unwrap();
    let private_lock = fs::read(fixture.dir("private").join("oh-package-lock.json5")).unwrap();
    fs::write(
        fixture.root.join("oh-package.json5"),
        r#"{name:'root',dependencies:{missing:'workspace:*'}}"#,
    )
    .unwrap();
    fs::write(
        fixture.root.join("build-profile.json5"),
        r#"{modules:[
        {name:'app',srcPath:'packages/app'}, {name:'private',srcPath:'packages/private'}
    ]}"#,
    )
    .unwrap();
    fs::write(
        fixture.root.join("oh-package-lock.json5"),
        "root lock sentinel",
    )
    .unwrap();
    fixture.succeed(
        &fixture.dir("private"),
        &["uninstall", "runtime", "--filter", "app,app"],
    );
    fixture.removed("app", "runtime", "dependencies");
    assert!(fixture.dir("app").join("oh_modules/development").exists());
    assert_eq!(
        fs::read(fixture.dir("private").join("oh-package.json5")).unwrap(),
        private_manifest
    );
    assert_eq!(
        fs::read(fixture.dir("private").join("oh-package-lock.json5")).unwrap(),
        private_lock
    );
    assert!(fixture.dir("private").join("oh_modules/runtime").exists());
    assert_eq!(
        fs::read(fixture.root.join("oh-package-lock.json5")).unwrap(),
        b"root lock sentinel"
    );
    assert!(!fixture.root.join("oh_modules").exists());
}

#[test]
fn no_save_removes_selected_installation_but_preserves_manifest_bytes() {
    let fixture = Fixture::new();
    fixture.install();
    let original = fs::read(fixture.dir("private").join("oh-package.json5")).unwrap();
    fixture.succeed(
        &fixture.root,
        &[
            "uninstall",
            "runtime",
            "development",
            "--filter",
            "private",
            "--no-save",
        ],
    );
    assert_eq!(
        fs::read(fixture.dir("private").join("oh-package.json5")).unwrap(),
        original
    );
    for dependency in ["runtime", "development"] {
        assert!(!fixture
            .dir("private")
            .join("oh_modules")
            .join(dependency)
            .exists());
    }
    assert!(fixture.dir("private").join("oh_modules/dynamic").exists());
    assert!(fixture.dir("app").join("oh_modules/runtime").exists());
}

#[test]
fn ordinary_package_uninstall_accepts_a_relative_prefix() {
    let fixture = Fixture::new();
    fixture.install();
    fixture.succeed(
        fixture.root.parent().unwrap(),
        &[
            "uninstall",
            "runtime",
            "--prefix",
            "workspace space 卸载/packages/app",
        ],
    );
    fixture.removed("app", "runtime", "dependencies");
    assert!(fixture.dir("app").join("oh_modules/development").exists());
    assert!(fixture.dir("private").join("oh_modules/runtime").exists());
}

#[test]
fn invalid_arguments_or_selection_fail_before_modifying_members() {
    let fixture = Fixture::new();
    fixture.install();
    let original = fs::read(fixture.dir("app").join("oh-package.json5")).unwrap();
    let lock = fs::read(fixture.dir("app").join("oh-package-lock.json5")).unwrap();
    for args in [
        vec!["uninstall", "--workspace"],
        vec!["uninstall", "runtime", "development@1.0.0", "--workspace"],
        vec!["uninstall", "runtime", "--filter", "app,missing"],
        vec!["uninstall", "runtime", "--filter", "app,excluded"],
        vec!["uninstall", "runtime", "--filter", "app", "--all"],
        vec!["uninstall", "runtime", "--workspace", "--all"],
        vec!["uninstall", "runtime", "--workspace", "--prefix", "missing"],
        vec![
            "uninstall",
            "runtime",
            "--workspace",
            "--prefix",
            "ohpm-workspace.yaml",
        ],
    ] {
        assert!(
            !fixture.run(&fixture.root, &args).status.success(),
            "{args:?}"
        );
        assert_eq!(
            fs::read(fixture.dir("app").join("oh-package.json5")).unwrap(),
            original
        );
        assert_eq!(
            fs::read(fixture.dir("app").join("oh-package-lock.json5")).unwrap(),
            lock
        );
        assert!(fixture.dir("app").join("oh_modules/runtime").exists());
    }
    assert!(!fixture
        .run(
            fixture.root.parent().unwrap(),
            &["uninstall", "runtime", "--workspace"]
        )
        .status
        .success());
}

#[cfg(unix)]
#[test]
fn member_execution_is_sorted_deduplicated_and_stops_on_failure() {
    let fixture = Fixture::new();
    for name in ["app", "private"] {
        let mut value = fixture.manifest(name);
        value["hooks"] = json!({"preUninstall": "echo called >> hooks.log"});
        fixture.write_manifest(name, value);
    }
    fixture.succeed(
        &fixture.root,
        &["uninstall", "runtime", "--filter", "private,app,private"],
    );
    for name in ["app", "private"] {
        assert_eq!(
            fs::read_to_string(fixture.dir(name).join("hooks.log")).unwrap(),
            "called\n"
        );
    }
    let mut value = fixture.manifest("app");
    value["hooks"] = json!({"preUninstall": "exit 7"});
    fixture.write_manifest("app", value);
    let original = fs::read(fixture.dir("private").join("oh-package.json5")).unwrap();
    assert!(!fixture
        .run(
            &fixture.root,
            &["uninstall", "development", "--filter", "private,app"]
        )
        .status
        .success());
    assert_eq!(
        fs::read(fixture.dir("private").join("oh-package.json5")).unwrap(),
        original
    );
    assert_eq!(
        fs::read_to_string(fixture.dir("private").join("hooks.log")).unwrap(),
        "called\n"
    );
}
