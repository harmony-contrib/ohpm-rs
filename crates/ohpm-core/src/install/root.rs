//! Root node creation and manifest writes, mirroring
//! `lib/core/dependency/util/getRootNode.js`,
//! `lib/core/install/service/handleCliInput.js`,
//! `lib/core/install/common/updateDependencies.js`,
//! `lib/core/package/updatePackageJson.js` and
//! `lib/core/install/service/updateCommandLineInputDependencies.js`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::constants::MY_PACKAGE_JSON;
use crate::error::{OhpmError, Result};
use crate::install::graph::DependencyGraph;
use crate::install::modules::ProjectBuildProfile;
use crate::install::node::{DepType, Node, NodeData, Requirement};
use crate::install::spec::{parse_dependency, OhpaType, Spec};
use crate::package::Manifest;

/// The command driving the install pipeline (`GlobalState.CommandType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallCommand {
    Install,
    Update,
    Uninstall,
}

/// `PackageUtil.parse` — the name/version split used by the update/uninstall
/// argument validation (the last `@` splits; scoped names keep their scope).
/// Protocol specs (git URLs with `user@host`, aliases) are not versions.
pub fn parse_cli_pkg_version(raw: &str) -> Option<String> {
    if crate::install::spec::is_protocol_spec(raw) {
        return None;
    }
    let n = raw.rfind('@').unwrap_or(0);
    (n > 0).then(|| raw[n + 1..].to_string())
}

/// `getRootNode` — build a module's root node from its `oh-package.json5`.
/// The standard project root's name/version are blanked (the reference blanks
/// them for the project-level module).
pub fn get_root_node(
    module_root: &Path,
    link: bool,
    project: Option<&ProjectBuildProfile>,
    parameter: Option<&crate::install::parameter::Parameterization>,
) -> Result<Node> {
    let path = module_root.join(MY_PACKAGE_JSON);
    if !path.exists() {
        return Err(OhpmError::file_not_exist(&path));
    }
    let text = std::fs::read_to_string(&path)?;
    let mut manifest = Manifest::from_json5(&text)?;
    // `ParameterParsingChainManager.handle` — the `@module:` markers first,
    // then the `@param:` substitution (the reference's chain order).
    if let Some(project) = project {
        let mut value = manifest.to_json();
        parse_at_module(&mut value, project, module_root)?;
        if let Some(parameter) = parameter {
            parameter.parse_value(&manifest.name, &mut value)?;
        }
        let parsed: Manifest = serde_json::from_value(value)?;
        manifest = parsed;
    } else if let Some(parameter) = parameter {
        let mut value = manifest.to_json();
        parameter.parse_value(&manifest.name, &mut value)?;
        let parsed: Manifest = serde_json::from_value(value)?;
        manifest = parsed;
    }
    root_node_from_manifest(module_root, &manifest, link, project)
}

/// `ModuleParser.parseAtModuleOnPkgJson` — substitute the `@module:<name>`
/// markers in the manifest's dependency maps: the spec becomes the module's
/// directory (from the build-profile module map), and the module's package
/// name must match the dependency key.
pub fn parse_at_module(
    value: &mut serde_json::Value,
    project: &ProjectBuildProfile,
    module_root: &Path,
) -> Result<()> {
    for key in ["dependencies", "devDependencies", "dynamicDependencies", "overrides"] {
        let Some(v) = value.get_mut(key) else {
            continue;
        };
        let Some(map) = v.as_object_mut() else {
            continue;
        };
        let keys: Vec<String> = map.keys().cloned().collect();
        for k in keys {
            let child = map.get(&k).cloned().unwrap_or_default();
            let Some(s) = child.as_str().map(|s| s.to_string()) else {
                continue;
            };
            if !s.starts_with("@module:") {
                continue;
            }
            let module = s.trim().strip_prefix("@module:").unwrap_or("").to_string();
            let dir = project.get_module_path(&module).cloned().ok_or_else(|| {
                OhpmError::new(
                    "AtModuleModuleNameUnExistError",
                    format!(
                        "Configuration: \"{s}\" error in \"{}\", module name: \"{module}\" does not exist under the modules node in the file \"{}\".",
                        module_root.join(MY_PACKAGE_JSON).display(),
                        project.project_root.join(crate::constants::BUILD_PROFILE).display()
                    ),
                )
            })?;
            // `validConsistencyOfDepNameAndActualPkgName`.
            let manifest = crate::package::read_manifest_from_dir(&dir)?;
            if manifest.name != k {
                return Err(OhpmError::new(
                    "ParameterizationInconsistentDepNames",
                    format!(
                        "The local dependency \"{k}\" in \"{}\" does not match the actual name \"{}\"",
                        dir.display(),
                        manifest.name
                    ),
                ));
            }
            map.insert(k, serde_json::Value::String(dir.to_string_lossy().into_owned()));
        }
    }
    Ok(())
}

