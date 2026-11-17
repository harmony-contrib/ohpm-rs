//! `ohpm init` scaffolding coverage (non-interactive paths).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("init-case");
        fs::create_dir_all(&root).unwrap();
        Self { _temp: temp, root }
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

    fn succeed(&self, cwd: &Path, args: &[&str]) -> String {
        let output = self.run(cwd, args);
        assert!(
            output.status.success(),
            "{args:?}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn fail(&self, cwd: &Path, args: &[&str]) -> String {
        let output = self.run(cwd, args);
        assert!(!output.status.success(), "{args:?} unexpectedly succeeded");
        String::from_utf8_lossy(&output.stderr).into_owned()
    }
}

fn read_json5(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    json5::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}\n{text}", path.display()))
}

fn assert_png(path: &Path) {
    let bytes = fs::read(path).unwrap();
    assert_eq!(
        &bytes[..8],
        b"\x89PNG\r\n\x1a\n",
        "{} is not a PNG",
        path.display()
    );
}

#[test]
fn bare_init_writes_the_default_package_manifest() {
    let fixture = Fixture::new();
    fixture.succeed(&fixture.root, &["init", "-y"]);

    let manifest = read_json5(&fixture.root.join("oh-package.json5"));
    assert_eq!(manifest["name"], "init-case");
    assert_eq!(manifest["version"], "1.0.0");
    assert_eq!(manifest["main"], "index.ets");
}

#[test]
fn package_manifest_honors_name_group_and_path() {
    let fixture = Fixture::new();
    let target = fixture.root.join("nested/pkg");

    fixture.succeed(
        &fixture.root,
        &[
            "init",
            "-y",
            "--path",
            target.to_str().unwrap(),
            "--group",
            "demo",
        ],
    );

    let manifest = read_json5(&target.join("oh-package.json5"));
    assert_eq!(manifest["name"], "@demo/init-case");
}

#[test]
fn project_scaffold_has_dev_eco_layout() {
    let fixture = Fixture::new();
    let target = fixture.root.join("MyApp");
    fs::create_dir_all(&target).unwrap();

    fixture.succeed(&target, &["init", "-y", "--type", "project", "Demo"]);

    for path in [
        "AppScope/app.json5",
        "AppScope/resources/base/element/string.json",
        "AppScope/resources/base/media/background.png",
        "AppScope/resources/base/media/foreground.png",
        "AppScope/resources/base/media/layered_image.json",
        "build-profile.json5",
        "code-linter.json5",
        "hvigorfile.ts",
        "hvigor/hvigor-config.json5",
        "oh-package.json5",
        "entry/build-profile.json5",
        "entry/hvigorfile.ts",
        "entry/oh-package.json5",
        "entry/src/main/module.json5",
        "entry/src/main/ets/entryability/EntryAbility.ets",
        "entry/src/main/ets/entrybackupability/EntryBackupAbility.ets",
        "entry/src/main/ets/pages/Index.ets",
        "entry/src/main/resources/base/element/string.json",
        "entry/src/main/resources/base/media/startIcon.png",
        "entry/src/main/resources/base/profile/main_pages.json",
        "entry/src/ohosTest/module.json5",
        "entry/src/test/LocalUnit.test.ets",
    ] {
        assert!(target.join(path).is_file(), "missing {path}");
    }
    assert!(target.join("entry/src/main/resources/rawfile").is_dir());

    let app = read_json5(&target.join("AppScope/app.json5"));
    assert_eq!(app["app"]["bundleName"], "com.example.demo");

    let root_profile = read_json5(&target.join("build-profile.json5"));
    assert_eq!(
        root_profile["app"]["products"][0]["compatibleSdkVersion"],
        "5.0.0(12)"
    );
    assert_eq!(root_profile["modules"][0]["name"], "entry");
    assert_eq!(root_profile["modules"][0]["srcPath"], "./entry");

    let module = read_json5(&target.join("entry/src/main/module.json5"));
    assert_eq!(module["module"]["type"], "entry");
    assert_eq!(module["module"]["mainElement"], "EntryAbility");

    assert_png(&target.join("AppScope/resources/base/media/background.png"));
    assert_png(&target.join("entry/src/main/resources/base/media/startIcon.png"));
}

