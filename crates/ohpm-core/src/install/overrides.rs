//! Project-level overrides, mirroring `lib/core/overrides/` (the
//! `overrides` field of `oh-package.json5`) and
//! `lib/core/override-dependency-map/` (the `overrideDependencyMap` field).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::constants::MY_PACKAGE_JSON;
use crate::error::{OhpmError, Result};

/// The parsed `overrides` field + the `overrideDependencyMap` manager.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    /// Bare dependency name -> override spec (exact version, range, tag, or
    /// local path).
    pub overrides_map: BTreeMap<String, String>,
    pub override_dep_map: OverrideDepMapManager,
}

impl Overrides {
    /// Read `overrides` / `overrideDependencyMap` from the project manifest.
    pub fn from_manifest(project_root: &Path) -> Result<Option<Overrides>> {
        let path = project_root.join(MY_PACKAGE_JSON);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(_) => return Ok(None),
        };
        let value: serde_json::Value = json5::from_str(&text).map_err(|e| {
            OhpmError::manifest_read_failed(&path, &e.to_string())
        })?;
        Self::from_manifest_value(Some(&value), project_root)
    }

    /// Parse from an already-loaded manifest value (the resolver passes the
    /// parameterized view).
    pub fn from_manifest_value(
        value: Option<&serde_json::Value>,
        project_root: &Path,
    ) -> Result<Option<Overrides>> {
        let Some(value) = value else {
            return Ok(None);
        };
        let overrides = value.get("overrides");
        let map = match overrides {
            None => BTreeMap::new(),
            Some(v) => {
                let Some(obj) = v.as_object() else {
                    return Ok(Some(Overrides {
                        override_dep_map: OverrideDepMapManager::from_manifest_value(value.get("overrideDependencyMap"), project_root)?,
                        ..Default::default()
                    }));
                };
                obj.iter()
                    .map(|(k, v)| {
                        let spec = v.as_str().unwrap_or("").to_string();
                        (k.clone(), spec)
                    })
                    .collect()
            }
        };
        Ok(Some(Overrides {
            overrides_map: map,
            override_dep_map: OverrideDepMapManager::from_manifest_value(value.get("overrideDependencyMap"), project_root)?,
        }))
    }

    /// `resolveSpecWithOverrides` — a bare `Map.get(depName)`.
    pub fn resolve_spec_with_overrides(&self, name: &str) -> Option<&str> {
        self.overrides_map.get(name).map(|s| s.as_str())
    }
}

/// An override-dependency-map entry: the three dependency maps that REPLACE
/// the masked node's own maps.
#[derive(Debug, Clone, Default)]
pub struct OverrideDepEntry {
    pub dependencies: BTreeMap<String, String>,
    pub dev_dependencies: BTreeMap<String, String>,
    pub dynamic_dependencies: BTreeMap<String, String>,
}

/// `OverrideDepMapManager` — keys are `name` or `name@<exactVersion|tag|latest>`
/// (or a local path); values are file paths to JSON5 dep-map files.
#[derive(Debug, Clone, Default)]
pub struct OverrideDepMapManager {
    files: BTreeMap<String, PathBuf>,
    entries: BTreeMap<String, OverrideDepEntry>,
}

impl OverrideDepMapManager {
    pub fn from_manifest_value(v: Option<&serde_json::Value>, project_root: &Path) -> Result<OverrideDepMapManager> {
        let Some(v) = v else {
            return Ok(OverrideDepMapManager::default());
        };
        let Some(obj) = v.as_object() else {
            return Err(OhpmError::new(
                "OverrideDepMapValueTypeError",
                "The value type of \"overrideDependencyMap\" must be an object, for example: {\"liba\": \"./xxx.json5\"}.",
            ));
        };
        let mut manager = OverrideDepMapManager::default();
        for (key, value) in obj {
            let key = resolve_key(key, project_root)?;
            let file_path = match value.as_str() {
                Some(s) if !s.trim().is_empty() => {
                    let p = Path::new(s);
                    let p = if p.is_absolute() {
                        p.to_path_buf()
                    } else {
                        project_root.join(p)
                    };
                    if !p.exists() {
                        return Err(OhpmError::new(
                            "OverrideDepMapFileUnExistError",
                            format!("The overrideDependencyMap file \"{}\" does not exist.", p.display()),
                        ));
                    }
                    p
                }
                _ => {
                    return Err(OhpmError::new(
                        "OverrideDepMapFilePathEmptyError",
                        format!("The \"overrideDependencyMap\" value of \"{key}\" must be a file path."),
                    ));
                }
            };
            let entry = load_dep_map_file(&file_path, project_root)?;
            manager.files.insert(key.clone(), file_path);
            manager.entries.insert(key, entry);
        }
        Ok(manager)
    }

