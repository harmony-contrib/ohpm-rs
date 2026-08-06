//! Dependency nodes and node data, mirroring
//! `lib/core/dependency/struct/dependencyNode.js`,
//! `lib/core/dependency/util/mergeDependenciesInOrder.js` and the dep
//! builders (`dep-builder/implementor/ArtifactDepBuilderImpl.js`,
//! `SourceCodeDepBuilderImpl.js`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::constants::{compress, MY_MODULES};
use crate::error::{OhpmError, Result};
use crate::install::modules::ProjectBuildProfile;
use crate::install::spec::{is_local_dependency, is_local_file, OhpaType};

/// The dependency kinds (the reference has no optionalDependencies).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DepType {
    Prod,
    Dev,
    Dynamic,
    NoSave,
}

impl DepType {
    pub fn as_str(&self) -> &'static str {
        match self {
            DepType::Prod => "prod",
            DepType::Dev => "dev",
            DepType::Dynamic => "dynamic",
            DepType::NoSave => "noSave",
        }
    }
}

/// The resolved node record (`DepNodeData`).
#[derive(Debug, Clone, Default)]
pub struct NodeData {
    /// The node's own name (the real package name; for aliases/workspace
    /// deps this is the target/member name).
    pub name: String,
    /// The declared dependency key when it differs from `name` (aliases,
    /// workspace alias form). Used for specifier keys and symlinks.
    pub declared_name: String,
    pub version: String,
    pub actual_name: String,
    pub pinned_spec: String,
    pub save_spec: String,
    pub fetch_spec: String,
    pub ohpa_type: OhpaType,
    /// "ohpm" | "local" (npm white-list deferred).
    pub registry_type: String,
    pub package_type: Option<String>,
    pub is_root: bool,
    pub is_link: bool,
    pub is_shared: bool,
    /// The store-dir name (`name@version` with `/` `\` `:` -> `+`), or the
    /// source dir for linked source code.
    pub save_root_dir: String,
    /// `oh_modules/<name>`, or the source dir for linked source code.
    pub pkg_store_dir: String,
    pub integrity: Option<String>,
    pub shasum: Option<String>,
    pub resolved: String,
    pub dependencies: BTreeMap<String, String>,
    pub dev_dependencies: BTreeMap<String, String>,
    pub dynamic_dependencies: BTreeMap<String, String>,
    /// Set when resolution failed; the graph build then rethrows the error.
    pub unmet: Option<OhpmError>,
    /// The node's deps were modified by `exclusions` or
    /// `overrideDependencyMap` (`maskedByOverrideDependencyMap` in the
    /// lockfile packages and the install record).
    pub masked_by_override_dependency_map: bool,
    /// The `overrideDependencyMap` entry replacing the node's own maps
    /// (post-exclusion). The node's requirements are built from these.
    pub masked_deps: Option<MaskedDeps>,
    /// HSP fields (`ArtifactDepBuilderImpl`): the `hspStoreDir`
    /// (`oh_modules/.hsp/<saveRootDir>`) and `hspName` (`name.hsp` with
    /// `/`/`:` -> `+`) for bundle-app HSP packages.
    pub hsp_store_dir: String,
    pub hsp_name: String,
    pub hsp_type: Option<String>,
    pub is_debug_hsp: bool,
    pub resolved_hsp: Option<String>,
    pub integrity_hsp: Option<String>,
}

/// `ArtifactDepBuilderImpl` — the HSP store-dir/name fields for a bundle-app
/// HSP package (empty otherwise).
pub fn hsp_fields(
    name: &str,
    package_type: Option<&str>,
    hsp_type: Option<&str>,
    save_root_dir: &str,
) -> (String, String) {
    if package_type == Some(crate::constants::HSP_PACKAGE_TYPE)
        && hsp_type == Some(crate::constants::HSP_TYPE_BUNDLE_APP)
    {
        (
            format!("{MY_MODULES}/{}/{save_root_dir}", crate::constants::HSP_DIR),
            format!("{}.hsp", name.replace('/', "+").replace(':', "+")),
        )
    } else {
        (String::new(), String::new())
    }
}

/// The three dependency maps of an `overrideDependencyMap` entry (the mask
/// replacement for a node's own maps).
#[derive(Debug, Clone, Default)]
pub struct MaskedDeps {
    pub dependencies: BTreeMap<String, String>,
    pub dev_dependencies: BTreeMap<String, String>,
    pub dynamic_dependencies: BTreeMap<String, String>,
}

impl NodeData {
    /// `resolveStoreRootDir` — `<projectRoot>/oh_modules/.ohpm`.
    pub fn resolve_store_root(&self, project_root: &Path) -> PathBuf {
        project_root.join(MY_MODULES).join(".ohpm")
    }

