pub mod checkout;

pub(crate) use self::checkout::checkout_tree;

use crate::config;
use crate::types::Version;
use anyhow::{Context, Result};
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// The Git cache has been opened/initialized but no remote has been contacted yet.
pub struct Fresh;

/// A remote ref has been discovered — the `RefKind` is embedded so that
/// `fetch_shallow` cannot be called without first calling `discover_ref`.
pub struct RemoteDiscovered(pub RefKind);

// Shared bare git repository — the central object cache for all worktrees.
//
// **Typestate pattern** — The `S` generic encodes the current lifecycle state.
// - [`Fresh`]: repo is ready, no remote ref has been resolved yet.
// - [`RemoteDiscovered`]: a remote ref has been resolved (carries the `RefKind`).
//
// The following transitions are enforced at compile time:
// ```ignore
// Fresh ──discover_ref──▶ RemoteDiscovered ──fetch_shallow──▶ Fresh ──checkout_worktree──▶ (staged)
// staged ──register_worktree──▶ ()
// ```
//
// `checkout_worktree` and `register_worktree` are separate so installs can be
// staged: the checkout is built in a sibling directory (with any previous
// installation left intact) and only registered once it is swapped into place.

/// Whether a remote ref is a tag or a branch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RefKind {
    Tag,
    Branch,
}

pub struct GitCache<S> {
    pub(crate) repo: gix::Repository,
    pub(crate) path: PathBuf,
    pub(crate) state: S,
}

impl<S> GitCache<S> {
    /// Path to the bare repo root.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Remove a worktree and prune stale metadata.
    pub fn remove_worktree(&self, version: &Version) {
        // Best-effort: cleanup must not fail the whole command if locking is
        // unavailable, but should still serialize with concurrent fetches.
        let _lock = match git_cache_lock() {
            Ok(l) => Some(l),
            Err(e) => {
                eprintln!("Warning: could not lock git cache for worktree cleanup: {e}");
                None
            }
        };
        let env_dir = match config::envs_dir() {
            Ok(d) => d.join(version.as_str()),
            Err(_) => return,
        };
        let worktrees_dir = self
            .repo
            .common_dir()
            .join("worktrees")
            .join(version.as_str());

        if worktrees_dir.exists() {
            std::fs::remove_dir_all(&worktrees_dir).ok();
        }

        let wt_path = self.repo.common_dir().join("worktrees");
        if wt_path.exists() {
            for e in std::fs::read_dir(&wt_path)
                .ok()
                .into_iter()
                .flatten()
                .flatten()
            {
                let gitdir_path = e.path().join("gitdir");
                if let Ok(content) = std::fs::read_to_string(&gitdir_path) {
                    let linked = content.trim();
                    if linked == env_dir.to_string_lossy() || !Path::new(linked).exists() {
                        std::fs::remove_dir_all(e.path()).ok();
                    }
                }
            }
        }

        self.repo.worktrees().ok();
    }

    /// Resolve a ref name (tag or branch) to an object id.
    fn resolve_ref(&self, version: &str) -> Result<gix::ObjectId> {
        for prefix in &["refs/tags/", "refs/heads/"] {
            let full_name = format!("{prefix}{version}");
            if let Ok(mut r) = self.repo.find_reference(&full_name)
                && let Ok(peeled) = r.peel_to_id()
            {
                return Ok(peeled.detach());
            }
        }
        anyhow::bail!("No local ref found for '{version}' — was it fetched?")
    }

    /// Retrieve the bare `gix::Repository` reference for low-level operations.
    pub fn repo(&self) -> &gix::Repository {
        &self.repo
    }
}

// Construction: uninitialized → Fresh

