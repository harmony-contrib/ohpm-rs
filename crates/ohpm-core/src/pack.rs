//! HAR packaging: build a `<name>-<version>.har` from a source directory.
//!
//! A HAR is a gzip-compressed tar with a `package/` prefix containing the
//! module's sources (`oh-package.json5`, `src/**`, `index.ets`, ...). File
//! inclusion follows the hvigor conventions:
//!
//! * tool/generated directories are always excluded (`oh_modules`,
//!   `node_modules`, `.hvigor`, `.git`, ...),
//! * a gitignore-style [`.ohpmignore`](https://developer.huawei.com/consumer/cn/forum/topic/0201145899734297171)
//!   file at the module root filters out additional files.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::constants::{HAR_SUFFIX, MY_PACKAGE_JSON};
use crate::error::{OhpmError, Result};
use crate::package::Manifest;
use crate::publish::validate;

/// Result of a successful pack.
#[derive(Debug, Clone)]
pub struct PackOutcome {
    pub har_path: PathBuf,
    pub entry_count: usize,
    pub size_bytes: u64,
    pub name: String,
    pub version: String,
}

/// Directory names excluded at any depth: tool/generated/build output dirs.
const EXCLUDED_DIRS: [&str; 10] = [
    "oh_modules",
    "node_modules",
    ".hvigor",
    ".git",
    ".idea",
    ".ohpm",
    ".tmp",
    ".cxx", // CMake cache
    "build", // hvigor build outputs
    "target", // Rust build outputs
];

/// File names excluded anywhere in the tree.
fn excluded_file(name: &str) -> bool {
    name == ".DS_Store"
        || name == "oh-package-lock.json5"
        || name == ".ohpmignore"
        || name.to_lowercase().ends_with(HAR_SUFFIX)
}

/// A parsed `.ohpmignore` rule.
struct IgnoreRule {
    negated: bool,
    pattern: glob::Pattern,
    has_slash: bool,
    dir_only: bool,
}

/// Gitignore-style matcher for `.ohpmignore`.
#[derive(Default)]
struct IgnoreMatcher {
    rules: Vec<IgnoreRule>,
}

impl IgnoreMatcher {
    /// Parse an `.ohpmignore` file (missing file -> empty matcher).
    fn from_file(path: &Path) -> Result<Self> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)?;
        let mut rules = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (negated, body) = match line.strip_prefix('!') {
                Some(rest) => (true, rest.trim()),
                None => (false, line),
            };
            if body.is_empty() {
                continue;
            }
            let dir_only = body.ends_with('/');
            let body = body.trim_end_matches('/');
            let Ok(pattern) = glob::Pattern::new(body) else {
                continue;
            };
            rules.push(IgnoreRule {
                negated,
                pattern,
                has_slash: body.contains('/'),
                dir_only,
            });
        }
        Ok(Self { rules })
    }

    /// Whether `rel` (a path relative to the module root) is ignored.
    fn is_ignored(&self, rel: &str) -> bool {
        let mut ignored = false;
        let is_dir = rel.ends_with('/');
        let rel = rel.trim_end_matches('/');
        let base = rel.rsplit('/').next().unwrap_or(rel);
        for rule in &self.rules {
            if rule.dir_only && !is_dir {
                continue;
            }
            let matched = rule.pattern.matches(rel)
                || (!rule.has_slash && rule.pattern.matches(base));
            if matched {
                ignored = !rule.negated;
            }
        }
        ignored
    }
}

/// Pack the module at `source` into `<flat-name>-<version>.har` in
/// `output_dir`. `output_dir` is created if needed.
pub fn pack(source: &Path, output_dir: &Path) -> Result<PackOutcome> {
    let manifest_path = source.join(MY_PACKAGE_JSON);
    let text = std::fs::read_to_string(&manifest_path).map_err(|e| {
        OhpmError::new(
            "ManifestNotFound",
            format!("No {} found in \"{}\": {e}", MY_PACKAGE_JSON, source.display()),
        )
    })?;
    let manifest = Manifest::from_json5(&text)?;
    validate::validate_manifest_basics(&manifest)?;

    let flat_name = manifest.name.replace('/', "-").replace('@', "");
    let har_path = output_dir.join(format!("{flat_name}-{}{}", manifest.version, HAR_SUFFIX));
    if let Some(parent) = har_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let matcher = IgnoreMatcher::from_file(&source.join(".ohpmignore"))?;
    let files = collect_files(source, &matcher)?;

    let f = File::create(&har_path)?;
    let enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
    let mut tar = tar::Builder::new(enc);
    let mut size_bytes = 0u64;
    for (rel, abs) in &files {
        let entry_path = format!("package/{rel}");
        tar.append_path_with_name(abs, &entry_path)
            .map_err(|e| OhpmError::new("PackWriteError", format!("{entry_path}: {e}")))?;
        size_bytes += std::fs::metadata(abs).map(|m| m.len()).unwrap_or(0);
    }
    let enc = tar.into_inner().map_err(|e| OhpmError::new("PackWriteError", e.to_string()))?;
    enc.finish()
        .map_err(|e| OhpmError::new("PackWriteError", e.to_string()))?;

    Ok(PackOutcome {
        har_path,
        entry_count: files.len(),
        size_bytes,
        name: manifest.name,
        version: manifest.version,
    })
}

