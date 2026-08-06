//! Install targets, mirroring `lib/core/install-targets/` — the
//! `--target_path` option points at a directory with a `dependencyMap.json5`
//! describing a build target: `basePath`, `targetName`, `rootDependency`,
//! `dependencyMap` (module name -> oh-package.json5 path) and `modules`.
//! Target mode changes the module roots, the lockfile name
//! (`oh-package-<target>-lock.json5`) and the install-record location
//! (`<target>/resolve-conflict/<module>`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::constants::BUILD_PROFILE;
use crate::error::{OhpmError, Result};

/// `DEPENDENCY_MAP_JSON`.
pub const DEPENDENCY_MAP_JSON: &str = "dependencyMap.json5";
/// `DEFAULT_TARGET_NAME`.
pub const DEFAULT_TARGET_NAME: &str = "default";

/// Lexically normalize a path (`path.resolve` — `..` and `.` components
/// collapsed without touching the filesystem).
pub fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            std::path::Component::CurDir => {}
            _ => out.push(c.as_os_str()),
        }
    }
    out
}

/// The parsed `dependencyMap.json5` (`DependencyMapFile`).
#[derive(Debug, Clone, Default)]
pub struct DependencyMapFile {
    /// The target directory.
    pub target_path: PathBuf,
    /// `basePath` (defaults to the target path).
    pub base_path: PathBuf,
    /// `targetName` (empty when absent).
    pub target_name: String,
    /// `rootDependency` — resolved against the target path.
    pub root_dependency: PathBuf,
    /// module name -> resolved oh-package.json5 path.
    pub dependency_map: BTreeMap<String, PathBuf>,
    /// module name -> parsed manifest.
    pub oh_pkg_json_map: BTreeMap<String, serde_json::Value>,
    /// The `modules` array (srcPath resolved against basePath).
    pub modules: Vec<serde_json::Value>,
    /// module name -> package name (from the module manifests).
    pkg_name_map: BTreeMap<String, String>,
}

impl DependencyMapFile {
    /// `loadDepMapFile` — parse + validate the dependencyMap.json5.
    pub fn load(target_path: &Path) -> Result<DependencyMapFile> {
        let file_path = target_path.join(DEPENDENCY_MAP_JSON);
        if !file_path.is_file() {
            return Err(OhpmError::new(
                "TargetFileUnExistError",
                format!("The file \"{}\" does not exist.", file_path.display()),
            ));
        }
        let text = std::fs::read_to_string(&file_path).map_err(|e| {
            OhpmError::new(
                "TargetFileReadError",
                format!("Failed to read \"{}\": {e}", file_path.display()),
            )
        })?;
        let value: serde_json::Value = json5::from_str(&text).map_err(|e| {
            OhpmError::new(
                "TargetFileParseError",
                format!("Failed to parse \"{}\": {e}", file_path.display()),
            )
        })?;
        let mut file = DependencyMapFile {
            target_path: target_path.to_path_buf(),
            ..Default::default()
        };
        file.base_path = value
            .get("basePath")
            .and_then(|b| b.as_str())
            .map(|b| {
                let p = Path::new(b.trim());
                let joined = if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    target_path.join(p)
                };
                normalize_path(&joined)
            })
            .unwrap_or_else(|| target_path.to_path_buf());
        file.target_name = value
            .get("targetName")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        file.root_dependency = value
            .get("rootDependency")
            .and_then(|r| r.as_str())
            .map(|r| {
                let p = Path::new(r);
                let joined = if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    target_path.join(p)
                };
                normalize_path(&joined)
            })
            .unwrap_or_default();
        file.load_dependency_map(&value)?;
        file.load_modules(&value);
        Ok(file)
    }

    /// `loadDependencyMap` + `loadOhPkgJsons` — resolve the module map paths
    /// and read each module's manifest.
    fn load_dependency_map(&mut self, value: &serde_json::Value) -> Result<()> {
        let Some(map) = value.get("dependencyMap").and_then(|m| m.as_object()) else {
            return Ok(());
        };
        for (module, path) in map {
            let Some(raw) = path.as_str().filter(|s| !s.trim().is_empty()) else {
                continue;
            };
            let p = Path::new(raw);
            let joined = if p.is_absolute() {
                p.to_path_buf()
            } else {
                self.target_path.join(p)
            };
            let resolved = normalize_path(&joined);
            self.dependency_map.insert(module.trim().to_string(), resolved.clone());
            // The dependencyMap values ARE the oh-package.json5 file paths.
            let manifest = std::fs::read_to_string(&resolved)
                .ok()
                .and_then(|t| json5::from_str::<serde_json::Value>(&t).ok())
                .unwrap_or_default();
            let name = manifest
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or(module)
                .to_string();
            self.pkg_name_map.insert(module.trim().to_string(), name);
            self.oh_pkg_json_map.insert(module.trim().to_string(), manifest);
        }
        // `rootDependency` — its manifest becomes the project-level view.
        if !self.root_dependency.as_os_str().is_empty() && self.root_dependency.is_file() {
            if let Some(text) = std::fs::read_to_string(&self.root_dependency).ok() {
                if let Ok(v) = json5::from_str::<serde_json::Value>(&text) {
                    self.oh_pkg_json_map
                        .insert(KEY_PROJECT_OH_PKG.to_string(), v);
                }
            }
        }
        Ok(())
    }

    /// `loadModules` — resolve each module's `srcPath` against the base path.
    fn load_modules(&mut self, value: &serde_json::Value) {
        let Some(modules) = value.get("modules").and_then(|m| m.as_array()) else {
            return;
        };
        for mut module in modules.clone() {
            if let Some(src) = module.get("srcPath").and_then(|s| s.as_str()) {
                let p = Path::new(src);
                let joined = if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    self.base_path.join(p)
                };
                let resolved = normalize_path(&joined);
                module["srcPath"] = serde_json::Value::String(resolved.to_string_lossy().into_owned());
            }
            self.modules.push(module);
        }
    }

    /// `getModuleNames` — the dependencyMap keys.
    pub fn get_module_names(&self) -> Vec<String> {
        self.dependency_map.keys().cloned().collect()
    }

    /// `getPkgNameByModuleName`.
    pub fn get_pkg_name_by_module_name(&self, module: &str) -> Option<&str> {
        self.pkg_name_map.get(module).map(|s| s.as_str())
    }

    /// `getModuleOhPkgJson` — the module's (cached) manifest.
    pub fn get_module_oh_pkg_json(&self, module: &str) -> serde_json::Value {
        self.oh_pkg_json_map.get(module).cloned().unwrap_or_default()
    }

    /// `needChangeLockFileName` — a target name other than `default`.
    pub fn need_change_lock_file_name(&self) -> bool {
        !self.target_name.is_empty() && self.target_name != DEFAULT_TARGET_NAME
    }
}