#[test]
fn har_scaffold_creates_a_static_library_project() {
    let fixture = Fixture::new();
    let target = fixture.root.join("HarProject");
    fs::create_dir_all(&target).unwrap();

    fixture.succeed(&target, &["init", "-y", "--type", "har", "@demo/my-lib"]);

    let manifest = read_json5(&target.join("my_lib/oh-package.json5"));
    assert_eq!(manifest["name"], "@demo/my-lib");
    assert_eq!(manifest["main"], "Index.ets");

    let module = read_json5(&target.join("my_lib/src/main/module.json5"));
    assert_eq!(module["module"]["type"], "har");
    assert_eq!(module["module"]["name"], "my_lib");

    let hvigor = fs::read_to_string(target.join("my_lib/hvigorfile.ts")).unwrap();
    assert!(hvigor.contains("harTasks"));

    let root_profile = read_json5(&target.join("build-profile.json5"));
    assert_eq!(root_profile["modules"][0]["name"], "my_lib");
    assert_eq!(root_profile["modules"][0]["srcPath"], "./my_lib");

    let app = read_json5(&target.join("AppScope/app.json5"));
    assert_eq!(app["app"]["bundleName"], "com.example.my_lib");
}

#[test]
fn hsp_scaffold_creates_a_shared_library_project() {
    let fixture = Fixture::new();
    let target = fixture.root.join("HspProject");
    fs::create_dir_all(&target).unwrap();

    fixture.succeed(
        &target,
        &["init", "-y", "--type", "hsp", "@demo/shared-lib"],
    );

    let module = read_json5(&target.join("shared_lib/src/main/module.json5"));
    assert_eq!(module["module"]["type"], "shared");
    assert_eq!(module["module"]["pages"], "$profile:main_pages");

    let hvigor = fs::read_to_string(target.join("shared_lib/hvigorfile.ts")).unwrap();
    assert!(hvigor.contains("hspTasks"));

    let index = fs::read_to_string(target.join("shared_lib/Index.ets")).unwrap();
    assert!(index.contains("./src/main/ets/utils/Calc"));

    let root_profile = read_json5(&target.join("build-profile.json5"));
    assert_eq!(
        root_profile["modules"][0]["targets"][0]["applyToProducts"][0],
        "default"
    );
    assert!(target
        .join("shared_lib/src/main/resources/base/media")
        .is_dir());
}

#[test]
fn native_init_scaffolds_rust_project_and_package() {
    let fixture = Fixture::new();
    let target = fixture.root.join("Native");
    fs::create_dir_all(&target).unwrap();

    fixture.succeed(
        &target,
        &[
            "init",
            "--type",
            "native",
            "--package",
            "@demo/native",
            "my-native",
        ],
    );

    let project = target.join("my-native");
    for path in [
        "Cargo.toml",
        "build.rs",
        "src/lib.rs",
        ".gitignore",
        "package/oh-package.json5",
        "package/index.ets",
        "package/src/main/module.json5",
    ] {
        assert!(project.join(path).is_file(), "missing {path}");
    }

    let cargo = fs::read_to_string(project.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("my-native"), "{cargo}");

    let manifest = read_json5(&project.join("package/oh-package.json5"));
    assert_eq!(manifest["name"], "@demo/native");
}

