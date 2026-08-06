//! Error types for ohpm-core.
//!
//! Mirrors the error taxonomy of the reference ohpm implementation
//! (`lib/error/errors.js`), flattened into a single `OhpmError` for Rust.

/// The ohpm error type. Carries a stable `code` string (for scripting/exit
/// handling) plus a human-readable message.
#[derive(Debug, Clone, thiserror::Error)]
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

    /// Append a detail line to the message (used when wrapping lower-level
    /// errors without changing the stable code).
    pub fn with_detail(self, detail: &str) -> Self {
        Self {
            code: self.code,
            message: format!("{} ({detail})", self.message),
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

    // ---- install ----------------------------------------------------------

    pub fn git_ls_remote_failed(repo: &str, detail: &str) -> Self {
        Self::new(
            "GitLsRemoteFailed",
            format!("Failed to list references of git repository \"{repo}\": {detail}"),
        )
    }

    pub fn git_ambiguous_ref(repo: &str, prefix: &str, count: usize) -> Self {
        Self::new(
            "GitAmbiguousRef",
            format!(
                "The short commit \"{prefix}\" of repository \"{repo}\" matches {count} commits; use a longer prefix or an exact commit."
            ),
        )
    }

    pub fn git_ref_not_found(repo: &str, spec: &str) -> Self {
        Self::new(
            "GitRefNotFound",
            format!("Could not resolve \"{spec}\" to a commit of \"{repo}\"."),
        )
    }

    pub fn git_semver_no_match(repo: &str, range: &str) -> Self {
        Self::new(
            "GitSemverNoMatch",
            format!("Could not resolve \"{range}\" to a commit of \"{repo}\"."),
        )
    }

    pub fn git_checkout_failed(repo: &str, commit: &str, detail: &str) -> Self {
        Self::new(
            "GitCheckoutFailed",
            format!(
                "Failed to materialize commit \"{commit}\" of \"{repo}\": {detail}"
            ),
        )
    }

    pub fn alias_pkg_invalid(raw: &str) -> Self {
        Self::new(
            "AliasPkgInvalid",
            format!(
                "Invalid alias \"{raw}\": the alias target must be \"ohpm:<pkg>[@<version|range|tag>]\" with a registry spec."
            ),
        )
    }

    pub fn workspace_pkg_not_found(name: &str, spec: &str) -> Self {
        Self::new(
            "WorkspacePkgNotFound",
            format!(
                "\"{name}\" (declared \"{spec}\") is in the dependencies but no package named \"{name}\" is present in the workspace."
            ),
        )
    }

    pub fn workspace_no_matching_version(name: &str, range: &str, versions: &str) -> Self {
        Self::new(
            "WorkspaceNoMatchingVersion",
            format!(
                "No version of \"{name}\" in the workspace matches \"{range}\". Available versions: {versions}"
            ),
        )
    }

    pub fn update_has_version(version: &str) -> Self {
        Self::new(
            "UpdateHasVersion",
            format!("Update arguments must not contain package version specifier \"{version}\"."),
        )
    }

    pub fn uninstall_no_pkg() -> Self {
        Self::new(
            "UninstallNoPkg",
            "Must provide a package name to uninstall in global repository.",
        )
    }

    pub fn uninstall_has_version(version: &str) -> Self {
        Self::new(
            "UninstallHasVersion",
            format!("Uninstall arguments must not contain package version specifier \"{version}\"."),
        )
    }

    pub fn install_field_is_empty(field: &str) -> Self {
        Self::new(
            "FieldISEmptyError",
            format!("{field} cannot be empty"),
        )
    }

    pub fn install_no_match(name: &str, version: &str) -> Self {
        Self::new(
            "InstallNoMatch",
            format!(
                "Couldn't find a version matching \"{version}\" for package \"{name}\"."
            ),
        )
    }

    pub fn install_pkg_to_local_failed() -> Self {
        Self::new(
            "InstallPkgToLocalFailed",
            "Install package to local folder failed.",
        )
    }

    pub fn install_invalid_cli_input_pkg(pkg: &str) -> Self {
        Self::new(
            "InstallInvalidCliInputPkg",
            format!("Handle cli input, invalid pkg: {pkg}"),
        )
    }

    pub fn dep_install_package_size_exceed() -> Self {
        Self::new(
            "DepInstallPackageSizeExceed",
            "The size of the compressed package cannot exceed 500MB",
        )
    }

    pub fn dep_install_directory_traversal(name: &str) -> Self {
        Self::new(
            "DepInstallDirectoryTraversal",
            format!("{name}, cannot contain special characters .. or ../"),
        )
    }

    pub fn dep_install_read_pkg_json_from_tarball(tarball_path: &std::path::Path) -> Self {
        Self::new(
            "DepInstallReadPkgJsonFromTarballError",
            format!("Read package json from {} failed.", tarball_path.display()),
        )
    }

    pub fn dep_install_unknown_dep_type(dep_type: &str, name: &str, spec: &str) -> Self {
        Self::new(
            "DepInstallUnknownDepTypeError",
            format!("Unknown dependency type: {dep_type} of pkg: {name}@{spec}."),
        )
    }

    pub fn dep_builder_same_as_pkg(name: &str, version: &str, dep_name: &str, dep_version: &str) -> Self {
        Self::new(
            "DepBuilderSameAsThePkgName",
            format!(
                "The dependency name cannot be the same as the package name, {name}@{version} -> {dep_name}@{dep_version}."
            ),
        )
    }

    pub fn dep_builder_invalid_dependency(cur: &str, cur_version: &str, root: &str, root_version: &str) -> Self {
        Self::new(
            "DepBuilderInvalidDependency",
            format!(
                "Invalid dependency {cur}@{cur_version} -> {root}@{root_version}. The name of an indirect dependency cannot be the same as the module name."
            ),
        )
    }

    pub fn dep_builder_invalid_dep_version(version: &str, dep_key: &str) -> Self {
        Self::new(
            "DepBuilderInvalidDepVersion",
            format!("The version \"{version}\" of dependency \"{dep_key}\" is invalid"),
        )
    }

    pub fn dep_builder_unknown_ohpa_type(ohpa_type: &str) -> Self {
        Self::new(
            "DepBuilderUnknownOhpaType",
            format!("Unknown ohpa type: \"{ohpa_type}\""),
        )
    }

    pub fn dep_builder_build_dependency_node_failed() -> Self {
        Self::new(
            "DepBuilderBuildDependencyNodeFailed",
            "DependencyNode build failed.",
        )
    }

    pub fn parse_dependency_failed(pkg: &str) -> Self {
        Self::new(
            "ParseDependencyFailed",
            format!(
                "ParsePkg fail, package \"{pkg}\". Check whether the \"dependencies\", \"devDependencies\", \"dynamicDependencies\" in the oh-package.json5 file or the package parameter entered via the command line are correct."
            ),
        )
    }

    pub fn fetcher_registry_fetch_pkg_info_failed(name: &str, spec: &str, registries: &str) -> Self {
        Self::new(
            "FetcherRegistryFetchPkgInfoFailed",
            format!(
                "FetchPackageInfo: \"{name}@{spec}\" failed. NOTFOUND package '{name}@{spec}' not found from all the registries [{registries}]"
            ),
        )
    }

    pub fn fetcher_local_artifact_fetch_local_package_failed(tarball_path: &std::path::Path) -> Self {
        Self::new(
            "FetcherLocalArtifactFetchLocalPackageFailed",
            format!(
                "Fetch local file package error, {} does not exist. Please execute the command: \"ohpm clean\" and try again.",
                tarball_path.display()
            ),
        )
    }

    pub fn fetcher_local_artifact_fetch_metadata_failed(dep_name: &str, tarball_path: &std::path::Path) -> Self {
        Self::new(
            "FetcherLocalArtifactFetchMetadataFailed",
            format!(
                "Fetch dependency \"{dep_name}\" metadata from \"{}\" failed",
                tarball_path.display()
            ),
        )
    }

    pub fn fetcher_local_artifact_not_found_pkg_json(path: &std::path::Path) -> Self {
        Self::new(
            "FetcherLocalArtifactNotFoundPkgJsonInLocalPkg",
            format!(
                "Fetch local package error, the oh-package.json5 file is missing in {}.",
                path.display()
            ),
        )
    }

    pub fn fetcher_source_code_fetch_failed(path: &std::path::Path) -> Self {
        Self::new(
            "FetcherSourceCodeFetchSourceCodeFailed",
            format!("Fetch local folder package error, {} does not exist.", path.display()),
        )
    }

    pub fn locker_invalid_spec(spec: &str) -> Self {
        Self::new(
            "LockerInvalidSpec",
            format!("Invalid specifier value: {spec} in the oh-package-lock.json5"),
        )
    }

    pub fn locker_invalid_specifier(spec_value: &str) -> Self {
        Self::new(
            "LockerInvalidSpecifier",
            format!("Invalid specifier value: {spec_value}"),
        )
    }

    pub fn cache_invalid_package(name: &str, version: &str) -> Self {
        Self::new(
            "CacheInvalidCachePackage",
            format!(
                "The integrity check of package: {name}@{version} failed. 1. Execute \"ohpm clean\" and \"ohpm cache clean\" to clear the cache, and then execute \"ohpm install\". 2. Check whether the integrity of the package in the registry is correct."
            ),
        )
    }

    pub fn ohpa_pkg_invalid() -> Self {
        Self::new(
            "OhpaPkgInvalid",
            "Invalid param of pkg, the pkg type is string.",
        )
    }

    pub fn ohpa_name_empty() -> Self {
        Self::new("OhpaNameEmpty", "Name cannot be empty.")
    }

    pub fn ohpa_name_too_long() -> Self {
        Self::new("OhpaNameTooLong", "Name cannot be longer than 214 characters.")
    }

    pub fn ohpa_name_blank() -> Self {
        Self::new(
            "OhpaNameWithBlank",
            "Name cannot contain leading or trailing spaces.",
        )
    }

    pub fn ohpa_name_uri() -> Self {
        Self::new(
            "OhpaNameUri",
            "Name can only contain URL-friendly characters",
        )
    }

    pub fn ohpa_name_invalid_start() -> Self {
        Self::new("OhpaNameInvalidStart", "Name cannot start with '.' or '_'")
    }

    pub fn ohpa_name_special() -> Self {
        Self::new(
            "OhpaNameWithSpecial",
            "Name can no longer contain special characters in (~'!()*)",
        )
    }

    pub fn ohpa_name_blacklist(name: &str) -> Self {
        Self::new(
            "OhpaNameInBlacklist",
            format!("\"{name}\" is a blacklisted name."),
        )
    }

    pub fn ohpa_spec_uri(spec: &str, raw: &str) -> Self {
        Self::new(
            "OhpaSpecUri",
            format!(
                "Invalid tag name \"{spec}\" of package \"{raw}\": Tags may not have any characters that encodeURIComponent encodes."
            ),
        )
    }

    pub fn tag_pkg_invalid(tag: &str, pkg: &str) -> Self {
        Self::new(
            "TagPkgInvalid",
            format!(
                "Invalid tag \"{tag}\" of package \"{pkg}\": The tag must start with a letter or a number and can only consist of letters, numbers, periods (\".\"), hyphens (\"-\") and underscores (\"_\"), with a maximum length of 60 characters, and cannot be \"latest\"."
            ),
        )
    }

    pub fn file_not_exist(path: &std::path::Path) -> Self {
        Self::new(
            "FileNotExist",
            format!("File: {} does not exist.", path.display()),
        )
    }

    pub fn file_not_found_oh_pkg_json5(pkg_store_dir: &std::path::Path) -> Self {
        Self::new(
            "FileNotFoundOhPkgJson5",
            format!("oh-package.json5 not found in {}", pkg_store_dir.display()),
        )
    }

    pub fn file_symlink_dir_failed(link: &std::path::Path, real: &std::path::Path, msg: &str) -> Self {
        Self::new(
            "FileSymLinkDirFailed",
            format!(
                "Link {} to {} failed. error: {msg}",
                link.display(),
                real.display()
            ),
        )
    }

    pub fn fs_rename_file_error(from: &std::path::Path, to: &std::path::Path) -> Self {
        Self::new(
            "FsRenameFileError",
            format!("Rename {} -> {} Error.", from.display(), to.display()),
        )
    }

    pub fn extract_and_ignore_failed(path: &std::path::Path) -> Self {
        Self::new(
            "ExtractAndIgnoreTargetFolderError",
            format!("An error occurred during extraction {}.", path.display()),
        )
    }

    pub fn executor_number_invalid(param_name: &str, min: i64, max: i64) -> Self {
        Self::new(
            "ExecutorNumberInvalid",
            format!("Expected \"{param_name}\" to be a integer number of [{min}, {max}]"),
        )
    }

    pub fn option_invalid(command: &str, result: &str) -> Self {
        Self::new(
            "OptionInvalid",
            format!("{command}{result}"),
        )
    }

    pub fn invalid_prefix_option(command: &str) -> Self {
        Self::new(
            "InvalidPrefixOption",
            format!("{command} - invalid prefix value. The oh-package.json5 file does not exist in the directory."),
        )
    }

    pub fn dep_node_not_found(name: &str, fetch_spec: &str) -> Self {
        Self::new(
            "DepNodeNotFound",
            format!("The dependency \"{name}@{fetch_spec}\" not found in the dependency final graph"),
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
