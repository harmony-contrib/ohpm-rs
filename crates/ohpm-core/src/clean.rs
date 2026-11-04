//! Clean installed modules and lockfiles in a project, package, or workspace.

use std::path::{Path, PathBuf};

use crate::constants::{MY_MODULES, MY_PACKAGE_JSON};
use crate::error::{OhpmError, Result};
use crate::filesystem::DirectoryLink;
use crate::install::modules::ProjectBuildProfile;
use crate::paths::PackagePath;
use crate::workspace::Workspace;

/// Own the validated cleanup scope and apply it without expanding member selection.
pub struct Cleaner {
    roots: Vec<PathBuf>,
    keep_lockfile: bool,
}

impl Cleaner {
    /// Preserve project-wide build-profile cleanup, with a standalone package fallback.
    pub fn for_directory(directory: &Path, keep_lockfile: bool) -> Result<Self> {
        let directory = PackagePath::canonicalize(directory)?;
        let project = directory.ancestors().find_map(ProjectBuildProfile::load);
        let roots = if let Some(project) = project {
            if !project.project_root.join(MY_PACKAGE_JSON).is_file() {
                return Err(Self::missing_project(&directory));
            }
            let mut roots = project.module_roots;
            roots.push(project.project_root);
            roots
        } else {
            let prefix = crate::config::find_local_prefix(&directory)
                .filter(|prefix| prefix.join(MY_PACKAGE_JSON).is_file())
                .ok_or_else(|| Self::missing_project(&directory))?;
            vec![prefix]
        };
        Self::new(roots, keep_lockfile)
    }

    /// Filters are exact package names; validate all names before cleaning any member.
    pub fn for_workspace(
        workspace: &Workspace,
        filter: &[String],
        keep_lockfile: bool,
    ) -> Result<Self> {
        let roots = workspace
            .filtered_members(filter)?
            .into_iter()
            .map(|member| member.dir.clone())
            .collect();
        Self::new(roots, keep_lockfile)
    }

    fn new(roots: Vec<PathBuf>, keep_lockfile: bool) -> Result<Self> {
        let mut roots = roots
            .into_iter()
            .map(|root| {
                let path = PackagePath::canonicalize(&root)
                    .map_err(|error| Self::module_error(&root, error))?;
                if !path.is_dir() {
                    return Err(Self::module_error(
                        &root,
                        std::io::Error::new(std::io::ErrorKind::NotADirectory, "not a directory"),
                    ));
                }
                Ok(path)
            })
            .collect::<Result<Vec<_>>>()?;
        roots.sort();
        roots.dedup();
        Ok(Self {
            roots,
            keep_lockfile,
        })
    }

    pub fn run(self) -> Result<()> {
        // Enumerate every selected directory before removing anything, including invalid
        // build-profile module paths. Native paths retain spaces, Unicode and link identity.
        let mut entries = Vec::new();
        for root in &self.roots {
            for entry in std::fs::read_dir(root).map_err(|error| Self::module_error(root, error))? {
                let entry = entry.map_err(|error| Self::module_error(root, error))?;
                let name = entry.file_name();
                let is_lockfile = name.to_str().is_some_and(|name| {
                    name.starts_with("oh-package") && name.ends_with("-lock.json5")
                });
                if name == MY_MODULES || (is_lockfile && !self.keep_lockfile) {
                    entries.push(entry.path());
                }
            }
        }
        entries.sort();
        entries.dedup();
        for path in entries {
            log::debug!("clean: {}", path.display());
            DirectoryLink::remove_entry(&path).map_err(|error| Self::module_error(&path, error))?;
        }
        Ok(())
    }

    fn missing_project(directory: &Path) -> OhpmError {
        OhpmError::new(
            "NotFoundProjectRootError",
            format!(
                "No project or package containing {MY_PACKAGE_JSON} found from {}",
                directory.display()
            ),
        )
    }

