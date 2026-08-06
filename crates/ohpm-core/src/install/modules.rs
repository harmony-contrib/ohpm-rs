//! Project build-profile parsing and module roots, mirroring
//! `lib/core/install-targets/ProjectBuildProfile.js` and
//! `lib/core/install/service/getModuleRootDirs.js`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::constants::BUILD_PROFILE;

/// The parsed `build-profile.json5` project (mirrors `projectBuildProfile`).
#[derive(Debug, Clone, Default)]
pub struct ProjectBuildProfile {
    pub project_root: PathBuf,
    pub module_roots: Vec<PathBuf>,
    pub module_map: BTreeMap<String, PathBuf>,
}

impl ProjectBuildProfile {
    /// `projectBuildProfile.init` — parse `<projectRoot>/build-profile.json5`
    /// and load the `modules` array. Returns `None` when the file is missing or
    /// malformed (the config layer treats that as "no project root").
    pub fn load(project_root: &Path) -> Option<ProjectBuildProfile> {
        let text = std::fs::read_to_string(project_root.join(BUILD_PROFILE)).ok()?;
        let json: serde_json::Value = json5::from_str(&text).ok()?;
        if json.get("modules").is_none() {
            return None;
        }
        let mut pbp = ProjectBuildProfile {
            project_root: project_root.to_path_buf(),
            ..Default::default()
        };
        pbp.load_module_roots(&json);
        Some(pbp)
    }

    pub fn get_module_roots(&self) -> &[PathBuf] {
        &self.module_roots
    }

    /// `getModulePath` — the module root for a module name.
    pub fn get_module_path(&self, name: &str) -> Option<&PathBuf> {
        self.module_map.get(name)
    }

    /// `getModuleName` — the module name for a (slash-normalized) module root.
    pub fn get_module_name(&self, dir: &Path) -> String {
        let slash = dir.to_string_lossy().replace('\\', "/");
        for (name, root) in &self.module_map {
            let root = root.to_string_lossy().replace('\\', "/");
            if root == slash {
                return name.clone();
            }
        }
        String::new()
    }

    /// `loadModuleRoots` — resolve each module's `srcPath` against the project
    /// root; entries without a `srcPath` are skipped.
    fn load_module_roots(&mut self, json: &serde_json::Value) {
        let Some(modules) = json.get("modules").and_then(|m| m.as_array()) else {
            return;
        };
        for module in modules {
            let Some(src_path) = module.get("srcPath").and_then(|s| s.as_str()) else {
                continue;
            };
            // `path.resolve(projectRoot, srcPath)` — absolute stays, relative
            // joins and normalizes (`./` and `..` components).
            let resolved = crate::workspace::resolve_file_spec(&self.project_root, &format!("file:{src_path}"));
            let name = module.get("name").and_then(|n| n.as_str()).unwrap_or("");
            self.module_roots.push(resolved.clone());
            if !name.is_empty() {
                self.module_map.insert(name.to_string(), resolved);
            }
        }
    }
}

/// `getModuleRootDirs.js` — `[prefix]` normally; with `--all` (or the
/// `install_all` config) the build-profile module roots plus the project root.
pub fn module_roots(
    prefix: &Path,
    all: bool,
    project: Option<&ProjectBuildProfile>,
) -> Vec<PathBuf> {
    if !all {
        return vec![prefix.to_path_buf()];
    }
    match project {
        Some(pbp) => {
            let mut roots = pbp.module_roots.clone();
            roots.push(pbp.project_root.clone());
            roots
        }
        None => vec![prefix.to_path_buf()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    #[test]
    fn load_build_profile() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "build-profile.json5",
            r#"{
  "app": { "products": [] },
  "modules": [
    { "name": "entry", "srcPath": "./entry", "targets": [] },
    { "name": "lib", "srcPath": "/abs/lib" },
    { "srcPath": "./noname" },
    {}
  ]
}"#,
        );
        let pbp = ProjectBuildProfile::load(dir.path()).expect("load");
        assert_eq!(pbp.module_roots.len(), 3);
        assert_eq!(pbp.module_roots[0], dir.path().join("entry"));
        assert_eq!(pbp.module_roots[1], PathBuf::from("/abs/lib"));
        assert_eq!(pbp.module_roots[2], dir.path().join("noname"));
        assert_eq!(pbp.get_module_path("entry"), Some(&dir.path().join("entry")));
        assert_eq!(pbp.get_module_path("lib"), Some(&PathBuf::from("/abs/lib")));
        assert_eq!(pbp.get_module_name(&dir.path().join("entry")), "entry");
        assert_eq!(pbp.get_module_name(&dir.path().join("other")), "");
    }

    #[test]
    fn load_build_profile_missing_or_bad() {
        let dir = TempDir::new().unwrap();
        assert!(ProjectBuildProfile::load(dir.path()).is_none());
        write(dir.path(), "build-profile.json5", "not json{");
        assert!(ProjectBuildProfile::load(dir.path()).is_none());
        write(dir.path(), "build-profile.json5", "{ \"app\": {} }");
        assert!(ProjectBuildProfile::load(dir.path()).is_none());
    }

    #[test]
    fn module_roots_single_vs_all() {
        let dir = TempDir::new().unwrap();
        let prefix = dir.path().join("entry");
        assert_eq!(module_roots(&prefix, false, None), vec![prefix.clone()]);
        // No project → --all degenerates to the prefix.
        assert_eq!(module_roots(&prefix, true, None), vec![prefix.clone()]);
        let pbp = ProjectBuildProfile::load(dir.path());
        assert!(pbp.is_none());
        write(
            dir.path(),
            "build-profile.json5",
            "{ \"modules\": [{ \"name\": \"entry\", \"srcPath\": \"./entry\" }] }",
        );
        let pbp = ProjectBuildProfile::load(dir.path()).unwrap();
        let roots = module_roots(&prefix, true, Some(&pbp));
        assert_eq!(roots, vec![dir.path().join("entry"), dir.path().to_path_buf()]);
    }
}
