//! Content-addressed blob cache: `<dir>/blobs/<sha256>`.
//!
//! * A file only gets its final name after its SHA-256 was verified, by atomic rename from `<dir>/tmp`.
//! * A cached file is re-hashed the first time it is handed out after being idle, so a corrupted disk
//!   cannot feed a match.
//! * Least-recently-used eviction keeps the total under a byte cap; blobs in use are pinned and never
//!   evicted (so the cap can be exceeded only by what running jobs hold).

use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

#[derive(Debug)]
pub enum CacheError<E> {
    /// The fetch callback failed.
    Fetch(E),
    /// What was fetched does not hash to its name; nothing was kept.
    HashMismatch {
        expected: String,
        actual: String,
    },
    Io(io::Error),
}

struct Entry {
    size: u64,
    last_used: u64,
    pins: u32,
}

#[derive(Default)]
struct State {
    entries: HashMap<String, Entry>,
    total: u64,
    clock: u64,
    /// Hashes being fetched or verified right now; others wait instead of fetching twice.
    busy: HashSet<String>,
}

pub struct BlobCache {
    blobs: PathBuf,
    tmp: PathBuf,
    cap: u64,
    state: Mutex<State>,
    changed: Condvar,
    tmp_seq: AtomicU64,
}

/// A cached blob that cannot be evicted while this value lives.
pub struct Pinned {
    cache: Arc<BlobCache>,
    sha: String,
    path: PathBuf,
}

impl Pinned {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Pinned {
    fn drop(&mut self) {
        let mut st = self.cache.lock();
        if let Some(e) = st.entries.get_mut(&self.sha) {
            e.pins = e.pins.saturating_sub(1);
        }
        self.cache.evict(&mut st);
    }
}

pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(crate::hex(&hasher.finalize()));
        }
        hasher.update(&buf[..n]);
    }
}

impl BlobCache {
    /// Open (or create) a cache. Leftover temp files of a crashed run are removed; existing blobs are
    /// adopted, oldest modification time first in the eviction order.
    pub fn open(dir: &Path, cap_bytes: u64) -> io::Result<Arc<BlobCache>> {
        let (blobs, tmp) = (dir.join("blobs"), dir.join("tmp"));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&blobs)?;
        std::fs::create_dir_all(&tmp)?;
        let mut found = Vec::new();
        for entry in std::fs::read_dir(&blobs)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let meta = entry.metadata()?;
            if meta.is_file() && crate::is_sha256(&name) {
                found.push((
                    meta.modified().unwrap_or(std::time::UNIX_EPOCH),
                    name,
                    meta.len(),
                ));
            } else {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        found.sort();
        let mut st = State::default();
        for (_, name, size) in found {
            st.clock += 1;
            st.total += size;
            st.entries.insert(
                name,
                Entry {
                    size,
                    last_used: st.clock,
                    pins: 0,
                },
            );
        }
        let cache = BlobCache {
            blobs,
            tmp,
            cap: cap_bytes,
            state: Mutex::new(st),
            changed: Condvar::new(),
            tmp_seq: AtomicU64::new(0),
        };
        cache.evict(&mut cache.lock());
        Ok(Arc::new(cache))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn total_bytes(&self) -> u64 {
        self.lock().total
    }

    pub fn contains(&self, sha: &str) -> bool {
        self.lock().entries.contains_key(sha)
    }

    /// Drop least-recently-used unpinned blobs until the cache fits its cap.
    fn evict(&self, st: &mut State) {
        while st.total > self.cap {
            // A busy blob is being verified for a job that is about to pin it.
            let idle = st
                .entries
                .iter()
                .filter(|(k, e)| e.pins == 0 && !st.busy.contains(*k));
            let victim = idle
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| k.clone());
            let Some(sha) = victim else { break };
            self.forget(st, &sha);
        }
    }

    fn forget(&self, st: &mut State, sha: &str) {
        if let Some(e) = st.entries.remove(sha) {
            st.total -= e.size;
        }
        let _ = std::fs::remove_file(self.blobs.join(sha));
    }

