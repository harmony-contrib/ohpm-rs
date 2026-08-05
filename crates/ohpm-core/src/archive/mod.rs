//! HAR / HSP / TGZ archive handling: listing and extraction.
//!
//! HAR files are gzip-compressed tars with entries prefixed `package/`.
//! `.tgz` publish bundles contain exactly one `.har` plus one `.hsp`
//! (see `hspDetect.js`).

pub mod integrity;

use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::Path;

use crate::constants::{HAR_SUFFIX, HSP_SUFFIX, TGZ_SUFFIX};
use crate::error::{OhpmError, Result};

/// A single entry discovered by [`list`].
#[derive(Debug, Clone)]
pub struct TarEntry {
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
}

/// Whether `path` refers to a `.tgz` file (`isTgzFile`).
pub fn is_tgz_file(path: &str) -> bool {
    !path.is_empty() && path.to_lowercase().ends_with(TGZ_SUFFIX)
}

fn open_reader(path: &Path) -> Result<tar::Archive<BufReader<flate2::read::GzDecoder<File>>>> {
    let f = File::open(path)?;
    let gz = flate2::read::GzDecoder::new(f);
    Ok(tar::Archive::new(BufReader::new(gz)))
}

/// List all entries of a `.har`/`.tgz` archive.
pub fn list(path: &Path) -> Result<Vec<TarEntry>> {
    let mut ar = open_reader(path)?;
    let mut out = Vec::new();
    for entry in ar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        let size = entry.size();
        let is_dir = entry.header().entry_type().is_dir();
        let _ = &mut entry;
        out.push(TarEntry { path, size, is_dir });
    }
    Ok(out)
}

/// Extract entries from an archive into `dest`.
///
/// * `strip` — number of leading path components to drop (e.g. `package/`).
/// * `file_list` — when `Some`, only extract entries whose original path is
///   in this set (used to pull just the `.har`/`.hsp` out of a `.tgz`).
pub fn extract(path: &Path, dest: &Path, strip: u32, file_list: Option<&[String]>) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    let mut ar = open_reader(path)?;
    for entry in ar.entries()? {
        let mut entry = entry?;
        let raw_path = entry.path()?.to_string_lossy().into_owned();
        if let Some(list) = file_list {
            if !list.iter().any(|p| p == &raw_path) {
                continue;
            }
        }
        let rel = strip_components(&raw_path, strip);
        if rel.is_empty() {
            continue;
        }
        let out_path = dest.join(rel);
        if entry.header().entry_type().is_dir() {
            std::fs::create_dir_all(&out_path)?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = File::create(&out_path)?;
        std::io::copy(&mut entry, &mut f)?;
        f.flush()?;
    }
    Ok(())
}

fn strip_components(path: &str, strip: u32) -> String {
    if strip == 0 {
        return path.to_string();
    }
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if (strip as usize) < parts.len() {
        parts[strip as usize..].join("/")
    } else {
        String::new()
    }
}

/// Detect the `.har` + `.hsp` pair inside a `.tgz` (`hspDetect.js`).
///
/// Returns `Some((interface_har, hsp))` when the archive contains exactly one
/// `.har` and one `.hsp` entry; `None` for non-tgz files or mismatched content.
pub fn hsp_detect(path: &Path) -> Result<Option<(String, String)>> {
    if !is_tgz_file(&path.to_string_lossy()) {
        return Ok(None);
    }
    let entries = list(path)?;
    let mut har: Option<String> = None;
    let mut hsp: Option<String> = None;
    for e in entries {
        if e.path.ends_with(HAR_SUFFIX) {
            har = Some(e.path.clone());
        } else if e.path.ends_with(HSP_SUFFIX) {
            hsp = Some(e.path.clone());
        }
    }
    match (har, hsp) {
        (Some(h), Some(s)) => Ok(Some((h, s))),
        _ => Ok(None),
    }
}

