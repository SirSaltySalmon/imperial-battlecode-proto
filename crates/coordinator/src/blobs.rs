//! Content-addressed blobs (bot modules, maps, replays). The trait is the whole surface the rest of the
//! coordinator uses, so swapping the local directory for S3 later is local to this file.

use crate::util::{is_sha256, random_hex, sha256_hex};
use std::io;
use std::path::{Path, PathBuf};

pub trait BlobStore: Send + Sync {
    /// Store `bytes` under `sha256`. Fails with `InvalidData` if the content does not hash to it.
    /// Returns true if the blob is new, false if it already existed.
    fn put(&self, sha256: &str, bytes: &[u8]) -> io::Result<bool>;
    fn get(&self, sha256: &str) -> io::Result<Option<Vec<u8>>>;
    fn exists(&self, sha256: &str) -> bool;
}

/// `<root>/ab/cdef…` (first two hex digits are the directory), written to `<root>/tmp` and renamed into place.
pub struct FsBlobs {
    root: PathBuf,
}

impl FsBlobs {
    pub fn open(root: &Path) -> io::Result<FsBlobs> {
        let tmp = root.join("tmp");
        std::fs::create_dir_all(&tmp)?;
        // Leftovers of writes interrupted by a crash.
        for entry in std::fs::read_dir(&tmp)?.flatten() {
            let _ = std::fs::remove_file(entry.path());
        }
        Ok(FsBlobs {
            root: root.to_path_buf(),
        })
    }

    fn path(&self, sha256: &str) -> Option<PathBuf> {
        is_sha256(sha256).then(|| self.root.join(&sha256[..2]).join(&sha256[2..]))
    }
}

impl BlobStore for FsBlobs {
    fn put(&self, sha256: &str, bytes: &[u8]) -> io::Result<bool> {
        let path = self
            .path(sha256)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not a sha256"))?;
        if sha256_hex(bytes) != sha256 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "content does not hash to the given sha256",
            ));
        }
        if path.is_file() {
            return Ok(false);
        }
        std::fs::create_dir_all(path.parent().expect("blob path has a parent"))?;
        let tmp = self.root.join("tmp").join(random_hex(12));
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        match std::fs::rename(&tmp, &path) {
            Ok(()) => Ok(true),
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                // Lost a race with another writer of the same content: that is success.
                if path.is_file() {
                    Ok(false)
                } else {
                    Err(e)
                }
            }
        }
    }

    fn get(&self, sha256: &str) -> io::Result<Option<Vec<u8>>> {
        let Some(path) = self.path(sha256) else {
            return Ok(None);
        };
        match std::fs::read(path) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn exists(&self, sha256: &str) -> bool {
        self.path(sha256).is_some_and(|p| p.is_file())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_verify() {
        let dir = std::env::temp_dir().join(format!("ibc-blobs-{}", random_hex(6)));
        let store = FsBlobs::open(&dir).unwrap();
        let sha = sha256_hex(b"hello");
        assert!(!store.exists(&sha));
        assert!(store.put(&sha, b"hello").unwrap());
        assert!(!store.put(&sha, b"hello").unwrap());
        assert_eq!(store.get(&sha).unwrap().as_deref(), Some(&b"hello"[..]));
        assert!(dir.join(&sha[..2]).join(&sha[2..]).is_file());
        assert_eq!(
            store.put(&sha, b"other").unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert!(store.get(&sha256_hex(b"absent")).unwrap().is_none());
        assert!(store.get("../../etc/passwd").unwrap().is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