    /// `resolveSaveRootDir` — roots resolve to the project root itself;
    /// otherwise `<storeRoot>/<saveRootDir>`.
    pub fn resolve_save_root(&self, project_root: &Path) -> PathBuf {
        if self.is_root {
            project_root.to_path_buf()
        } else {
            self.resolve_store_root(project_root).join(&self.save_root_dir)
        }
    }

    /// `resolvePkgStoreDir` — roots use the raw `pkgStoreDir`; otherwise
    /// `<saveRoot>/<pkgStoreDir>`.
    pub fn resolve_pkg_store_dir(&self, project_root: &Path) -> PathBuf {
        if self.is_root {
            PathBuf::from(&self.pkg_store_dir)
        } else {
            self.resolve_save_root(project_root).join(&self.pkg_store_dir)
        }
    }

    /// `nodeKey` — `name@pinnedSpec`.
    pub fn node_key(&self) -> String {
        format!("{}@{}", self.name, self.pinned_spec)
    }

    /// `resolveHspStoreDir` — roots resolve to the project root itself;
    /// otherwise `<projectRoot>/<hspStoreDir>`.
    pub fn resolve_hsp_store_dir(&self, project_root: &Path) -> PathBuf {
        if self.is_root {
            project_root.to_path_buf()
        } else {
            project_root.join(&self.hsp_store_dir)
        }
    }
}

/// A requirement edge (a dependency of a node).
#[derive(Debug, Clone)]
pub struct Requirement {
    pub spec: String,
    pub dep_type: DepType,
}

/// A node in the dependency graph (`DependencyNode`).
#[derive(Debug, Clone)]
pub struct Node {
    pub data: Arc<NodeData>,
    /// The node's own dep type (roots are "prod"; children inherit the
    /// requirement's type, deeper children inherit the parent's).
    pub dep_type: DepType,
    pub requirements: BTreeMap<String, Requirement>,
    /// The node's deps were replaced by an overrideDependencyMap entry.
    pub masked_by_override_dependency_map: bool,
}

impl Node {
    /// `mergeDependenciesInOrder` — merge prod -> dev -> dynamic -> noSave
    /// requirement maps into one, rejecting a dependency that shares the
    /// package's own name.
    ///
    /// For non-root nodes, dev requirements are included only when the node is
    /// a build-profile module root.
    pub fn with_requirements(
        data: Arc<NodeData>,
        dep_type: DepType,
        dev: Option<BTreeMap<String, String>>,
        dynamic: BTreeMap<String, String>,
        prod: BTreeMap<String, String>,
        project: Option<&ProjectBuildProfile>,
    ) -> Result<Node> {
        Self::with_requirements_masked(data, dep_type, dev, dynamic, prod, project, false)
    }

    /// Like `with_requirements`, but with the override-dependency-map mask
    /// semantics: when `masked` the three maps are REPLACED by the override
    /// entry (missing keys become empty), and `maskedByOverrideDependencyMap`
    /// is recorded.
    pub fn with_requirements_masked(
        data: Arc<NodeData>,
        dep_type: DepType,
        dev: Option<BTreeMap<String, String>>,
        dynamic: BTreeMap<String, String>,
        prod: BTreeMap<String, String>,
        project: Option<&ProjectBuildProfile>,
        masked: bool,
    ) -> Result<Node> {
        let include_dev = data.is_root || is_module_root(&data.pkg_store_dir, project);
        let mut requirements = BTreeMap::new();
        for (dep_type, map) in [
            (DepType::Prod, prod),
            (DepType::Dev, if include_dev { dev.unwrap_or_default() } else { BTreeMap::new() }),
            (DepType::Dynamic, dynamic),
        ] {
            for (name, spec) in map {
                if name == data.name {
                    return Err(OhpmError::dep_builder_same_as_pkg(
                        &data.name,
                        &data.pinned_spec,
                        &name,
                        &spec,
                    ));
                }
                requirements.insert(
                    name,
                    Requirement {
                        spec,
                        dep_type,
                    },
                );
            }
        }
        Ok(Node {
            data,
            dep_type,
            requirements,
            masked_by_override_dependency_map: masked,
        })
    }

    /// The `dependencies` getter — prod requirements only.
    pub fn dependencies(&self) -> BTreeMap<String, String> {
        self.requirements
            .iter()
            .filter(|(_, r)| r.dep_type == DepType::Prod)
            .map(|(n, r)| (n.clone(), r.spec.clone()))
            .collect()
    }