/// Recursively collect the files to pack, sorted for deterministic output.
fn collect_files(source: &Path, matcher: &IgnoreMatcher) -> Result<Vec<(String, PathBuf)>> {
    let mut out = BTreeMap::new();
    walk(source, source, matcher, &mut out)?;
    Ok(out.into_iter().collect())
}

fn walk(
    root: &Path,
    dir: &Path,
    matcher: &IgnoreMatcher,
    out: &mut BTreeMap<String, PathBuf>,
) -> Result<()> {
    let entries = std::fs::read_dir(dir)?;
    let mut children: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    children.sort();
    for path in children {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().into_owned();
        let is_dir = path.is_dir();

        if is_dir {
            if EXCLUDED_DIRS.contains(&name.as_str()) {
                continue;
            }
            if matcher.is_ignored(&format!("{rel}/")) {
                continue;
            }
            walk(root, &path, matcher, out)?;
        } else {
            if excluded_file(&name) {
                continue;
            }
            if matcher.is_ignored(&rel) {
                continue;
            }
            out.insert(rel, path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive;
    use tempfile::TempDir;

    fn write(dir: &Path, rel: &str, content: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    fn pack_fixture() -> (TempDir, TempDir) {
        let src = TempDir::new().unwrap();
        write(&src.path(), "oh-package.json5", "{ name: \"@ohos-rs/ability\", version: \"1.0.0\" }\n");
        write(&src.path(), "index.ets", "export {};\n");
        write(&src.path(), "src/main/ets/a.ets", "// a\n");
        // default excludes
        write(&src.path(), "oh_modules/dep/index.ets", "// dep\n");
        write(&src.path(), "node_modules/x/index.ets", "// x\n");
        write(&src.path(), ".hvigor/cache/f", "x");
        write(&src.path(), ".DS_Store", "junk");
        write(&src.path(), "nested.har", "not a real har");
        write(&src.path(), "oh-package-lock.json5", "{}");
        let out = TempDir::new().unwrap();
        (src, out)
    }

    #[test]
    fn pack_basic_contents_and_default_excludes() {
        let (src, out) = pack_fixture();
        let result = pack(src.path(), out.path()).unwrap();
        assert_eq!(result.name, "@ohos-rs/ability");
        assert_eq!(result.version, "1.0.0");
        assert_eq!(
            result.har_path.file_name().unwrap().to_string_lossy(),
            "ohos-rs-ability-1.0.0.har"
        );

        let entries = archive::list(&result.har_path).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"package/oh-package.json5"));
        assert!(paths.contains(&"package/index.ets"));
        assert!(paths.contains(&"package/src/main/ets/a.ets"));
        // default excludes
        assert!(!paths.iter().any(|p| p.contains("oh_modules")));
        assert!(!paths.iter().any(|p| p.contains("node_modules")));
        assert!(!paths.iter().any(|p| p.contains(".hvigor")));
        assert!(!paths.iter().any(|p| p.contains(".DS_Store")));
        assert!(!paths.iter().any(|p| p.ends_with(".har") && *p != "package/oh-package.json5"));
        assert!(!paths.iter().any(|p| p.contains("oh-package-lock.json5")));
        assert_eq!(result.entry_count, 3);
    }

    #[test]
    fn ohpmignore_patterns() {
        let (src, out) = pack_fixture();
        write(
            &src.path(),
            ".ohpmignore",
            "# comment\nbuild/\n*.map\ndocs/**\n!docs/keep.md\n",
        );
        write(&src.path(), "build/gen.txt", "x");
        write(&src.path(), "src/bundle.js.map", "x");
        write(&src.path(), "docs/a.md", "x");
        write(&src.path(), "docs/keep.md", "x");

        let result = pack(src.path(), out.path()).unwrap();
        let entries = archive::list(&result.har_path).unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
        assert!(!paths.contains(&"package/build/gen.txt"));
        assert!(!paths.contains(&"package/src/bundle.js.map"));
        assert!(!paths.contains(&"package/docs/a.md"));
        assert!(paths.contains(&"package/docs/keep.md"), "negation must re-include");
    }

    #[test]
    fn pack_invalid_manifest_rejected() {
        let src = TempDir::new().unwrap();
        write(&src.path(), "oh-package.json5", "{ name: \"x\" }\n"); // no version
        let out = TempDir::new().unwrap();
        let err = pack(src.path(), out.path()).unwrap_err();
        assert_eq!(err.code, "VersionEmptyError");
    }
}
