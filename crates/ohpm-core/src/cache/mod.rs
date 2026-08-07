//! The local store (`cache` config) management surface — the pnpm `pnpm store`
//! equivalent. The store is the content-addressed directory shared by every
//! project on the machine (default `~/.ohpm/cache`), holding `content-v1`
//! (downloaded archives keyed by digest) and, in hard-link mode,
//! `extracted-v1` (the shared extracted trees).
//!
//! `status` verifies the content store integrity (≈ `pnpm store status`);
//! `add` pre-fetches packages into the store without installing them
//! (≈ `pnpm store add`).

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::error::{OhpmError, Result};
use crate::install::node::{DepType, Node, NodeData};
use crate::install::packument::{fetch_packument, PackumentCache};
use crate::install::resolver::get_pinned_version;
use crate::install::spec::{parse_dependency, OhpaType};
use crate::install::store::{get_pkg_cache_path, hex_digest, StoreContext};
use crate::registry::RegistryClient;

/// `pnpm store path` — the effective store dir after config resolution
/// (`cache` config → `~/.ohpm/cache`).
pub fn store_path(config: &Config) -> PathBuf {
    config.cache_dir()
}

/// `cleanAllCaches` — remove the content dir, the harball dir and the
/// hard-link `extracted-v1` layer (an ohpm-rs extension; derived content,
/// rebuilt on demand).
pub fn clean_all(config: &Config) -> Result<()> {
    let cache_root = config.cache_dir();
    for dir in ["content-v1", "harball", "extracted-v1"] {
        let path = cache_root.join(dir);
        if path.exists() {
            std::fs::remove_dir_all(&path)?;
        }
    }
    Ok(())
}

/// `pnpm store status` — verify every cached archive in `content-v1` against
/// its path-derived digest. Returns the corrupted files (empty = intact).
///
/// The digest is the last three path components (`<aa>/<bb>/<rest>`); the
/// optional `<alg>/` directory (the reference layout) is ignored. The
/// algorithm follows the digest length (sha1 = 40 hex, sha512 = 128 hex).
pub fn status(config: &Config) -> Result<Vec<PathBuf>> {
    let content_dir = config.cache_dir().join("content-v1");
    if !content_dir.exists() {
        return Ok(Vec::new());
    }
    let mut corrupted = Vec::new();
    let mut chain: Vec<String> = Vec::new();
    walk_content_v1(&content_dir, &mut chain, &mut corrupted)?;
    Ok(corrupted)
}

fn walk_content_v1(dir: &Path, chain: &mut Vec<String>, corrupted: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let ty = entry.file_type()?;
        if ty.is_dir() {
            chain.push(entry.file_name().to_string_lossy().into_owned());
            walk_content_v1(&path, chain, corrupted)?;
            chain.pop();
        } else if ty.is_file() && !verify_cache_file(&path, chain) {
            corrupted.push(path);
        }
    }
    Ok(())
}

/// Re-hash one cached archive and compare with the digest in its path.
fn verify_cache_file(file: &Path, chain: &[String]) -> bool {
    if chain.len() < 2 {
        return false;
    }
    let digest = format!(
        "{}{}{}",
        chain[chain.len() - 2],
        chain[chain.len() - 1],
        file.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    );
    let algo = match digest.len() {
        40 => "sha1",
        128 => "sha512",
        _ => return false,
    };
    if !digest.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    let Ok(bytes) = std::fs::read(file) else {
        return false;
    };
    hex_digest(algo, &bytes) == digest
}

/// `pnpm store add` — pre-fetch `<pkg>[@<spec>]` archives into `content-v1`
/// without installing them. Only registry packages are supported (versions,
/// ranges and `tag:` specs; local/git/workspace inputs are rejected). Returns
/// the number of packages fetched.
pub async fn add(client: &RegistryClient, config: &Config, specs: &[String]) -> Result<u64> {
    let cwd = std::env::current_dir()?;
    let packument_cache = PackumentCache::default();
    let ctx = StoreContext::new(client.clone(), config.clone(), cwd.clone(), 1, true);
    let mut fetched = 0u64;
    for raw in specs {
        let parsed = parse_dependency(raw, &cwd)?;
        if !matches!(parsed.ohpa_type, OhpaType::Version | OhpaType::Range | OhpaType::Tag) {
            return Err(OhpmError::new(
                "CacheAddNotSupport",
                format!(
                    "Not support pre-fetching \"{raw}\" for \"ohpm\" cache add - only registry packages (name[@version | @tag:<tag>]) can be fetched."
                ),
            ));
        }
        let packument = fetch_packument(
            client,
            config,
            &parsed.name,
            &parsed.fetch_spec,
            &packument_cache,
        )
        .await?;
        let pinned = get_pinned_version(&parsed.name, &parsed.fetch_spec, &packument)?;
        let meta = packument.versions.get(&pinned).cloned().unwrap_or_default();
        // The tarball URL and digests: registry packuments carry them under
        // `dist` (the top-level fields are the lockfile form) — the same
        // mapping as `build_dep_node_data` in the resolver.
        let resolved = if meta.resolved.is_empty() {
            meta.dist.as_ref().map(|d| d.tarball.clone()).unwrap_or_default()
        } else {
            meta.resolved.clone()
        };
        let integrity = meta
            .integrity
            .clone()
            .or_else(|| meta.dist.as_ref().and_then(|d| d.integrity.clone()));
        let shasum = meta
            .shasum
            .clone()
            .or_else(|| meta.dist.as_ref().and_then(|d| d.shasum.clone()));
        let node = Node {
            data: std::sync::Arc::new(NodeData {
                name: parsed.name.clone(),
                version: pinned.clone(),
                pinned_spec: pinned.clone(),
                resolved,
                integrity,
                shasum,
                ..Default::default()
            }),
            dep_type: DepType::NoSave,
            requirements: Default::default(),
            masked_by_override_dependency_map: false,
        };
        get_pkg_cache_path(&ctx, &node).await?;
        fetched += 1;
    }
    Ok(fetched)
}
