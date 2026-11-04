use crate::workspace::Workspace;
use crate::{OhpmError, Result};

use super::{suggestions, ScriptRunner};

/// Execute a script across selected workspace members, including private packages.
pub struct WorkspaceScriptRunner<'a> {
    workspace: &'a Workspace,
}

impl<'a> WorkspaceScriptRunner<'a> {
    pub fn new(workspace: &'a Workspace) -> Self {
        Self { workspace }
    }

    /// Skip members without the alias, validate every selected plan first, and
    /// run in directory order until the first failure. Return the run count.
    pub fn run(&self, alias: &str, args: &[String], filter: &[String]) -> Result<usize> {
        super::compiler::validate_alias(alias)?;
        super::params::validate(args, false)?;
        let mut members = self.workspace.filtered_members(filter)?;
        members.sort_by(|left, right| left.dir.cmp(&right.dir));
        let mut plans = Vec::new();
        for member in &members {
            if member
                .manifest
                .extra
                .get("scripts")
                .and_then(|scripts| scripts.get(alias))
                .is_none()
            {
                continue;
            }
            let runner = ScriptRunner::from_member(member);
            let nodes = runner.compile(alias, args).map_err(|e| {
                e.with_detail(&format!(
                    "workspace package {} at {}",
                    member.manifest.name,
                    member.dir.display()
                ))
            })?;
            plans.push((runner, nodes));
        }
        if plans.is_empty() {
            let error = OhpmError::new(
                "NoWorkspaceScript",
                format!("No selected workspace package defines the script \"{alias}\"."),
            );
            let aliases = members.iter().flat_map(|member| {
                member
                    .manifest
                    .extra
                    .get("scripts")
                    .and_then(|scripts| scripts.as_object())
                    .into_iter()
                    .flat_map(|scripts| scripts.keys().map(String::as_str))
            });
            return Err(suggestions::append(error, alias, aliases));
        }
        for (runner, nodes) in &plans {
            runner.run_compiled(alias, nodes)?;
        }
        Ok(plans.len())
    }
}
