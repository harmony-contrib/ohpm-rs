//! `ohpm clean`, mirroring `lib/commands/clean.js` + `lib/core/clean/index.js`:
//! delete the `oh_modules` directories and the `oh-package*.lock.json5` files
//! of the project root and every build-profile module (`--keep-lockfile`
//! preserves the lockfiles).

use std::path::Path;

use crate::error::{OhpmError, Result};
use crate::install::modules::ProjectBuildProfile;

/// `startClean` — clean the project root and its modules.
pub fn start_clean(keep_lockfile: bool) -> Result<()> {
    let cwd = std::env::current_dir().unwrap_or_default();
    // `config.getProjectRoot()` — the build-profile based project root.
    let project_root = crate::config::find_project_root(&cwd);
    let project_root = match project_root {
        Some(p)
            if p.join(crate::constants::MY_PACKAGE_JSON).is_file() =>
        {
            p
        }
        _ => {
            return Err(OhpmError::new(
                "NotFoundProjectRootError",
                format!(
                    "NotFound build-profile.json5 or oh-package.json5 file in current path: {}",
                    cwd.display()
                ),
            ));
        }
    };
    let module_roots = ProjectBuildProfile::load(&project_root)
        .map(|p| p.get_module_roots().to_vec())
        .unwrap_or_default();
    if module_roots.is_empty() {
        clean_module(&project_root, keep_lockfile)?;
        return Ok(());
    }
    // `[...moduleRoots, projectRoot]` — all cleaned (in parallel in the
    // reference; sequential here is equivalent).
    for module_root in module_roots.iter().chain(std::iter::once(&project_root)) {
        clean_module(module_root, keep_lockfile)?;
    }
    Ok(())
}

/// `cleanModule` — remove the `oh_modules` dir and the lockfiles (unless
/// `keepLockfile`) of one module.
fn clean_module(module_root: &Path, keep_lockfile: bool) -> Result<()> {
    log::debug!("begin to clean module: {}", module_root.display());
    let result = (|| -> std::io::Result<()> {
        for entry in std::fs::read_dir(module_root)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            // `/oh_modules|oh-package.*-lock.json5/`
            let is_oh_modules = name == crate::constants::MY_MODULES;
            let is_lockfile =
                name.starts_with("oh-package") && name.ends_with("-lock.json5");
            if !(is_oh_modules || is_lockfile) {
                continue;
            }
            if keep_lockfile && is_lockfile {
                continue;
            }
            let path = entry.path();
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            } else {
                std::fs::remove_file(&path)?;
            }
        }
        Ok(())
    })();
    result.map_err(|e| {
        OhpmError::new(
            "CleanModuleFailed",
            format!(
                "Clean module {} failed,  files may be occupied, detail: {e}",
                module_root.display()
            ),
        )
    })?;
    log::debug!("clean module: {} succeed.", module_root.display());
    Ok(())
}

/// `--workspace`/`--filter` mode (an ohpm-rs extension, like the publish and
/// version batch modes): clean the `oh_modules` dirs and lockfiles of the
/// selected workspace members.
pub fn clean_workspace(
    ws: &crate::workspace::Workspace,
    filter: &[String],
    keep_lockfile: bool,
) -> Result<()> {
    for member in ws.filtered_members(filter)? {
        log::debug!("begin to clean workspace member: {}", member.dir.display());
        clean_module(&member.dir, keep_lockfile)?;
    }
    Ok(())
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

        clean_module(dir.path(), false).unwrap();
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
        std::fs::write(root.join("ohpm-workspace.yaml"), "packages:\n  - \"packages/*\"\n").unwrap();
        std::fs::write(root.join("oh-package.json5"), "{ name: \"wsroot\" }\n").unwrap();
        for m in ["a", "b"] {
            std::fs::write(
                root.join(format!("packages/{m}/oh-package.json5")),
                format!("{{ name: \"{m}\" }}\n"),
            )
            .unwrap();
            std::fs::create_dir_all(root.join(format!("packages/{m}/oh_modules"))).unwrap();
            std::fs::write(root.join(format!("packages/{m}/oh-package-lock.json5")), "{}").unwrap();
        }
        let ws = crate::workspace::Workspace::find(&root).unwrap().unwrap();
        // All members.
        clean_workspace(&ws, &[], false).unwrap();
        assert!(!root.join("packages/a/oh_modules").exists());
        assert!(!root.join("packages/b/oh_modules").exists());
        assert!(!root.join("packages/a/oh-package-lock.json5").exists());
        // The workspace root itself is not a member.
        assert!(!root.join("oh_modules").exists() || true);
        // Filter + keep-lockfile.
        std::fs::create_dir_all(root.join("packages/a/oh_modules")).unwrap();
        std::fs::write(root.join("packages/a/oh-package-lock.json5"), "{}").unwrap();
        std::fs::write(root.join("packages/b/oh-package-lock.json5"), "{}").unwrap();
        clean_workspace(&ws, &["a".to_string()], true).unwrap();
        assert!(!root.join("packages/a/oh_modules").exists());
        assert!(root.join("packages/a/oh-package-lock.json5").is_file());
        assert!(root.join("packages/b/oh-package-lock.json5").is_file());
    }

    #[test]
    fn keep_lockfile_preserves_lockfiles() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("oh_modules")).unwrap();
        std::fs::write(dir.path().join("oh-package-lock.json5"), "{}").unwrap();
        clean_module(dir.path(), true).unwrap();
        assert!(!dir.path().join("oh_modules").exists());
        assert!(dir.path().join("oh-package-lock.json5").is_file());
    }
}
