//! Parameter validation and replacement from ohpm's `ParamParser`.

use crate::{OhpmError, Result};

pub(super) fn validate(args: &[String], nested: bool) -> Result<&[String]> {
    if args.iter().any(|arg| arg == "--prefix") {
        return Err(OhpmError::new(
            "PrefixInvalidError",
            "The --prefix option cannot be configured under the scripts node in oh-package.json5!",
        ));
    }
    let args = if nested && !args.is_empty() {
        if args[0] != "--" {
            return Err(OhpmError::new(
                "InvalidParamConfigError",
                "Script parameters must begin with \"--\" to indicate that you need to add or override parameters!",
            ));
        }
        &args[1..]
    } else {
        args
    };
    let mut previous_is_key = false;
    for (index, arg) in args.iter().enumerate() {
        if is_key(arg) {
            previous_is_key = true;
        } else if index == 0 {
            return Err(OhpmError::new(
                "InvalidHeaderParamError",
                "Invalid script param, the first param must start with '-' or '--'",
            ));
        } else if !previous_is_key {
            return Err(OhpmError::new(
                "InvalidParamError",
                "Invalid script param, the valid format is (-key/--key value), (-key/--key=value), (-key/--key=a=b)!",
            ));
        } else {
            previous_is_key = false;
        }
    }
    // JavaScript measures UTF-16 code units, including the joining spaces.
    let len = args
        .iter()
        .map(|arg| arg.encode_utf16().count())
        .sum::<usize>()
        + args.len().saturating_sub(1);
    if len > 2048 {
        return Err(OhpmError::new(
            "InvalidParamLen",
            "The total length of params specified in script alias must be in [1, 2048]!",
        ));
    }
    Ok(args)
}

fn is_key(arg: &str) -> bool {
    arg.strip_prefix("--")
        .or_else(|| arg.strip_prefix('-'))
        .and_then(|name| name.as_bytes().first())
        .is_some_and(|first| first.is_ascii_alphanumeric() || *first == b'_')
}

fn key_name(arg: &str) -> &str {
    let key = arg.split_once('=').map_or(arg, |(key, _)| key);
    key.strip_prefix("--")
        .or_else(|| key.strip_prefix('-'))
        .unwrap_or(key)
}

pub(super) fn merge(mut command: Vec<String>, args: &[String]) -> Vec<String> {
    for arg in args.iter().filter(|arg| is_key(arg)) {
        if let Some(index) = command
            .iter()
            .position(|old| is_key(old) && key_name(old) == key_name(arg))
        {
            let count = if !command[index].contains('=')
                && command.get(index + 1).is_some_and(|next| !is_key(next))
            {
                2
            } else {
                1
            };
            command.drain(index..index + count);
        }
    }
    command.extend_from_slice(args);
    command
}