impl GitCache<Fresh> {
    /// Open the existing bare repo at `{cache_root}/git`, or initialise one.
    ///
    /// Opening an existing repo is lock-free; only the creation path takes the
    /// cross-process git cache lock, so two parallel installs that both find an
    /// uninitialised cache cannot race on `git init`.
    pub fn open_or_init() -> Result<Self> {
        let path = config::git_cache_dir()?;
        if path.join("HEAD").exists() {
            let repo = gix::open(&path)
                .with_context(|| format!("Failed to open git cache at {}", path.display()))?;
            return Ok(Self {
                repo,
                path,
                state: Fresh,
            });
        }
        let _lock = git_cache_lock()?;
        let repo = if path.join("HEAD").exists() {
            // Another process initialised it while we waited for the lock.
            gix::open(&path)
                .with_context(|| format!("Failed to open git cache at {}", path.display()))?
        } else {
            init_bare_repo(&path)?
        };
        Ok(Self {
            repo,
            path,
            state: Fresh,
        })
    }

    /// Transition **Fresh → RemoteDiscovered** by asking the remote which ref
    /// exists for `version`.
    ///
    /// **Optimization:** First checks if the ref already exists in the local
    /// bare repository (from a previous fetch that pulled in multiple refs).
    /// Only connects to the remote if no local ref is found.
    pub fn discover_ref(
        self,
        remote_url: &str,
        version: &Version,
    ) -> Result<GitCache<RemoteDiscovered>> {
        let tag_ref = format!("refs/tags/{}", version.as_str());
        let branch_ref = format!("refs/heads/{}", version.as_str());

        // Check locally first — skip network call if the ref is already cached.
        if let Ok(mut r) = self.repo.find_reference(&tag_ref)
            && r.peel_to_id().is_ok()
        {
            if crate::is_verbose() {
                eprintln!("[debug] Found local tag ref {tag_ref}");
            }
            return Ok(GitCache {
                repo: self.repo,
                path: self.path,
                state: RemoteDiscovered(RefKind::Tag),
            });
        }
        if let Ok(mut r) = self.repo.find_reference(&branch_ref)
            && r.peel_to_id().is_ok()
        {
            if crate::is_verbose() {
                eprintln!("[debug] Found local branch ref {branch_ref}");
            }
            return Ok(GitCache {
                repo: self.repo,
                path: self.path,
                state: RemoteDiscovered(RefKind::Branch),
            });
        }

        if crate::is_verbose() {
            eprintln!(
                "[debug] No local ref found for {}, querying remote {remote_url}",
                version
            );
        }

        let remote = self
            .repo
            .remote_at(remote_url)
            .with_context(|| format!("Failed to create remote for {remote_url}"))?;

        let connection = remote
            .connect(gix::remote::Direction::Fetch)
            .with_context(|| format!("Failed to connect to {remote_url}"))?;

        let (ref_map, _handshake) = connection
            .ref_map(gix::progress::Discard, Default::default())
            .with_context(|| format!("Failed to list refs from {remote_url}"))?;

        for r in &ref_map.remote_refs {
            let name: &gix::bstr::BStr = match r {
                gix::protocol::handshake::Ref::Direct { full_ref_name, .. }
                | gix::protocol::handshake::Ref::Peeled { full_ref_name, .. }
                | gix::protocol::handshake::Ref::Symbolic { full_ref_name, .. }
                | gix::protocol::handshake::Ref::Unborn { full_ref_name, .. } => {
                    full_ref_name.as_ref()
                }
            };
            if name == tag_ref.as_str() {
                return Ok(GitCache {
                    repo: self.repo,
                    path: self.path,
                    state: RemoteDiscovered(RefKind::Tag),
                });
            }
            if name == branch_ref.as_str() {
                return Ok(GitCache {
                    repo: self.repo,
                    path: self.path,
                    state: RemoteDiscovered(RefKind::Branch),
                });
            }
        }

        anyhow::bail!(
            "Could not find a remote tag or branch named '{}' at {remote_url}",
            version
        )
    }
}

// Discovered state: must call fetch_shallow before checking out