    pub fn dev_dependencies(&self) -> BTreeMap<String, String> {
        self.requirements
            .iter()
            .filter(|(_, r)| r.dep_type == DepType::Dev)
            .map(|(n, r)| (n.clone(), r.spec.clone()))
            .collect()
    }

    pub fn dynamic_dependencies(&self) -> BTreeMap<String, String> {
        self.requirements
            .iter()
            .filter(|(_, r)| r.dep_type == DepType::Dynamic)
            .map(|(n, r)| (n.clone(), r.spec.clone()))
            .collect()
    }

    pub fn node_key(&self) -> String {
        self.data.node_key()
    }

    pub fn is_local(&self) -> bool {
        is_local_dependency(&self.data.pinned_spec)
    }
}

/// `isModuleRoot` — whether a `pkgStoreDir` path is a build-profile module root.
pub fn is_module_root(pkg_store_dir: &str, project: Option<&ProjectBuildProfile>) -> bool {
    match project {
        Some(pbp) => {
            let p = PathBuf::from(pkg_store_dir);
            pbp.get_module_roots().iter().any(|root| root == &p)
        }
        None => false,
    }
}

/// `util/getPkgStoreDirName.js` — `name@<spec|hash>` with `/` `\` `:` -> `+`.
/// Local deps use the content/path hash, registry deps the pinned version.
pub fn pkg_store_dir_name(name: &str, spec: &str, hash: &str) -> String {
    let base = if is_local_dependency(spec) {
        format!("{name}@{hash}")
    } else {
        format!("{name}@{spec}")
    };
    base.replace(['/', '\\', ':'], compress::REPLACE_TARGET)
}

/// `fileContentHash` — sha256 of the file content, base64, lowercased.
pub fn file_content_hash(path: &Path) -> Result<String> {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    let bytes = std::fs::read(path)?;
    hasher.update(&bytes);
    let digest = hasher.finalize();
    Ok(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, digest)
        .to_lowercase())
}

/// `getFilePathHash` — a slash-normalized path, or its sha256/base64/lowercase
/// hash when longer than `compressConfig.PATH_LEN`.
pub fn file_path_hash(path: &str) -> String {
    let slash = path.replace('\\', "/");
    if slash.len() <= compress::PATH_LEN {
        return slash;
    }
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(slash.as_bytes());
    let digest = hasher.finalize();
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, digest).to_lowercase()
}

/// `ensureOhPackageJson5.js` — rename `package.json` to `oh-package.json5` in
/// an extracted store dir (delete `package.json` when both exist; error when
/// neither does).
pub fn ensure_oh_package_json5(store_dir: &Path) -> Result<()> {
    let package_json = store_dir.join(crate::constants::PACKAGE_JSON);
    let my_json = store_dir.join(crate::constants::MY_PACKAGE_JSON);
    let has_package_json = package_json.exists();
    if my_json.exists() {
        if has_package_json {
            std::fs::remove_file(&package_json)?;
        }
        return Ok(());
    }
    if !has_package_json {
        return Err(OhpmError::file_not_found_oh_pkg_json5(store_dir));
    }
    std::fs::rename(&package_json, &my_json)?;
    Ok(())
}

/// `depNodeVersionCompare` — node-semver comparison with a build-metadata
/// tiebreak (used by the max-version strategy).
pub fn dep_node_version_compare(a: &NodeData, b: &NodeData) -> std::cmp::Ordering {
    use node_semver::Version;
    match (Version::parse(&a.version), Version::parse(&b.version)) {
        (Ok(va), Ok(vb)) => {
            match va.cmp(&vb) {
                std::cmp::Ordering::Equal => {
                    // The reference breaks ties on build metadata.
                    va.build.cmp(&vb.build)
                }
                other => other,
            }
        }
        _ => std::cmp::Ordering::Equal,
    }
}

