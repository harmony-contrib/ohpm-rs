//! Package integrity hashes (sha1 + sha512) in ssri format, mirroring
//! `lib/tools/ssri` used by `getPkgContent`.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use base64::Engine;
use sha1::{Digest, Sha1};
use sha2::Sha512;

use crate::error::Result;

/// Integrity digests of a package file.
#[derive(Debug, Clone)]
pub struct Integrity {
    /// Base64 SHA-1 digest.
    pub sha1: String,
    /// Base64 SHA-512 digest.
    pub sha512: String,
}

impl Integrity {
    /// The ssri integrity string the registry validates: the single
    /// `sha512-<base64>` entry. The reference `getIntegrity()` picks one
    /// algorithm (sha512 per the prioritized list), so `dist.integrity` /
    /// `integrity_hsp` carry sha512 only.
    pub fn to_ssri(&self) -> String {
        format!("sha512-{}", self.sha512)
    }

    /// SHA-1 base64 digest used as the package `shasum` (display only).
    pub fn shasum(&self) -> &str {
        &self.sha1
    }
}

/// Compute sha1 + sha512 of a file in a single pass.
pub fn integrity_of_file(path: &Path) -> Result<Integrity> {
    let mut f = File::open(path)?;
    let mut sha1 = Sha1::new();
    let mut sha512 = Sha512::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        sha1.update(&buf[..n]);
        sha512.update(&buf[..n]);
    }
    let sha1 = base64::engine::general_purpose::STANDARD.encode(sha1.finalize());
    let sha512 = base64::engine::general_purpose::STANDARD.encode(sha512.finalize());
    Ok(Integrity { sha1, sha512 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_matches_expected() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("f.bin");
        std::fs::write(&p, b"hello world\n").unwrap();
        let integrity = integrity_of_file(&p).unwrap();
        // sha1 of "hello world\n" is a known value.
        let expected = base64::engine::general_purpose::STANDARD
            .encode(Sha1::digest(b"hello world\n"));
        assert_eq!(integrity.sha1, expected);
        assert!(integrity.to_ssri().starts_with("sha512-"));
    }
}
