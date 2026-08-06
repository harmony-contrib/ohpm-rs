//! The install subsystem: dependency resolution, lockfile, store and
//! `oh_modules` linking, mirroring the reference `lib/core/install/`.
//!
//! The pipeline mirrors pnpm's Rust engine (pacquet): resolution → lockfile →
//! store → linking, in a single program. `install`, `update` and `uninstall`
//! share one pipeline (`run_pipeline`), like the reference's `installModules`.

pub mod alarm;
pub mod exclusions;
pub mod filelock;
pub mod git;
pub mod graph;
pub mod hooks;
pub mod lock_record;
pub mod version_conflict;
pub mod lockfile;
pub mod mtime;
pub mod modules;
pub mod parameter;
pub mod node;
pub mod overrides;
pub mod packument;
pub mod resolver;
pub mod root;
pub mod semver;
pub mod spec;
pub mod store;
pub mod symlink;
pub mod targets;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::{default::types, Config};
use crate::error::{OhpmError, Result};
use crate::registry::RegistryClient;
use root::InstallCommand;

/// `ohpm install` options (mirrors `lib/core/install/common/definitions.js`).
#[derive(Debug, Clone)]
pub struct InstallOptions {
    /// `--no-save` — install without writing oh-package.json5.
    pub save: bool,
    pub save_dev: bool,
    pub save_prod: bool,
    pub save_dynamic: bool,
    /// `--no-link` — copy source-code deps instead of symlinking.
    pub link: bool,
    /// `--all` — install all project modules' dependencies.
    pub all: bool,
    /// `--all-modules` (update) — act on every module root.
    pub all_modules: bool,
    /// `--tag-filter` (update) — only update tag dependencies whose tag
    /// matches this regex.
    pub tag_filter: Option<String>,
    /// `--prefix`.
    pub prefix: Option<PathBuf>,
    /// `--parameter-file` — the parameter file path (parameterization).
    pub parameter_file: Option<PathBuf>,
    /// `--target_path` — the install-targets context (Task 4).
    pub target_path: Option<PathBuf>,
    pub registry: Option<String>,
    pub fetch_timeout: Option<u64>,
    pub strict_ssl: Option<bool>,
    pub max_concurrent: Option<u64>,
    pub retry_times: Option<u32>,
    pub retry_interval: Option<u64>,
}

impl Default for InstallOptions {
    fn default() -> Self {
        InstallOptions {
            save: true,
            save_dev: false,
            save_prod: true,
            save_dynamic: false,
            link: true,
            all: false,
            all_modules: false,
            tag_filter: None,
            prefix: None,
            parameter_file: None,
            target_path: None,
            registry: None,
            fetch_timeout: None,
            strict_ssl: None,
            max_concurrent: None,
            retry_times: None,
            retry_interval: None,
        }
    }
}

/// The install outcome.
#[derive(Debug, Clone)]
pub struct InstallOutcome {
    pub module_roots: Vec<PathBuf>,
    pub installed: usize,
    pub cli_input_names: Vec<String>,
}

/// The update outcome.
#[derive(Debug, Clone)]
pub struct UpdateOutcome {
    pub module_roots: Vec<PathBuf>,
    pub installed: usize,
}

/// The uninstall outcome.
#[derive(Debug, Clone)]
pub struct UninstallOutcome {
    pub module_roots: Vec<PathBuf>,
    pub installed: usize,
}

struct PipelineOutcome {
    module_roots: Vec<PathBuf>,
    installed: usize,
    cli_input_names: Vec<String>,
}

/// `core/install/service/install.js` — `install`.
pub async fn install(
    client: &RegistryClient,
    config: &Config,
    prefix: &Path,
    args: &[String],
    opts: &InstallOptions,
) -> Result<InstallOutcome> {
    // `checkInvalidCmdWhileParameterization` — installing named packages is
    // forbidden while the project is parameterized.
    let project_root = crate::config::find_project_root(prefix)
        .map(|p| p.clone())
        .unwrap_or_else(|| prefix.to_path_buf());
    let parameterized = parameter::Parameterization::setup(config, &project_root, opts.parameter_file.as_deref())?
        .is_some();
    if parameterized && !args.is_empty() {
        return Err(OhpmError::new(
            "ParameterizationForbiddenInstallError",
            "The \"ohpm install <pkg>\" command cannot be executed when the \"parameterFile\" is configured.",
        ));
    }
    let outcome = run_pipeline(client, config, prefix, args, opts, InstallCommand::Install).await?;
    Ok(InstallOutcome {
        module_roots: outcome.module_roots,
        installed: outcome.installed,
        cli_input_names: outcome.cli_input_names,
    })
}

