//! Typo hints supplement missing-script errors without changing execution.

use crate::OhpmError;

pub(super) fn append<'a>(
    mut error: OhpmError,
    requested: &str,
    aliases: impl Iterator<Item = &'a str>,
) -> OhpmError {
    let requested_len = requested.chars().count();
    let mut candidates: Vec<_> = aliases
        .filter(|alias| *alias != requested && super::compiler::validate_alias(alias).is_ok())
        .filter_map(|alias| {
            let distance = strsim::damerau_levenshtein(requested, alias);
            let length = requested_len.max(alias.len());
            (distance <= 3 && distance * 3 <= length).then_some((distance, alias))
        })
        .collect();
    candidates.sort_unstable();
    candidates.dedup();
    if !candidates.is_empty() {
        error
            .message
            .push_str("\nDid you mean one of these scripts?");
        for (_, alias) in candidates.into_iter().take(3) {
            error.message.push_str("\n  ");
            error.message.push_str(alias);
        }
    }
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_handle_transpositions_are_ranked_deduplicated_and_limited() {
        let error = append(
            OhpmError::new("ScriptUnExistError", "missing"),
            "biuld",
            [
                "buildz",
                "buildx",
                "build",
                "build",
                "buildy",
                "unrelated",
                "Build",
            ]
            .into_iter(),
        );
        assert_eq!(error.code, "ScriptUnExistError");
        assert_eq!(
            error.message,
            "missing\nDid you mean one of these scripts?\n  build\n  buildx\n  buildy"
        );
    }

    #[test]
    fn unrelated_aliases_do_not_produce_noise() {
        let error = append(
            OhpmError::new("ScriptUnExistError", "missing"),
            "xyz",
            ["build", "test", "lint"].into_iter(),
        );
        assert_eq!(error.message, "missing");
    }
}
