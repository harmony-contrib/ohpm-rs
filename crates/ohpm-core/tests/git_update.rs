//! Local Git repositories exercise mutable refs and immutable commit pins.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use ohpm_core::config::Config;
use ohpm_core::install::{install, update, InstallOptions};
use ohpm_core::registry::RegistryClient;
use serde_json::{json, Value};

struct GitProject {
    _temp: tempfile::TempDir,
    repo: PathBuf,
    project: PathBuf,
    url: String,
    config: Config,
    client: RegistryClient,
}

impl GitProject {
    fn new() -> Self {
        let temp = tempfile::TempDir::new().unwrap();
        let repo = temp.path().join("source.git");
        let project = temp.path().join("project");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&project).unwrap();
        let url = url::Url::from_directory_path(&repo).unwrap().to_string();
        let mut config = Config::with_user_rc(temp.path().join("user.ohpmrc"));
        config.load(&project, None).unwrap();
        config.set_cli("cache", temp.path().join("cache").to_str().unwrap());
        config.set_cli("registry", "http://127.0.0.1:1/ohpm/");
        config.set_cli("enable_unified_lockfile", "false");
        let client = RegistryClient::from_config(&config).unwrap();
        let fixture = Self {
            _temp: temp,
            repo,
            project,
            url,
            config,
            client,
        };
        fixture.git(&["init", "-q", "-b", "main"]);
        fixture
    }

    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(&self.repo)
            .env("GIT_AUTHOR_NAME", "ohpm test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "ohpm test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn commit_package(&self, sub: &str, version: &str) -> String {
        let package = self.repo.join(sub);
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("oh-package.json5"),
            json!({
                "name": "gitpkg", "version": version
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(package.join("version.txt"), version).unwrap();
        self.git(&["add", "."]);
        self.git(&["commit", "-q", "-m", version]);
        self.git(&["rev-parse", "HEAD"])
    }

    fn dependencies(&self, fragments: &[(&str, &str)]) {
        let dependencies: BTreeMap<_, _> = fragments
            .iter()
            .map(|(name, fragment)| ((*name).to_owned(), format!("{}{}", self.url, fragment)))
            .collect();
        std::fs::write(
            self.project.join("oh-package.json5"),
            json!({
                "name": "consumer", "version": "1.0.0", "dependencies": dependencies
            })
            .to_string(),
        )
        .unwrap();
    }

    async fn install(&self) {
        install(
            &self.client,
            &self.config,
            &self.project,
            &[],
            &InstallOptions::default(),
        )
        .await
        .unwrap();
    }

    async fn update(&self, packages: &[&str]) {
        update(
            &self.client,
            &self.config,
            &self.project,
            &packages.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>(),
            &InstallOptions::default(),
        )
        .await
        .unwrap();
    }

    fn version(&self, name: &str) -> String {
        std::fs::read_to_string(
            self.project
                .join("oh_modules")
                .join(name)
                .join("version.txt"),
        )
        .unwrap()
    }

    fn lock(&self) -> Value {
        Self::read_json(&self.project.join("oh-package-lock.json5"))
    }

    fn manifest(&self) -> Value {
        Self::read_json(&self.project.join("oh-package.json5"))
    }

    fn read_json(path: &Path) -> Value {
        json5::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }
}

#[tokio::test]
async fn update_refreshes_head_and_preserves_unselected_git_pins() {
    let fx = GitProject::new();
    fx.commit_package("", "1.0.0");
    fx.dependencies(&[("first", ""), ("second", "#main")]);
    let manifest = fx.manifest();
    fx.install().await;
    let before = fx.lock();
    fx.commit_package("", "1.1.0");
    fx.install().await;
    assert_eq!(fx.lock(), before);
    fx.update(&["first"]).await;
    assert_eq!(fx.version("first"), "1.1.0");
    assert_eq!(fx.version("second"), "1.0.0");
    assert_eq!(fx.manifest(), manifest);
    fx.update(&[]).await;
    assert_eq!(fx.version("second"), "1.1.0");
}

#[tokio::test]
async fn update_resolves_nondefault_branches_and_git_subdirectories() {
    let fx = GitProject::new();
    fx.commit_package("packages/lib", "1.0.0");
    fx.git(&["checkout", "-q", "-b", "feature/next"]);
    fx.commit_package("packages/lib", "1.1.0");
    fx.git(&["checkout", "-q", "main"]);
    fx.dependencies(&[("gitpkg", "#heads/feature/next&path:packages/lib")]);
    let manifest = fx.manifest();
    fx.install().await;
    assert_eq!(fx.version("gitpkg"), "1.1.0");
    fx.git(&["checkout", "-q", "feature/next"]);
    let latest = fx.commit_package("packages/lib", "1.2.0");
    fx.git(&["checkout", "-q", "main"]);
    fx.update(&[]).await;
    assert_eq!(fx.version("gitpkg"), "1.2.0");
    assert!(fx
        .lock()
        .to_string()
        .contains(&format!("{latest}&path:packages/lib")));
    assert_eq!(fx.manifest(), manifest);
    assert!(!fx.project.join("oh_modules/gitpkg/packages").exists());
}

#[tokio::test]
async fn update_peels_annotated_tags_to_commits() {
    let fx = GitProject::new();
    fx.commit_package("", "1.0.0");
    fx.git(&["tag", "-a", "stable", "-m", "stable"]);
    fx.dependencies(&[("gitpkg", "#stable")]);
    fx.install().await;
    let latest = fx.commit_package("", "1.1.0");
    fx.git(&["tag", "-f", "-a", "stable", "-m", "stable"]);
    fx.update(&[]).await;
    assert_eq!(fx.version("gitpkg"), "1.1.0");
    assert!(fx.lock().to_string().contains(&latest));
    assert!(!fx
        .lock()
        .to_string()
        .contains(&fx.git(&["rev-parse", "stable"])));
}

#[tokio::test]
async fn update_selects_new_git_semver_tags_within_the_declared_range() {
    let fx = GitProject::new();
    fx.commit_package("", "1.0.0");
    fx.git(&["tag", "v1.0.0"]);
    fx.dependencies(&[("gitpkg", "#semver:^1.0.0")]);
    let manifest = fx.manifest();
    fx.install().await;
    fx.commit_package("", "1.2.0");
    fx.git(&["tag", "-a", "v1.2.0", "-m", "v1.2.0"]);
    fx.commit_package("", "2.0.0");
    fx.git(&["tag", "v2.0.0"]);
    fx.update(&[]).await;
    assert_eq!(fx.version("gitpkg"), "1.2.0");
    assert_eq!(fx.manifest(), manifest);
}

#[tokio::test]
async fn update_keeps_full_commit_pins_and_reuses_cached_content_offline() {
    let fx = GitProject::new();
    let pinned = fx.commit_package("packages/lib", "1.0.0");
    fx.dependencies(&[("gitpkg", &format!("#{pinned}&path:packages/lib"))]);
    fx.install().await;
    let before = fx.lock();
    fx.commit_package("packages/lib", "1.1.0");
    fx.update(&[]).await;
    assert_eq!(fx.lock(), before);
    std::fs::remove_dir_all(&fx.repo).unwrap();
    fx.update(&[]).await;
    assert_eq!(fx.version("gitpkg"), "1.0.0");
    assert_eq!(fx.lock(), before);
}

#[tokio::test]
async fn failed_git_update_preserves_the_existing_lock_and_links() {
    let fx = GitProject::new();
    fx.commit_package("", "1.0.0");
    fx.dependencies(&[("gitpkg", "#main")]);
    fx.install().await;
    let before = fx.lock();
    std::fs::remove_dir_all(&fx.repo).unwrap();
    let error = update(
        &fx.client,
        &fx.config,
        &fx.project,
        &[],
        &InstallOptions::default(),
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "GitLsRemoteFailed");
    assert_eq!(fx.lock(), before);
    assert_eq!(fx.version("gitpkg"), "1.0.0");
}
