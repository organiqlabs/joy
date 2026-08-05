//! Cross-process advisory file locking for the shared caches.
//!
//! Concurrent `joy` processes (e.g. parallel `toolchain install` runs) share
//! the bare git object cache and the engine cache; without a lock, one process
//! can observe another's half-written state. Flutter solves the same problem
//! with a `cache.lock` lockfile — here each cache is guarded by an
//! `flock`-style exclusive lock (via [`fs2`]) on a small lockfile that lives
//! *alongside* the cache directory, never inside it, so `joy gc` can remove
//! cache contents without unlinking the lock.

use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::Path;

/// An exclusive advisory lock on a cache, released when dropped.
///
/// The lock file itself is intentionally left on disk after release: unlinking
/// it would let a third process create a *new* inode and acquire a lock that no
/// longer conflicts with processes still waiting on the old inode.
pub struct FileLock {
    file: File,
}

impl FileLock {
    /// Acquire the exclusive lock for `path`, blocking until it is available.
    ///
    /// Prints a notice to stderr if another process currently holds the lock,
    /// so a user who launched two installs in parallel isn't left wondering
    /// why the second one is silent.
    pub fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create lock directory {}", parent.display()))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            // Never truncate: the lock file is only ever locked, not written.
            .truncate(false)
            .open(path)
            .with_context(|| format!("Failed to open lock file {}", path.display()))?;
        match file.try_lock_exclusive() {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {
                eprintln!("Waiting for another joy process to release the cache lock...");
                file.lock_exclusive()
                    .with_context(|| format!("Failed to lock {}", path.display()))?;
            }
            Err(e) => return Err(e).with_context(|| format!("Failed to lock {}", path.display())),
        }
        Ok(Self { file })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn lock_path(tag: &str) -> std::path::PathBuf {
        let p =
            std::env::temp_dir().join(format!("joy_lock_test_{tag}_{}.lock", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn lock_can_be_acquired_released_and_reacquired() {
        let path = lock_path("reacquire");
        {
            let _lock = FileLock::acquire(&path).unwrap();
        }
        let _lock = FileLock::acquire(&path).unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn lock_blocks_a_second_acquire_until_released() {
        let path = lock_path("exclusion");
        let first = FileLock::acquire(&path).unwrap();

        let second_path = path.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let lock = FileLock::acquire(&second_path).unwrap();
            let _ = tx.send(());
            drop(lock);
        });

        // Give the second thread time to start and block on the held lock.
        std::thread::sleep(Duration::from_millis(50));
        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "second acquire must block while the first lock is held"
        );

        drop(first);
        rx.recv_timeout(Duration::from_secs(10))
            .expect("second acquire must succeed after the first lock is released");
        handle.join().unwrap();
        let _ = std::fs::remove_file(&path);
    }
}