/// The shared root-node construction: manifest -> root `NodeData`
/// (target mode passes the dependencyMap's cached manifest).
pub fn root_node_from_manifest(
    module_root: &Path,
    manifest: &Manifest,
    link: bool,
    project: Option<&ProjectBuildProfile>,
) -> Result<Node> {
    let mut manifest = manifest.clone();
    let is_project_root = project.is_some_and(|p| &p.project_root == module_root);
    if is_project_root {
        manifest.name = String::new();
        manifest.version = String::new();
    }
    let data = NodeData {
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        actual_name: manifest.name.clone(),
        save_spec: manifest.version.clone(),
        fetch_spec: module_root.to_string_lossy().into_owned(),
        pinned_spec: module_root.to_string_lossy().into_owned(),
        pkg_store_dir: module_root.to_string_lossy().into_owned(),
        save_root_dir: module_root.to_string_lossy().into_owned(),
        ohpa_type: OhpaType::SourceCode,
        is_root: true,
        is_link: link,
        is_shared: project.is_some(),
        registry_type: "local".to_string(),
        ..Default::default()
    };
    Node::with_requirements(
        Arc::new(data),
        DepType::Prod,
        Some(manifest.dev_dependencies),
        manifest.dynamic_dependencies,
        manifest.dependencies,
        project,
    )
}

/// `handleCliInput` — add/remove/warn about CLI packages on the root node's
/// requirements, dispatched by `command`. Returns the cli-input names for the
/// INSTALL branch (the reference's module-global `cliInputNames`); UPDATE and
/// UNINSTALL never write new dependencies.
pub fn handle_cli_input(
    prefix: &Path,
    args: &[String],
    root: &mut Node,
    opts: &crate::install::InstallOptions,
    command: InstallCommand,
    parameter: Option<&crate::install::parameter::Parameterization>,
) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for raw in args {
        if raw.is_empty() {
            log::warn!("handleCliInput: empty package name.");
            continue;
        }
        let parsed = parse_dependency(raw, prefix)?;
        let name = resolve_cli_name(&parsed);
        let name = match name {
            Some(n) => n,
            None => return Err(OhpmError::install_invalid_cli_input_pkg(&parsed.raw)),
        };
        match command {
            InstallCommand::Install => {
                // `canModify` — an existing entry whose value is parameterized
                // cannot be rewritten by the CLI.
                if root.requirements.contains_key(&name) {
                    if let Some(p) = parameter {
                        if !p.can_modify(&prefix.join(MY_PACKAGE_JSON), &name)? {
                            return Err(OhpmError::new(
                                "ParameterizationForbiddenInstallError",
                                format!(
                                    "The \"ohpm install {name}\" command cannot be executed when the \"parameterFile\" is configured."
                                ),
                            ));
                        }
                    }
                }
                if opts.save {
                    names.push(name.clone());
                    update_dependencies(root, &name, &parsed.raw_spec, opts);
                } else {
                    root.requirements.insert(
                        name,
                        Requirement {
                            spec: parsed.fetch_spec.clone(),
                            dep_type: DepType::NoSave,
                        },
                    );
                }
            }
            InstallCommand::Update => {
                if !root.requirements.contains_key(&name) {
                    log::warn!(
                        "The package you want to update: \"{name}\" does not exist in {}",
                        prefix.join(MY_PACKAGE_JSON).display()
                    );
                }
            }
            InstallCommand::Uninstall => {
                if root.requirements.contains_key(&name) {
                    if let Some(p) = parameter {
                        if !p.can_modify(&prefix.join(MY_PACKAGE_JSON), &name)? {
                            return Err(OhpmError::new(
                                "ParameterizationForbiddenUninstallError",
                                format!(
                                    "The \"uninstall\" command cannot be executed when the \"parameterFile\" is configured."
                                ),
                            ));
                        }
                    }
                    root.requirements.remove(&name);
                } else {
                    log::warn!(
                        "The package you want to uninstall: \"{name}\" does not exist in {}",
                        prefix.join(MY_PACKAGE_JSON).display()
                    );
                }
            }
        }
    }
    Ok(names)
}