/// `util/isLocalFile.js` — whether a spec points at a local artifact.
pub fn is_local_artifact(spec: &str) -> bool {
    is_local_file(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(name: &str, version: &str, pinned: &str) -> Arc<NodeData> {
        Arc::new(NodeData {
            name: name.to_string(),
            declared_name: String::new(),
            version: version.to_string(),
            actual_name: name.to_string(),
            pinned_spec: pinned.to_string(),
            save_spec: "^1.0.0".to_string(),
            fetch_spec: "latest".to_string(),
            ohpa_type: OhpaType::Range,
            registry_type: "ohpm".to_string(),
            package_type: None,
            is_root: false,
            is_link: false,
            is_shared: true,
            save_root_dir: pkg_store_dir_name(name, pinned, ""),
            pkg_store_dir: format!("{MY_MODULES}/{name}"),
            integrity: None,
            shasum: None,
            resolved: String::new(),
            dependencies: BTreeMap::new(),
            dev_dependencies: BTreeMap::new(),
            dynamic_dependencies: BTreeMap::new(),
            unmet: None,
            masked_by_override_dependency_map: false,
            masked_deps: None,
            hsp_store_dir: String::new(),
            hsp_name: String::new(),
            hsp_type: None,
            is_debug_hsp: false,
            resolved_hsp: None,
            integrity_hsp: None,
        })
    }

    #[test]
    fn store_dir_names() {
        assert_eq!(pkg_store_dir_name("@ohos/foo", "1.2.3", ""), "@ohos+foo@1.2.3");
        assert_eq!(pkg_store_dir_name("bar", "2.0.0", ""), "bar@2.0.0");
        assert_eq!(pkg_store_dir_name("foo", "../lib", "h"), "foo@h");
        // "1:0" is not a valid URL (digit-leading scheme) -> a local dep.
        assert_eq!(pkg_store_dir_name("a:b", "1:0", ""), "a+b@");
    }

    #[test]
    fn path_hashes() {
        assert_eq!(file_path_hash("/a/short"), "/a/short");
        let long = "/x/".repeat(20);
        let h = file_path_hash(&long);
        assert!(h.len() < long.len());
        assert_eq!(h, h.to_lowercase());
        // content hash of a file
        let dir = tempfile::TempDir::new().unwrap();
        let f = dir.path().join("a.tgz");
        std::fs::write(&f, "hello").unwrap();
        let h1 = file_content_hash(&f).unwrap();
        // sha256("hello"), base64, lowercased.
        assert_eq!(h1, "lpjnul+wow4m6dsqxbninhswhlwfp0jecwqzypolmcq=");
    }

    #[test]
    fn requirements_merge_and_same_name() {
        let n = data("foo", "1.0.0", "1.0.0");
        let mut dev = BTreeMap::new();
        dev.insert("bar".to_string(), "^1.0.0".to_string());
        let mut prod = BTreeMap::new();
        prod.insert("baz".to_string(), "2.0.0".to_string());
        // Non-root without module-root status: dev deps dropped.
        let node = Node::with_requirements(n.clone(), DepType::Prod, Some(dev.clone()), BTreeMap::new(), prod.clone(), None).unwrap();
        assert!(node.requirements.contains_key("baz"));
        assert!(!node.requirements.contains_key("bar"));
        assert_eq!(node.requirements["baz"].dep_type, DepType::Prod);

        // Root: dev kept.
        let mut root = (*n).clone();
        root.is_root = true;
        let node = Node::with_requirements(Arc::new(root), DepType::Prod, Some(dev.clone()), BTreeMap::new(), prod.clone(), None).unwrap();
        assert_eq!(node.requirements["bar"].dep_type, DepType::Dev);

        // Same-name dependency rejected.
        let mut bad = prod.clone();
        bad.insert("foo".to_string(), "1.1.0".to_string());
        let err = Node::with_requirements(n.clone(), DepType::Prod, None, BTreeMap::new(), bad, None).unwrap_err();
        assert_eq!(err.code, "DepBuilderSameAsThePkgName");
    }

    #[test]
    fn path_resolution() {
        let n = data("foo", "1.2.3", "1.2.3");
        let root = Path::new("/proj");
        assert_eq!(n.resolve_store_root(root), Path::new("/proj/oh_modules/.ohpm"));
        assert_eq!(
            n.resolve_save_root(root),
            Path::new("/proj/oh_modules/.ohpm/foo@1.2.3")
        );
        assert_eq!(
            n.resolve_pkg_store_dir(root),
            Path::new("/proj/oh_modules/.ohpm/foo@1.2.3/oh_modules/foo")
        );
        let mut root_node = (*n).clone();
        root_node.is_root = true;
        root_node.pkg_store_dir = "/proj".to_string();
        assert_eq!(root_node.resolve_pkg_store_dir(root), Path::new("/proj"));
        assert_eq!(root_node.resolve_save_root(root), Path::new("/proj"));
    }

    #[test]
    fn version_compare() {
        let a = data("x", "1.2.3", "1.2.3");
        let b = data("x", "1.2.4", "1.2.4");
        assert_eq!(dep_node_version_compare(&b, &a), std::cmp::Ordering::Greater);
        assert_eq!(dep_node_version_compare(&a, &a), std::cmp::Ordering::Equal);
        let a_build = data("x", "1.2.3+b1", "1.2.3+b1");
        assert_eq!(dep_node_version_compare(&a_build, &a), std::cmp::Ordering::Greater);
    }
}
