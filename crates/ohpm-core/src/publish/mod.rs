//! Publish orchestration, mirroring `lib/core/publish/PublishCore.js`.

pub mod meta;
pub mod uploader;
pub mod validate;

use std::path::{Path, PathBuf};

use crate::archive;
use crate::config::{default::types, Config};
use crate::constants;
use crate::error::{OhpmError, Result};
use crate::package::{self, Manifest};
use crate::registry::login::LoginOverrides;
use crate::registry::{auth, RegistryClient};

/// Everything the publish/prepublish flow needs from the caller.
#[derive(Debug, Clone, Default)]
pub struct PublishRequest {
    /// Path to the `.har` or `.tgz` package.
    pub file: String,
    /// `--tag` option (defaults to `latest`).
    pub tag: Option<String>,
    /// `--publish_registry` option (overrides config).
    pub publish_registry: Option<String>,
    /// CLI overrides for the SSH login flow.
    pub login: LoginOverrides,
    /// `--timeout` in milliseconds (overrides config `fetch_timeout`).
    pub timeout: Option<u64>,
    /// The source directory of the published package. Used to resolve `file:`
    /// dependencies in workspace mode (defaults to the current directory).
    pub package_root: Option<PathBuf>,
    /// Validate everything (packing, metadata, auth configuration) but do not
    /// upload — no network requests are made.
    pub dry_run: bool,
}

/// Outcome of a successful publish, surfaced by the CLI.
#[derive(Debug, Clone)]
pub struct PublishOutcome {
    pub name: String,
    pub version: String,
    pub additional_msg: Option<String>,
    /// True when the exact package version was already present in the target
    /// registry and no upload was attempted.
    pub skipped: bool,
    /// True when this was a `--dry-run` (no upload happened).
    pub dry_run: bool,
    /// Total package size in bytes.
    pub pkg_size: u64,
    /// Number of files in the package archive(s).
    pub file_num: usize,
}

/// State gathered during validation, shared by publish and prepublish.
struct PublishContext {
    manifest: Manifest,
    is_tgz: bool,
    har_path: PathBuf,
    hsp_path: Option<PathBuf>,
    cache_dir: PathBuf,
    /// Cache dir holding an auto-packed har for directory inputs.
    extra_cache: Option<PathBuf>,
    size: u64,
    file_num: usize,
    registry: String,
    tag: String,
    login: LoginOverrides,
}

/// Run the full publish flow (validation + upload). With `req.dry_run`, every
/// local step runs (packing, metadata, auth configuration) but nothing is
/// uploaded and no network request is made.
pub async fn publish(client: &RegistryClient, config: &Config, req: &PublishRequest) -> Result<PublishOutcome> {
    let ctx = validate_and_prepare(config, req, true).await?;
    let outcome = if req.dry_run {
        do_publish_dry_run(config, &ctx).await?
    } else {
        do_publish(client, config, &ctx).await?
    };
    cleanup(&ctx);
    Ok(outcome)
}

/// Run validation only, without uploading. Like the reference `prepublish`,
/// no registry is required.
pub async fn prepublish(config: &Config, req: &PublishRequest) -> Result<PublishOutcome> {
    let ctx = validate_and_prepare(config, req, false).await?;
    let name = ctx.manifest.name.clone();
    let version = ctx.manifest.version.clone();
    let pkg_size = ctx.size;
    let file_num = ctx.file_num;
    cleanup(&ctx);
    Ok(PublishOutcome {
        name,
        version,
        additional_msg: None,
        skipped: false,
        dry_run: false,
        pkg_size,
        file_num,
    })
}