impl GitCache<RemoteDiscovered> {
    /// Transition **RemoteDiscovered → Fresh** by shallow-fetching the
    /// previously-discovered ref into the shared bare repository.
    pub fn fetch_shallow(self, remote_url: &str, version: &Version) -> Result<GitCache<Fresh>> {
        let _lock = git_cache_lock()?;
        let kind = &self.state.0;
        let refspec = match kind {
            RefKind::Tag => format!(
                "+refs/tags/{}:refs/tags/{}",
                version.as_str(),
                version.as_str()
            ),
            RefKind::Branch => {
                format!(
                    "+refs/heads/{}:refs/heads/{}",
                    version.as_str(),
                    version.as_str()
                )
            }
        };

        let remote = self
            .repo
            .remote_at(remote_url)
            .with_context(|| format!("Failed to create remote for {remote_url}"))?;

        let connection = remote
            .connect(gix::remote::Direction::Fetch)
            .with_context(|| format!("Failed to connect to {remote_url}"))?;

        let ref_spec = gix::refspec::parse(
            refspec.as_str().into(),
            gix::refspec::parse::Operation::Fetch,
        )?;
        let opts = gix::remote::ref_map::Options {
            extra_refspecs: vec![ref_spec.to_owned()],
            ..Default::default()
        };

        let prepare = connection
            .prepare_fetch(gix::progress::Discard, opts)
            .with_context(|| format!("Failed to prepare fetch for {version}"))?;

        let outcome = prepare
            .with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(
                NonZeroU32::new(1).unwrap(),
            ))
            .receive(gix::progress::Discard, &AtomicBool::new(false))
            .with_context(|| format!("Failed to fetch {version} from {remote_url}"))?;

        if matches!(outcome.status, gix::remote::fetch::Status::Change { .. }) {
            if crate::is_verbose() {
                eprintln!("[debug] Fetched Flutter {version} from {remote_url}");
            } else {
                eprintln!("Fetched Flutter {version}");
            }
        } else if crate::is_verbose() {
            eprintln!("[debug] Shallow fetch for {version}: no change (already up-to-date)");
        }

        Ok(GitCache {
            repo: self.repo,
            path: self.path,
            state: Fresh,
        })
    }
}

// Worktree operations: callable on Fresh (after fetch)

impl GitCache<Fresh> {
    /// Check out a version's tree into `dest` **without** registering it as a
    /// worktree in the bare repo.
    ///
    /// Registration is deliberately deferred to [`GitCache::register_worktree`]
    /// so a staged install can check the replacement out (and download its
    /// engines) without ever touching the previous worktree's registration —
    /// a failed install leaves the old installation fully intact.
    pub fn checkout_worktree(&self, version: &Version, dest: &Path) -> Result<()> {
        let _lock = git_cache_lock()?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| "Failed to create parent directory for worktree".to_string())?;
        }

        let commit = match self.resolve_ref(version.as_str()) {
            Ok(id) => id,
            Err(e) => {
                anyhow::bail!("Could not resolve ref for version '{}': {e}", version)
            }
        };

        let commit_obj = self
            .repo
            .find_object(commit)
            .with_context(|| format!("Failed to find object for {commit}"))?;
        let tree = commit_obj
            .peel_to_tree()
            .with_context(|| "Failed to peel commit to tree".to_string())?;

        std::fs::create_dir_all(dest).with_context(|| {
            format!("Failed to create worktree directory at {}", dest.display())
        })?;

        checkout_tree(&tree, dest)
            .with_context(|| format!("Failed to checkout worktree at {}", dest.display()))?;

        Ok(())
    }

    /// Register `version` as a linked worktree at `env_dir` in the shared bare
    /// repository (writes `worktrees/{version}/{HEAD,commondir,gitdir}` and the
    /// worktree's `.git` gitlink).
    ///
    /// Overwrites any previous registration for this version — call only after
    /// the new checkout is in place at `env_dir`.
    pub fn register_worktree(&self, version: &Version, env_dir: &Path) -> Result<()> {
        let _lock = git_cache_lock()?;
        let commit = match self.resolve_ref(version.as_str()) {
            Ok(id) => id,
            Err(e) => {
                anyhow::bail!("Could not resolve ref for version '{}': {e}", version)
            }
        };

        let worktrees_dir = self
            .repo
            .common_dir()
            .join("worktrees")
            .join(version.as_str());
        std::fs::create_dir_all(&worktrees_dir).with_context(|| {
            format!(
                "Failed to create worktree metadata at {}",
                worktrees_dir.display()
            )
        })?;

        std::fs::write(worktrees_dir.join("HEAD"), format!("{commit}\n"))
            .with_context(|| "Failed to write HEAD for worktree".to_string())?;

        std::fs::write(worktrees_dir.join("commondir"), "../..\n")
            .with_context(|| "Failed to write commondir for worktree".to_string())?;

        std::fs::write(
            worktrees_dir.join("gitdir"),
            format!("{}\n", env_dir.display()),
        )
        .with_context(|| "Failed to write gitdir for worktree".to_string())?;

        let gitlink_content = format!(
            "gitdir: {}/worktrees/{}\n",
            self.path.display(),
            version.as_str()
        );
        std::fs::write(env_dir.join(".git"), &gitlink_content)
            .with_context(|| "Failed to write .git file for worktree".to_string())?;

        Ok(())
    }

    /// Snapshot the bare-repo worktree registration files for `version` (HEAD,
    /// commondir, gitdir) so a failed replacement install can restore the
    /// previous linkage. Returns `(path, original content)` triples; `None`
    /// content means the file did not exist and must be removed on restore.
    pub fn snapshot_worktree_registration(
        &self,
        version: &Version,
    ) -> Vec<(PathBuf, Option<String>)> {
        let dir = self
            .repo
            .common_dir()
            .join("worktrees")
            .join(version.as_str());
        ["HEAD", "commondir", "gitdir"]
            .iter()
            .map(|f| {
                let path = dir.join(f);
                let content = std::fs::read_to_string(&path).ok();
                (path, content)
            })
            .collect()
    }
}