/// `core/install/service/update.js` — `updateSymlink`.
pub async fn update(
    client: &RegistryClient,
    config: &Config,
    prefix: &Path,
    args: &[String],
    opts: &InstallOptions,
) -> Result<UpdateOutcome> {
    // `PackageUtil.parse` — update arguments must not carry versions.
    for raw in args {
        if let Some(version) = root::parse_cli_pkg_version(raw) {
            return Err(OhpmError::update_has_version(&version));
        }
    }
    let outcome = run_pipeline(client, config, prefix, args, opts, InstallCommand::Update).await?;
    Ok(UpdateOutcome {
        module_roots: outcome.module_roots,
        installed: outcome.installed,
    })
}

/// `core/install/service/uninstall.js` — `uninstallSymlink`.
pub async fn uninstall(
    client: &RegistryClient,
    config: &Config,
    prefix: &Path,
    args: &[String],
    opts: &InstallOptions,
) -> Result<UninstallOutcome> {
    if args.is_empty() {
        return Err(OhpmError::uninstall_no_pkg());
    }
    for raw in args {
        if let Some(version) = root::parse_cli_pkg_version(raw) {
            return Err(OhpmError::uninstall_has_version(&version));
        }
    }
    // `uninstallSymlink` forces all save kinds so the manifest rewrite removes
    // the uninstalled packages from every dependency map.
    let mut opts = opts.clone();
    opts.save_dynamic = true;
    opts.save_dev = true;
    opts.save_prod = true;
    let outcome = run_pipeline(client, config, prefix, args, &opts, InstallCommand::Uninstall).await?;
    Ok(UninstallOutcome {
        module_roots: outcome.module_roots,
        installed: outcome.installed,
    })
}