/// `KEY_PROJECT_OH_PKG`.
pub const KEY_PROJECT_OH_PKG: &str = "project_root";

/// `TargetManager` — the singleton target state (one per install run here).
#[derive(Debug, Clone, Default)]
pub struct TargetManager {
    target_path: Option<PathBuf>,
    file: Option<DependencyMapFile>,
    /// The build-profile module map (module name -> path), used by
    /// `getModuleRoots`.
    module_map: BTreeMap<String, PathBuf>,
}

impl TargetManager {
    pub fn new() -> TargetManager {
        TargetManager::default()
    }

    /// `validateTargetPath` — the path must exist and be a directory.
    pub fn validate_target_path(path: &str) -> Result<()> {
        if path.trim().is_empty() {
            return Ok(());
        }
        let p = Path::new(path);
        let resolved = if p.is_absolute() {
            p.to_path_buf()
        } else {
            std::env::current_dir().unwrap_or_default().join(p)
        };
        if !resolved.exists() {
            return Err(OhpmError::new(
                "TargetPathUnExistError",
                format!("The target path \"{}\" does not exist.", resolved.display()),
            ));
        }
        if !resolved.is_dir() {
            return Err(OhpmError::new(
                "TargetNotDirPathError",
                format!("The target path \"{}\" is not a directory.", resolved.display()),
            ));
        }
        Ok(())
    }

    /// `init` — load the dependencyMap.json5 of the target path (a no-op when
    /// no target path is set).
    pub fn init(&mut self, target_path: &Path, project_root: &Path) -> Result<()> {
        if target_path.as_os_str().is_empty() {
            self.target_path = None;
            self.file = None;
            return Ok(());
        }
        Self::validate_target_path(&target_path.to_string_lossy())?;
        let file = DependencyMapFile::load(target_path)?;
        // `reflushModuleRoots` — the build-profile module map is rebuilt from
        // the dependencyMap's modules.
        self.module_map = module_map_from_modules(&file.modules, &file.base_path, project_root);
        self.target_path = Some(target_path.to_path_buf());
        self.file = Some(file);
        Ok(())
    }

    pub fn is_target_mod(&self) -> bool {
        self.target_path.is_some()
    }

    /// `getTargetName`.
    pub fn get_target_name(&self) -> String {
        self.file
            .as_ref()
            .map(|f| f.target_name.clone())
            .unwrap_or_default()
    }

    /// `needChangeLockFileName`.
    pub fn need_change_lock_file_name(&self) -> bool {
        self.file
            .as_ref()
            .map(|f| f.need_change_lock_file_name())
            .unwrap_or(false)
    }

