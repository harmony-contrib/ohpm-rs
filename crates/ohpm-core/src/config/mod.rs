//! ohpm configuration: `.ohpmrc` files + `OHPM_*` environment overrides.
//!
//! Mirrors `lib/config/config.js` with the addition of an environment layer
//! (see [`env`]) that enables non-interactive, CI-driven publishing.
//!
//! Precedence (highest first): **env > cli > cwd > project > user > default**.

pub mod default;
pub mod env;
pub mod loader;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::constants::{PM_DIR, PM_RC};
use crate::error::{OhpmError, Result};

use self::default::{access_token_type, types, ConfigValue};

/// Config sources in precedence order (highest first).
/// CLI flags win over `OHPM_*` env vars, which win over `.ohpmrc` files.
pub const SOURCE_ORDER: [&str; 6] = ["cli", "env", "cwd", "project", "user", "default"];

pub struct Config {
    /// source name -> (key -> value)
    data: BTreeMap<String, BTreeMap<String, ConfigValue>>,
    user_rc_path: PathBuf,
    loaded: bool,
}

impl Config {
    pub fn new() -> Self {
        let data = SOURCE_ORDER.iter().map(|s| (s.to_string(), BTreeMap::new())).collect();
        Self {
            data,
            user_rc_path: Self::user_rc_path(),
            loaded: false,
        }
    }

