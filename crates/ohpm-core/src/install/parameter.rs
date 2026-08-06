//! Install parameterization, mirroring `lib/core/parameter/` — the
//! `parameterFile` field of `oh-package.json5` (or the `--parameter-file`
//! option) carries a JSON5 file of parameters substituted into the
//! `@param:key.path` markers of the project manifest's support fields
//! (`version`, `dependencies`, `devDependencies`, `dynamicDependencies`,
//! `overrides`).

use std::path::{Path, PathBuf};

use crate::config::{default::types, Config};
use crate::error::{OhpmError, Result};
use crate::install::spec::is_local_dependency;

/// `PREFIX_PARAMETER` — the substitution marker.
pub const PREFIX_PARAMETER: &str = "@param:";
/// `KEY_PARAMETER_FILE_PATH` — the manifest field naming the parameter file.
pub const KEY_PARAMETER_FILE_PATH: &str = "parameterFile";
/// The `@param:` regex (the reference's `REGEX_PARAMETER`).
const REGEX_PARAMETER: &str = r"^@param:.+([.].+)*$";

/// `SUPPORT_PARAMETER_KEY` — the manifest fields that get substituted.
pub const SUPPORT_PARAMETER_KEYS: [&str; 5] = [
    "version",
    "dependencies",
    "devDependencies",
    "dynamicDependencies",
    "overrides",
];

/// `isParameter` — the value (or its serialization) contains a marker.
pub fn is_parameter(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(s) => s.contains(PREFIX_PARAMETER),
        _ => serde_json::to_string(value)
            .map(|s| s.contains(PREFIX_PARAMETER))
            .unwrap_or(false),
    }
}

/// `ParameterizationManager` — the config file + the manifest parser.
#[derive(Debug)]
pub struct Parameterization {
    file: ParameterizationConfigFile,
    /// `parameterizationCache` — name -> parsed manifest.
    cache: std::sync::Mutex<std::collections::BTreeMap<String, serde_json::Value>>,
}