/// Publish every (filtered, publishable) workspace member. `base` is the
/// request template (tag/registry/auth/timeout); each member's directory is
/// packed and published with its own `package_root`. `publish: false` members
/// are skipped. Stops at the first failure.
pub async fn publish_workspace(
    client: &RegistryClient,
    config: &Config,
    ws: &crate::workspace::Workspace,
    filter: &[String],
    base: &PublishRequest,
) -> Result<Vec<PublishOutcome>> {
    let mut outcomes = Vec::new();
    for member in select_publishable(ws, filter)? {
        let mut req = base.clone();
        req.file = member.dir.to_string_lossy().into_owned();
        req.package_root = Some(member.dir.clone());
        let outcome = publish(client, config, &req).await?;
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

/// Validate every (filtered, publishable) workspace member without uploading.
pub async fn prepublish_workspace(
    config: &Config,
    ws: &crate::workspace::Workspace,
    filter: &[String],
    base: &PublishRequest,
) -> Result<Vec<PublishOutcome>> {
    let mut outcomes = Vec::new();
    for member in select_publishable(ws, filter)? {
        let mut req = base.clone();
        req.file = member.dir.to_string_lossy().into_owned();
        req.package_root = Some(member.dir.clone());
        let outcome = prepublish(config, &req).await?;
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

fn select_publishable<'a>(
    ws: &'a crate::workspace::Workspace,
    filter: &[String],
) -> Result<Vec<&'a crate::workspace::Member>> {
    let selected = ws.filtered_members(filter)?;
    Ok(selected
        .into_iter()
        .filter(|m| {
            if !m.manifest.publishable() {
                log::info!("skip {}: publish is false", m.manifest.name);
                false
            } else {
                true
            }
        })
        .collect())
}

/// Shared validation for publish and prepublish.
async fn validate_and_prepare(
    config: &Config,
    req: &PublishRequest,
    need_registry: bool,
) -> Result<PublishContext> {
    // 1. path + tag validation
    validate::valid_pkg_path(&req.file)?;
    package::validate::validate_tag(req.tag.as_deref())?;

    // 1b. a directory input is packed into a fresh har first.
    let file = Path::new(&req.file);
    let (file, is_tgz, mut extra_cache) = if file.is_dir() {
        let cache = gen_cache_path(config);
        let outcome = crate::pack::pack(file, &cache)?;
        log::info!(
            "packed {} files from \"{}\" -> {}",
            outcome.entry_count,
            file.display(),
            outcome.har_path.display()
        );
        (outcome.har_path, false, Some(cache))
    } else {
        (file.to_path_buf(), archive::is_tgz_file(&req.file), None)
    };

    // 2. manifest
    let (manifest, mut har_path, hsp_path, cache_dir) = get_manifest(config, &file, is_tgz)?;

    // 3. author fix
    let mut manifest = manifest;

    // 3a. `publish: false` packages cannot be published.
    if !manifest.publishable() {
        return Err(OhpmError::new(
            "PublishForbidden",
            format!(
                "The package \"{}\" is marked \"publish: false\" and cannot be published.",
                manifest.name
            ),
        ));
    }

    package::fix_author(&mut manifest)?;

    // 3a. workspace mode: resolve `file:` dependencies before publishing so the
    // uploaded metadata never references local paths.
    let package_root = req
        .package_root
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let mut rewritten_deps = 0usize;
    if let Some(ws) = crate::workspace::Workspace::find(&package_root)? {
        let n = crate::workspace::process_file_dependencies(&ws, &package_root, &mut manifest)?;
        let n2 = crate::workspace::process_workspace_dependencies(&ws, &package_root, &mut manifest)?;
        let total = n + n2;
        if total > 0 {
            log::info!(
                "workspace mode: rewrote {total} file:/workspace: dependencies in \"{}\"",
                manifest.name
            );
        }
        rewritten_deps = total;
    }

    // 3b. When the workspace rewrite changed the dependencies, the packaged
    // manifest must match the metadata — the registry rejects mismatched
    // package data ("The OHPM package data does not match"). The extracted
    // package is re-packed with the processed manifest. Tool-version/tag
    // fields stay metadata-only, like the reference (patchManifest runs after
    // this point).
    if rewritten_deps > 0 {
        // The extracted package content (plain har: the cache dir itself;
        // tgz: the interface-har extraction dir).
        let content_dir = manifest
            .har_cache
            .clone()
            .unwrap_or_else(|| cache_dir.clone());
        let manifest_text = serde_json::to_string_pretty(&manifest)?;
        std::fs::write(
            content_dir.join(constants::MY_PACKAGE_JSON),
            format!("{manifest_text}\n"),
        )?;
        let repack_dir = extra_cache.get_or_insert_with(|| gen_cache_path(config));
        let outcome = crate::pack::pack(&content_dir, repack_dir)?;
        log::info!(
            "re-packed {} files with the processed manifest -> {}",
            outcome.entry_count,
            outcome.har_path.display()
        );
        har_path = outcome.har_path;
    }

    // 3c. patch tool versions (`_nodeVersion`, `_ohpmVersion`) like the
    // reference `patchManifest`; the registry requires `_ohpmVersion`.
    package::patch_manifest(&mut manifest);

    // 4. publish registry (only publish needs one)
    let registry = if need_registry {
        get_publish_registry(config, req.publish_registry.as_deref())?
    } else {
        String::new()
    };

    // 5. package validation
    let har_pkg = validate::get_pkg_content(&har_path)?;
    validate::validate_pkg_size(&har_pkg)?;
    validate::validate_manifest_basics(&manifest)?;
    validate::validate_name_suffix(&manifest.name)?;
    validate::validate_field_lengths(&manifest)?;
    // Dependency specs must be publishable (no bare local paths, no
    // `file:../`, valid tags, spec length <= 128). Runs after the workspace
    // rewrite so resolved `file:` deps already became versions.
    validate::validate_dependency_specs(&manifest.dependencies)?;
    validate::validate_dependency_specs(&manifest.dynamic_dependencies)?;

    let mut hsp_pkg = None;
    if is_tgz {
        package::validate::validate_package_type_for_tgz(manifest.package_type())?;
        let hsp = hsp_path.as_ref().ok_or_else(OhpmError::package_type_empty)?;
        hsp_pkg = Some(validate::valid_hsp_content(hsp)?);
    }

    let size = har_pkg.size + hsp_pkg.as_ref().map(|p| p.size).unwrap_or(0);
    let file_num = har_pkg.entry_count + hsp_pkg.as_ref().map(|p| p.entry_count).unwrap_or(0);
    let tag = req.tag.clone().unwrap_or_else(|| constants::LATEST.to_string());
    // The reference mutates the manifest with the tag (`getPublishOptions`),
    // so the published version entry carries it.
    manifest.tag = Some(tag.clone());

    Ok(PublishContext {
        manifest,
        is_tgz,
        har_path,
        hsp_path,
        cache_dir,
        extra_cache,
        size,
        file_num,
        registry,
        tag,
        login: req.login.clone(),
    })
}

async fn do_publish(
    client: &RegistryClient,
    config: &Config,
    ctx: &PublishContext,
) -> Result<PublishOutcome> {
    // Authenticate before probing so private publish registries can expose
    // their packuments. If this exact version already exists, publishing is
    // idempotent: return a skipped outcome without building metadata or
    // uploading package bytes.
    let token = auth::resolve_write_token(client.http(), config, &ctx.registry, &ctx.login).await?;
    if client
        .is_version_published(
            &ctx.registry,
            &ctx.manifest.name,
            &ctx.manifest.version,
            &token,
        )
        .await?
    {
        return Ok(PublishOutcome {
            name: ctx.manifest.name.clone(),
            version: ctx.manifest.version.clone(),
            additional_msg: None,
            skipped: true,
            dry_run: false,
            pkg_size: ctx.size,
            file_num: ctx.file_num,
        });
    }

    // Build the metadata document.
    let hsp_meta = match (&ctx.hsp_path, ctx.is_tgz) {
        (Some(hsp), true) => {
            let pkg = validate::get_pkg_content(hsp)?;
            Some(meta::HspMeta {
                integrity: pkg.integrity,
                hsp_type: constants::HSP_TYPE_BUNDLE_APP.to_string(),
            })
        }
        _ => None,
    };
    let har_pkg = validate::get_pkg_content(&ctx.har_path)?;
    let meta_ctx = meta::MetaContext {
        registry: ctx.registry.clone(),
        tag: ctx.tag.clone(),
        har_integrity: har_pkg.integrity,
        hsp: hsp_meta,
        har_abs: Some(ctx.har_path.clone()),
    };
    let mut metadata = meta::build_har_metadata(&ctx.manifest, &meta_ctx)?;

    // Upload with retry + stream->attachment fallback.
    let threshold = config.get_number(types::USE_STREAM_THRESHOLD_SIZE).max(0) as u64;
    let source = uploader::PackageSource {
        har_path: &ctx.har_path,
        hsp_path: ctx.hsp_path.as_deref(),
        size_bytes: ctx.size,
        is_tgz: ctx.is_tgz,
    };
    let result =
        uploader::publish_package(client, &ctx.registry, &token, &mut metadata, &source, threshold)
            .await?;

    let additional_msg = result
        .body
        .get("additionalMsg")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string());

    Ok(PublishOutcome {
        name: ctx.manifest.name.clone(),
        version: ctx.manifest.version.clone(),
        additional_msg,
        skipped: false,
        dry_run: false,
        pkg_size: ctx.size,
        file_num: ctx.file_num,
    })
}

/// Dry run: build the metadata and verify the auth configuration locally, but
/// skip the login request and the upload entirely.
async fn do_publish_dry_run(config: &Config, ctx: &PublishContext) -> Result<PublishOutcome> {
    let har_pkg = validate::get_pkg_content(&ctx.har_path)?;
    let meta_ctx = meta::MetaContext {
        registry: ctx.registry.clone(),
        tag: ctx.tag.clone(),
        har_integrity: har_pkg.integrity,
        hsp: None,
        har_abs: Some(ctx.har_path.clone()),
    };
    let metadata = meta::build_har_metadata(&ctx.manifest, &meta_ctx)?;

    // Auth: a configured token is used as-is; otherwise the SSH-key login
    // inputs must all be present and the key must parse (no network).
    let auth_desc = if !auth::configured_write_token(config, &ctx.registry).is_empty() {
        "access token".to_string()
    } else {
        let login_ctx = crate::registry::login::LoginContext::resolve(config, &ctx.login)?;
        crate::registry::login::validate_key(&login_ctx)?;
        "ssh-key login".to_string()
    };

    log::info!(
        "dry run: {}@{} would be published to {} with tag \"{}\" ({} bytes, {} files, auth: {auth_desc})",
        ctx.manifest.name,
        ctx.manifest.version,
        ctx.registry,
        ctx.tag,
        ctx.size,
        ctx.file_num
    );
    let _ = metadata;

    Ok(PublishOutcome {
        name: ctx.manifest.name.clone(),
        version: ctx.manifest.version.clone(),
        additional_msg: Some(auth_desc),
        skipped: false,
        dry_run: true,
        pkg_size: ctx.size,
        file_num: ctx.file_num,
    })
}

/// Determine the publish registry: `--publish_registry` > config
/// `publish_registry`. Empty is an error (`getAndValidPublishRegistry`).
pub fn get_publish_registry(config: &Config, cli_registry: Option<&str>) -> Result<String> {
    let mut r = cli_registry.map(|s| s.to_string()).unwrap_or_default();
    if r.is_empty() {
        r = config.publish_registry();
    }
    if r.is_empty() {
        return Err(OhpmError::publish_registry_error());
    }
    Ok(crate::config::ensure_trailing_slash(&r))
}

/// Load the manifest and extract the archive into a fresh cache directory.
fn get_manifest(
    config: &Config,
    file: &Path,
    is_tgz: bool,
) -> Result<(Manifest, PathBuf, Option<PathBuf>, PathBuf)> {
    let cache_dir = gen_cache_path(config);

    if is_tgz {
        let (har_entry, hsp_entry) = archive::hsp_detect(file)?.ok_or_else(|| {
            OhpmError::invalid_tgz_file(&file.to_string_lossy())
        })?;
        archive::extract(file, &cache_dir, 0, Some(&[har_entry.clone(), hsp_entry.clone()]))?;
        let har_path = cache_dir.join(&har_entry);
        let hsp_path = cache_dir.join(&hsp_entry);
        let har_cache = cache_dir.join(stem_name(&har_entry));
        let manifest = package::read_manifest_from_archive(&har_path, &har_cache)?;
        Ok((manifest, har_path, Some(hsp_path), cache_dir))
    } else {
        let har_path = file.to_path_buf();
        let manifest = package::read_manifest_from_archive(file, &cache_dir)?;
        Ok((manifest, har_path, None, cache_dir))
    }
}

fn stem_name(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `<cache>/harBall/<uuid>/`
fn gen_cache_path(config: &Config) -> PathBuf {
    let dir = config
        .cache_dir()
        .join("harBall")
        .join(uuid::Uuid::new_v4().simple().to_string());
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn cleanup(ctx: &PublishContext) {
    let _ = std::fs::remove_dir_all(&ctx.cache_dir);
    if let Some(extra) = &ctx.extra_cache {
        let _ = std::fs::remove_dir_all(extra);
    }
}

/// Unpublish request (`lib/core/publish/unpublish.js`).
#[derive(Debug, Clone, Default)]
pub struct UnpublishRequest {
    /// `name[@version]`.
    pub pkg: String,
    /// Unpublish all versions without a specified version.
    pub force: bool,
    /// `--publish_registry` override.
    pub publish_registry: Option<String>,
    pub login: LoginOverrides,
}

/// Delete a package version (or all versions with `--force`) from the
/// publish registry.
pub async fn unpublish(
    client: &RegistryClient,
    config: &Config,
    req: &UnpublishRequest,
) -> Result<()> {
    // Split "name[@version]"; the version separator is the first "@" after a
    // leading "@" scope (mirroring the reference).
    let (name, version) = split_pkg_name(&req.pkg);
    validate_unpublish(&version, req.force)?;

    let registry = get_publish_registry(config, req.publish_registry.as_deref())?;
    let token = auth::resolve_write_token(client.http(), config, &registry, &req.login).await?;

    let url = format!(
        "{}{}",
        registry,
        crate::registry::url_encode_pkg_name(&name)
    );
    let body = serde_json::json!({ "version": version });

    let resp = client
        .http()
        .delete(&url)
        .header("command", "unpublish")
        .header("version", "v1")
        .header("user-agent", constants::user_agent())
        .header("Authorization", token)
        .header("Content-Type", "application/json; charset=UTF-8")
        .body(serde_json::to_vec(&body)?)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(OhpmError::response_status(status.as_u16(), &text));
    }
    Ok(())
}

fn validate_unpublish(version: &str, force: bool) -> Result<()> {
    if version.is_empty() && !force {
        return Err(OhpmError::new(
            "DeleteAllVersionPkgNotForce",
            "A version must be specified to unpublish, or use the \"--force\" option to unpublish \
             all versions.",
        ));
    }
    Ok(())
}

/// Split `@scope/name@1.0.0` into `(@scope/name, 1.0.0)`.
fn split_pkg_name(pkg: &str) -> (String, String) {
    // The version separator is the first "@" after a leading "@" scope.
    // For scoped names the search starts after the scope prefix, so +1.
    let at_pos = if let Some(body) = pkg.strip_prefix('@') {
        body.find('@').map(|i| i + 1).unwrap_or(usize::MAX)
    } else {
        pkg.find('@').unwrap_or(usize::MAX)
    };
    if at_pos < pkg.len() {
        (pkg[..at_pos].to_string(), pkg[at_pos + 1..].to_string())
    } else {
        (pkg.to_string(), String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_pkg_name_handles_scopes() {
        assert_eq!(
            split_pkg_name("@ohos/foo@1.0.0"),
            ("@ohos/foo".to_string(), "1.0.0".to_string())
        );
        assert_eq!(split_pkg_name("com.example.foo"), ("com.example.foo".to_string(), String::new()));
        assert_eq!(split_pkg_name("@ohos/foo"), ("@ohos/foo".to_string(), String::new()));
    }

    #[test]
    fn unpublish_requires_version_or_force() {
        assert!(validate_unpublish("1.0.0", false).is_ok());
        assert!(validate_unpublish("", true).is_ok());
        let err = validate_unpublish("", false).unwrap_err();
        assert_eq!(err.code, "DeleteAllVersionPkgNotForce");
    }
}
