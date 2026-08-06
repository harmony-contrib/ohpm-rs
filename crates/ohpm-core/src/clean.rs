//! `ohpm clean`, mirroring `lib/commands/clean.js` + `lib/core/clean/index.js`:
//! delete the `oh_modules` directories and the `oh-package*.lock.json5` files
//! of the project root and every build-profile module (`--keep-lockfile`
//! preserves the lockfiles).

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::{OhpmError, Result};
use crate::install::modules::ProjectBuildProfile;

/// `startClean` — clean the project root and its modules.
pub fn start_clean(config: &Config, keep_lockfile: bool) -> Result<()> {
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

/// The cleaned module roots (for the CLI's cost message).
pub fn module_roots_to_clean(config: &Config) -> Result<Vec<PathBuf>> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let project_root = crate::config::find_project_root(&cwd);
    let Some(project_root) = project_root else {
        return Ok(Vec::new());
    };
    let mut roots = ProjectBuildProfile::load(&project_root)
        .map(|p| p.get_module_roots().to_vec())
        .unwrap_or_default();
    roots.push(project_root);
    Ok(roots)
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
    fn keep_lockfile_preserves_lockfiles() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("oh_modules")).unwrap();
        std::fs::write(dir.path().join("oh-package-lock.json5"), "{}").unwrap();
        clean_module(dir.path(), true).unwrap();
        assert!(!dir.path().join("oh_modules").exists());
        assert!(dir.path().join("oh-package-lock.json5").is_file());
    }
}