/// The shared pipeline (`installModules` + the per-command wrapper steps).
async fn run_pipeline(
    client: &RegistryClient,
    config: &Config,
    prefix: &Path,
    args: &[String],
    opts: &InstallOptions,
    command: InstallCommand,
) -> Result<PipelineOutcome> {
    let max_concurrent = opts
        .max_concurrent
        .unwrap_or(config.get_number(types::MAX_CONCURRENT) as u64)
        .max(1) as usize;
    let retry_times = opts
        .retry_times
        .unwrap_or(config.get_number(types::RETRY_TIMES) as u32);
    let retry_interval = opts
        .retry_interval
        .unwrap_or(config.get_number(types::RETRY_INTERVAL) as u64);

    // 0. cross-process lock (`enable_cross_process_lock`, default off).
    let _lock = filelock::OhpmLock::acquire(config, prefix).await?;

    // 1. module roots + project root (build-profile based). The `install_all`
    // config defaults to true (`getModuleRootDirs` reads it too). Target mode
    // (`--target_path`) switches the module roots to the dependencyMap's.
    let install_all = opts.all || config.get_bool(types::INSTALL_ALL);
    let project = crate::config::find_project_root(prefix)
        .and_then(|p| modules::ProjectBuildProfile::load(&p));
    let project_root = project
        .as_ref()
        .map(|p| p.project_root.clone())
        .unwrap_or_else(|| prefix.to_path_buf());
    let mut targets = targets::TargetManager::new();
    if let Some(target_path) = &opts.target_path {
        targets::TargetManager::validate_target_path(&target_path.to_string_lossy())?;
        targets.init(target_path, &project_root)?;
    }
    let module_roots = if targets.is_target_mod() {
        targets::target_module_roots(&targets, prefix, install_all, project.as_ref())?
    } else {
        modules::module_roots(prefix, install_all, project.as_ref())
    };
    let lock_name = if targets.need_change_lock_file_name() {
        lockfile::lock_file_name(&targets.get_target_name())
    } else {
        lockfile::lock_file_name("")
    };

    // 1b. lifecycle hooks — preInstall / preUninstall.
    let root_refs: Vec<&std::path::Path> = module_roots.iter().map(|p| p.as_path()).collect();
    match command {
        InstallCommand::Install | InstallCommand::Update => {
            hooks::run_hooks(&root_refs, hooks::HookEvent::PreInstall)?;
        }
        InstallCommand::Uninstall => {
            hooks::run_hooks(&root_refs, hooks::HookEvent::PreUninstall)?;
        }
    }

    // 2. root nodes + CLI input (`getRootNodeForInstallation`: UPDATE with
    // `--all-modules`, or the prefix module, gets the CLI input handling).
    let workspace = crate::workspace::Workspace::find(prefix)?;
    // `validParameterFileConfig` — parameterization (None when not configured).
    let parameter = parameter::Parameterization::setup(config, &project_root, opts.parameter_file.as_deref())?;
    let resolver = Arc::new(resolver::Resolver::new(
        client.clone(),
        config.clone(),
        project_root.clone(),
        workspace,
        parameter.as_ref(),
        &lock_name,
    )?);
    let mut roots: Vec<(PathBuf, Arc<node::Node>)> = Vec::new();
    let mut cli_input_names = Vec::new();
    let handle_cli_on_all = command == InstallCommand::Update && opts.all_modules;
    for module_root in &module_roots {
        let mut root = root::get_root_node(module_root, opts.link, project.as_ref(), parameter.as_ref())?;
        // Target mode: the module manifest may come from the dependencyMap.
        if let Some(manifest) = targets.target_module_manifest(module_root) {
            if !manifest.is_null() && manifest.as_object().is_some_and(|m| !m.is_empty()) {
                let text = serde_json::to_string(&manifest).unwrap_or_default();
                let mut manifest = crate::package::Manifest::from_json5(&text)?;
                if let Some(p) = parameter.as_ref() {
                    let mut value = manifest.to_json();
                    p.parse_value(&manifest.name, &mut value)?;
                    manifest = serde_json::from_value(value)?;
                }
                root = root::root_node_from_manifest(module_root, &manifest, opts.link, project.as_ref())?;
            }
        }
        if handle_cli_on_all || module_root == prefix {
            cli_input_names =
                root::handle_cli_input(module_root, args, &mut root, opts, command, parameter.as_ref())?;
        }
        roots.push((module_root.clone(), Arc::new(root)));
    }
    *resolver.cli_input_names.lock().unwrap() = cli_input_names.clone();
    // `syncLoadLockers` — lockers exist even when the graph ends up empty.
    resolver.ensure_lockers(&module_roots).await;

    // 2b. update: delete the matching lockfile specifiers (or clear them).
    if command == InstallCommand::Update {
        update_delete_specifiers(&resolver, &roots, &module_roots, prefix, args, opts).await;
    }

    // 3. graph build — one graph per module root (mirrors installMultiModules).
    let mut graphs = Vec::new();
    for (module_root, root) in &roots {
        let graph = graph::build_graphs(
            &resolver,
            vec![(module_root.clone(), root.clone())],
            max_concurrent,
            retry_times,
            retry_interval,
            project.as_ref(),
        )
        .await?;
        graphs.push(graph);
    }

    // 3b. alarms + conflict resolution (mirrors `installModules`):
    // `recordConflictMessage` then `resolveConflictInAllGraphs` — the lockers
    // are rewritten to the max-satisfying versions; the graphs were already
    // flattened by `build_graphs` (pickNode/flatGraph consult the final cache
    // in resolve mode).
    let mut strict_alarm = crate::install::alarm::StrictConflictAlarm::new();
    {
        let versions = resolver.versions.lock().await.clone();
        let fetch_specs = resolver.fetch_specs.lock().await.clone();
        let max_versions: std::collections::BTreeMap<String, String> = resolver
            .max_satisfying
            .lock()
            .await
            .iter()
            .chain(resolver.max_local.lock().await.iter())
            .map(|(k, v)| (k.clone(), v.pinned_spec.clone()))
            .collect();
        for (graph, (module_root, _)) in graphs.iter().zip(&roots) {
            if resolver.strict_mode {
                strict_alarm.record(
                    graph,
                    &module_root.to_string_lossy(),
                    &versions,
                    &fetch_specs,
                    &max_versions,
                );
            }
        }
        if resolver.resolve_conflict {
            for (graph, (module_root, _)) in graphs.iter().zip(&roots) {
                resolver
                    .resolve_conflict_to_lockfile(module_root, graph)
                    .await?;
            }
        }
    }
    if resolver.strict_mode {
        strict_alarm.print();
    }

    // 3c. name-consistency alarms — `enforce_dependency_key` or the
    // build-profile OHMUrl config makes the inconsistencies an error.
    let enforce = config.get_bool(types::ENFORCE_DEPENDENCY_KEY)
        || project.as_ref().map(|p| p.use_ohmurl()).unwrap_or(false);
    let mut name_alarm = crate::install::alarm::NameInconsistencyAlarm::new(enforce);
    let mut case_alarm = crate::install::alarm::CaseInconsistencyAlarm::new();
    for graph in &graphs {
        name_alarm.record(graph);
        case_alarm.record(graph);
    }
    name_alarm.print()?;
    case_alarm.print();

    // 4. install phase (download/extract into the store).
    let store = Arc::new(store::StoreContext::new(
        client.clone(),
        config.clone(),
        project_root.clone(),
        max_concurrent,
    ));
    for graph in &graphs {
        store::install_phase(&store, graph).await?;
    }
    let installed = store.installed_count().await;

    // 5. symlink phase + phantom cleanup + tmp cleanup.
    for graph in &graphs {
        symlink::symlink_phase(&store, graph).await?;
    }
    let keep_names: std::collections::BTreeSet<String> = graphs
        .iter()
        .flat_map(|g| g.max_satisfying_names())
        .collect();
    symlink::clean_phantom_links(&project_root, &keep_names)?;
    store::delete_useless_tmp_dir(&project_root)?;

    // 6. install record (`oh_modules/.ohpm/lock.json5`).
    let settings = lock_record::LockSettings {
        resolve_conflict: lock_record::need_resolve_conflict(config),
        resolve_conflict_strict: config.get_bool(types::RESOLVE_CONFLICT_STRICT),
        install_all,
    };
    let record = lock_record::resolve(
        &graphs,
        &project_root,
        &settings,
        resolver.overrides.as_ref(),
        resolver.exclusions.lock().await.as_ref(),
    );
    lock_record::write_lock_record(&project_root, &record)?;

    // 7. manifest update (command-dependent). Target mode writes the resolved
    // module manifests into `<target>/resolve-conflict/<module>`
    // (`savePackageJsonOfResolveConflict`).
    if targets.is_target_mod() && command == InstallCommand::Install && opts.save {
        for (module_root, root) in &roots {
            let dir = targets.get_resolve_conflict_path(module_root);
            std::fs::create_dir_all(&dir)?;
            let manifest_path = dir.join(crate::constants::MY_PACKAGE_JSON);
            let mut manifest: serde_json::Value = std::fs::read_to_string(&manifest_path)
                .ok()
                .and_then(|t| json5::from_str(&t).ok())
                .unwrap_or_else(|| {
                    targets
                        .target_module_manifest(module_root)
                        .unwrap_or_else(|| serde_json::json!({}))
                });
            for key in ["dependencies", "devDependencies", "dynamicDependencies"] {
                let Some(map) = manifest.get_mut(key).and_then(|m| m.as_object_mut()) else {
                    continue;
                };
                for (name, spec) in map.iter_mut() {
                    let Some(req) = root.requirements.get(name) else {
                        continue;
                    };
                    // `d()` — the resolved version from the locker (local deps
                    // keep their path spec).
                    if crate::install::spec::is_local_dependency(&req.spec) {
                        continue;
                    }
                    let rel = crate::install::lockfile::relative_spec(module_root, &req.spec);
                    if let Some(locked) = resolver
                        .with_locker(module_root, |locker| locker.get_lock_spec(name, &rel))
                        .await
                    {
                        if let Ok((_, version)) = crate::install::lockfile::parse_spec_key(&locked) {
                            *spec = serde_json::Value::String(version);
                        }
                    }
                }
            }
            std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;
        }
    }
    match command {
        InstallCommand::Install => {
            if let Some(updated) = root::update_command_line_input_dependencies(
                &graphs[0],
                &cli_input_names,
                prefix,
            )? {
                root::update_pkg_json(prefix, &updated, opts, Some(&cli_input_names))?;
            }
        }
        InstallCommand::Update => {
            let targets: Vec<PathBuf> = if opts.all_modules {
                module_roots.clone()
            } else {
                vec![prefix.to_path_buf()]
            };
            for module_root in &targets {
                if let Some((_, root)) = roots.iter().find(|(d, _)| d == module_root) {
                    root::update_pkg_json(module_root, &root.requirements, opts, None)?;
                }
            }
        }
        InstallCommand::Uninstall => {
            if let Some((_, root)) = roots.iter().find(|(d, _)| d == prefix) {
                root::update_pkg_json(prefix, &root.requirements, opts, None)?;
            }
        }
    }

    // 7b. lifecycle hooks — postInstall / postUninstall.
    match command {
        InstallCommand::Install | InstallCommand::Update => {
            hooks::run_hooks(&root_refs, hooks::HookEvent::PostInstall)?;
        }
        InstallCommand::Uninstall => {
            hooks::run_hooks(&root_refs, hooks::HookEvent::PostUninstall)?;
        }
    }

    // 8. flush the lockers (oh-package-lock.json5).
    resolver.flush_lockers().await?;

    // 9. cleanup: empty oh_modules dirs.
    for module_root in &module_roots {
        store::delete_empty_oh_modules_dir(module_root)?;
    }

    Ok(PipelineOutcome {
        module_roots,
        installed,
        cli_input_names,
    })
}

