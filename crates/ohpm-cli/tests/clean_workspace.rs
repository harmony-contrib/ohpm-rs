//! Real CLI cleanup coverage, including isolated workspace selection.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::json;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace space 清理");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("ohpm-workspace.yaml"),
            "packages:\n  - 'packages/*'\nexclude:\n  - 'packages/excluded'\n",
        )
        .unwrap();
        let fixture = Self { _temp: temp, root };
        for name in ["app", "private", "excluded"] {
            fixture.package(&fixture.root.join("packages").join(name), name);
        }
        fixture.artifacts(&fixture.root);
        fixture
    }

    fn package(&self, directory: &Path, name: &str) {
        fs::create_dir_all(directory).unwrap();
        fs::write(
            directory.join("oh-package.json5"),
            serde_json::to_vec(&json!({
                "name": name, "version": "1.0.0", "publish": name != "private"
            }))
            .unwrap(),
        )
        .unwrap();
        self.artifacts(directory);
        fs::write(directory.join("source.ets"), "export {};\n").unwrap();
    }

    fn artifacts(&self, directory: &Path) {
        fs::create_dir_all(directory.join("oh_modules")).unwrap();
        fs::write(directory.join("oh_modules/installed"), "payload").unwrap();
        for name in ["oh-package-lock.json5", "oh-package-debug-lock.json5"] {
            fs::write(directory.join(name), "lock sentinel").unwrap();
        }
    }

    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_ohpm-rs"))
            .args(args)
            .current_dir(cwd)
            .env("OHPM_CACHE", self.root.join("cache"))
            .env("OHPM_LOG_LEVEL", "error")
            .output()
            .unwrap()
    }

    fn succeed(&self, cwd: &Path, args: &[&str]) {
        let output = self.run(cwd, args);
        assert!(
            output.status.success(),
            "{args:?}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn intact(&self, directory: &Path) {
        assert_eq!(
            fs::read(directory.join("oh_modules/installed")).unwrap(),
            b"payload"
        );
        assert_eq!(
            fs::read(directory.join("oh-package-lock.json5")).unwrap(),
            b"lock sentinel"
        );
    }

    fn cleaned(&self, directory: &Path) {
        assert!(!directory.join("oh_modules").exists());
        assert!(!directory.join("oh-package-lock.json5").exists());
        assert!(!directory.join("oh-package-debug-lock.json5").exists());
        assert!(directory.join("oh-package.json5").is_file());
        assert!(directory.join("source.ets").is_file());
    }
}

#[test]
fn filter_implies_workspace_and_prefix_handles_spaces_without_root_manifest() {
    let fixture = Fixture::new();
    assert!(!fixture.root.join("oh-package.json5").exists());
    fixture.succeed(
        fixture.root.parent().unwrap(),
        &[
            "clean",
            "--prefix",
            "workspace space 清理",
            "--filter",
            "private,app,private",
            "--keep-lockfile",
        ],
    );
    for name in ["app", "private"] {
        let directory = fixture.root.join("packages").join(name);
        assert!(!directory.join("oh_modules").exists());
        assert_eq!(
            fs::read(directory.join("oh-package-lock.json5")).unwrap(),
            b"lock sentinel"
        );
        assert_eq!(
            fs::read(directory.join("oh-package-debug-lock.json5")).unwrap(),
            b"lock sentinel"
        );
    }
    fixture.intact(&fixture.root.join("packages/excluded"));
    fixture.intact(&fixture.root);
}

#[test]
fn workspace_cleans_private_members_and_respects_excludes() {
    let fixture = Fixture::new();
    fixture.succeed(
        &fixture.root.join("packages/app"),
        &["clean", "--workspace"],
    );
    fixture.cleaned(&fixture.root.join("packages/app"));
    fixture.cleaned(&fixture.root.join("packages/private"));
    fixture.intact(&fixture.root.join("packages/excluded"));
    fixture.intact(&fixture.root);
}

#[test]
fn invalid_selection_is_rejected_before_any_cleanup() {
    let fixture = Fixture::new();
    for args in [
        vec!["clean", "--filter", "app,missing"],
        vec!["clean", "--filter", "app,excluded"],
        vec!["clean", "--workspace", "--prefix", "missing"],
        vec!["clean", "--workspace", "--prefix", "ohpm-workspace.yaml"],
    ] {
        assert!(
            !fixture.run(&fixture.root, &args).status.success(),
            "{args:?}"
        );
        fixture.intact(&fixture.root.join("packages/app"));
        fixture.intact(&fixture.root);
    }
    let missing_workspace = fixture.run(fixture.root.parent().unwrap(), &["clean", "--workspace"]);
    assert!(!missing_workspace.status.success());
    assert!(String::from_utf8_lossy(&missing_workspace.stderr).contains("ohpm-workspace.yaml"));
}

#[test]
fn standalone_package_prefix_and_alias_preserve_then_remove_locks() {
    let fixture = Fixture::new();
    fixture.succeed(&fixture.root, &["cls", "--prefix", "packages/app", "--kl"]);
    let app = fixture.root.join("packages/app");
    assert!(!app.join("oh_modules").exists());
    assert!(app.join("oh-package-lock.json5").is_file());
    fixture.succeed(&fixture.root, &["clean", "--prefix", "packages/app"]);
    fixture.cleaned(&app);
    fixture.intact(&fixture.root.join("packages/private"));
}

#[test]
fn ordinary_project_cleanup_includes_build_profile_modules_from_nested_directory() {
    let fixture = Fixture::new();
    fixture.package(&fixture.root, "root");
    fs::write(
        fixture.root.join("build-profile.json5"),
        r#"{modules:[
        {name:'app', srcPath:'packages/app'}, {name:'private', srcPath:'packages/private'}
    ]}"#,
    )
    .unwrap();
    fs::create_dir_all(fixture.root.join("packages/app/src")).unwrap();
    fixture.succeed(&fixture.root.join("packages/app/src"), &["clean"]);
    fixture.cleaned(&fixture.root);
    fixture.cleaned(&fixture.root.join("packages/app"));
    fixture.cleaned(&fixture.root.join("packages/private"));
    fixture.intact(&fixture.root.join("packages/excluded"));
}

#[test]
fn invalid_build_profile_module_does_not_partially_clean_project() {
    let fixture = Fixture::new();
    fixture.package(&fixture.root, "root");
    fs::write(
        fixture.root.join("build-profile.json5"),
        r#"{modules:[
        {name:'app', srcPath:'packages/app'}, {name:'missing', srcPath:'packages/missing'}
    ]}"#,
    )
    .unwrap();
    assert!(!fixture.run(&fixture.root, &["clean"]).status.success());
    fixture.intact(&fixture.root);
    fixture.intact(&fixture.root.join("packages/app"));
}