    fn pin(self: &Arc<Self>, st: &mut State, sha: &str) -> Pinned {
        st.clock += 1;
        let clock = st.clock;
        let e = st.entries.get_mut(sha).expect("pinned blob is indexed");
        e.pins += 1;
        e.last_used = clock;
        Pinned {
            cache: self.clone(),
            sha: sha.to_string(),
            path: self.blobs.join(sha),
        }
    }

    /// The blob named `sha`, fetched with `fetch(temp_path)` if it is not cached. `sha` must be a valid
    /// SHA-256 name (it becomes a file name).
    pub fn get<E>(
        self: &Arc<Self>,
        sha: &str,
        fetch: impl FnOnce(&Path) -> Result<(), E>,
    ) -> Result<Pinned, CacheError<E>> {
        if !crate::is_sha256(sha) {
            return Err(CacheError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("not a sha256: {sha:?}"),
            )));
        }
        let cached = {
            let mut st = self.lock();
            while st.busy.contains(sha) {
                st = self.changed.wait(st).unwrap_or_else(|e| e.into_inner());
            }
            match st.entries.get(sha).map(|e| e.pins) {
                // In use by another job: it was verified when that job pinned it.
                Some(pins) if pins > 0 => return Ok(self.pin(&mut st, sha)),
                found => {
                    st.busy.insert(sha.to_string());
                    found.is_some()
                }
            }
        };
        let result = self.verify_or_fetch(sha, cached, fetch);
        let mut st = self.lock();
        st.busy.remove(sha);
        self.changed.notify_all();
        let size = result?;
        if !st.entries.contains_key(sha) {
            st.total += size;
            st.entries.insert(
                sha.to_string(),
                Entry {
                    size,
                    last_used: 0,
                    pins: 0,
                },
            );
        }
        let pinned = self.pin(&mut st, sha);
        self.evict(&mut st);
        Ok(pinned)
    }

    /// With `sha` marked busy: make `<blobs>/<sha>` exist with verified content. Returns its size.
    fn verify_or_fetch<E>(
        &self,
        sha: &str,
        cached: bool,
        fetch: impl FnOnce(&Path) -> Result<(), E>,
    ) -> Result<u64, CacheError<E>> {
        let dest = self.blobs.join(sha);
        if cached {
            if sha256_file(&dest).ok().as_deref() == Some(sha) {
                // Refresh the modification time so the eviction order survives a restart.
                if let Ok(f) = std::fs::File::options().append(true).open(&dest) {
                    let _ = f.set_modified(std::time::SystemTime::now());
                }
                return std::fs::metadata(&dest)
                    .map(|m| m.len())
                    .map_err(CacheError::Io);
            }
            crate::log!(
                "warn",
                "cached blob is missing or corrupt, fetching it again",
                sha256 = sha
            );
            self.forget(&mut self.lock(), sha);
        }
        let tmp = self.tmp.join(format!(
            "{sha}.{}.{}",
            std::process::id(),
            self.tmp_seq.fetch_add(1, Ordering::Relaxed)
        ));
        let fetched = fetch(&tmp).map_err(CacheError::Fetch).and_then(|()| {
            let actual = sha256_file(&tmp).map_err(CacheError::Io)?;
            if actual != sha {
                return Err(CacheError::HashMismatch {
                    expected: sha.to_string(),
                    actual,
                });
            }
            let size = std::fs::metadata(&tmp).map_err(CacheError::Io)?.len();
            std::fs::rename(&tmp, &dest).map_err(CacheError::Io)?;
            Ok(size)
        });
        if fetched.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        fetched
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ibc-worker-cache-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn put(cache: &Arc<BlobCache>, bytes: &[u8]) -> Result<Pinned, CacheError<Infallible>> {
        cache.get(&crate::sha256_hex(bytes), |p| {
            std::fs::write(p, bytes).unwrap();
            Ok(())
        })
    }

    #[test]
    fn stores_verifies_and_serves_from_cache() {
        let dir = temp_dir("basic");
        let cache = BlobCache::open(&dir, 1 << 20).unwrap();
        let sha = crate::sha256_hex(b"hello");
        let p = put(&cache, b"hello").unwrap();
        assert_eq!(std::fs::read(p.path()).unwrap(), b"hello");
        assert_eq!(p.path().file_name().unwrap().to_str().unwrap(), sha);
        drop(p);
        // A hit must not call fetch.
        let hit = cache
            .get::<Infallible>(&sha, |_| panic!("fetched a cached blob"))
            .unwrap();
        assert_eq!(cache.total_bytes(), 5);
        drop(hit);
        assert_eq!(std::fs::read_dir(dir.join("tmp")).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hash_mismatch_is_rejected_and_leaves_nothing() {
        let dir = temp_dir("mismatch");
        let cache = BlobCache::open(&dir, 1 << 20).unwrap();
        let sha = crate::sha256_hex(b"expected");
        let r = cache.get::<Infallible>(&sha, |p| {
            std::fs::write(p, b"something else").unwrap();
            Ok(())
        });
        assert!(matches!(r, Err(CacheError::HashMismatch { .. })));
        assert!(!cache.contains(&sha));
        assert!(!dir.join("blobs").join(&sha).exists());
        assert_eq!(std::fs::read_dir(dir.join("tmp")).unwrap().count(), 0);
        assert!(matches!(
            cache.get::<Infallible>("../../etc/passwd", |_| Ok(())),
            Err(CacheError::Io(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn evicts_least_recently_used_but_never_a_pinned_blob() {
        let dir = temp_dir("evict");
        let cache = BlobCache::open(&dir, 250).unwrap();
        let blob = |i: u8| vec![i; 100];
        let sha = |i: u8| crate::sha256_hex(&blob(i));
        drop(put(&cache, &blob(1)).unwrap());
        drop(put(&cache, &blob(2)).unwrap());
        // Touch 1 so that 2 is now the least recently used.
        drop(
            cache
                .get::<Infallible>(&sha(1), |_| panic!("cached"))
                .unwrap(),
        );
        drop(put(&cache, &blob(3)).unwrap());
        assert!(cache.contains(&sha(1)) && !cache.contains(&sha(2)) && cache.contains(&sha(3)));
        assert!(!dir.join("blobs").join(sha(2)).exists());
        assert_eq!(cache.total_bytes(), 200);

        // Pinned blobs survive even when the cap is exceeded; eviction catches up when they are released.
        let p1 = cache
            .get::<Infallible>(&sha(1), |_| panic!("cached"))
            .unwrap();
        let p3 = cache
            .get::<Infallible>(&sha(3), |_| panic!("cached"))
            .unwrap();
        let p4 = put(&cache, &blob(4)).unwrap();
        assert_eq!(cache.total_bytes(), 300);
        assert!(p1.path().exists() && p3.path().exists() && p4.path().exists());
        drop((p1, p3, p4));
        assert!(cache.total_bytes() <= 250);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reopens_with_existing_blobs_and_refetches_a_corrupted_one() {
        let dir = temp_dir("reopen");
        let sha = crate::sha256_hex(b"payload");
        {
            let cache = BlobCache::open(&dir, 1 << 20).unwrap();
            drop(put(&cache, b"payload").unwrap());
        }
        std::fs::write(dir.join("tmp").join("leftover"), b"x").unwrap();
        let cache = BlobCache::open(&dir, 1 << 20).unwrap();
        assert!(cache.contains(&sha));
        assert_eq!(std::fs::read_dir(dir.join("tmp")).unwrap().count(), 0);
        std::fs::write(dir.join("blobs").join(&sha), b"bit rot").unwrap();
        let mut fetched = false;
        let p = cache
            .get::<Infallible>(&sha, |p| {
                fetched = true;
                std::fs::write(p, b"payload").unwrap();
                Ok(())
            })
            .unwrap();
        assert!(fetched);
        assert_eq!(std::fs::read(p.path()).unwrap(), b"payload");
        assert_eq!(cache.total_bytes(), 7);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
