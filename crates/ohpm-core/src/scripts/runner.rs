use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use super::compiler::{ScriptCompiler, ScriptNode};
use crate::config::find_local_prefix;
use crate::constants::MY_PACKAGE_JSON;
use crate::paths::PackagePath;
use crate::{OhpmError, Result};

/// Owns the package root and scripts snapshot for a script invocation.
pub struct ScriptRunner {
    root: PathBuf,
    scripts: Value,
}

impl ScriptRunner {
    /// Discover the nearest package, or use exactly the explicit prefix.
    pub fn new(cwd: &Path, prefix: Option<&Path>) -> Result<Self> {
        let root = match prefix {
            Some(prefix) => {
                let root = cwd.join(prefix);
                if !root.join(MY_PACKAGE_JSON).exists() {
                    return Err(OhpmError::invalid_prefix_option("script"));
                }
                root
            }
            None => find_local_prefix(cwd).ok_or_else(|| {
                OhpmError::new(
                    "PkgUnExistError",
                    "The oh-package.json5 does not exist in the current directory or any parent directory!",
                )
            })?,
        };
        let root = PackagePath::canonicalize(&root)?;
        let path = root.join(MY_PACKAGE_JSON);
        let mut manifest: Value = std::fs::read_to_string(&path)
            .map_err(|e| parse_error(&path, &e.to_string()))
            .and_then(|text| {
                json5::from_str(&text).map_err(|e| parse_error(&path, &e.to_string()))
            })?;
        let scripts = manifest
            .get_mut("scripts")
            .map(Value::take)
            .unwrap_or(Value::Null);
        Ok(Self { root, scripts })
    }

    /// Reuse workspace discovery's snapshot instead of reading the manifest again.
    pub(super) fn from_member(member: &crate::workspace::Member) -> Self {
        Self {
            root: member.dir.clone(),
            scripts: member
                .manifest
                .extra
                .get("scripts")
                .cloned()
                .unwrap_or(Value::Null),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Validate and expand aliases, then run synchronously with inherited IO
    /// and environment. Like ohpm, any unsuccessful script produces exit 1
    /// at the CLI boundary, irrespective of the child's particular exit code.
    pub fn run(&self, alias: &str, args: &[String]) -> Result<()> {
        let nodes = self.compile(alias, args)?;
        self.run_compiled(alias, &nodes)
    }

    pub(super) fn compile(&self, alias: &str, args: &[String]) -> Result<Vec<ScriptNode>> {
        ScriptCompiler::new(&self.scripts).compile(alias, args)
    }

    pub(super) fn run_compiled(&self, alias: &str, nodes: &[ScriptNode]) -> Result<()> {
        if self.execute(nodes)? {
            Ok(())
        } else {
            Err(OhpmError::new(
                "ScriptFail",
                format!(
                    "Run script failed. alias: {alias}, execPath: {}.",
                    self.root.display()
                ),
            ))
        }
    }

    fn execute(&self, nodes: &[ScriptNode]) -> Result<bool> {
        let mut success = false;
        for node in nodes {
            match node {
                // ohpm stops this entire alias at the first short circuit,
                // including later operators of the other kind.
                ScriptNode::And if !success => break,
                ScriptNode::Or if success => break,
                ScriptNode::And | ScriptNode::Or => {}
                ScriptNode::Alias(children) => success = self.execute(children)?,
                ScriptNode::Command { name, content } => {
                    println!("\n> script alias: {name}, cwd: {}", self.root.display());
                    println!("> {content}\n");
                    std::io::stdout().flush()?;
                    success = self
                        .shell(content)
                        .status()
                        .is_ok_and(|status| status.success());
                }
            }
        }
        Ok(success)
    }

    fn shell(&self, content: &str) -> Command {
        #[cfg(windows)]
        let mut command = {
            use std::os::windows::process::CommandExt;

            let shell = std::env::var_os("ComSpec").unwrap_or_else(|| "cmd".into());
            let mut command = Command::new(shell);
            // Match spawnSync's windowsVerbatimArguments for shell syntax.
            command.args(["/d", "/s", "/c"]).raw_arg(content);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let shell = std::env::var_os("SHELL").unwrap_or_else(|| "sh".into());
            let mut command = Command::new(shell);
            command.args(["-c", content]);
            command
        };
        command.current_dir(&self.root);
        command
    }
}

fn parse_error(path: &Path, detail: &str) -> OhpmError {
    OhpmError::new("ParsePkgError", "Failed to parse oh-package.json5!")
        .with_detail(&format!("{}: {detail}", path.display()))
}