/// `f(o)` in `handleCliInput` — the package name, read from the source dir or
/// tarball when the CLI input is an unnamed local path.
fn resolve_cli_name(parsed: &Spec) -> Option<String> {
    if !parsed.name.is_empty() {
        return Some(parsed.name.clone());
    }
    let path = PathBuf::from(&parsed.fetch_spec);
    match parsed.ohpa_type {
        OhpaType::SourceCode => {
            let manifest = crate::package::read_manifest_from_dir(&path).ok()?;
            (!manifest.name.is_empty()).then(|| manifest.name)
        }
        OhpaType::File => {
            let manifest = crate::install::resolver::read_manifest_from_tar(&path).ok()?;
            (!manifest.name.is_empty()).then(|| manifest.name)
        }
        _ => None,
    }
}

/// `updateDependencies.js` — `saveDynamic ? dynamic : saveDev ? dev : save ? prod`.
fn update_dependencies(root: &mut Node, name: &str, raw_spec: &str, opts: &crate::install::InstallOptions) {
    let (spec, dep_type) = if opts.save_dynamic {
        (raw_spec.to_string(), DepType::Dynamic)
    } else if opts.save_dev {
        (raw_spec.to_string(), DepType::Dev)
    } else {
        (raw_spec.to_string(), DepType::Prod)
    };
    root.requirements.insert(
        name.to_string(),
        Requirement { spec, dep_type },
    );
}

/// `updateCommandLineInputDependencies` — rewrite the prefix root's
/// requirements to the resolved save specs (so `ohpm install foo` writes
/// `"foo": "^1.2.3"` into the manifest). Returns the updated requirements for
/// the prefix root.
pub fn update_command_line_input_dependencies(
    graph: &DependencyGraph,
    cli_input_names: &[String],
    root_dir: &Path,
) -> Result<Option<BTreeMap<String, Requirement>>> {
    if cli_input_names.is_empty() {
        return Ok(None);
    }
    let Some((_, root)) = graph.roots.iter().find(|(d, _)| d == root_dir) else {
        return Ok(None);
    };
    let root = root.clone();
    let names: Vec<String> = root.requirements.keys().cloned().collect();
    let mut updated: BTreeMap<String, Requirement> = BTreeMap::new();
    for name in names {
        let req = root.requirements.get(&name).cloned().unwrap();
        let node = graph.pick_node(&name, &req.spec, &root)?;
        let mut spec = node.data.save_spec.clone();
        if node.data.registry_type == "local" {
            spec = format!(
                "file:{}",
                relative_slash(root_dir, Path::new(&node.data.fetch_spec))
            );
        }
        if cli_input_names.contains(&name) {
            updated.insert(
                name,
                Requirement {
                    spec,
                    dep_type: req.dep_type,
                },
            );
        }
    }
    Ok(Some(updated))
}