/// Restore worktree registration files from a snapshot taken by
/// [`GitCache::snapshot_worktree_registration`]. Best-effort: failures are
/// logged, since a restore during rollback cannot itself fail the install.
pub(crate) fn restore_worktree_registration(snapshot: &[(PathBuf, Option<String>)]) {
    let had_registration = snapshot.iter().any(|(_, content)| content.is_some());
    for (path, content) in snapshot {
        match content {
            Some(original) => {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::write(path, original) {
                    eprintln!(
                        "Warning: failed to restore worktree metadata at {}: {e}",
                        path.display()
                    );
                }
            }
            None => {
                let _ = std::fs::remove_file(path);
            }
        }
    }
    // If there was no prior registration, remove the now-empty registration
    // directory a failed fresh install may have created.
    if !had_registration && let Some(dir) = snapshot.first().and_then(|(p, _)| p.parent()) {
        let _ = std::fs::remove_dir(dir); // only succeeds when empty
    }
}

/// Path to the central bare Git repository used as object cache.
pub fn git_cache_path() -> Result<PathBuf> {
    config::git_cache_dir()
}

/// Exclusive cross-process lock guarding git cache mutations.
fn git_cache_lock() -> Result<crate::lock::FileLock> {
    crate::lock::FileLock::acquire(&config::git_cache_lock_path()?)
}

/// Initialise a fresh bare repository at `path`.
/// Callers must hold the git cache lock when the cache might already exist.
fn init_bare_repo(path: &Path) -> Result<gix::Repository> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("Failed to create git cache directory at {}", path.display()))?;
    let ts_repo = gix::ThreadSafeRepository::init_opts(
        path,
        gix::create::Kind::Bare,
        gix::create::Options::default(),
        gix::open::Options::default(),
    )
    .with_context(|| format!("Failed to initialise bare git cache at {}", path.display()))?;
    let _ = std::fs::create_dir_all(path.join("objects").join("info"));
    Ok(ts_repo.into())
}

/// Calculate total size of the git object cache on disk.
pub fn cache_size() -> u64 {
    let path = match git_cache_path() {
        Ok(p) => p,
        Err(_) => return 0,
    };
    if !path.exists() {
        return 0;
    }
    crate::util::dir_size(&path)
}