impl Parameterization {
    /// `validParameterFileOptions` + `isParameterFileConfiged` — resolve the
    /// parameter file: the CLI `--parameter-file` (or the `parameter_file`
    /// config) first, then the manifest's `parameterFile` field. `None` when
    /// neither is configured.
    pub fn setup(config: &Config, project_root: &Path, cli_path: Option<&Path>) -> Result<Option<Parameterization>> {
        if let Some(cli) = cli_path.filter(|p| !p.as_os_str().is_empty()) {
            let abs = if cli.is_absolute() {
                cli.to_path_buf()
            } else {
                project_root.join(cli)
            };
            if !abs.is_file() {
                return Err(OhpmError::new(
                    "CliInputParameterFileNotExist",
                    "The parameterFile file entered in the command line does not found!",
                ));
            }
            return Ok(Some(Parameterization {
                file: ParameterizationConfigFile::load(&abs)?,
                cache: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            }));
        }
        // The `parameter_file` config (`.ohpmrc`).
        let configured = config.get_string(types::PARAMETER_FILE);
        if !configured.trim().is_empty() {
            let raw = configured.as_str();
            let p = Path::new(raw);
            let abs = if p.is_absolute() {
                p.to_path_buf()
            } else {
                project_root.join(p)
            };
            if !abs.is_file() {
                return Err(OhpmError::new(
                    "ConfiguredProjectLevelParameterFileNotExist",
                    format!("The parameterFile file \"{}\" does not exist.", abs.display()),
                ));
            }
            return Ok(Some(Parameterization {
                file: ParameterizationConfigFile::load(&abs)?,
                cache: std::sync::Mutex::new(std::collections::BTreeMap::new()),
            }));
        }
        // The manifest `parameterFile` field.
        let text = std::fs::read_to_string(project_root.join(crate::constants::MY_PACKAGE_JSON)).ok();
        let Some(text) = text else {
            return Ok(None);
        };
        let Ok(value) = json5::from_str::<serde_json::Value>(&text) else {
            return Ok(None);
        };
        let Some(raw) = value
            .get(KEY_PARAMETER_FILE_PATH)
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
        else {
            return Ok(None);
        };
        let p = Path::new(raw);
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            project_root.join(p)
        };
        if !abs.is_file() {
            return Err(OhpmError::new(
                "ConfiguredProjectLevelParameterFileNotExist",
                format!("The parameterFile file \"{}\" does not exist.", abs.display()),
            ));
        }
        Ok(Some(Parameterization {
            file: ParameterizationConfigFile::load(&abs)?,
            cache: std::sync::Mutex::new(std::collections::BTreeMap::new()),
        }))
    }

    /// `OhPkgParameterizationParser.parse` — substitute the `@param:` markers
    /// in the manifest's support fields (cached per package name).
    pub fn parse_value(&self, name: &str, value: &mut serde_json::Value) -> Result<()> {
        if !is_parameter(value) {
            return Ok(());
        }
        if let Some(cached) = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(name) {
            *value = cached.clone();
            return Ok(());
        }
        for key in SUPPORT_PARAMETER_KEYS {
            let Some(mut v) = value.get(key).cloned() else {
                continue;
            };
            self.parse_parameter(key, &mut v)?;
            value[key] = v;
        }
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.to_string(), value.clone());
        Ok(())
    }

    /// `parseParameter` — objects recurse into their children; parameterized
    /// leaves are replaced by the resolved value (local values are checked
    /// against the actual package name).
    fn parse_parameter(&self, key: &str, value: &mut serde_json::Value) -> Result<()> {
        if value.is_object() {
            let obj = value.as_object_mut().unwrap();
            let keys: Vec<String> = obj.keys().cloned().collect();
            for k in keys {
                let mut child = obj.get(&k).cloned().unwrap_or_default();
                self.parse_parameter(&k, &mut child)?;
                obj.insert(k, child);
            }
        } else if is_parameter(value) {
            let raw = value.as_str().unwrap_or_default();
            let resolved = self.file.find_version(raw)?;
            if is_local_dependency(&resolved) {
                self.file.valid_consistency_of_dep_name_and_actual_pkg_name(key, &resolved)?;
            }
            *value = serde_json::Value::String(resolved);
        }
        Ok(())
    }

    /// `canModify` — whether the manifest's parameterized fields leave the
    /// given key untouched (a parameterized value under the key cannot be
    /// modified by `ohpm install`/`uninstall`).
    pub fn can_modify(&self, manifest_path: &Path, root_name: &str) -> Result<bool> {
        let text = std::fs::read_to_string(manifest_path).map_err(|_| {
            OhpmError::new("PkgNotFound", "The package root does not exist or is inaccessible.")
        })?;
        let value: serde_json::Value = json5::from_str(&text)?;
        for key in SUPPORT_PARAMETER_KEYS {
            let Some(v) = value.get(key) else {
                continue;
            };
            if !self.recursive_check(key, v, root_name) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// `recursiveCheck` — false when the value under `root_name` is a
    /// parameterized leaf.
    fn recursive_check(&self, key: &str, value: &serde_json::Value, root_name: &str) -> bool {
        if value.is_object() {
            let Some(child) = value.get(root_name) else {
                return true;
            };
            return self.recursive_check(root_name, child, root_name);
        }
        !(key == root_name && is_parameter(value))
    }
}

/// `ParameterizationConfigFile` — the parsed parameter JSON5 file.
#[derive(Debug)]
pub struct ParameterizationConfigFile {
    path: PathBuf,
    parameter_json: serde_json::Value,
}

impl ParameterizationConfigFile {
    /// Parse the parameter file (JSON5 object).
    pub fn load(path: &Path) -> Result<ParameterizationConfigFile> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            OhpmError::new(
                "ReadParameterFileError",
                format!("Failed to read the parameter file \"{}\": {e}", path.display()),
            )
        })?;
        let parameter_json: serde_json::Value = json5::from_str(&text).map_err(|e| {
            OhpmError::new(
                "ParseParameterFileError",
                format!("Failed to parse the parameter file \"{}\": {e}", path.display()),
            )
        })?;
        Ok(ParameterizationConfigFile {
            path: path.to_path_buf(),
            parameter_json,
        })
    }

    /// `findVersion` — resolve `@param:key.path` through the parameter JSON
    /// with the longest-prefix strategy.
    pub fn find_version(&self, raw: &str) -> Result<String> {
        if !self.pre_find(raw) {
            log::warn!("parameterKey '{raw}' is empty or invalid!");
            return Ok(raw.to_string());
        }
        let parts: Vec<&str> = raw[PREFIX_PARAMETER.len()..].split('.').collect();
        let resolved = self.match_longest_prefix(&parts, &self.parameter_json);
        // `postFind` — local deps resolve against the project root.
        let resolved = if is_local_dependency(&resolved) {
            self.resolve_local_dependency_path(&resolved)
        } else {
            resolved
        };
        if resolved.is_empty() {
            return Err(OhpmError::new(
                "ParameterUnExistError",
                format!(
                    "Failed to find the leaf node matching the parameter \"{}\" in \"{}\".",
                    parts.join("."),
                    self.path.display()
                ),
            ));
        }
        Ok(resolved)
    }

    /// `matchValueWithLongestPrefixStrategy` — try the longest key prefixes;
    /// an object found at a longer prefix becomes the base for the next
    /// (shorter) lookup, and objects found below the top recurse with the
    /// remaining parts.
    fn match_longest_prefix(&self, parts: &[&str], root: &serde_json::Value) -> String {
        let mut current: Option<&serde_json::Value> = None;
        for a in (1..=parts.len()).rev() {
            let key = parts[..a].join(".");
            let v = match current {
                Some(c) => c.get(&key),
                None => root.get(&key),
            };
            match v {
                Some(serde_json::Value::String(s)) => return s.clone(),
                Some(obj) if obj.is_object() && a != parts.len() => {
                    let sub = self.match_longest_prefix(&parts[a..], obj);
                    if !sub.is_empty() {
                        return sub;
                    }
                    current = None;
                }
                Some(obj) if obj.is_object() => {
                    // At the top the object becomes the base for shorter keys.
                    current = Some(obj);
                }
                _ => {
                    current = None;
                }
            }
        }
        String::new()
    }

    /// `preFind` — the key must carry the `@param:` marker.
    fn pre_find(&self, key: &str) -> bool {
        let re = regex::Regex::new(REGEX_PARAMETER).unwrap();
        re.is_match(key)
    }

    /// `resolveLocalDependencyPath` — `file:`-prefixed local values resolve
    /// against the parameter file's directory (values are relative to it).
    fn resolve_local_dependency_path(&self, spec: &str) -> String {
        let stripped = spec.trim_start_matches("file:").trim();
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let abs = if Path::new(stripped).is_absolute() {
            PathBuf::from(stripped)
        } else if stripped.is_empty() {
            parent.to_path_buf()
        } else {
            parent.join(stripped)
        };
        abs.to_string_lossy().replace('\\', "/")
    }

    /// `validConsistencyOfDepNameAndActualPkgName` — a local parameter value's
    /// package name must match the dependency key.
    pub fn valid_consistency_of_dep_name_and_actual_pkg_name(&self, dep_name: &str, resolved: &str) -> Result<()> {
        let path = Path::new(resolved);
        let manifest = if path.is_dir() {
            crate::install::resolver::read_manifest_from_dir(path)
        } else {
            crate::install::resolver::read_manifest_from_tar(path)
        }
        .map_err(|_| {
            OhpmError::new(
                "ReadOhPkgJsonError",
                format!(
                    "Failed to read the oh-package.json5 of the parameter \"{dep_name}\" at \"{resolved}\"."
                ),
            )
        })?;
        if dep_name != manifest.name {
            return Err(OhpmError::new(
                "ParameterizationInconsistentDepNames",
                format!(
                    "The dependency \"{dep_name}\" resolves to \"{resolved}\" whose actual package name is \"{}\".",
                    manifest.name
                ),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(content: &str) -> ParameterizationConfigFile {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("params.json5");
        std::fs::write(&path, content).unwrap();
        ParameterizationConfigFile::load(&path).unwrap()
    }

    #[test]
    fn find_version_nested() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("params.json5");
        std::fs::write(&path, "{ version: { \"@ohos/foo\": \"1.2.3\" }, deps: { liba: \"2.0.0\" } }\n").unwrap();
        let f = ParameterizationConfigFile::load(&path).unwrap();
        assert_eq!(f.find_version("@param:version.@ohos/foo").unwrap(), "1.2.3");
        assert_eq!(f.find_version("@param:deps.liba").unwrap(), "2.0.0");
        // A missing leaf resolves as an empty local path against the parameter
        // file's directory (the reference's behavior).
        assert_eq!(
            f.find_version("@param:nope.missing").unwrap(),
            dir.path().to_string_lossy()
        );
        // A key without the marker passes through unchanged (with a warning).
        assert_eq!(f.find_version("plain").unwrap(), "plain");
    }

    #[test]
    fn parse_substitutes_manifest_fields() {
        let dir = tempfile::TempDir::new().unwrap();
        let param_path = dir.path().join("params.json5");
        std::fs::write(&param_path, "{ deps: { foo: \"1.2.3\" } }\n").unwrap();
        let cfg = Config::default();
        let p = Parameterization::setup(&cfg, dir.path(), None).unwrap();
        assert!(p.is_none(), "no manifest -> no parameterization");

        std::fs::write(
            dir.path().join("oh-package.json5"),
            "{ name: \"entry\", version: \"1.0.0\", dependencies: { foo: \"@param:deps.foo\" }, parameterFile: \"./params.json5\" }\n",
        )
        .unwrap();
        let p = Parameterization::setup(&cfg, dir.path(), None).unwrap().unwrap();
        let mut value: serde_json::Value = json5::from_str(
            "{ name: \"entry\", version: \"1.0.0\", dependencies: { foo: \"@param:deps.foo\" } }\n",
        )
        .unwrap();
        p.parse_value("entry", &mut value).unwrap();
        assert_eq!(value["dependencies"]["foo"], "1.2.3");

        // canModify: the parameterized foo cannot be modified by install.
        assert!(!p.can_modify(&dir.path().join("oh-package.json5"), "foo").unwrap());
        assert!(p.can_modify(&dir.path().join("oh-package.json5"), "bar").unwrap());
    }

    #[test]
    fn cli_path_must_exist() {
        let dir = tempfile::TempDir::new().unwrap();
        let cfg = Config::default();
        let err = Parameterization::setup(
            &cfg,
            dir.path(),
            Some(&dir.path().join("missing.json5")),
        )
        .unwrap_err();
        assert_eq!(err.code, "CliInputParameterFileNotExist");
    }
}
