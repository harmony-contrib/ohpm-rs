//! Exercise script execution through the real CLI, without Node or ohpm.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{json, Value};

struct Fixture {
    temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new(scripts: Value) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project space 库");
        fs::create_dir_all(root.join("sub/deep")).unwrap();
        fs::write(
            root.join("oh-package.json5"),
            serde_json::to_vec(&json!({"scripts": scripts})).unwrap(),
        )
        .unwrap();
        Self { temp, root }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ohpm-rs"));
        command
            .arg("run")
            .current_dir(self.root.join("sub/deep"))
            .env("OHPM_LOG_LEVEL", "error");
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }

    fn succeed(&self, args: &[&str]) -> Output {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn contents(&self, name: &str) -> String {
        fs::read_to_string(self.root.join(name))
            .unwrap()
            .trim()
            .to_owned()
    }

    fn workspace(&self) {
        fs::write(
            self.root.join("ohpm-workspace.yaml"),
            "packages:\n  - 'packages/*'\nexclude:\n  - '@fixture/excluded'\n",
        )
        .unwrap();
    }

    fn member(&self, directory: &str, name: &str, scripts: Value) -> PathBuf {
        let dir = self.root.join("packages").join(directory);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("oh-package.json5"),
            serde_json::to_vec(&json!({
                "name": name, "version": "1.0.0", "publish": false, "scripts": scripts
            }))
            .unwrap(),
        )
        .unwrap();
        dir
    }
}

#[test]
fn workspace_runs_private_members_in_path_order_and_skips_missing_and_excluded_scripts() {
    let fixture = Fixture::new(json!({"build": "echo root > root.txt"}));
    fixture.workspace();
    // Create in reverse order to ensure directory order drives execution.
    fixture.member(
        "b",
        "@fixture/b",
        json!({"build": "echo b --mode default >> ../../order.txt"}),
    );
    fixture.member(
        "a",
        "@fixture/a",
        json!({"build": "echo a --mode default >> ../../order.txt"}),
    );
    fixture.member("missing", "@fixture/missing", json!({"other": "exit 1"}));
    fixture.member("excluded", "@fixture/excluded", json!({"build": "exit 1"}));
    let output = fixture.succeed(&["build", "--workspace", "--", "--mode=release"]);
    assert_eq!(
        fixture.contents("order.txt").lines().collect::<Vec<_>>(),
        ["a --mode=release", "b --mode=release"]
    );
    assert!(!fixture.root.join("root.txt").exists());
    assert!(String::from_utf8_lossy(&output.stdout).contains("2 workspace package(s)"));
    // Ordinary run still uses only the local package inside a workspace.
    fixture.succeed(&["build"]);
    assert_eq!(fixture.contents("root.txt"), "root");
}

