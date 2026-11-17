//! Interactive project wizard (`ohpm init` in a terminal without `--type`).

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use dialoguer::theme::ColorfulTheme;
use dialoguer::{Confirm, Input, Select};

use super::scaffold::{default_bundle_name, InitKind, InitOptions};
use super::{
    apply_group, check_package_name, directory_name, existing_manifest_name, normalize_path,
};
use crate::cli::InitArgs;

const DEFAULT_API_VERSION: &str = "5.0.0(12)";

pub fn prompt(args: &InitArgs, cwd: &Path) -> Result<Option<InitOptions>> {
    let theme = ColorfulTheme::default();

    let kinds = [
        InitKind::Project,
        InitKind::Har,
        InitKind::Hsp,
        InitKind::Package,
    ];
    let choice = Select::with_theme(&theme)
        .with_prompt("What do you want to create?")
        .items([
            "Application project (project)",
            "Static library project (har)",
            "Shared library project (hsp)",
            "Package manifest only (oh-package.json5)",
        ])
        .default(0)
        .interact()
        .context("interactive prompt failed")?;
    let kind = kinds[choice];

    let default_name = args.name.clone().unwrap_or_else(|| {
        if kind == InitKind::Project {
            directory_name(cwd)
        } else {
            package_default_name(cwd)
        }
    });
    let name_prompt = if kind == InitKind::Project {
        "Project name"
    } else {
        "Package name"
    };
    let name = Input::<String>::with_theme(&theme)
        .with_prompt(name_prompt)
        .default(default_name)
        .interact_text()
        .context("interactive prompt failed")?;
    let name = name.trim().to_string();
    if name.is_empty() {
        bail!("The name must not be empty.");
    }

    let mut group = None;
    let mut bundle_name = None;
    let mut api_version = None;

    if kind == InitKind::Project {
        let default_bundle = args
            .bundle_name
            .clone()
            .unwrap_or_else(|| default_bundle_name(&name));
        let bundle = Input::<String>::with_theme(&theme)
            .with_prompt("Bundle name")
            .default(default_bundle)
            .interact_text()
            .context("interactive prompt failed")?;
        let bundle = bundle.trim().to_string();
        if !bundle.is_empty() {
            bundle_name = Some(bundle);
        }
    } else {
        let default_group = existing_manifest_name(cwd)
            .and_then(|name| {
                name.strip_prefix('@')
                    .and_then(|rest| rest.split_once('/'))
                    .map(|(group, _)| group.to_string())
            })
            .or_else(|| args.group.clone())
            .unwrap_or_default();
        let value = Input::<String>::with_theme(&theme)
            .with_prompt("Scope group (optional, e.g. example)")
            .allow_empty(true)
            .default(default_group)
            .interact_text()
            .context("interactive prompt failed")?;
        let value = value.trim().trim_start_matches('@').to_string();
        if !value.is_empty() {
            group = Some(value);
        }
    }

    let name = if kind == InitKind::Project {
        name
    } else {
        let name = apply_group(name, group.as_deref());
        check_package_name(&name)?;
        name
    };

    if matches!(kind, InitKind::Har | InitKind::Hsp | InitKind::Project) {
        let default_api = args
            .api_version
            .clone()
            .unwrap_or_else(|| DEFAULT_API_VERSION.to_string());
        let value = Input::<String>::with_theme(&theme)
            .with_prompt("SDK/API version")
            .default(default_api)
            .interact_text()
            .context("interactive prompt failed")?;
        api_version = Some(value.trim().to_string());
    }

    let default_path = args
        .path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| ".".to_string());
    let path = Input::<String>::with_theme(&theme)
        .with_prompt("Output directory")
        .default(default_path)
        .interact_text()
        .context("interactive prompt failed")?;
    let dir = resolve_dir(cwd, &path);

    let confirm = Confirm::with_theme(&theme)
        .with_prompt(format!("Create {} in {}?", kind.as_str(), dir.display()))
        .default(true)
        .interact()
        .context("interactive prompt failed")?;
    if !confirm {
        println!("Cancelled.");
        return Ok(None);
    }

    Ok(Some(InitOptions {
        kind,
        dir,
        name,
        bundle_name: if kind == InitKind::Project {
            bundle_name
        } else {
            args.bundle_name.clone()
        },
        api_version,
        force: args.force,
    }))
}

fn resolve_dir(cwd: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value.trim());
    let path = if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    };
    normalize_path(path)
}

/// Lowercase the directory name and drop characters that are invalid in a
/// package name, so the default prompt value passes validation.
fn package_default_name(cwd: &Path) -> String {
    if let Some(existing) = existing_manifest_name(cwd) {
        return existing;
    }
    let base = directory_name(cwd).to_lowercase();
    let mut cleaned = String::new();
    for ch in base.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_' || ch == '.' {
            cleaned.push(ch);
        } else if !cleaned.ends_with('-') {
            cleaned.push('-');
        }
    }
    let cleaned = cleaned
        .trim_matches(|c: char| c == '-' || c == '_' || c == '.')
        .to_string();
    if cleaned.is_empty() {
        "unnamed".to_string()
    } else {
        cleaned
    }
}