    fn module_error(path: &Path, error: std::io::Error) -> OhpmError {
        OhpmError::new(
            "CleanModuleFailed",
            format!(
                "Clean {} failed, files may be occupied, detail: {error}",
                path.display()
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_removes_modules_and_lockfiles() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("oh_modules")).unwrap();
        std::fs::write(dir.path().join("oh-package-lock.json5"), "{}").unwrap();
        std::fs::write(dir.path().join("oh-package-debug-lock.json5"), "{}").unwrap();
        std::fs::write(dir.path().join("oh-package.json5"), "{}").unwrap();
        std::fs::write(dir.path().join("src.ets"), "").unwrap();

        Cleaner::for_directory(dir.path(), false)
            .unwrap()
            .run()
            .unwrap();
        assert!(!dir.path().join("oh_modules").exists());
        assert!(!dir.path().join("oh-package-lock.json5").exists());
        assert!(!dir.path().join("oh-package-debug-lock.json5").exists());
        assert!(dir.path().join("oh-package.json5").is_file());
        assert!(dir.path().join("src.ets").is_file());
    }

    #[test]
    fn workspace_mode_cleans_members() {
        let dir = tempfile::TempDir::new().unwrap();
        let root = dir.path().join("ws");
        std::fs::create_dir_all(root.join("packages/a")).unwrap();
        std::fs::create_dir_all(root.join("packages/b")).unwrap();
        std::fs::write(
            root.join("ohpm-workspace.yaml"),
            "packages:\n  - \"packages/*\"\n",
        )
        .unwrap();
        std::fs::write(root.join("oh-package.json5"), "{ name: \"wsroot\" }\n").unwrap();
        for m in ["a", "b"] {
            std::fs::write(
                root.join(format!("packages/{m}/oh-package.json5")),
                format!("{{ name: \"{m}\" }}\n"),
            )
            .unwrap();
            std::fs::create_dir_all(root.join(format!("packages/{m}/oh_modules"))).unwrap();
            std::fs::write(
                root.join(format!("packages/{m}/oh-package-lock.json5")),
                "{}",
            )
            .unwrap();
        }
        std::fs::create_dir_all(root.join("oh_modules")).unwrap();
        let ws = crate::workspace::Workspace::find(&root).unwrap().unwrap();
        // All members.
        Cleaner::for_workspace(&ws, &[], false)
            .unwrap()
            .run()
            .unwrap();
        assert!(!root.join("packages/a/oh_modules").exists());
        assert!(!root.join("packages/b/oh_modules").exists());
        assert!(!root.join("packages/a/oh-package-lock.json5").exists());
        // The workspace root itself is not a member.
        assert!(root.join("oh_modules").is_dir());
        // Filter + keep-lockfile.
        std::fs::create_dir_all(root.join("packages/a/oh_modules")).unwrap();
        std::fs::write(root.join("packages/a/oh-package-lock.json5"), "{}").unwrap();
        std::fs::write(root.join("packages/b/oh-package-lock.json5"), "{}").unwrap();
        Cleaner::for_workspace(&ws, &["a".to_string()], true)
            .unwrap()
            .run()
            .unwrap();
        assert!(!root.join("packages/a/oh_modules").exists());
        assert!(root.join("packages/a/oh-package-lock.json5").is_file());
        assert!(root.join("packages/b/oh-package-lock.json5").is_file());
    }

    #[test]
    fn keep_lockfile_preserves_lockfiles() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("oh_modules")).unwrap();
        std::fs::write(dir.path().join("oh-package-lock.json5"), "{}").unwrap();
        std::fs::write(dir.path().join("oh-package.json5"), "{}").unwrap();
        Cleaner::for_directory(dir.path(), true)
            .unwrap()
            .run()
            .unwrap();
        assert!(!dir.path().join("oh_modules").exists());
        assert!(dir.path().join("oh-package-lock.json5").is_file());
    }

    #[test]
    fn clean_removes_directory_links_without_deleting_their_contents() {
        let temp = tempfile::TempDir::new().unwrap();
        let package = temp.path().join("package");
        let shared = temp.path().join("shared modules");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(package.join(MY_PACKAGE_JSON), "{}").unwrap();
        std::fs::write(shared.join("payload"), "shared dependency").unwrap();
        DirectoryLink::new(&shared, &package.join(MY_MODULES))
            .ensure()
            .unwrap();

        Cleaner::for_directory(&package, false)
            .unwrap()
            .run()
            .unwrap();

        assert!(std::fs::symlink_metadata(package.join(MY_MODULES)).is_err());
        assert_eq!(
            std::fs::read(shared.join("payload")).unwrap(),
            b"shared dependency"
        );
    }
}
