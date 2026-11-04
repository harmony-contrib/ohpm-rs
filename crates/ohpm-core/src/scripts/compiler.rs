//! Compile all reachable aliases before executing any shell commands.
//!
//! The splitting, whitespace normalization, last-command parameter forwarding,
//! and short-circuit plan deliberately follow ohpm rather than shell grammar.

use serde_json::Value;

use super::{params, suggestions};
use crate::{OhpmError, Result};

#[derive(Debug)]
pub(super) enum ScriptNode {
    Command { name: String, content: String },
    Alias(Vec<ScriptNode>),
    And,
    Or,
}

pub(super) struct ScriptCompiler<'a> {
    scripts: &'a Value,
    active: Vec<String>,
}

impl<'a> ScriptCompiler<'a> {
    pub(super) fn new(scripts: &'a Value) -> Self {
        Self {
            scripts,
            active: Vec::new(),
        }
    }

    pub(super) fn compile(&mut self, alias: &str, args: &[String]) -> Result<Vec<ScriptNode>> {
        validate_alias(alias)?;
        let args = params::validate(args, !self.active.is_empty())?;
        if self.active.iter().any(|ancestor| ancestor == alias) {
            return Err(OhpmError::new(
                "DirectedCycleError",
                "The script contains circular invocation, please check the configuration!",
            ));
        }
        let script = self.scripts.get(alias).ok_or_else(|| {
            let error = OhpmError::new(
                "ScriptUnExistError",
                format!(
                    "Script alias: {alias}. The script alias does not exist in oh-package.json5!"
                ),
            );
            suggestions::append(
                error,
                alias,
                self.scripts
                    .as_object()
                    .into_iter()
                    .flat_map(|scripts| scripts.keys().map(String::as_str)),
            )
        })?;
        let content = match script {
            Value::String(content) => content.as_str(),
            Value::Null | Value::Bool(false) => "",
            _ => return Err(invalid_script()),
        };
        if content.trim().is_empty() {
            return Err(OhpmError::new(
                "ScriptEmptyError",
                format!("The script \"{alias}\" content is empty!"),
            ));
        }
        if content.contains("ohpm run")
            && content.contains(" -- ")
            && (content.contains("&&") || content.contains("||"))
        {
            return Err(OhpmError::new(
                "ScriptContentError",
                format!("Script alias: {alias}. If the '--' identifier is referenced in the alias, only one 'ohpm run' command can be executed!"),
            ));
        }
        self.active.push(alias.to_owned());
        let parts = split_commands(content);
        let mut nodes = Vec::with_capacity(parts.len());
        for (index, part) in parts.iter().enumerate() {
            nodes.push(match *part {
                "&&" => ScriptNode::And,
                "||" => ScriptNode::Or,
                command => self
                    .compile_command(command, if index + 1 == parts.len() { args } else { &[] })?,
            });
        }
        self.active.pop();
        Ok(nodes)
    }

    fn compile_command(&mut self, content: &str, args: &[String]) -> Result<ScriptNode> {
        let mut tokens = params::merge(
            content.split_whitespace().map(str::to_owned).collect(),
            args,
        );
        let name = tokens.first().ok_or_else(invalid_script)?;
        if name.eq_ignore_ascii_case("ohpm") {
            if tokens.len() < 2 {
                return Err(invalid_script());
            }
            if tokens[1] != "run" {
                return Err(command_error());
            }
            if tokens.len() < 3 {
                return Err(invalid_script());
            }
            return self
                .compile(&tokens[2], &tokens[3..])
                .map(ScriptNode::Alias);
        }
        if name.contains(['(', '[', '{']) {
            return Err(OhpmError::new(
                "InvalidCommandError",
                format!("Invalid command, the command containing '(', '[', '{{' does not exist!, position: {name}"),
            ));
        }
        let name = name.clone();
        if let Some(index) = tokens.iter().position(|token| token == "--") {
            tokens.remove(index);
        }
        let content = tokens.join(" ");
        // ScriptRunner also rejects non-run ohpm commands in a shell fragment.
        for part in content.split("||").flat_map(|part| part.split("&&")) {
            let part = part.trim();
            if part.starts_with("ohpm") && part.split(' ').nth(1) != Some("run") {
                return Err(command_error());
            }
        }
        Ok(ScriptNode::Command { name, content })
    }
}