/// Versions of installed toolchains whose worktrees are linked into the shared
/// bare git cache (their `envs/<version>/.git` is a gitlink *file* pointing at
/// the bare repo, not a real `.git` directory). Deleting the bare repo would
/// orphan these worktrees, so `joy gc --git` must refuse while any exist.
pub(crate) fn git_linked_toolchains() -> Result<Vec<String>> {
    let envs = config::envs_dir()?;
    let mut linked = Vec::new();
    if !envs.exists() {
        return Ok(linked);
    }
    for entry in std::fs::read_dir(&envs)
        .with_context(|| format!("Failed to read installed toolchains at {}", envs.display()))?
    {
        let entry = entry?;
        if entry.path().join(".git").is_file()
            && let Some(name) = entry.file_name().to_str()
        {
            linked.push(name.to_string());
        }
    }
    linked.sort();
    Ok(linked)
}

/// Remove all cached bare repo data and re-initialise.
///
/// Refuses to run while any installed git-based toolchain's worktree is linked
/// into the shared repo: removing it would leave those worktrees' `.git`
/// gitlinks dangling, breaking in-tree git operations (e.g. `flutter upgrade`)
/// and forcing a silent full reinstall on next use.
pub fn clear_cache() -> Result<()> {
    // Take the git lock *before* the linked-toolchain check: concurrent installs
    // take the same lock to create worktrees, so no new worktree can appear
    // between the check and the deletion below.
    let _lock = git_cache_lock()?;
    let linked = git_linked_toolchains()?;
    if !linked.is_empty() {
        anyhow::bail!(
            "Refusing to clear the shared Git cache: installed toolchain(s) {} \
            are git-based worktrees linked to it. Uninstall them first with \
            'joy toolchain remove <version>'.",
            linked.join(", ")
        );
    }
    let path = git_cache_path()?;
    if path.exists() {
        std::fs::remove_dir_all(&path).context("Failed to remove git cache")?;
    }
    init_bare_repo(&path)?;
    Ok(())
}

/// Check whether a worktree's `.git` pointer is still valid.
pub fn worktree_is_valid(version: &str) -> bool {
    worktree_is_valid_str(version)
}