#[test]
fn api_version_controls_model_and_sdk() {
    let fixture = Fixture::new();
    let target = fixture.root.join("Api20");
    fs::create_dir_all(&target).unwrap();

    fixture.succeed(
        &target,
        &[
            "init",
            "-y",
            "--type",
            "project",
            "--api-version",
            "20",
            "Demo",
        ],
    );

    let root_manifest = read_json5(&target.join("oh-package.json5"));
    assert_eq!(root_manifest["modelVersion"], "6.0.0");
    assert_eq!(root_manifest["devDependencies"]["@ohos/hypium"], "1.0.24");

    let profile = read_json5(&target.join("build-profile.json5"));
    assert_eq!(
        profile["app"]["products"][0]["targetSdkVersion"],
        "6.0.0(20)"
    );
}

#[test]
fn older_api_uses_the_single_image_icon_layout() {
    let fixture = Fixture::new();
    let target = fixture.root.join("Api10");
    fs::create_dir_all(&target).unwrap();

    fixture.succeed(
        &target,
        &[
            "init",
            "-y",
            "--type",
            "project",
            "--api-version",
            "4.0.0(10)",
            "Demo",
        ],
    );

    assert!(target
        .join("AppScope/resources/base/media/app_icon.png")
        .is_file());
    assert!(!target
        .join("AppScope/resources/base/media/layered_image.json")
        .exists());
    assert!(target
        .join("entry/src/main/resources/base/media/icon.png")
        .is_file());

    let app = read_json5(&target.join("AppScope/app.json5"));
    assert_eq!(app["app"]["icon"], "$media:app_icon");

    let module = read_json5(&target.join("entry/src/main/module.json5"));
    assert_eq!(module["module"]["abilities"][0]["icon"], "$media:icon");
}

#[test]
fn scaffold_refuses_to_overwrite_without_force() {
    let fixture = Fixture::new();
    let target = fixture.root.join("Overwrite");
    fs::create_dir_all(&target).unwrap();

    fixture.succeed(&target, &["init", "-y", "--type", "har", "demo-lib"]);
    let stderr = fixture.fail(&target, &["init", "-y", "--type", "har", "demo-lib"]);
    assert!(stderr.contains("Refusing to overwrite"), "{stderr}");

    fixture.succeed(
        &target,
        &["init", "-y", "--type", "har", "demo-lib", "--force"],
    );
}

#[test]
fn invalid_combinations_are_rejected() {
    let fixture = Fixture::new();

    let stderr = fixture.fail(
        &fixture.root,
        &["init", "-y", "--type", "project", "--group", "demo", "Demo"],
    );
    assert!(stderr.contains("--group"), "{stderr}");

    let stderr = fixture.fail(&fixture.root, &["init", "-y", "--type", "har", "MyLib"]);
    assert!(stderr.contains("invalid"), "{stderr}");

    let stderr = fixture.fail(&fixture.root, &["init", "-y", "--api-version", "12"]);
    assert!(stderr.contains("--api-version"), "{stderr}");

    let stderr = fixture.fail(
        &fixture.root,
        &[
            "init",
            "-y",
            "--type",
            "project",
            "--bundle-name",
            "bad bundle",
            "Demo",
        ],
    );
    assert!(stderr.contains("bundle name"), "{stderr}");
}

#[test]
fn project_name_is_escaped_in_manifests() {
    let fixture = Fixture::new();
    let target = fixture.root.join("Escaped");
    fs::create_dir_all(&target).unwrap();

    fixture.succeed(
        &target,
        &["init", "-y", "--type", "project", "Demo \"quoted\""],
    );

    let strings = read_json5(&target.join("AppScope/resources/base/element/string.json"));
    assert_eq!(strings["string"][0]["value"], "Demo \"quoted\"");
}

#[test]
fn api_version_rejects_unexpected_characters() {
    let fixture = Fixture::new();
    let stderr = fixture.fail(
        &fixture.root,
        &[
            "init",
            "-y",
            "--type",
            "project",
            "--api-version",
            "5.0.0(12)\"",
            "Demo",
        ],
    );
    assert!(stderr.contains("invalid --api-version"), "{stderr}");
}
