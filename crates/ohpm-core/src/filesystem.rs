//! Package links: relative symlinks on Unix, unprivileged junctions on Windows.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use crate::paths::PackagePath;

pub(crate) struct DirectoryLink<'a> {
    target: &'a Path,
    link: &'a Path,
}

impl<'a> DirectoryLink<'a> {
    pub(crate) fn new(target: &'a Path, link: &'a Path) -> Self {
        Self { target, link }
    }

    pub(crate) fn ensure(&self) -> io::Result<()> {
        if self.target == self.link {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "link and target are the same path",
            ));
        }
        let target = self.target.canonicalize()?;
        if let Some(parent) = self.link.parent() {
            fs::create_dir_all(parent)?;
        }
        if let Ok(metadata) = fs::symlink_metadata(self.link) {
            let existing = self.link.canonicalize().ok();
            if existing.as_ref() == Some(&target) {
                if metadata.file_type().is_symlink() || Self::is_directory_link(&metadata) {
                    return Ok(());
                }
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "link would replace its own target",
                ));
            }
            Self::remove_entry(self.link)?;
        }
        #[cfg(unix)]
        {
            let parent = self
                .link
                .parent()
                .unwrap_or(Path::new("."))
                .canonicalize()?;
            std::os::unix::fs::symlink(PackagePath::relative(&parent, &target), self.link)
        }
        #[cfg(windows)]
        {
            junction::create(&target, self.link)
        }
    }

    #[cfg(windows)]
    fn is_directory_link(metadata: &fs::Metadata) -> bool {
        use std::os::windows::fs::MetadataExt;
        // FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT, including dangling junctions.
        metadata.file_attributes() & 0x410 == 0x410
    }

    #[cfg(not(windows))]
    fn is_directory_link(_metadata: &fs::Metadata) -> bool {
        false
    }

    /// Remove the link itself; never descend into a Windows junction's target.
    pub(crate) fn remove_entry(path: &Path) -> io::Result<()> {
        let metadata = fs::symlink_metadata(path)?;
        if Self::is_directory_link(&metadata) {
            fs::remove_dir(path)
        } else if metadata.file_type().is_symlink() || !metadata.is_dir() {
            fs::remove_file(path)
        } else {
            fs::remove_dir_all(path)
        }
    }
}

/// Defer archive/Git links until every regular target has been materialized.
#[derive(Default)]
pub(crate) struct LinkPlan {
    links: Vec<(PathBuf, PathBuf)>,
}

impl LinkPlan {
    pub(crate) fn push(&mut self, target: PathBuf, link: PathBuf) {
        self.links.push((target, link));
    }

    pub(crate) fn apply(self) -> io::Result<()> {
        #[cfg(unix)]
        for (target, link) in self.links {
            if fs::symlink_metadata(&link).is_ok() {
                DirectoryLink::remove_entry(&link)?;
            }
            std::os::unix::fs::symlink(target, link)?;
        }
        #[cfg(windows)]
        {
            let mut pending = self.links;
            while !pending.is_empty() {
                let previous = pending.len();
                let mut unresolved = Vec::new();
                for (target, link) in pending {
                    let resolved = link.parent().unwrap_or(Path::new(".")).join(&target);
                    let Ok(metadata) = fs::metadata(&resolved) else {
                        unresolved.push((target, link));
                        continue;
                    };
                    if fs::symlink_metadata(&link).is_ok() {
                        DirectoryLink::remove_entry(&link)?;
                    }
                    if metadata.is_dir() {
                        DirectoryLink::new(&resolved, &link).ensure()?;
                    } else if let Err(error) = std::os::windows::fs::symlink_file(&target, &link) {
                        if error.kind() != io::ErrorKind::PermissionDenied
                            && error.raw_os_error() != Some(1314)
                        {
                            return Err(error);
                        }
                        // Package files share a volume; hard links need no symlink privilege.
                        fs::hard_link(&resolved, &link)?;
                    }
                }
                if unresolved.len() == previous {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "package contains a dangling or cyclic symbolic link",
                    ));
                }
                pending = unresolved;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_links_reuse_retarget_and_remove_without_deleting_targets() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("source space 库");
        let second = temp.path().join("second");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        fs::write(first.join("data"), b"first").unwrap();
        fs::write(second.join("data"), b"second").unwrap();
        let link = temp.path().join("modules/link");
        DirectoryLink::new(&first, &link).ensure().unwrap();
        DirectoryLink::new(&first, &link).ensure().unwrap();
        assert_eq!(fs::read(link.join("data")).unwrap(), b"first");
        DirectoryLink::new(&second, &link).ensure().unwrap();
        assert_eq!(fs::read(link.join("data")).unwrap(), b"second");
        DirectoryLink::remove_entry(&link).unwrap();
        assert_eq!(fs::read(first.join("data")).unwrap(), b"first");
        assert_eq!(fs::read(second.join("data")).unwrap(), b"second");
    }

    #[test]
    fn deferred_links_preserve_payload_and_resolve_chains() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("payload"), b"actual payload").unwrap();
        let mut plan = LinkPlan::default();
        plan.push("middle".into(), temp.path().join("first"));
        plan.push("payload".into(), temp.path().join("middle"));
        plan.apply().unwrap();
        assert_eq!(
            fs::read(temp.path().join("first")).unwrap(),
            b"actual payload"
        );
    }
}
