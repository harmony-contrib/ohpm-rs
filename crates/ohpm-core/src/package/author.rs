//! Author normalization and validation, mirroring `lib/core/publish/fixer.js`.

use regex::Regex;

use super::manifest::{Author, AuthorValue};
use crate::error::{OhpmError, Result};

// `fixer.js`: name 0..128 chars, email `^[a-zA-Z0-9_\-.]+@[a-zA-Z0-9_\-.]+$`
// max 64, url matches the scheme/host pattern max 1024. Look-around from the
// JS regex is rewritten as explicit length + pattern checks.
fn valid_name(name: &str) -> bool {
    name.chars().count() <= 128
}

fn valid_email(email: &str) -> bool {
    if email.chars().count() > 64 {
        return false;
    }
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    };
    let mut parts = email.split('@');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(a), Some(b), None) => ok(a) && ok(b),
        _ => false,
    }
}

fn valid_url(url: &str) -> bool {
    if url.chars().count() > 1024 {
        return false;
    }
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r"^((https|http|ftp|rtsp|mms)://)?([a-zA-Z0-9\u{4e00}-\u{9fa5}\-]+\.)+([a-zA-Z0-9\u{4e00}-\u{9fa5}\-]+)(:[0-9]{1,5})?([/\?].*)?$",
        )
        .unwrap()
    });
    re.is_match(url)
}

fn check_length(field: &str, value: &str, max: usize) -> Result<()> {
    if value.chars().count() > max {
        return Err(OhpmError::over_maximum_length(field));
    }
    Ok(())
}

fn validate_content(name: &str, email: &str, url: &str) -> Result<()> {
    if !name.is_empty() && !valid_name(name) {
        return Err(OhpmError::invalid_author_content("name"));
    }
    if !email.is_empty() && !valid_email(email) {
        return Err(OhpmError::invalid_author_content("email"));
    }
    if !url.is_empty() && !valid_url(url) {
        return Err(OhpmError::invalid_author_content("url"));
    }
    Ok(())
}

/// Normalize an `author` value into the object form, validating lengths and
/// content. Mirrors `fixAuthorField` + `parseAuthor`.
pub fn fix_author(author: &AuthorValue) -> Result<Author> {
    match author {
        AuthorValue::Object(a) => {
            check_length("author.name", &a.name, 128)?;
            check_length("author.email", &a.email, 64)?;
            check_length("author.url", &a.url, 256)?;
            validate_content(&a.name, &a.email, &a.url)?;
            Ok(a.clone())
        }
        AuthorValue::String(s) => parse_author_string(s),
    }
}

/// Parse `"Name <email> (url)"` into `{ name, email, url }`.
fn parse_author_string(s: &str) -> Result<Author> {
    let mut author = Author::default();

    // email: `<...>`
    if let (Some(start), Some(end)) = (s.find('<'), s.find('>')) {
        let email = &s[start + 1..end];
        author.email = email.trim().to_string();
        check_length("author.email", email, 64)?;
    }
    // url: `(...)`
    if let (Some(start), Some(end)) = (s.find('('), s.find(')')) {
        let url = &s[start + 1..end];
        author.url = url.trim().to_string();
        check_length("author.url", url, 256)?;
    }
    // name: everything before `<` or `(` (whichever comes first).
    let name_end = [s.find('<'), s.find('(')]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(s.len());
    let name = s[..name_end].trim();
    author.name = name.to_string();
    check_length("author.name", name, 128)?;

    validate_content(&author.name, &author.email, &author.url)?;
    Ok(author)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_string() {
        let a = fix_author(&AuthorValue::String("John <john@x.com> (https://john.dev)".into())).unwrap();
        assert_eq!(a.name, "John");
        assert_eq!(a.email, "john@x.com");
        assert_eq!(a.url, "https://john.dev");
    }

    #[test]
    fn parse_no_email() {
        let a = fix_author(&AuthorValue::String("OnlyName".into())).unwrap();
        assert_eq!(a.name, "OnlyName");
        assert!(a.email.is_empty());
    }

    #[test]
    fn object_passthrough() {
        let a = AuthorValue::Object(Author {
            name: "N".into(),
            email: "e@x.y".into(),
            url: "".into(),
        });
        assert_eq!(fix_author(&a).unwrap().name, "N");
    }

    #[test]
    fn bad_email_rejected() {
        let err = fix_author(&AuthorValue::String("A <not-an-email> (x)".into())).unwrap_err();
        assert_eq!(err.code, "InValidAuthorContentError");
    }

    #[test]
    fn overlong_name_rejected() {
        let long = "a".repeat(200);
        let err = fix_author(&AuthorValue::String(long)).unwrap_err();
        assert_eq!(err.code, "OverMaximumLengthError");
    }
}