/// Read the whole content of the single `oh-package.json5` inside a har/tgz.
/// Used for a lightweight manifest read without full extraction.
pub fn read_entry_content(path: &Path, entry_path: &str) -> Result<Vec<u8>> {
    let mut ar = open_reader(path)?;
    for entry in ar.entries()? {
        let mut entry = entry?;
        let raw = entry.path()?.to_string_lossy().into_owned();
        if raw == entry_path {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }
    Err(OhpmError::new(
        "EntryNotFound",
        format!("The archive \"{}\" does not contain \"{entry_path}\".", path.display()),
    ))
}

/// Find the `oh-package.json5` entry path inside an archive, accounting for
/// the `package/` prefix.
/// Find the package's own `oh-package.json5` inside an archive.
///
/// The package manifest lives at the archive root (shallowest entry); nested
/// `oh-package.json5` files belong to bundled sub-packages (e.g. NAPI
/// type-stub packages under `src/main/cpp/types/`, which ship their `.d.ts`
/// with the har) and are ignored. Only multiple entries at the same depth are
/// ambiguous.
pub fn find_manifest_entry(path: &Path) -> Result<String> {
    let entries = list(path)?;
    let candidates: Vec<&str> = entries
        .iter()
        .filter(|e| e.path.ends_with(crate::constants::MY_PACKAGE_JSON) && !e.is_dir)
        .map(|e| e.path.as_str())
        .collect();
    if candidates.is_empty() {
        return Err(OhpmError::new(
            "ManifestNotFound",
            format!("No {} found in \"{}\".", crate::constants::MY_PACKAGE_JSON, path.display()),
        ));
    }
    let depth = |p: &str| p.split('/').count();
    let min_depth = candidates.iter().map(|p| depth(p)).min().unwrap();
    let shallowest: Vec<&&str> = candidates.iter().filter(|p| depth(p) == min_depth).collect();
    match shallowest.len() {
        1 => Ok(shallowest[0].to_string()),
        _ => Err(OhpmError::new(
            "ManifestAmbiguous",
            format!("Multiple {} entries found in \"{}\".", crate::constants::MY_PACKAGE_JSON, path.display()),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn build_har_with_manifest(name: &str, version: &str) -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("package/entry")).unwrap();
        let manifest = format!(
            "{{ name: \"{name}\", version: \"{version}\", main: \"index.ets\" }}\n"
        );
        std::fs::write(src.join("package/oh-package.json5"), manifest).unwrap();
        std::fs::write(src.join("package/entry/index.ets"), "export {}").unwrap();

        let har_path = dir.path().join(format!("{name}-{version}.har"));
        let f = File::create(&har_path).unwrap();
        let enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
        let mut tar = tar::Builder::new(enc);
        tar.append_dir_all("package", src.join("package")).unwrap();
        let enc = tar.into_inner().unwrap();
        enc.finish().unwrap();
        (dir, har_path)
    }

    #[test]
    fn find_manifest_prefers_root_over_nested_stub() {
        // A har with a root manifest plus a nested NAPI type-stub package.
        let (dir, har) = build_har_with_manifest("com.example.a", "1.0.0");
        let mut add = |rel: &str, content: &str| {
            let src = dir.path().join("src");
            std::fs::create_dir_all(src.join(rel).parent().unwrap()).unwrap();
            std::fs::write(src.join(rel), content).unwrap();
            let f = File::create(&har).unwrap();
            let enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
            let mut tar = tar::Builder::new(enc);
            tar.append_dir_all("package", src.join("package")).unwrap();
            let enc = tar.into_inner().unwrap();
            enc.finish().unwrap();
        };
        add(
            "package/src/main/cpp/types/libfoo/oh-package.json5",
            "{ name: \"libfoo.so\", version: \"1.0.0\" }\n",
        );
        add("package/src/main/cpp/types/libfoo/Index.d.ts", "declare const x: number;\n");

        let entry = find_manifest_entry(&har).unwrap();
        assert_eq!(entry, "package/oh-package.json5", "the root manifest wins");
    }

    #[test]
    fn list_and_extract_har() {
        let (dir, har) = build_har_with_manifest("com.example.a", "1.0.0");
        let entries = list(&har).unwrap();
        assert!(entries.iter().any(|e| e.path == "package/oh-package.json5"));

        let dest = dir.path().join("out");
        extract(&har, &dest, 1, None).unwrap();
        assert!(dest.join("oh-package.json5").exists());
        assert!(dest.join("entry/index.ets").exists());
    }

    #[test]
    fn hsp_detect_on_tgz() {
        let (dir, har) = build_har_with_manifest("com.example.b", "1.0.0");
        // build a tgz containing the har + a fake hsp
        let tgz_path = dir.path().join("pkg.tgz");
        let f = File::create(&tgz_path).unwrap();
        let enc = flate2::write::GzEncoder::new(f, flate2::Compression::default());
        let mut tar = tar::Builder::new(enc);
        tar.append_file("libs/a.har", &mut File::open(&har).unwrap()).unwrap();
        tar.append_file("libs/b.hsp", &mut File::open(&har).unwrap()).unwrap();
        let enc = tar.into_inner().unwrap();
        enc.finish().unwrap();

        let (har_entry, hsp_entry) = hsp_detect(&tgz_path).unwrap().unwrap();
        assert_eq!(har_entry, "libs/a.har");
        assert_eq!(hsp_entry, "libs/b.hsp");

        // extraction with file_list pulls just those two
        let dest = dir.path().join("out2");
        extract(&tgz_path, &dest, 0, Some(&[har_entry, hsp_entry])).unwrap();
        assert!(dest.join("libs/a.har").exists());
        assert!(dest.join("libs/b.hsp").exists());
    }
}