    pub fn has_config(&self) -> bool {
        !self.entries.is_empty()
    }

    /// The resolved keys (`name` | `name@<spec>`).
    pub fn keys(&self) -> std::collections::BTreeSet<String> {
        self.files.keys().cloned().collect()
    }

    /// `getOverrideDepFile` — try `name@version` then `name`.
    pub fn get(&self, name: &str, version: &str) -> Option<&OverrideDepEntry> {
        self.entries
            .get(&format!("{name}@{version}"))
            .or_else(|| self.entries.get(name))
    }

    pub fn need_override(&self, name: &str, version: &str) -> bool {
        self.get(name, version).is_some()
    }

    /// All entries (for the install record).
    pub fn get_entries(&self) -> &BTreeMap<String, OverrideDepEntry> {
        &self.entries
    }

    /// The resolved override config for a node (mask replacement maps).
    pub fn get_config(&self, name: &str, version: &str) -> Option<OverrideDepEntry> {
        self.get(name, version).cloned()
    }
}

/// `resolveKey` — `name` or `name@<exactVersion|standardTag|latest|localPath>`,
/// local specs resolved against the project root (`resolveLocalSpec`).
fn resolve_key(raw: &str, project_root: &Path) -> Result<String> {
    if raw.trim().is_empty() {
        return Err(OhpmError::new(
            "OverrideDepMapKeyEmptyError",
            "The \"overrideDependencyMap\" key cannot be empty.",
        ));
    }
    let (name, spec) = split_key(raw);
    if spec.is_empty() {
        return Ok(name.to_string());
    }
    if crate::install::spec::is_local_dependency(spec) {
        let stripped = spec.strip_prefix("file:").unwrap_or(spec);
        let p = Path::new(stripped);
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            project_root.join(p)
        };
        return Ok(format!("{name}@{}", abs.to_string_lossy()));
    }
    // `validSpec` — exact version, standard tag or latest only.
    if node_semver::Version::parse(spec).is_err()
        && !crate::install::spec::is_standard_tag_dependency(spec)
        && spec != crate::constants::LATEST
    {
        return Err(OhpmError::new(
            "OverrideDepMapInvalidSpecError",
            format!("Invalid specification: \"{spec}\" in \"overrideDependencyMap\"."),
        ));
    }
    Ok(raw.to_string())
}

/// `splitKeyToNameAndSpec` — split at the last `@` (`lastIndexOf`): no `@` or
/// a leading `@` (scoped name) yields no spec; a trailing `@` is stripped.
fn split_key(raw: &str) -> (&str, &str) {
    match raw.rfind('@') {
        Some(i) if i > 0 && i == raw.len() - 1 => (&raw[..i], ""),
        Some(i) if i > 0 => (&raw[..i], &raw[i + 1..]),
        _ => (raw, ""),
    }
}

/// `loadDepMapFile` — read the JSON5 dep-map file, validate its specs.
fn load_dep_map_file(path: &Path, project_root: &Path) -> Result<OverrideDepEntry> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        OhpmError::new(
            "OverrideDepMapFileReadError",
            format!("Failed to read \"{}\": {e}", path.display()),
        )
    })?;
    let value: serde_json::Value = json5::from_str(&text).map_err(|e| {
        OhpmError::new(
            "OverrideDepMapFileParseError",
            format!("Failed to parse \"{}\": {e}", path.display()),
        )
    })?;
    let mut entry = OverrideDepEntry::default();
    for (key, target) in [
        ("dependencies", &mut entry.dependencies),
        ("devDependencies", &mut entry.dev_dependencies),
        ("dynamicDependencies", &mut entry.dynamic_dependencies),
    ] {
        if let Some(map) = value.get(key).and_then(|m| m.as_object()) {
            for (name, spec) in map {
                let spec = spec.as_str().unwrap_or("").to_string();
                *target = BTreeMap::new();
                target.insert(name.clone(), spec);
            }
        }
    }
    // `odm_r2_project_root`-style local resolution — resolved against the
    // project root.
    for map in [&mut entry.dependencies, &mut entry.dev_dependencies, &mut entry.dynamic_dependencies] {
        for (_, spec) in map.iter_mut() {
            if crate::install::spec::is_local_dependency(spec) {
                let stripped = spec.strip_prefix("file:").unwrap_or(spec);
                *spec = crate::workspace::resolve_file_spec(project_root, &format!("file:{stripped}"))
                    .to_string_lossy()
                    .into_owned();
            }
        }
    }
    Ok(entry)
}