    /// `getModuleRoots` — the dependencyMap module paths (via the module map).
    pub fn get_module_roots(&self) -> Result<Vec<PathBuf>> {
        let Some(file) = &self.file else {
            return Ok(Vec::new());
        };
        let mut roots = Vec::new();
        for module in file.get_module_names() {
            let Some(path) = self.module_map.get(&module) else {
                return Err(OhpmError::new(
                    "ModulePathUnExistError",
                    format!(
                        "The module \"{module}\" is not found in \"{}\".",
                        self.target_path
                            .as_ref()
                            .unwrap_or(&PathBuf::new())
                            .join(DEPENDENCY_MAP_JSON)
                            .display()
                    ),
                ));
            };
            roots.push(path.clone());
        }
        Ok(roots)
    }

    /// `getModuleOhPkgJson` — the cached module manifest.
    pub fn get_module_oh_pkg_json(&self, module: &str) -> serde_json::Value {
        self.file
            .as_ref()
            .map(|f| f.get_module_oh_pkg_json(module))
            .unwrap_or_default()
    }

    /// `getResolveConflictPath` — `<target>/resolve-conflict/<moduleName>`.
    pub fn get_resolve_conflict_path(&self, module_root: &Path) -> PathBuf {
        let name = module_root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.target_path
            .clone()
            .unwrap_or_default()
            .join("resolve-conflict")
            .join(name)
    }

    /// The module manifest to use for a module root in target mode (empty when
    /// the module is not in the dependency map — the disk manifest is used).
    pub fn target_module_manifest(&self, module_root: &Path) -> Option<serde_json::Value> {
        let file = self.file.as_ref()?;
        let slash = module_root.to_string_lossy().replace('\\', "/");
        for (module, path) in &file.dependency_map {
            // The dependencyMap values are oh-package.json5 FILE paths; the
            // module roots are their parent directories.
            let dir = path.parent().map(|p| p.to_string_lossy().replace('\\', "/"));
            if dir.as_deref() == Some(slash.as_str()) {
                return Some(file.get_module_oh_pkg_json(module));
            }
        }
        None
    }
}

/// Rebuild the module name -> path map from the dependencyMap `modules`
/// (mirrors `ProjectBuildProfile.reflushModuleRoots`).
fn module_map_from_modules(
    modules: &[serde_json::Value],
    base_path: &Path,
    _project_root: &Path,
) -> BTreeMap<String, PathBuf> {
    let mut map = BTreeMap::new();
    for module in modules {
        let Some(name) = module.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        let Some(src) = module.get("srcPath").and_then(|s| s.as_str()) else {
            continue;
        };
        let p = Path::new(src);
        let joined = if p.is_absolute() {
            p.to_path_buf()
        } else {
            base_path.join(p)
        };
        map.insert(name.to_string(), normalize_path(&joined));
    }
    map
}

/// `getModuleRootDirs` in target mode — the dependency-map module roots.
pub fn target_module_roots(
    targets: &TargetManager,
    prefix: &Path,
    install_all: bool,
    project: Option<&crate::install::modules::ProjectBuildProfile>,
) -> Result<Vec<PathBuf>> {
    if !targets.is_target_mod() {
        return Ok(crate::install::modules::module_roots(prefix, install_all, project));
    }
    let roots = targets.get_module_roots()?;
    Ok(roots)
}

/// Whether a project root has a `build-profile.json5` (target mode validates
/// the module map against it).
pub fn has_build_profile(project_root: &Path) -> bool {
    project_root.join(BUILD_PROFILE).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_map_parsing() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("target");
        std::fs::create_dir_all(target.join("modules/entry")).unwrap();
        std::fs::write(
            target.join("modules/entry/oh-package.json5"),
            "{ name: \"entry\", version: \"1.0.0\" }\n",
        )
        .unwrap();
        std::fs::write(
            target.join(DEPENDENCY_MAP_JSON),
            "{\n  basePath: \".\",\n  targetName: \"default\",\n  rootDependency: \"./entry/oh-package.json5\",\n  dependencyMap: { entry: \"./modules/entry/oh-package.json5\" },\n  modules: [{ name: \"entry\", srcPath: \"./modules/entry\" }],\n}\n",
        )
        .unwrap();
        let file = DependencyMapFile::load(&target).unwrap();
        assert_eq!(file.target_name, "default");
        assert!(!file.need_change_lock_file_name());
        assert_eq!(file.get_module_names(), vec!["entry"]);
        assert_eq!(file.get_pkg_name_by_module_name("entry"), Some("entry"));
        assert_eq!(
            file.dependency_map["entry"],
            target.join("modules/entry/oh-package.json5")
        );
    }

    #[test]
    fn target_name_changes_lock_name() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("t");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(
            target.join(DEPENDENCY_MAP_JSON),
            "{ targetName: \"debug\" }\n",
        )
        .unwrap();
        let file = DependencyMapFile::load(&target).unwrap();
        assert!(file.need_change_lock_file_name());
        assert_eq!(file.target_name, "debug");
    }
}
