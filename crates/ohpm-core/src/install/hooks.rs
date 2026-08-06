//! Script hooks, mirroring `lib/core/scripts/ScriptRunner.js` — the
//! `hooks` section of `oh-package.json5` (`preInstall`, `postInstall`,
//! `preUninstall`, `postUninstall`...), run synchronously per module root.

use std::path::Path;

use crate::constants::MY_PACKAGE_JSON;
use crate::error::{OhpmError, Result};

/// The lifecycle hook events wired into the install pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    PreInstall,
    PostInstall,
    PreUninstall,
    PostUninstall,
}

impl HookEvent {
    fn name(&self) -> &'static str {
        match self {
            HookEvent::PreInstall => "preInstall",
            HookEvent::PostInstall => "postInstall",
            HookEvent::PreUninstall => "preUninstall",
            HookEvent::PostUninstall => "postUninstall",
        }
    }
}

/// `ScriptRunner.runHook` — run the matching hook script of every module
/// root; a missing `hooks` section is a no-op, a non-zero exit is `HookFail`.
pub fn run_hooks(module_roots: &[&Path], event: HookEvent) -> Result<()> {
    for module_root in module_roots {
        let manifest_path = module_root.join(MY_PACKAGE_JSON);
        let Ok(text) = std::fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(value) = json5::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Some(command) = value
            .get("hooks")
            .and_then(|h| h.get(event.name()))
            .and_then(|c| c.as_str())
        else {
            continue;
        };
        log::info!("> script hook: {}, cwd: {}", event.name(), module_root.display());
        let status = std::process::Command::new(std::env::var("SHELL").unwrap_or_else(|_| "sh".into()))
            .args(["-c", command])
            .current_dir(module_root)
            .status();
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => return Err(OhpmError::hook_fail(module_root, event.name(), s.code())),
            Err(e) => return Err(OhpmError::hook_fail(module_root, event.name(), None).with_detail(&e.to_string())),
        }
    }
    Ok(())
}