fn relative_slash(from: &Path, to: &Path) -> String {
    let from_parts: Vec<_> = from.components().collect();
    let to_parts: Vec<_> = to.components().collect();
    let mut common = 0;
    while common < from_parts.len() && common < to_parts.len() && from_parts[common] == to_parts[common] {
        common += 1;
    }
    let mut parts: Vec<String> = (0..from_parts.len() - common).map(|_| "..".to_string()).collect();
    for c in &to_parts[common..] {
        parts.push(c.as_os_str().to_string_lossy().into_owned());
    }
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

/// `updatePkgJson.js` — rewrite the manifest's dependency maps when changed;
/// parameterized manifests (`${`) are skipped. `requirements` is the
/// post-`updateCommandLineInputDependencies` root requirement map.
///
/// `cli_input_names` is `Some` for INSTALL (empty means "don't write") and
/// `None` for UPDATE/UNINSTALL (always write, like the reference's `undefined`).
pub fn update_pkg_json(
    prefix: &Path,
    requirements: &BTreeMap<String, Requirement>,
    opts: &crate::install::InstallOptions,
    cli_input_names: Option<&[String]>,
) -> Result<bool> {
    let write = match cli_input_names {
        Some(names) => !names.is_empty(),
        None => true,
    };
    if !write || !opts.save {
        return Ok(false);
    }
    let path = prefix.join(MY_PACKAGE_JSON);
    if !path.exists() {
        return Ok(false);
    }
    let text = std::fs::read_to_string(&path)?;
    if text.contains("${") {
        // Parameterized manifest — the reference refuses to rewrite it.
        return Ok(false);
    }
    let mut manifest: serde_json::Value = json5::from_str(&text)?;
    let filter = |t: DepType| -> BTreeMap<String, String> {
        requirements
            .iter()
            .filter(|(_, r)| r.dep_type == t)
            .map(|(n, r)| (n.clone(), r.spec.clone()))
            .collect()
    };
    let deps = filter(DepType::Prod);
    let dev_deps = filter(DepType::Dev);
    let dynamic_deps = filter(DepType::Dynamic);
    let unchanged = |current: Option<&serde_json::Value>, next: &BTreeMap<String, String>| {
        match current {
            None => next.is_empty(),
            Some(v) => {
                serde_json::to_value(next).ok().as_ref() == Some(v)
            }
        }
    };
    let old_deps = manifest.get("dependencies");
    let old_dev = manifest.get("devDependencies");
    let old_dynamic = manifest.get("dynamicDependencies");
    if unchanged(old_deps, &deps)
        && unchanged(old_dev, &dev_deps)
        && unchanged(old_dynamic, &dynamic_deps)
    {
        return Ok(false);
    }
    if let Some(obj) = manifest.as_object_mut() {
        obj.insert("dependencies".to_string(), serde_json::to_value(deps).unwrap());
        obj.insert("devDependencies".to_string(), serde_json::to_value(dev_deps).unwrap());
        obj.insert("dynamicDependencies".to_string(), serde_json::to_value(dynamic_deps).unwrap());
    }
    std::fs::write(&path, serde_json::to_string_pretty(&manifest)?)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::InstallOptions;
    use std::fs;

    fn write_manifest(dir: &Path, deps: &str, dev: &str, dynamic: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join(MY_PACKAGE_JSON),
            format!(
                "{{ name: \"entry\", version: \"1.0.0\", dependencies: {deps}, devDependencies: {dev}, dynamicDependencies: {dynamic} }}\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn root_node_from_manifest() {
        let dir = tempfile::TempDir::new().unwrap();
        write_manifest(dir.path(), "{ \"foo\": \"^1.0.0\" }", "{ \"bar\": \"2.0.0\" }", "{}");
        let root = get_root_node(dir.path(), true, None, None).unwrap();
        assert_eq!(root.data.name, "entry");
        assert_eq!(root.data.version, "1.0.0");
        assert!(root.data.is_root);
        assert_eq!(root.requirements["foo"].spec, "^1.0.0");
        assert_eq!(root.requirements["foo"].dep_type, DepType::Prod);
        assert_eq!(root.requirements["bar"].dep_type, DepType::Dev);
        assert_eq!(root.node_key(), format!("entry@{}", dir.path().display()));
    }

    #[test]
    fn project_root_blanks_name() {
        let dir = tempfile::TempDir::new().unwrap();
        write_manifest(dir.path(), "{}", "{}", "{}");
        fs::write(
            dir.path().join("build-profile.json5"),
            "{ \"modules\": [] }",
        )
        .unwrap();
        let project = ProjectBuildProfile::load(dir.path()).unwrap();
        let root = get_root_node(dir.path(), true, Some(&project), None).unwrap();
        assert_eq!(root.data.name, "");
        assert_eq!(root.data.version, "");
    }

    #[test]
    fn cli_input_handling() {
        let dir = tempfile::TempDir::new().unwrap();
        write_manifest(dir.path(), "{}", "{}", "{}");
        let mut root = get_root_node(dir.path(), true, None, None).unwrap();
        let opts = InstallOptions {
            save: true,
            save_dev: true,
            ..Default::default()
        };
        let names = handle_cli_input(dir.path(), &["foo@^2.0.0".to_string()], &mut root, &opts, InstallCommand::Install, None).unwrap();
        assert_eq!(names, vec!["foo"]);
        assert_eq!(root.requirements["foo"].spec, "^2.0.0");
        assert_eq!(root.requirements["foo"].dep_type, DepType::Dev);

        // --no-save uses the fetch spec and no cli names.
        let mut root = get_root_node(dir.path(), true, None, None).unwrap();
        let opts = InstallOptions {
            save: false,
            ..Default::default()
        };
        let names = handle_cli_input(dir.path(), &["foo".to_string()], &mut root, &opts, InstallCommand::Install, None).unwrap();
        assert!(names.is_empty());
        assert_eq!(root.requirements["foo"].spec, "latest");
        assert_eq!(root.requirements["foo"].dep_type, DepType::NoSave);

        // Bare name with save: requirement spec is empty (parses as latest).
        let mut root = get_root_node(dir.path(), true, None, None).unwrap();
        let opts = InstallOptions::default();
        let names = handle_cli_input(dir.path(), &["foo".to_string()], &mut root, &opts, InstallCommand::Install, None).unwrap();
        assert_eq!(names, vec!["foo"]);
        assert_eq!(root.requirements["foo"].spec, "");
    }

    #[test]
    fn update_pkg_json_rewrites() {
        let dir = tempfile::TempDir::new().unwrap();
        write_manifest(dir.path(), "{}", "{}", "{}");
        let mut root = get_root_node(dir.path(), true, None, None).unwrap();
        let opts = InstallOptions::default();
        let names = handle_cli_input(dir.path(), &["foo".to_string()], &mut root, &opts, InstallCommand::Install, None).unwrap();
        // Resolve the requirement spec like updateCommandLineInputDependencies.
        root.requirements.insert(
            "foo".to_string(),
            Requirement {
                spec: "^1.2.3".to_string(),
                dep_type: DepType::Prod,
            },
        );
        let written = update_pkg_json(dir.path(), &root.requirements, &opts, Some(&names)).unwrap();
        assert!(written);
        let text = fs::read_to_string(dir.path().join(MY_PACKAGE_JSON)).unwrap();
        assert!(text.contains("\"foo\": \"^1.2.3\""));
        assert!(text.contains("\"devDependencies\": {}"));
        // Unchanged → no rewrite.
        let written2 = update_pkg_json(dir.path(), &root.requirements, &opts, Some(&names)).unwrap();
        assert!(!written2);

        // Parameterized manifests are skipped.
        let param = dir.path().join("param");
        fs::create_dir_all(&param).unwrap();
        fs::write(
            param.join(MY_PACKAGE_JSON),
            "{ name: \"x\", version: \"${VERSION}\" }\n",
        )
        .unwrap();
        let mut root2 = get_root_node(&param, true, None, None).unwrap();
        let names2 = handle_cli_input(&param, &["foo".to_string()], &mut root2, &opts, InstallCommand::Install, None).unwrap();
        root2.requirements.insert(
            "foo".to_string(),
            Requirement {
                spec: "^1.0.0".to_string(),
                dep_type: DepType::Prod,
            },
        );
        assert!(!update_pkg_json(&param, &root2.requirements, &opts, Some(&names2)).unwrap());
    }
}

#[cfg(test)]
mod at_module_tests {
    use super::*;

    #[test]
    fn module_marker_resolves_to_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let proj = dir.path().join("proj");
        let lib1 = proj.join("lib1");
        std::fs::create_dir_all(&lib1).unwrap();
        std::fs::write(
            lib1.join(MY_PACKAGE_JSON),
            "{ name: \"lib1\", version: \"1.0.0\" }\n",
        )
        .unwrap();
        let project = ProjectBuildProfile {
            project_root: proj.clone(),
            module_map: BTreeMap::from([("lib1".to_string(), lib1.clone())]),
            ..Default::default()
        };
        let mut value: serde_json::Value = json5::from_str(
            "{ dependencies: { \"lib1\": \"@module:lib1\" } }\n",
        )
        .unwrap();
        parse_at_module(&mut value, &project, &proj).unwrap();
        assert_eq!(
            value["dependencies"]["lib1"],
            serde_json::Value::String(lib1.to_string_lossy().into_owned())
        );

        // A missing module errors.
        let mut value: serde_json::Value =
            json5::from_str("{ dependencies: { \"nope\": \"@module:nope\" } }\n").unwrap();
        let err = parse_at_module(&mut value, &project, &proj).unwrap_err();
        assert_eq!(err.code, "AtModuleModuleNameUnExistError");

        // A name mismatch errors.
        std::fs::write(
            lib1.join(MY_PACKAGE_JSON),
            "{ name: \"other\", version: \"1.0.0\" }\n",
        )
        .unwrap();
        let mut value: serde_json::Value =
            json5::from_str("{ dependencies: { \"lib1\": \"@module:lib1\" } }\n").unwrap();
        let err = parse_at_module(&mut value, &project, &proj).unwrap_err();
        assert_eq!(err.code, "ParameterizationInconsistentDepNames");
    }
}