/// `update.js` — delete the lockfile specifiers of the packages being updated
/// (or clear all specifiers for `update` with no arguments).
async fn update_delete_specifiers(
    resolver: &Arc<resolver::Resolver>,
    roots: &[(PathBuf, Arc<node::Node>)],
    module_roots: &[PathBuf],
    prefix: &Path,
    args: &[String],
    opts: &InstallOptions,
) {
    let has_targets = !args.is_empty() || opts.tag_filter.is_some();
    if !has_targets {
        // `clearSpecifiers(prefix, options)` — all lockers when --all-modules.
        if opts.all_modules {
            for module_root in module_roots {
                resolver.clear_specifiers(module_root).await;
            }
        } else {
            resolver.clear_specifiers(prefix).await;
        }
        return;
    }
    let targets: Vec<PathBuf> = if opts.all_modules {
        module_roots.to_vec()
    } else {
        vec![prefix.to_path_buf()]
    };
    let filter = opts
        .tag_filter
        .as_ref()
        .and_then(|f| regex::Regex::new(f).ok());
    for module_root in &targets {
        let Some((_, root)) = roots.iter().find(|(d, _)| d == module_root) else {
            continue;
        };
        // `pkgs ? keys filtered by pkgs : all keys`
        let names: Vec<String> = if args.is_empty() {
            root.requirements.keys().cloned().collect()
        } else {
            args.iter()
                .filter(|a| root.requirements.contains_key(*a))
                .cloned()
                .collect()
        };
        for name in names {
            let spec = root.requirements[&name].spec.clone();
            if let Some(filter) = &filter {
                // `isStandardTagDependency(spec) && filter.test(tag)`
                if !crate::install::spec::is_standard_tag_dependency(&spec) {
                    continue;
                }
                let tag = &spec[crate::constants::TAG_PREFIX.len()..];
                if !filter.is_match(tag) {
                    continue;
                }
            }
            resolver
                .delete_specifier(module_root, &format!("{name}@{spec}"))
                .await;
        }
    }
}

/// Whether the install root has a manifest (the CLI validates the prefix).
pub fn has_manifest(prefix: &Path) -> bool {
    prefix.join(crate::constants::MY_PACKAGE_JSON).is_file()
}
