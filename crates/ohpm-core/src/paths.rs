//! Conversion at the package-manifest/lockfile path boundary.

use std::path::{Component, Path, PathBuf};

pub(crate) struct PackagePath;

impl PackagePath {
    /// Reject native prefixes and traversal after stripping the archive's package prefix.
    pub(crate) fn archive_entry(destination: &Path, relative: &str) -> std::io::Result<PathBuf> {
        let path = Path::new(relative);
        if path
            .components()
            .any(|part| !matches!(part, Component::Normal(_) | Component::CurDir))
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "archive entry escapes its destination",
            ));
        }
        Ok(destination.join(path))
    }

    pub(crate) fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
        #[cfg(windows)]
        {
            dunce::canonicalize(path)
        }
        #[cfg(not(windows))]
        {
            path.canonicalize()
        }
    }

    /// Keep paths native until their final manifest/lockfile serialization.
    pub(crate) fn serialize(path: &Path) -> String {
        #[cfg(windows)]
        {
            dunce::simplified(path).to_string_lossy().replace('\\', "/")
        }
        #[cfg(not(windows))]
        {
            path.to_string_lossy().into_owned()
        }
    }

    /// Different Windows volumes/shares cannot be made relative.
    pub(crate) fn relative(from: &Path, to: &Path) -> PathBuf {
        #[cfg(windows)]
        let (from, to) = (dunce::simplified(from), dunce::simplified(to));
        pathdiff::diff_paths(to, from).unwrap_or_else(|| to.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_preserve_spaces_and_unicode() {
        let base = Path::new("workspace space").join("应用");
        let target = Path::new("workspace space").join("库");
        assert_eq!(
            PackagePath::relative(&base, &target),
            Path::new("..").join("库")
        );
    }

    #[test]
    fn archive_entries_stay_within_the_destination() {
        let root = Path::new("destination");
        assert_eq!(
            PackagePath::archive_entry(root, "assets/data").unwrap(),
            root.join("assets/data")
        );
        assert!(PackagePath::archive_entry(root, "../outside").is_err());
        assert!(PackagePath::archive_entry(root, "/outside").is_err());
        #[cfg(windows)]
        for path in [
            r"C:\outside",
            r"..\outside",
            r"\\server\share\outside",
            r"C:outside",
        ] {
            assert!(PackagePath::archive_entry(root, path).is_err(), "{path}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_drives_unc_and_verbatim_paths() {
        assert_eq!(
            PackagePath::relative(Path::new(r"C:\work\app"), Path::new(r"D:\lib")),
            Path::new(r"D:\lib")
        );
        assert_eq!(
            PackagePath::serialize(Path::new(r"\\?\C:\work\库")),
            "C:/work/库"
        );
        // Verbatim UNC paths retain their prefix so long-path semantics survive.
        assert_eq!(
            PackagePath::serialize(Path::new(r"\\?\UNC\server\share\库")),
            "//?/UNC/server/share/库"
        );
        assert_eq!(
            PackagePath::serialize(Path::new(r"\\server\share\库")),
            "//server/share/库"
        );
        assert_eq!(
            PackagePath::relative(
                Path::new(r"\\server\one\app"),
                Path::new(r"\\server\two\lib")
            ),
            Path::new(r"\\server\two\lib")
        );
        assert_eq!(
            PackagePath::relative(Path::new(r"\\?\C:\work\app"), Path::new(r"C:\work\lib")),
            Path::new(r"..\lib")
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_relative_paths_preserve_non_utf8_names() {
        use std::os::unix::ffi::OsStringExt;
        let name = std::ffi::OsString::from_vec(vec![b'l', 0xff]);
        let target = Path::new("/workspace").join(&name);
        assert_eq!(
            PackagePath::relative(Path::new("/workspace/app"), &target),
            Path::new("..").join(name)
        );
        assert_eq!(
            PackagePath::serialize(Path::new(r"/workspace/a\b")),
            r"/workspace/a\b"
        );
    }
}