#[test]
fn workspace_filter_implies_batch_mode_deduplicates_and_supports_prefix_without_root_manifest() {
    let fixture = Fixture::new(json!({}));
    fixture.workspace();
    fs::remove_file(fixture.root.join("oh-package.json5")).unwrap();
    fixture.member(
        "a",
        "@fixture/a",
        json!({"build": "echo a >> ../../order.txt"}),
    );
    fixture.member(
        "b",
        "@fixture/b",
        json!({"build": "echo b >> ../../order.txt"}),
    );
    fixture.member(
        "unselected",
        "@fixture/unselected",
        json!({"build": "ohpm install"}),
    );
    let output = fixture
        .command()
        .current_dir(fixture.temp.path())
        .args([
            "build",
            "--prefix",
            "project space 库",
            "--filter",
            "@fixture/b,@fixture/a,@fixture/b",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fixture.contents("order.txt").lines().collect::<Vec<_>>(),
        ["a", "b"]
    );
    let member = fixture.root.join("packages/a");
    let output = fixture
        .command()
        .current_dir(member)
        .args(["build", "--workspace", "--filter", "@fixture/b"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        fixture.contents("order.txt").lines().collect::<Vec<_>>(),
        ["a", "b", "b"]
    );
}

#[test]
fn workspace_validates_all_plans_before_execution_and_stops_on_runtime_failure() {
    let fixture = Fixture::new(json!({}));
    fixture.workspace();
    fixture.member(
        "a",
        "@fixture/a",
        json!({"build": "echo a >> ../../order.txt"}),
    );
    fixture.member("b", "@fixture/b", json!({"build": "ohpm run build"}));
    fixture.member(
        "c",
        "@fixture/c",
        json!({"build": "echo c >> ../../order.txt"}),
    );
    let output = fixture.run(&["build", "--workspace"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("@fixture/b"));
    assert!(!fixture.root.join("order.txt").exists());
    fixture.member("b", "@fixture/b", json!({"build": "exit 7"}));
    let output = fixture.run(&["build", "--workspace"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(fixture.contents("order.txt"), "a");
}

#[test]
fn workspace_reports_invalid_scope_filters_and_all_missing_scripts() {
    let fixture = Fixture::new(json!({}));
    let output = fixture.run(&["build", "--workspace"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("ohpm-workspace.yaml"));
    fixture.workspace();
    fixture.member("a", "@fixture/a", json!({}));
    for (args, message) in [
        (
            vec!["build", "--workspace"],
            "No selected workspace package defines",
        ),
        (
            vec!["build", "--filter", "@fixture/absent"],
            "No workspace package matches",
        ),
        (
            vec!["build", "--workspace", "--prefix", "missing"],
            "workspace prefix is not a directory",
        ),
    ] {
        let output = fixture.run(&args);
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(message),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    // A declared but empty script is an error, not a missing-script skip.
    fixture.member("a", "@fixture/a", json!({"build": ""}));
    let output = fixture.run(&["build", "--workspace"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("content is empty"));
}

// Linux filesystems accept arbitrary non-NUL bytes; macOS APFS rejects them.
#[cfg(target_os = "linux")]
#[test]
fn package_paths_preserve_non_utf8_components() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let fixture = Fixture::new(json!({}));
    let dir = fixture.root.join(OsString::from_vec(vec![b'p', 0xff]));
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("oh-package.json5"),
        "{scripts: {build: 'echo done > result.txt'}}",
    )
    .unwrap();
    let output = fixture
        .command()
        .args(["build", "--prefix"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(dir.join("result.txt")).unwrap().trim(),
        "done"
    );
}

#[test]
fn discovers_nearest_json5_package_and_executes_in_its_directory() {
    let fixture = Fixture::new(json!({}));
    let manifest = "{ // JSON5 with no name/version\n scripts: { build: 'echo built > result.txt', prebuild: 'echo wrong > hook.txt', }, }";
    fs::write(fixture.root.join("oh-package.json5"), manifest).unwrap();
    let output = fixture.succeed(&["build"]);
    assert_eq!(fixture.contents("result.txt"), "built");
    assert!(!fixture.root.join("hook.txt").exists());
    assert_eq!(fixture.contents("oh-package.json5"), manifest);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Run script completed"));

    fs::write(
        fixture.root.join("sub/oh-package.json5"),
        "{scripts: {build: 'echo nearest > result.txt'}}",
    )
    .unwrap();
    fixture.succeed(&["build"]);
    assert_eq!(fixture.contents("sub/result.txt"), "nearest");
}

#[test]
fn prefix_selects_exact_package_from_outside_with_spaces_and_unicode() {
    let fixture = Fixture::new(json!({"build": "echo chosen > result.txt"}));
    for args in [
        vec!["--prefix", "project space 库", "build"],
        vec![
            "build",
            "--prefix",
            "project space 库",
            "--log_level",
            "warn",
            "--debug",
        ],
    ] {
        let output = fixture
            .command()
            .current_dir(fixture.temp.path())
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fixture.contents("result.txt"), "chosen");
    }
    let output = fixture.run(&["build", "--prefix", "."]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid prefix"));
}

#[test]
fn parameters_override_both_key_forms_and_only_reach_the_last_command() {
    let fixture = Fixture::new(json!({
        "build": "echo first --name original > first.txt && echo last -name=old --keep yes > last.txt"
    }));
    fixture.succeed(&["build", "--", "--name", "new", "-extra=a=b", "--flag"]);
    assert_eq!(fixture.contents("first.txt"), "first --name original");
    assert_eq!(
        fixture.contents("last.txt"),
        "last --keep yes --name new -extra=a=b --flag"
    );
    // Help/debug/log_level after the separator belong to the script.
    fixture.succeed(&["build", "--", "--help", "--debug", "--log_level=warn"]);
    assert_eq!(
        fixture.contents("last.txt"),
        "last -name=old --keep yes --help --debug --log_level=warn"
    );
    assert!(!fixture.run(&["build", "--name", "new"]).status.success());
}

#[test]
fn expands_nested_and_repeated_aliases_without_an_ohpm_executable() {
    let fixture = Fixture::new(json!({
        "base": "echo --name original >> result.txt",
        "middle": "ohpm run base -- --name middle",
        "outer": "ohpm run middle -- --name outer",
        "twice": "ohpm run base && ohpm run base"
    }));
    fixture.succeed(&["outer", "--", "-name=cli"]);
    assert_eq!(fixture.contents("result.txt"), "-name=cli");
    fixture.succeed(&["twice"]);
    assert_eq!(fixture.contents("result.txt").lines().count(), 3);
}

#[test]
fn rejects_entire_invalid_plan_before_any_script_runs() {
    let fixture = Fixture::new(json!({
        "cycle": "ohpm run other", "other": "ohpm run cycle",
        "invalid": "echo side-effect > result.txt && ohpm run cycle",
        "missing": "echo side-effect > result.txt && ohpm run absent",
        "forbidden": "echo side-effect > result.txt && ohpm install"
    }));
    for (alias, message) in [
        ("invalid", "circular invocation"),
        ("missing", "does not exist in oh-package.json5"),
        ("forbidden", "Cannot execute ohpm commands"),
    ] {
        let output = fixture.run(&[alias]);
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains(message));
        assert!(!fixture.root.join("result.txt").exists());
    }
}

#[test]
fn logical_operators_match_ohpm_short_circuit_and_failure_exit_status() {
    let fixture = Fixture::new(json!({
        "fail": "exit 7",
        "and": "exit 7 && echo bad > bad.txt || echo recovery > recovery.txt",
        "or": "echo good > good.txt || echo bad > bad.txt && echo tail > tail.txt",
        "recover": "ohpm run fail || echo recovered > recovery.txt",
        "quoted": "echo \"a && b || c\" > quoted.txt && echo done > done.txt"
    }));
    assert_eq!(fixture.run(&["fail"]).status.code(), Some(1));
    assert_eq!(fixture.run(&["and"]).status.code(), Some(1));
    assert!(!fixture.root.join("recovery.txt").exists());
    fixture.succeed(&["or"]);
    assert!(!fixture.root.join("bad.txt").exists());
    assert!(!fixture.root.join("tail.txt").exists());
    fixture.succeed(&["recover"]);
    assert_eq!(fixture.contents("recovery.txt"), "recovered");
    fixture.succeed(&["quoted"]);
    assert!(fixture.contents("quoted.txt").contains("a && b || c"));
    assert_eq!(fixture.contents("done.txt"), "done");
}

#[test]
fn missing_manifest_bad_json_and_missing_alias_fail_cleanly() {
    let fixture = Fixture::new(json!({}));
    assert!(!fixture.run(&[]).status.success());
    assert!(fixture.run(&["--help"]).status.success());
    let missing = fixture
        .command()
        .current_dir(fixture.temp.path())
        .arg("build")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&missing.stderr).contains("oh-package.json5 does not exist"));
    let absent = fixture.run(&["build"]);
    assert!(String::from_utf8_lossy(&absent.stderr).contains("script alias does not exist"));
    fs::write(fixture.root.join("oh-package.json5"), "{scripts: broken}").unwrap();
    let invalid = fixture.run(&["build"]);
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("Failed to parse oh-package.json5"));
}

#[test]
fn missing_scripts_suggest_nearby_aliases_including_nested_calls() {
    let fixture = Fixture::new(json!({
        "build": "echo unexpected > result.txt",
        "release": "ohpm run biuld",
        "fail": "exit 7",
        "fall": "echo unexpected > result.txt"
    }));
    for alias in ["biuld", "release"] {
        let output = fixture.run(&[alias]);
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains("\n  build"));
    }
    assert!(!fixture.root.join("result.txt").exists());
    for alias in ["unrelated", "fail"] {
        let output = fixture.run(&[alias]);
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("Did you mean"));
    }
}

#[test]
fn workspace_suggestions_only_include_selected_members_and_do_not_execute_them() {
    let fixture = Fixture::new(json!({}));
    fixture.workspace();
    fixture.member(
        "a",
        "@fixture/a",
        json!({"build": "echo unexpected > result.txt"}),
    );
    fixture.member(
        "b",
        "@fixture/b",
        json!({"build": "echo unexpected > result.txt", "buildx": "exit 7"}),
    );
    let output = fixture.run(&["biuld", "--workspace"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.lines().filter(|line| line.trim() == "build").count(),
        1
    );
    assert!(stderr.contains("\n  buildx"));
    let output = fixture.run(&["biuld", "--filter", "@fixture/a"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("buildx"));
    assert!(!fixture.root.join("packages/a/result.txt").exists());
    assert!(!fixture.root.join("packages/b/result.txt").exists());
}

#[cfg(unix)]
#[test]
fn inherits_environment_stdin_stdout_stderr_and_honors_shell() {
    use std::io::Write;
    use std::process::Stdio;

    let fixture = Fixture::new(json!({
        "io": "printf '%s\\n' \"$OHPM_RUN_TEST_VALUE\"; cat; echo child-stderr >&2",
        "bash": "[[ 1 == 1 ]]"
    }));
    let mut child = fixture
        .command()
        .arg("io")
        .env("SHELL", "/bin/sh")
        .env("OHPM_RUN_TEST_VALUE", "inherited value")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"input payload\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("inherited value\ninput payload"));
    assert!(String::from_utf8_lossy(&output.stderr).contains("child-stderr"));
    // '(' / '[' / '{' in a command name is forbidden, even in shells that support it.
    assert!(!fixture
        .command()
        .arg("bash")
        .env("SHELL", "/bin/bash")
        .output()
        .unwrap()
        .status
        .success());
    assert!(!fixture
        .command()
        .arg("io")
        .env("SHELL", fixture.root.join("missing-shell"))
        .output()
        .unwrap()
        .status
        .success());
}
