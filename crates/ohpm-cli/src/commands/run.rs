//! `ohpm run <alias> [-- <args...>]`.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use ohpm_core::config::{default::types, Config};
use ohpm_core::scripts::{ScriptRunner, WorkspaceScriptRunner};
use ohpm_core::workspace::{Workspace, WORKSPACE_CONFIG};

use crate::cli::RunArgs;

pub async fn run(args: &RunArgs) -> Result<()> {
    RunCommand { args }.execute()
}

struct RunCommand<'a> {
    args: &'a RunArgs,
}

impl RunCommand<'_> {
    fn execute(&self) -> Result<()> {
        let start = std::time::Instant::now();
        let cwd = std::env::current_dir()?;
        if self.args.workspace || !self.args.filter.is_empty() {
            self.run_workspace(&cwd)?;
        } else {
            let runner = ScriptRunner::new(&cwd, self.args.prefix.as_deref())?;
            self.configure_logger(runner.root())?;
            runner
                .run(&self.args.alias, &self.args.args)
                .with_context(|| {
                    format!(
                        "Failed to run script \"{}\" in {}",
                        self.args.alias,
                        runner.root().display()
                    )
                })?;
        }
        let elapsed = start.elapsed().as_millis();
        super::output::succeed(&format!(
            "\n> Run script completed in {}s {}ms",
            elapsed / 1000,
            elapsed % 1000
        ));
        Ok(())
    }

    fn run_workspace(&self, cwd: &Path) -> Result<()> {
        let anchor = cwd.join(self.args.prefix.as_deref().unwrap_or(Path::new(".")));
        if !anchor.is_dir() {
            anyhow::bail!(
                "The workspace prefix is not a directory: {}",
                anchor.display()
            );
        }
        let workspace = Workspace::find(&anchor)?.ok_or_else(|| {
            anyhow!("No {WORKSPACE_CONFIG} found walking up from {}; --workspace/--filter require a workspace.", anchor.display())
        })?;
        self.configure_logger(&workspace.root)?;
        let count = WorkspaceScriptRunner::new(&workspace).run(
            &self.args.alias,
            &self.args.args,
            &self.args.filter,
        )?;
        super::output::output(&format!("Ran script in {count} workspace package(s)"));
        Ok(())
    }

    fn configure_logger(&self, root: &Path) -> Result<()> {
        let mut config = Config::new();
        config.load(root, None)?;
        if let Some(level) = &self.args.log_level {
            config.set_cli(types::LOG_LEVEL, level);
        }
        let level = config.get_string(types::LOG_LEVEL);
        crate::logger::set_level(if self.args.debug { "debug" } else { &level });
        Ok(())
    }
}
