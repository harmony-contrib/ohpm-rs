//! The local-artifact mtime/content-hash cache, mirroring
//! `PackageLockerManager`'s `readLocalArtifactMtimeCache` /
//! `writeLocalArtifactMtimeCache`:
//! `~/.ohpm/.mtime/<sha512-hex(projectRoot)>` holding
//! `{ "<absPath>": "<mtimeMs>", "<absPath>#SHA256": "<sha256-base64-lower>" }`.

use std::collections::BTreeMap;
use std::path::Path;

use crate::config::default::default_cache;
use crate::error::Result;
use crate::install::node::file_content_hash;

/// The `#SHA256` suffix of the content-hash cache keys.
pub const SUFFIX_CONTENT_HASH: &str = "#SHA256";

/// The mtime/hash cache for one project root.
#[derive(Debug, Default)]
pub struct MtimeCache {
    inner: BTreeMap<String, String>,
    path: Option<std::path::PathBuf>,
    dirty: bool,
}

impl MtimeCache {
    /// Load the cache file (like the reference's constructor read).
    pub fn load(project_root: &Path) -> MtimeCache {
        Self::load_with_cache_dir(&default_cache().parent().map(|p| p.join(".mtime")).unwrap_or_else(|| Path::new(".").join(".mtime")), project_root)
    }

    /// Test-friendly variant with an explicit cache dir.
    pub fn load_with_cache_dir(cache_dir: &Path, project_root: &Path) -> MtimeCache {
        let path = mtime_cache_path_in(cache_dir, project_root);
        let inner = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| json5::from_str::<BTreeMap<String, String>>(&t).ok())
            .unwrap_or_default();
        MtimeCache {
            inner,
            path: Some(path),
            dirty: false,
        }
    }

    /// `getLocalArtifactMtime` — the cached mtimeMs string of an artifact.
    pub fn get_mtime(&self, path: &Path) -> Option<&str> {
        self.inner.get(path.to_string_lossy().as_ref()).map(|s| s.as_str())
    }

    /// `getLocalArtifactHash` — the cached content hash of an artifact.
    pub fn get_hash(&self, path: &Path) -> Option<&str> {
        self.inner
            .get(&format!("{}{SUFFIX_CONTENT_HASH}", path.to_string_lossy()))
            .map(|s| s.as_str())
    }

    /// `getFileStoreDirName` — reuse the cached hash when the mtime still
    /// matches; otherwise recompute and update the hash entry.
    pub fn get_file_store_dir_name(&mut self, name: &str, abs_path: &Path) -> Result<String> {
        let current_mtime = read_modify_time(abs_path);
        let cached_mtime = self.get_mtime(abs_path);
        let hash = if cached_mtime == Some(current_mtime.as_str()) {
            match self.get_hash(abs_path) {
                Some(h) => h.to_string(),
                None => self.recompute_hash(abs_path)?,
            }
        } else {
            self.recompute_hash(abs_path)?
        };
        Ok(crate::install::node::pkg_store_dir_name(
            name,
            abs_path.to_string_lossy().as_ref(),
            &hash,
        ))
    }

    fn recompute_hash(&mut self, abs_path: &Path) -> Result<String> {
        let hash = file_content_hash(abs_path)?;
        self.inner
            .insert(format!("{}{SUFFIX_CONTENT_HASH}", abs_path.to_string_lossy()), hash.clone());
        self.dirty = true;
        Ok(hash)
    }

    /// `updateGlobalMtimeCacheAfterInstallation` — record the artifact mtime.
    pub fn update_mtime(&mut self, path: &Path, mtime: String) {
        self.inner.insert(path.to_string_lossy().into_owned(), mtime);
        self.dirty = true;
    }

    /// `flushAllLockers` — write the cache when it exists or has entries.
    pub fn save(&self) -> Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if !self.dirty && !path.exists() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(&self.inner)?)?;
        Ok(())
    }
}

/// `FsBlockingUtil.readModifyTime` — the mtimeMs string ("" when missing).
pub fn read_modify_time(path: &Path) -> String {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default()
}

/// `<HOME>/.ohpm/.mtime/<sha512-hex(projectRoot)>`.
pub fn mtime_cache_path(project_root: &Path) -> std::path::PathBuf {
    mtime_cache_path_in(
        &default_cache()
            .parent()
            .map(|p| p.join(".mtime"))
            .unwrap_or_else(|| Path::new(".").join(".mtime")),
        project_root,
    )
}

fn mtime_cache_path_in(cache_dir: &Path, project_root: &Path) -> std::path::PathBuf {
    use sha2::Digest;
    let mut h = sha2::Sha512::new();
    h.update(project_root.to_string_lossy().as_bytes());
    let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    cache_dir.join(hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_path_uses_home_and_sha512() {
        let p = mtime_cache_path(Path::new("/proj"));
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(name.len(), 128, "sha512 hex");
        assert!(name.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(p.to_string_lossy().contains("/.ohpm/.mtime/"));
    }

    #[test]
    fn reuse_vs_recompute() {
        let dir = tempfile::TempDir::new().unwrap();
        let f = dir.path().join("a.har");
        std::fs::write(&f, "content").unwrap();
        let mut cache = MtimeCache::default();
        let first = cache.get_file_store_dir_name("foo", &f).unwrap();
        assert!(first.starts_with("foo@"), "{first}");
        let hash = cache.get_hash(&f).unwrap().to_string();
        assert_eq!(hash, crate::install::node::file_content_hash(&f).unwrap());

        // Same mtime -> reused, no recompute (hash unchanged after write).
        std::fs::write(&f, "content").unwrap();
        let second = cache.get_file_store_dir_name("foo", &f).unwrap();
        assert_eq!(first, second);

        // Changed content -> new hash.
        std::fs::write(&f, "changed").unwrap();
        let third = cache.get_file_store_dir_name("foo", &f).unwrap();
        assert_ne!(first, third);
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let cache_dir = dir.path().join(".mtime");
        let mut cache = MtimeCache::load_with_cache_dir(&cache_dir, dir.path());
        cache.update_mtime(&dir.path().join("a.har"), "123".to_string());
        cache.save().unwrap();
        let loaded = MtimeCache::load_with_cache_dir(&cache_dir, dir.path());
        assert_eq!(loaded.get_mtime(&dir.path().join("a.har")), Some("123"));
    }
}