/// Internal — check worktree validity from a raw string.
fn worktree_is_valid_str(version: &str) -> bool {
    let env_dir = match config::envs_dir() {
        Ok(d) => d.join(version),
        Err(_) => return false,
    };
    let git_link = env_dir.join(".git");

    if !git_link.is_file() {
        return false;
    }

    let content = match std::fs::read_to_string(&git_link) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let gitdir_path = match content.strip_prefix("gitdir: ") {
        Some(p) => p.trim(),
        None => return false,
    };

    std::path::Path::new(gitdir_path).join("HEAD").exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT_ID: AtomicU32 = AtomicU32::new(70000);

    fn temp_dir() -> PathBuf {
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("joy_git_cache_test_{id}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    struct XdgGuard(PathBuf);

    impl Drop for XdgGuard {
        fn drop(&mut self) {
            unsafe {
                std::env::remove_var("XDG_DATA_HOME");
                std::env::remove_var("XDG_CACHE_HOME");
            }
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn setup_xdg() -> XdgGuard {
        let tmp = temp_dir();
        let data_home = tmp.join("xdg").join("data");
        let cache_home = tmp.join("xdg").join("cache");
        std::fs::create_dir_all(&data_home).unwrap();
        std::fs::create_dir_all(&cache_home).unwrap();
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &data_home);
            std::env::set_var("XDG_CACHE_HOME", &cache_home);
        }
        XdgGuard(tmp)
    }

    /// Create a fake installed toolchain. With `gitlink` the `.git` entry is a
    /// file (git-based worktree); otherwise a real directory (full clone).
    fn install_fake(version: &str, gitlink: bool) {
        let env_dir = config::envs_dir().unwrap().join(version);
        std::fs::create_dir_all(env_dir.join("bin")).unwrap();
        std::fs::write(env_dir.join("bin").join("flutter"), b"#!/bin/sh\necho fake").unwrap();
        if gitlink {
            std::fs::write(env_dir.join(".git"), "gitdir: /nonexistent\n").unwrap();
        } else {
            std::fs::create_dir_all(env_dir.join(".git")).unwrap();
        }
    }

    #[test]
    #[serial]
    fn detects_git_linked_toolchains() {
        let _guard = setup_xdg();
        install_fake("3.30.0", true);
        install_fake("3.29.0", false); // full clone — must not be flagged
        assert_eq!(git_linked_toolchains().unwrap(), vec!["3.30.0"]);
    }

    #[test]
    #[serial]
    fn detects_none_when_no_toolchains_installed() {
        let _guard = setup_xdg();
        assert!(git_linked_toolchains().unwrap().is_empty());
    }

    #[test]
    #[serial]
    fn clear_cache_refuses_when_toolchains_are_linked() {
        let _guard = setup_xdg();
        install_fake("3.30.0", true);
        let err = clear_cache().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Refusing"), "unexpected error: {msg}");
        assert!(
            msg.contains("3.30.0"),
            "error should name the linked version: {msg}"
        );
    }

    #[test]
    #[serial]
    fn clear_cache_succeeds_without_linked_toolchains() {
        let _guard = setup_xdg();
        install_fake("3.29.0", false); // full clone — safe to GC
        clear_cache().unwrap();
        assert!(
            git_cache_path().unwrap().join("HEAD").exists(),
            "git cache should be re-initialised after clearing"
        );
    }

    #[test]
    #[serial]
    fn snapshot_and_restore_worktree_registration() {
        let tmp = temp_dir();
        let path = tmp.join("bare");
        let repo = init_bare_repo(&path).unwrap();
        let cache = GitCache {
            repo,
            path: path.clone(),
            state: Fresh,
        };
        let version = Version::new("3.29.0").unwrap();

        // Simulate a previous git install's registration.
        let wt = path.join("worktrees").join(version.as_str());
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join("HEAD"), "old-head\n").unwrap();
        std::fs::write(wt.join("commondir"), "../..\n").unwrap();
        std::fs::write(wt.join("gitdir"), "/old/env/dir\n").unwrap();

        let snapshot = cache.snapshot_worktree_registration(&version);

        // Clobber with a new registration (as register_worktree would).
        std::fs::write(wt.join("HEAD"), "new-head\n").unwrap();
        std::fs::write(wt.join("gitdir"), "/new/env/dir\n").unwrap();

        restore_worktree_registration(&snapshot);

        assert_eq!(
            std::fs::read_to_string(wt.join("HEAD")).unwrap(),
            "old-head\n",
            "HEAD must be restored"
        );
        assert_eq!(
            std::fs::read_to_string(wt.join("gitdir")).unwrap(),
            "/old/env/dir\n",
            "gitdir must be restored"
        );
        assert_eq!(
            std::fs::read_to_string(wt.join("commondir")).unwrap(),
            "../..\n",
            "commondir must be restored"
        );
    }

    #[test]
    #[serial]
    fn restore_removes_registration_that_did_not_exist() {
        let tmp = temp_dir();
        let path = tmp.join("bare");
        let repo = init_bare_repo(&path).unwrap();
        let cache = GitCache {
            repo,
            path: path.clone(),
            state: Fresh,
        };
        let version = Version::new("3.29.0").unwrap();

        // No previous registration → snapshot is all None.
        let snapshot = cache.snapshot_worktree_registration(&version);
        assert!(snapshot.iter().all(|(_, c)| c.is_none()));

        // A failed install creates a partial registration...
        let wt = path.join("worktrees").join(version.as_str());
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join("HEAD"), "partial\n").unwrap();

        // ...which the restore must remove again.
        restore_worktree_registration(&snapshot);
        assert!(
            !wt.join("HEAD").exists(),
            "restore must remove registration files that did not exist before"
        );
    }
}
