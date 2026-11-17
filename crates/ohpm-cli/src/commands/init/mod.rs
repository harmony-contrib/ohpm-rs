//! `ohpm init` — create an `oh-package.json5`, or scaffold a standard
//! HarmonyOS project.
//!
//! Without `--type` the command is interactive when stdin is a terminal and
//! opens a small TUI. Scripts (non-TTY) and `-y` keep the reference behavior
//! and write the default `oh-package.json5`.
//!
//! `--type har|hsp|project` scaffolds the DevEco Studio project layout
//! (root `AppScope`/`hvigor` files plus a library or entry module) from CLI
//! flags only — no prompts — which keeps it usable in CI.

mod png;
mod scaffold;
mod templates;
mod tui;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use ohpm_core::constants::MY_PACKAGE_JSON;
use ohpm_core::package::validate::is_standard_package_name;

use super::{load_config, output};
use crate::cli::InitArgs;
use scaffold::{InitKind, InitOptions};

pub async fn run(args: &InitArgs) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let _config = load_config()?;

    let requested = args
        .project_type
        .as_deref()
        .map(InitKind::parse)
        .transpose()?;

    // A bare `init` in a terminal opens the TUI; everything else is
    // non-interactive so scripts and `-y` never block.
    let interactive = requested.is_none() && !args.yes && std::io::stdin().is_terminal();
    if interactive {
        return match tui::prompt(args, &cwd)? {
            Some(options) => execute(&options),
            None => Ok(()),
        };
    }

    execute(&non_interactive_options(
        args,
        &cwd,
        requested.unwrap_or(InitKind::Package),
    )?)
}

/// Build the options for the flag-driven path.
fn non_interactive_options(args: &InitArgs, cwd: &Path, kind: InitKind) -> Result<InitOptions> {
    let dir = target_dir(args, cwd);

    if kind == InitKind::Package {
        if args.bundle_name.is_some() || args.api_version.is_some() {
            bail!("--bundle-name and --api-version require --type har, hsp, or project.");
        }
        let base = args
            .name
            .clone()
            .or_else(|| existing_manifest_name(dir.as_path()))
            .or_else(|| existing_manifest_name(cwd))
            .unwrap_or_else(|| directory_name(cwd).to_lowercase());
        let name = apply_group(base, args.group.as_deref());
        check_package_name(&name)?;
        return Ok(InitOptions {
            kind,
            dir,
            name,
            bundle_name: None,
            api_version: None,
            force: args.force,
        });
    }

    let name = if kind == InitKind::Project {
        if args.group.is_some() {
            bail!("--group cannot be used with --type project (the entry module has no package scope).");
        }
        args.name.clone().unwrap_or_else(|| directory_name(cwd))
    } else {
        let base = args
            .name
            .clone()
            .unwrap_or_else(|| directory_name(cwd).to_lowercase());
        let name = apply_group(base, args.group.as_deref());
        check_package_name(&name)?;
        name
    };

    Ok(InitOptions {
        kind,
        dir,
        name,
        bundle_name: args.bundle_name.clone(),
        api_version: args.api_version.clone(),
        force: args.force,
    })
}

/// Write the manifest or scaffold the project.
fn execute(options: &InitOptions) -> Result<()> {
    if options.kind == InitKind::Package {
        check_package_name(&options.name)?;
        let (path, pretty) = write_package_manifest(&options.dir, &options.name)?;
        output::output(&format!("Wrote to {}:\n", path.display()));
        output::output(&pretty);
        return Ok(());
    }

    let plan = scaffold::plan(options)?;
    scaffold::write(options, &plan)?;

    output::output(&format!(
        "Created {} project in {}",
        options.kind.title(),
        options.dir.display()
    ));
    output::output(&format!("  {} files written", plan.files.len()));
    if options.kind != InitKind::Project {
        output::output(&format!("  package name: {}", options.name));
    }
    if let Some(bundle) = plan.bundle_name.as_deref() {
        output::output(&format!("  bundle name: {bundle}"));
    }
    output::output("");
    output::output("Next step:");
    output::output(&format!(
        "  cd {} && ohpm-rs install",
        quote_path(&options.dir)
    ));
    Ok(())
}

fn write_package_manifest(dir: &Path, name: &str) -> Result<(PathBuf, String)> {
    if dir.exists() && !dir.is_dir() {
        bail!("{} is not a directory.", dir.display());
    }
    std::fs::create_dir_all(dir).map_err(|e| anyhow!("failed to create {}: {e}", dir.display()))?;
    let manifest = serde_json::json!({
        "name": name,
        "version": "1.0.0",
        "description": "",
        "main": "index.ets",
        "author": "",
        "license": "ISC",
        "dependencies": {},
    });
    let pretty = serde_json::to_string_pretty(&manifest)?;
    let path = dir.join(MY_PACKAGE_JSON);
    std::fs::write(&path, format!("{pretty}\n"))?;
    Ok((path, pretty))
}

pub(crate) fn check_package_name(name: &str) -> Result<()> {
    if !is_standard_package_name(name) {
        return Err(anyhow!(
            "The package name \"{name}\" is invalid. It must be a valid ohpm package name \
             (lowercase letters, digits, '-', '_', '.'; at most 128 chars)."
        ));
    }
    Ok(())
}

pub(crate) fn apply_group(name: String, group: Option<&str>) -> String {
    match group {
        Some(group) => {
            let local = name
                .strip_prefix('@')
                .and_then(|s| s.split_once('/'))
                .map(|(_, local)| local)
                .unwrap_or(&name);
            format!("@{group}/{local}")
        }
        None => name,
    }
}

pub(crate) fn existing_manifest_name(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(MY_PACKAGE_JSON)).ok()?;
    let manifest = ohpm_core::package::Manifest::from_json5(&text).ok()?;
    (!manifest.name.is_empty()).then_some(manifest.name)
}

pub(crate) fn directory_name(dir: &Path) -> String {
    dir.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unnamed".to_string())
}

fn target_dir(args: &InitArgs, cwd: &Path) -> PathBuf {
    let path = match &args.path {
        Some(path) if path.is_absolute() => path.clone(),
        Some(path) => cwd.join(path),
        None => cwd.to_path_buf(),
    };
    normalize_path(path)
}

/// Lexically remove `.` components (and resolve `..` where possible) without
/// touching the filesystem: the target directory often does not exist yet.
pub(crate) fn normalize_path(path: PathBuf) -> PathBuf {
    use std::path::Component;

    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(component.as_os_str());
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn quote_path(path: &Path) -> String {
    let text = path.display().to_string();
    if text.contains(char::is_whitespace) {
        format!("\"{text}\"")
    } else {
        text
    }
}
