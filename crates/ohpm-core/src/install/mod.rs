//! The install subsystem: dependency resolution, lockfile, store and
//! `oh_modules` linking, mirroring the reference `lib/core/install/`.
//!
//! The pipeline mirrors pnpm's Rust engine (pacquet): resolution → lockfile →
//! store → linking, in a single program.

pub mod graph;
pub mod lock_record;
pub mod lockfile;
pub mod modules;
pub mod node;
pub mod packument;
pub mod resolver;
pub mod root;
pub mod semver;
pub mod spec;
pub mod store;
pub mod symlink;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::{default::types, Config};
use crate::error::Result;
use crate::registry::RegistryClient;

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
    /// `--prefix`.
    pub prefix: Option<PathBuf>,
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
            prefix: None,
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

/// `core/install/service/install.js` — the full pipeline:
/// module roots → root nodes → graph build → install → symlink → install
/// record → manifest update → lockfile flush.
pub async fn install(
    client: &RegistryClient,
    config: &Config,
    prefix: &Path,
    args: &[String],
    opts: &InstallOptions,
) -> Result<InstallOutcome> {
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

    // 1. module roots + project root (build-profile based). The `install_all`
    // config defaults to true (`getModuleRootDirs` reads it too).
    let install_all = opts.all || config.get_bool(types::INSTALL_ALL);
    let project = crate::config::find_project_root(prefix)
        .and_then(|p| modules::ProjectBuildProfile::load(&p));
    let project_root = project
        .as_ref()
        .map(|p| p.project_root.clone())
        .unwrap_or_else(|| prefix.to_path_buf());
    let module_roots = modules::module_roots(prefix, install_all, project.as_ref());

    // 2. root nodes + CLI input.
    let resolver = Arc::new(resolver::Resolver::new(
        client.clone(),
        config.clone(),
        project_root.clone(),
    ));
    let mut roots: Vec<(PathBuf, Arc<node::Node>)> = Vec::new();
    let mut cli_input_names = Vec::new();
    for module_root in &module_roots {
        let mut root = root::get_root_node(module_root, opts.link, project.as_ref())?;
        if module_root == prefix {
            cli_input_names = root::handle_cli_input(prefix, args, &mut root, opts)?;
        }
        roots.push((module_root.clone(), Arc::new(root)));
    }
     *resolver.cli_input_names.lock().unwrap() = cli_input_names.clone();

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
    let record = lock_record::resolve(&graphs, &project_root, &settings);
    lock_record::write_lock_record(&project_root, &record)?;

    // 7. manifest update for the install-root module.
    if let Some(updated) = root::update_command_line_input_dependencies(
        &graphs[0],
        &cli_input_names,
        prefix,
    )? {
        root::update_pkg_json(prefix, &updated, opts, &cli_input_names)?;
    }

    // 8. flush the lockers (oh-package-lock.json5).
    resolver.flush_lockers().await?;

    // 9. cleanup: empty oh_modules dirs.
    for module_root in &module_roots {
        store::delete_empty_oh_modules_dir(module_root)?;
    }

    Ok(InstallOutcome {
        module_roots,
        installed,
        cli_input_names,
    })
}

/// Whether the install root has a manifest (the CLI validates the prefix).
pub fn has_manifest(prefix: &Path) -> bool {
    prefix.join(crate::constants::MY_PACKAGE_JSON).is_file()
}
