//! Error types for ohpm-core.
//!
//! Mirrors the error taxonomy of the reference ohpm implementation
//! (`lib/error/errors.js`), flattened into a single `OhpmError` for Rust.

/// The ohpm error type. Carries a stable `code` string (for scripting/exit
/// handling) plus a human-readable message.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct OhpmError {
    /// Stable machine-readable code, e.g. `PublishKeyPathIsEmpty`.
    pub code: &'static str,
    pub message: String,
}

pub type Result<T> = std::result::Result<T, OhpmError>;

impl OhpmError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    // ---- validator -----------------------------------------------------

    pub fn pkg_empty() -> Self {
        Self::new(
            "PkgEmptyError",
            "The package path cannot be empty or a directory.",
        )
    }

    pub fn publish_registry_error() -> Self {
        Self::new(
            "PublishRegistryError",
            "The publish registry cannot be empty. Please configure \"publish_registry\" in the \
             .ohpmrc file, or specify the \"--publish_registry\" option.",
        )
    }

    pub fn tag_invalid(tag: &str) -> Self {
        Self::new(
            "TagInvalidError",
            format!("The tag \"{tag}\" is invalid. It must match ^[A-Za-z0-9][A-Za-z0-9._-]{{0,59}}$ and cannot be \"latest\"."),
        )
    }

    pub fn package_name_invalid(name: &str) -> Self {
        Self::new(
            "PackageNameInvalidError",
            format!(
                "The package name \"{name}\" is invalid. It must match \
                 ^(@(?!\\d|_|-)[a-z0-9\\-_]+(?<![_\\-])\\/)?(?!\\d|_|\\.)[a-z0-9\\-_.]+(?<![\\-_.])$ \
                 and be at most 128 characters."
            ),
        )
    }

    pub fn package_type_empty() -> Self {
        Self::new("PackageTypeEmptyError", "The \"packageType\" field cannot be empty.")
    }

    pub fn package_type_invalid() -> Self {
        Self::new(
            "PackageTypeInvalidError",
            "The \"packageType\" field must be \"InterfaceHar\" for .tgz packages.",
        )
    }

    pub fn invalid_author_content(author: &str) -> Self {
        Self::new(
            "InValidAuthorContentError",
            format!("The content of the \"author.{author}\" field is invalid."),
        )
    }

    pub fn over_maximum_length(field: &str) -> Self {
        Self::new(
            "OverMaximumLengthError",
            format!("The length of the \"{field}\" field exceeds the maximum allowed value."),
        )
    }

    pub fn invalid_tgz_file(path: &str) -> Self {
        Self::new(
            "InvalidTgzFile",
            format!("The file \"{path}\" is not a valid .tgz package: it must contain exactly one \
                     .har file and one .hsp file."),
        )
    }

    pub fn hsp_file_is_empty(path: &str) -> Self {
        Self::new("HspFileIsEmpty", format!("The hsp file \"{path}\" is empty."))
    }

    pub fn build_tgz_metadata_failed(name: &str, version: &str) -> Self {
        Self::new(
            "BuildTgzMetadataFailed",
            format!("Failed to build the metadata of the tgz package \"{name}@{version}\"."),
        )
    }

    pub fn manifest_read_failed(path: &std::path::Path, detail: &str) -> Self {
        Self::new(
            "ReadManifestFailed",
            format!("Failed to read the manifest \"{}\": {detail}", path.display()),
        )
    }

    // ---- publish / auth -------------------------------------------------

    pub fn key_path_is_empty() -> Self {
        Self::new(
            "KeyPathIsEmpty",
            "The \"key_path\" is empty. Please configure it in the .ohpmrc file, or set the \
             OHPM_KEY_PATH environment variable.",
        )
    }

    pub fn private_key_file_not_exist(key_path: &str) -> Self {
        Self::new(
            "PrivateKeyFileNotExist",
            format!("The private key file \"{key_path}\" does not exist."),
        )
    }

    pub fn key_path_is_dir(key_path: &str) -> Self {
        Self::new(
            "KeyPathIsDirError",
            format!("The private key path \"{key_path}\" is a directory, not a file."),
        )
    }

    pub fn publish_id_is_empty() -> Self {
        Self::new(
            "PublishIdIsEmpty",
            "The \"publish_id\" is empty. Please configure it in the .ohpmrc file, or set the \
             OHPM_PUBLISH_ID environment variable.",
        )
    }

    pub fn private_key_content_is_empty(key_path: &str) -> Self {
        Self::new(
            "PrivateKeyContentIsEmpty",
            format!("The content of the private key file \"{key_path}\" is empty."),
        )
    }

    pub fn not_support_private_key(key_path: &str) -> Self {
        Self::new(
            "NotSupportPrivateKey",
            format!("The private key file \"{key_path}\" is not encrypted (the PEM does not contain \
                     the \"ENCRYPTED\" marker). Only encrypted private keys are supported."),
        )
    }

    pub fn key_passphrase_missing() -> Self {
        Self::new(
            "KeyPassphraseMissing",
            "The private key is encrypted but no passphrase is available. Set the \
             OHPM_KEY_PASSPHRASE environment variable or the \"key_passphrase\" config item. \
             Non-interactive mode does not prompt for a passphrase.",
        )
    }

    pub fn signature_failed() -> Self {
        Self::new("SignatureFailed", "Failed to sign the login request with the private key.")
    }

    pub fn login_failed(detail: &str) -> Self {
        Self::new(
            "LoginFailed",
            format!("The login request failed: {detail}"),
        )
    }

    pub fn access_token_missing() -> Self {
        Self::new(
            "AccessTokenMissing",
            "No access token is available for publishing. Set the OHPM_ACCESS_TOKEN environment \
             variable, store the token in the .ohpmrc file as \"//registry/:_auth\", or configure \
             OHPM_PUBLISH_ID / OHPM_KEY_PATH / OHPM_KEY_PASSPHRASE to log in with a private key.",
        )
    }

    // ---- http -----------------------------------------------------------

    pub fn request_failed(detail: &str) -> Self {
        Self::new("RequestFailed", format!("The request failed: {detail}"))
    }

    pub fn response_status(status: u16, body: &str) -> Self {
        Self::new(
            "ResponseStatusError",
            format!("HttpCode {status}, {body}"),
        )
    }

    pub fn pkg_is_locked(pkg: &str) -> Self {
        Self::new(
            "PkgIsLocked",
            format!("The package \"{pkg}\" is locked, and the publish retry limit has been reached."),
        )
    }

    pub fn config_not_loaded() -> Self {
        Self::new("NotLoadedWhenSetting", "The config has not been loaded yet.")
    }

    pub fn config_key_not_exist(key: &str) -> Self {
        Self::new("KeyNotExist", format!("The config key \"{key}\" does not exist."))
    }

    pub fn config_set_param_error() -> Self {
        Self::new("SetCommandParamError", "Usage: ohpm config set <key> <value>")
    }

    pub fn config_get_param_error() -> Self {
        Self::new("GetCommandParamError", "Usage: ohpm config get <key>")
    }

    pub fn config_delete_param_error() -> Self {
        Self::new("DeleteCommandParamError", "Usage: ohpm config delete <key>")
    }

    pub fn config_protected_key(key: &str) -> Self {
        Self::new(
            "ProtectedKey",
            format!("The key \"{key}\" is protected and cannot be read or deleted."),
        )
    }

    pub fn config_subcommand_not_support(sub: &str) -> Self {
        Self::new(
            "SubcommandNotSupport",
            format!("The config subcommand \"{sub}\" is not supported."),
        )
    }
}

impl From<std::io::Error> for OhpmError {
    fn from(e: std::io::Error) -> Self {
        Self::new("IoError", e.to_string())
    }
}

impl From<serde_json::Error> for OhpmError {
    fn from(e: serde_json::Error) -> Self {
        Self::new("JsonError", e.to_string())
    }
}

impl From<json5::Error> for OhpmError {
    fn from(e: json5::Error) -> Self {
        Self::new("Json5Error", e.to_string())
    }
}

impl From<reqwest::Error> for OhpmError {
    fn from(e: reqwest::Error) -> Self {
        Self::new("RequestFailed", e.to_string())
    }
}