pub(super) fn validate_alias(alias: &str) -> Result<()> {
    if !alias.as_bytes().first().is_some_and(u8::is_ascii_lowercase) {
        return Err(OhpmError::new(
            "InvalidKey",
            format!(
                "Script alias: {alias}. The initial letter of the script alias must be lowercase!"
            ),
        ));
    }
    if alias.encode_utf16().count() > 128 {
        return Err(OhpmError::new(
            "InvalidKeyLen",
            format!("Script alias: {alias}. The script alias length must be in [1, 128]!"),
        ));
    }
    if !alias
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || b":_-".contains(&c))
    {
        return Err(OhpmError::new(
            "InvalidKeyRegex",
            format!("Script alias: {alias}. The script alias can contain only letters, digits, colons (:), underscores (_), and hyphens (-)!"),
        ));
    }
    Ok(())
}

fn invalid_script() -> OhpmError {
    OhpmError::new(
        "ScriptInvalidError",
        "The script content is invalid, 'command' or 'alias' is empty!",
    )
}

fn command_error() -> OhpmError {
    OhpmError::new(
        "CommandError",
        "Cannot execute ohpm commands other than `ohpm run` in scripts.",
    )
}

fn split_commands(content: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    for or_part in split_unquoted(content, "||") {
        if !parts.is_empty() {
            parts.push("||");
        }
        for (index, and_part) in split_unquoted(or_part, "&&").into_iter().enumerate() {
            if index != 0 {
                parts.push("&&");
            }
            parts.push(and_part);
        }
    }
    parts
}

fn split_unquoted<'a>(content: &'a str, operator: &str) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let (mut single, mut double) = (false, false);
    let mut start = 0;
    let mut chars = content.char_indices();
    while let Some((index, ch)) = chars.next() {
        match ch {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            _ if !single && !double && content[index..].starts_with(operator) => {
                let part = content[start..index].trim();
                if !part.is_empty() {
                    parts.push(part);
                }
                start = index + operator.len();
                chars.next();
            }
            _ => {}
        }
    }
    let tail = content[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_invalid_aliases_parameters_and_script_configurations() {
        let manifest = json!({"scripts": {
            "base": "echo ok", "empty": "  ", "invalid": 42,
            "cycle": "ohpm run cycle", "missing": "ohpm run absent",
            "bare": "ohpm", "noAlias": "ohpm run", "forbidden": "ohpm install",
            "badCommand": "(echo ok)", "badParams": "ohpm run base -key value",
            "badPrefix": "ohpm run base -- --prefix elsewhere",
            "badContent": "ohpm run base -- --key value && echo done"
        }});
        let long_alias = "a".repeat(129);
        let long_argument = format!("--key={}", "a".repeat(2043));
        for (alias, args, code) in [
            ("Uppercase", vec![], "InvalidKey"),
            ("1alias", vec![], "InvalidKey"),
            ("", vec![], "InvalidKey"),
            (long_alias.as_str(), vec![], "InvalidKeyLen"),
            ("bad.name", vec![], "InvalidKeyRegex"),
            ("empty", vec![], "ScriptEmptyError"),
            ("invalid", vec![], "ScriptInvalidError"),
            ("cycle", vec![], "DirectedCycleError"),
            ("missing", vec![], "ScriptUnExistError"),
            ("bare", vec![], "ScriptInvalidError"),
            ("noAlias", vec![], "ScriptInvalidError"),
            ("forbidden", vec![], "CommandError"),
            ("badCommand", vec![], "InvalidCommandError"),
            ("badParams", vec![], "InvalidParamConfigError"),
            ("badPrefix", vec![], "PrefixInvalidError"),
            ("badContent", vec![], "ScriptContentError"),
            ("base", vec!["positional"], "InvalidHeaderParamError"),
            ("base", vec!["--key", "one", "two"], "InvalidParamError"),
            ("base", vec!["--prefix", "elsewhere"], "PrefixInvalidError"),
            ("base", vec![long_argument.as_str()], "InvalidParamLen"),
        ] {
            let args = args.into_iter().map(str::to_owned).collect::<Vec<_>>();
            let error = ScriptCompiler::new(&manifest["scripts"])
                .compile(alias, &args)
                .unwrap_err();
            assert_eq!(error.code, code, "{alias} {args:?}: {error}");
        }
    }

    #[test]
    fn accepts_boundary_lengths_colon_aliases_and_utf16_argument_limit() {
        let alias = format!("a:{}", "Z_9-".repeat(31));
        let manifest = json!({"scripts": {&alias: "echo --x=old"}});
        let arg = format!("--x={}", "😀".repeat(1022));
        ScriptCompiler::new(&manifest["scripts"])
            .compile(&alias, std::slice::from_ref(&arg))
            .unwrap();
        assert_eq!(
            ScriptCompiler::new(&manifest["scripts"])
                .compile(&alias, &[arg + "a"])
                .unwrap_err()
                .code,
            "InvalidParamLen"
        );
        let alias = "a".repeat(128);
        ScriptCompiler::new(&json!({&alias: "echo ok"}))
            .compile(&alias, &[])
            .unwrap();
    }
}