    /// Path of the user-level `.ohpmrc` (`~/.ohpm/.ohpmrc`).
    pub fn user_rc_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(PM_DIR)
            .join(PM_RC)
    }

    pub fn user_rc(&self) -> &Path {
        &self.user_rc_path
    }

    /// Load configuration from all sources. `cwd` is the directory from which
    /// project/cwd `.ohpmrc` files are discovered. `cli` is an optional map of
    /// command-line overrides.
    pub fn load(&mut self, cwd: &Path, cli: Option<BTreeMap<String, ConfigValue>>) -> Result<()> {
        if self.loaded {
            return Err(OhpmError::config_not_loaded());
        }

        // 1. defaults
        self.data.get_mut("default").unwrap().extend(default::default_config());

        // 2. cli overrides
        if let Some(cli_map) = cli {
            self.data.get_mut("cli").unwrap().extend(cli_map);
        }

        // 3. project + cwd config files
        let project_root = find_project_root(cwd);
        let local_prefix = find_local_prefix(cwd);
        if let Some(pr) = &project_root {
            if cwd != pr {
                let cwd_rc = cwd.join(PM_RC);
                if cwd_rc.exists() {
                    self.load_file("cwd", &cwd_rc)?;
                }
            }
        }
        let base = project_root.or(local_prefix).unwrap_or_else(|| cwd.to_path_buf());
        let project_rc = base.join(PM_RC);
        if project_rc.exists() {
            self.load_file("project", &project_rc)?;
        }

        // 4. user config
        let user_rc = self.user_rc_path.clone();
        if user_rc.exists() {
            self.load_file("user", &user_rc)?;
        }

        // 5. env overrides (highest priority)
        let env_map = self.data.get_mut("env").unwrap();
        for (key, value) in env::overrides() {
            env_map.insert(key.to_string(), ConfigValue::from(value.as_str()));
        }

        self.loaded = true;
        Ok(())
    }

    fn load_file(&mut self, source: &str, path: &Path) -> Result<()> {
        let map = loader::read_file(path)?;
        self.data.get_mut(source).unwrap().extend(map);
        Ok(())
    }

    /// Get a config value from the highest-precedence source that has it.
    pub fn get(&self, key: &str) -> Option<&ConfigValue> {
        if !self.loaded {
            log::warn!("config not loaded; reading \"{key}\" returns default");
        }
        for source in SOURCE_ORDER {
            if let Some(v) = self.data.get(source).and_then(|m| m.get(key)) {
                return Some(v);
            }
        }
        None
    }

    /// Get a value as a string, defaulting to `""`.
    pub fn get_string(&self, key: &str) -> String {
        self.get(key).map(ConfigValue::as_str).unwrap_or_default()
    }

    /// Get a value as a bool.
    pub fn get_bool(&self, key: &str) -> bool {
        self.get(key).map(ConfigValue::as_bool).unwrap_or(false)
    }

    /// Get a value as an i64.
    pub fn get_number(&self, key: &str) -> i64 {
        self.get(key).map(ConfigValue::as_i64).unwrap_or(0)
    }

    /// All keys present in any source (deduplicated, unordered).
    pub fn effective_keys(&self) -> Vec<String> {
        let mut keys = std::collections::BTreeSet::new();
        for map in self.data.values() {
            for k in map.keys() {
                keys.insert(k.clone());
            }
        }
        keys.into_iter().collect()
    }

    /// Which source currently holds `key` (highest precedence), or `""`.
    pub fn find(&self, key: &str) -> &'static str {
        for source in SOURCE_ORDER {
            if self.data.get(source).is_some_and(|m| m.contains_key(key)) {
                return source;
            }
        }
        ""
    }

    /// Set a command-line override (highest precedence). Used for flags like
    /// `--timeout` that feed config-backed settings.
    pub fn set_cli(&mut self, key: &str, value: &str) {
        self.data
            .get_mut("cli")
            .unwrap()
            .insert(key.to_string(), ConfigValue::from(value));
    }

    /// Set a value in the user config. Token keys (ending in `:_auth` /
    /// `:_read_auth`) additionally update the internal auth-record index,
    /// mirroring `TypeValidate.js`.
    pub fn set(&mut self, key: &str, value: &str) {
        let user = self.data.get_mut("user").unwrap();
        let cv = ConfigValue::from(value);
        if key.ends_with(access_token_type::READ_WRITE) || key.ends_with(access_token_type::READ) {
            let suffix = if key.ends_with(access_token_type::READ_WRITE) {
                access_token_type::READ_WRITE
            } else {
                access_token_type::READ
            };
            // Rebuild the auth-record list for this suffix, capped at MAX_AUTH.
            let mut records: Vec<String> = user
                .get(suffix)
                .map(|v| {
                    v.as_str()
                        .split(',')
                        .map(|s| s.to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            if !records.contains(&key.to_string()) {
                records.push(key.to_string());
                records.truncate(default::MAX_AUTH);
            }
            user.insert(suffix.to_string(), ConfigValue::String(records.join(",")));
        }
        user.insert(key.to_string(), cv);
    }

    /// Delete a key from the user config.
    pub fn delete(&mut self, key: &str) -> bool {
        self.data
            .get_mut("user")
            .is_some_and(|m| m.remove(key).is_some())
    }

    /// Persist the user config to `~/.ohpm/.ohpmrc`. The internal auth-record
    /// index keys (`:_auth`, `:_read_auth`) are not persisted, matching
    /// `Config.toObject()` in the reference.
    pub fn save(&self) -> Result<()> {
        if !self.loaded {
            return Err(OhpmError::config_not_loaded());
        }
        let user = self.data.get("user").unwrap();
        let mut out = BTreeMap::new();
        for (k, v) in user {
            if k != access_token_type::READ && k != access_token_type::READ_WRITE {
                out.insert(k.clone(), v.clone());
            }
        }
        loader::write_file(&self.user_rc_path, &out)
    }

    /// Access token for `registry` with the given access-token suffix.
    pub fn access_token(&self, registry: &str, read_write: bool) -> String {
        let suffix = if read_write {
            access_token_type::READ_WRITE
        } else {
            access_token_type::READ
        };
        let key = format!("{}{}", strip_protocol(registry), suffix);
        self.get_string(&key)
    }

    /// Convenience: the effective `registry` (defaults to the public registry).
    pub fn registry(&self) -> String {
        let r = self.get_string(types::REGISTRY);
        if r.is_empty() {
            crate::constants::DEFAULT_REGISTRY.to_string()
        } else {
            ensure_trailing_slash(&r)
        }
    }

    /// Convenience: the effective `publish_registry` (may be empty).
    pub fn publish_registry(&self) -> String {
        let r = self.get_string(types::PUBLISH_REGISTRY);
        if r.is_empty() {
            r
        } else {
            ensure_trailing_slash(&r)
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}

/// Strip the `http:`/`https:` scheme prefix from a registry URL, mirroring
/// `getAuth.js`: `https://host/path/` -> `//host/path/`.
pub fn strip_protocol(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("http:") {
        rest.to_string()
    } else if let Some(rest) = url.strip_prefix("https:") {
        rest.to_string()
    } else {
        url.to_string()
    }
}

/// Ensure a registry URL ends with `/`.
pub fn ensure_trailing_slash(url: &str) -> String {
    if url.ends_with('/') {
        url.to_string()
    } else {
        format!("{url}/")
    }
}

/// Walk up from `dir` to find the nearest ancestor (inclusive) containing
/// `build-profile.json5` with a `modules` field (a OpenHarmony project root).
pub fn find_project_root(dir: &Path) -> Option<PathBuf> {
    let mut cur = dir.to_path_buf();
    loop {
        let bp = cur.join(crate::constants::BUILD_PROFILE);
        if bp.exists() {
            if let Ok(text) = std::fs::read_to_string(&bp) {
                if let Ok(v) = json5::from_str::<serde_json::Value>(&text) {
                    if v.get("modules").is_some() {
                        return Some(cur);
                    }
                }
            }
        }
        if !cur.pop() {
            return None;
        }
    }
}

/// Walk up from `dir` to find the nearest ancestor (inclusive) containing
/// `oh-package.json5`.
pub fn find_local_prefix(dir: &Path) -> Option<PathBuf> {
    let mut cur = dir.to_path_buf();
    loop {
        if cur.join(crate::constants::MY_PACKAGE_JSON).exists() {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// Serializes tests that mutate the global `OHPM_*` env (parallel tests
    /// would otherwise race on `std::env`).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn precedence_env_over_user() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = TempDir::new().unwrap();
        // Force a deterministic user rc path via a temp HOME.
        let home = TempDir::new().unwrap();
        std::env::set_var("OHPM_REGISTRY", "https://env.example/");
        let mut cfg = Config::new();
        cfg.user_rc_path = home.path().join(".ohpm/.ohpmrc");
        write(&cfg.user_rc_path, "registry=https://user.example/\n");
        cfg.load(&dir.path().to_path_buf(), None).unwrap();
        assert_eq!(cfg.get_string(types::REGISTRY), "https://env.example/");
        std::env::remove_var("OHPM_REGISTRY");
    }

    #[test]
    fn precedence_project_over_user() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = TempDir::new().unwrap();
        let project = TempDir::new().unwrap();
        let cwd = project.path().join("module");
        fs::create_dir_all(&cwd).unwrap();
        write(&project.path().join(PM_RC), "registry=https://project.example/\n");
        // Mark the project root so the .ohpmrc above is picked up.
        write(
            &project.path().join("oh-package.json5"),
            "{ name: \"x\", version: \"1.0.0\" }\n",
        );

        let mut cfg = Config::new();
        cfg.user_rc_path = home.path().join(".ohpm/.ohpmrc");
        write(&cfg.user_rc_path, "registry=https://user.example/\n");
        cfg.load(&cwd, None).unwrap();
        assert_eq!(cfg.get_string(types::REGISTRY), "https://project.example/");
    }

    #[test]
    fn token_round_trip_via_config() {
        let home = TempDir::new().unwrap();
        let dir = TempDir::new().unwrap();
        let mut cfg = Config::new();
        cfg.user_rc_path = home.path().join(".ohpm/.ohpmrc");
        cfg.load(&dir.path().to_path_buf(), None).unwrap();
        cfg.set("//ohpm.openharmony.cn/ohpm/:_auth", "sekrit");
        assert_eq!(cfg.access_token("https://ohpm.openharmony.cn/ohpm/", true), "sekrit");
        // Auth index key is not persisted.
        cfg.save().unwrap();
        let saved = fs::read_to_string(&cfg.user_rc_path).unwrap();
        assert!(saved.contains(":_auth\" = sekrit"), "{saved}");
        assert!(!saved.contains("_auth = //"), "{saved}");
    }

    #[test]
    fn strip_protocol_works() {
        assert_eq!(strip_protocol("https://a/b/"), "//a/b/");
        assert_eq!(strip_protocol("http://a/b/"), "//a/b/");
        assert_eq!(strip_protocol("//a/b/"), "//a/b/");
    }
}
