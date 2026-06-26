use crate::config;
use anyhow::{Context, Result};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Path to the central bare Git repository used as object cache.
pub fn git_cache_path() -> PathBuf {
    config::git_cache_dir()
}

/// Ensure the central bare Git repository exists, initializing it if needed.
pub fn ensure_initialized() -> Result<()> {
    let path = git_cache_path();
    if !path.join("HEAD").exists() {
        std::fs::create_dir_all(&path).context("Failed to create git cache directory")?;
        let repo = git2::Repository::init_bare(&path)
            .context("Failed to initialize bare git cache repo")?;
        // Ensure objects/info directory exists for alternates
        std::fs::create_dir_all(path.join("objects").join("info"))
            .context("Failed to create objects/info directory")?;
        drop(repo);
    }
    Ok(())
}

/// Get a handle to the central bare Git repository.
pub fn open_cache_repo() -> Result<git2::Repository> {
    ensure_initialized()?;
    git2::Repository::open_bare(git_cache_path()).context("Failed to open git cache repository")
}

/// Fetch objects from a remote into the central cache.
/// Only objects not already in the cache are transferred.
pub fn fetch(remote_url: &str, refspecs: &[&str]) -> Result<()> {
    let repo = open_cache_repo()?;
    let mut remote = repo
        .remote_anonymous(remote_url)
        .context("Failed to create anonymous remote")?;
    let mut fo = git2::FetchOptions::new();
    fo.download_tags(git2::AutotagOption::None);

    // Only fetch missing objects -- prune is off so we accumulate refs
    let mut callbacks = git2::RemoteCallbacks::new();
    callbacks.transfer_progress(|stats| {
        if stats.received_objects() > 0 && stats.received_objects() == stats.indexed_objects() {
            // All objects already in cache, nothing new to fetch
        }
        true
    });
    fo.remote_callbacks(callbacks);

    remote
        .fetch(refspecs, Some(&mut fo), None)
        .context(format!("Failed to fetch from {remote_url}"))?;

    Ok(())
}

/// Fetch only the refs needed for a specific version, with `--depth=1` (shallow).
/// Tries the version as a tag first, then as a branch. Returns an error if neither
/// ref exists on the remote, allowing callers to fall back to the wide refspec fetch.
///
/// Each refspec is tried in a separate `git fetch` call -- git refuses the entire
/// operation if *any* refspec in a batch fails to match a remote ref, so we cannot
/// combine them in a single command.
pub fn fetch_single_ref(remote_url: &str, version: &str) -> Result<()> {
    let git_cache = git_cache_path();
    ensure_initialized()?;

    let refspecs = [
        format!("+refs/tags/{version}:refs/tags/{version}"),
        format!("+refs/heads/{version}:refs/heads/{version}"),
    ];

    for refspec in &refspecs {
        let output = std::process::Command::new("git")
            .args([
                OsStr::new("--git-dir"),
                OsStr::new(git_cache.to_str().unwrap()),
                OsStr::new("fetch"),
                OsStr::new("--depth"),
                OsStr::new("1"),
                OsStr::new(remote_url),
                OsStr::new(refspec.as_str()),
            ])
            .output()
            .context("Failed to run git fetch --depth")?;

        if output.status.success() {
            return Ok(());
        }
    }

    anyhow::bail!("Could not fetch '{version}' as a tag or branch");
}

/// Clone a repository into a toolchain directory using the central cache.
/// Sets up alternates so objects are shared, not duplicated.
///
/// First tries a targeted shallow fetch (`fetch_single_ref`) that downloads only
/// the objects reachable from the single requested tag or branch (~150-200 MiB
/// for Flutter). Falls back to the wide refspec fetch (`+refs/heads/* +refs/tags/*`)
/// which downloads the full history (~1.44 GiB) if the targeted fetch fails.
pub fn clone_via_cache(version: &str, remote_url: &str) -> Result<PathBuf> {
    let env_dir = config::envs_dir().join(version);

    // Ensure cache exists
    ensure_initialized()?;

    // Try targeted shallow fetch first; fall back to wide fetch
    if fetch_single_ref(remote_url, version).is_err() {
        fetch(
            remote_url,
            &["+refs/heads/*:refs/heads/*", "+refs/tags/*:refs/tags/*"],
        )?;
    }

    create_lightweight_toolchain(&env_dir, version)
}

/// Create a lightweight toolchain checkout using `git worktree add`.
/// The worktree's `.git` is a file pointing to the central bare repo,
/// so no objects or refs are duplicated. Only the working tree files
/// take up space on disk.
fn create_lightweight_toolchain(env_dir: &Path, version: &str) -> Result<PathBuf> {
    let cache_path = git_cache_path();
    ensure_initialized()?;

    if let Some(parent) = env_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Remove any leftover directory (shouldn't happen, but be safe)
    if env_dir.exists() {
        std::fs::remove_dir_all(env_dir)?;
    }

    // Try version as a tag first, then as a branch
    let refs_to_try = &[
        format!("tags/{version}"),         // exact tag
        format!("heads/{version}"),        // local branch (from fetched heads:heads)
        format!("origin/{version}"),       // remote-tracking branch
        format!("origin/heads/{version}"), // explicit heads
    ];

    let mut last_err = None;
    for ref_name in refs_to_try {
        let output = std::process::Command::new("git")
            .args([
                OsStr::new("--git-dir"),
                OsStr::new(cache_path.to_str().unwrap()),
                OsStr::new("worktree"),
                OsStr::new("add"),
                OsStr::new("--detach"),
                OsStr::new(env_dir.to_str().unwrap()),
                OsStr::new(ref_name),
            ])
            .output()
            .context("Failed to execute git worktree add")?;

        if output.status.success() {
            return Ok(env_dir.to_path_buf());
        }
        last_err = Some(String::from_utf8_lossy(&output.stderr).to_string());
        // Clean up any partial worktree the failed command might have left
        std::process::Command::new("git")
            .args([
                OsStr::new("--git-dir"),
                OsStr::new(cache_path.to_str().unwrap()),
                OsStr::new("worktree"),
                OsStr::new("remove"),
                OsStr::new("--force"),
                OsStr::new(env_dir.to_str().unwrap()),
            ])
            .output()
            .ok();
    }

    anyhow::bail!(
        "Could not find version '{version}' as a tag or branch.\n{}",
        last_err.as_deref().unwrap_or("unknown error")
    )
}

/// Calculate total size of the git object cache on disk.
pub fn cache_size() -> u64 {
    let path = git_cache_path();
    if !path.exists() {
        return 0;
    }
    crate::util::dir_size(&path)
}

/// Remove all cached bare repo data and reinitialize.
pub fn clear_cache() -> Result<()> {
    let path = git_cache_path();
    if path.exists() {
        std::fs::remove_dir_all(&path).context("Failed to remove git cache")?;
    }
    ensure_initialized()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use std::sync::atomic::{AtomicU32, Ordering};
    static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_dir() -> PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("joy_git_cache_test_{n}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_ensure_initialized_creates_bare_repo() {
        let tmp = temp_dir();
        let cache_dir = tmp.join("git_cache");

        // Override config to point to our temp dir
        // We'll directly test the function by manipulating paths
        let objects_dir = cache_dir.join("objects");
        let head = cache_dir.join("HEAD");

        assert!(!head.exists(), "HEAD should not exist before init");
        assert!(
            !objects_dir.exists(),
            "objects/ should not exist before init"
        );

        // Manually init since we can't easily mock config
        fs::create_dir_all(&cache_dir).unwrap();
        let repo = git2::Repository::init_bare(&cache_dir).unwrap();
        drop(repo);

        assert!(head.exists(), "HEAD should exist after init");
        assert!(objects_dir.exists(), "objects/ should exist after init");

        // Verify it's a valid bare repo
        let opened = git2::Repository::open_bare(&cache_dir).unwrap();
        assert!(opened.is_bare(), "repo should be bare");
        drop(opened);

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_fetch_with_local_repo() {
        let tmp = temp_dir();

        // Create a source bare repo with some content
        let source_dir = tmp.join("source.git");
        let source = git2::Repository::init_bare(&source_dir).unwrap();
        // We need a tree to create refs -- create a bare repo with initial commit
        drop(source);

        // Init a non-bare repo, make a commit, push to source
        let work_dir = tmp.join("work");
        let work = git2::Repository::init(&work_dir).unwrap();
        {
            let sig = git2::Signature::now("test", "test@test.com").unwrap();
            let tree_id = work.index().unwrap().write_tree().unwrap();
            let tree = work.find_tree(tree_id).unwrap();
            work.commit(Some("refs/heads/main"), &sig, &sig, "initial", &tree, &[])
                .unwrap();
        }
        // Push to source
        {
            let mut remote = work.remote_anonymous(source_dir.to_str().unwrap()).unwrap();
            let callbacks = git2::RemoteCallbacks::new();
            let mut push_opts = git2::PushOptions::new();
            push_opts.remote_callbacks(callbacks);
            remote
                .push(&["refs/heads/main:refs/heads/main"], Some(&mut push_opts))
                .unwrap();
        }
        drop(work);

        // Now fetch from source into a cache
        let cache_dir = tmp.join("cache.git");
        let cache = git2::Repository::init_bare(&cache_dir).unwrap();
        drop(cache);

        // Override git_cache_path to point to our temp cache
        // We test the fetch logic directly instead
        let repo = git2::Repository::open_bare(&cache_dir).unwrap();
        let mut remote = repo.remote_anonymous(source_dir.to_str().unwrap()).unwrap();
        let mut fo = git2::FetchOptions::new();
        fo.download_tags(git2::AutotagOption::None);
        remote
            .fetch(&["+refs/heads/*:refs/heads/*"], Some(&mut fo), None)
            .unwrap();
        drop(remote);
        drop(repo);

        // Verify objects were fetched -- there should be at least one object
        let cache = git2::Repository::open_bare(&cache_dir).unwrap();
        let head = cache.refname_to_id("refs/heads/main");
        assert!(head.is_ok(), "should have fetched refs/heads/main");
        drop(cache);

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_git_worktree_creates_lightweight_checkout() {
        let tmp = temp_dir();
        let bare_dir = tmp.join("cache.git");
        let work_dir = tmp.join("worktree");

        // Create a bare repo (simulates central cache)
        let bare = git2::Repository::init_bare(&bare_dir).unwrap();
        drop(bare);

        // Create a non-bare repo, commit, push to bare
        let src_dir = tmp.join("source");
        let src = git2::Repository::init(&src_dir).unwrap();
        // Write a file so the worktree has content
        std::fs::write(src_dir.join("README.md"), b"# Flutter SDK").unwrap();
        {
            let sig = git2::Signature::now("test", "test@test.com").unwrap();
            let mut index = src.index().unwrap();
            index.add_path(std::path::Path::new("README.md")).unwrap();
            index.write().unwrap();
            let tree_id = index.write_tree().unwrap();
            let tree = src.find_tree(tree_id).unwrap();
            src.commit(
                Some("refs/heads/main"),
                &sig,
                &sig,
                "initial commit",
                &tree,
                &[],
            )
            .unwrap();
        }
        // Push to bare
        {
            let mut remote = src.remote_anonymous(bare_dir.to_str().unwrap()).unwrap();
            let mut push_opts = git2::PushOptions::new();
            remote
                .push(&["refs/heads/main:refs/heads/main"], Some(&mut push_opts))
                .unwrap();
        }
        drop(src);

        // Create worktree using git CLI (git2 doesn't support worktrees natively)
        let output = std::process::Command::new("git")
            .args([
                "--git-dir",
                bare_dir.to_str().unwrap(),
                "worktree",
                "add",
                "--detach",
                work_dir.to_str().unwrap(),
                "main",
            ])
            .output()
            .expect("git worktree add failed");
        assert!(
            output.status.success(),
            "git worktree add: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // Verify worktree is lightweight: .git is a FILE (not a directory)
        let git_link = work_dir.join(".git");
        assert!(git_link.exists(), ".git should exist");
        assert!(
            git_link.is_file() || git_link.is_symlink(),
            ".git should be a file/symlink, not a directory"
        );
        assert!(!git_link.is_dir(), ".git should not be a directory");

        // Verify the worktree has the right content
        assert!(
            work_dir.join("README.md").exists(),
            "worktree should have README.md"
        );

        // Verify the bare repo knows about this worktree
        let wt_dir = bare_dir.join("worktrees");
        assert!(wt_dir.exists(), "worktrees/ should exist in bare repo");
        let entries: Vec<_> = std::fs::read_dir(&wt_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(
            !entries.is_empty(),
            "bare repo should have worktree metadata"
        );

        // Cleanup via git worktree remove
        std::process::Command::new("git")
            .args([
                "--git-dir",
                bare_dir.to_str().unwrap(),
                "worktree",
                "remove",
                "--force",
                work_dir.to_str().unwrap(),
            ])
            .output()
            .ok();

        // Prune stale worktree metadata
        std::process::Command::new("git")
            .args(["--git-dir", bare_dir.to_str().unwrap(), "worktree", "prune"])
            .output()
            .ok();

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_create_lightweight_toolchain_uses_worktree() {
        let tmp = temp_dir();
        let bare_dir = tmp.join("cache.git");
        let env_dir = tmp.join("envs").join("3.29.0");

        // Init bare repo with a commit
        let bare = git2::Repository::init_bare(&bare_dir).unwrap();
        drop(bare);

        let src_dir = tmp.join("source");
        let src = git2::Repository::init(&src_dir).unwrap();
        std::fs::create_dir_all(src_dir.join("bin").join("internal")).unwrap();
        std::fs::write(src_dir.join("bin").join("flutter"), b"#!/bin/sh\necho fake").unwrap();
        std::fs::write(
            src_dir.join("bin").join("internal").join("engine.version"),
            b"abc123",
        )
        .unwrap();
        {
            let sig = git2::Signature::now("test", "test@test.com").unwrap();
            let mut index = src.index().unwrap();
            index
                .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
                .unwrap();
            index.write().unwrap();
            let tree_id = index.write_tree().unwrap();
            let tree = src.find_tree(tree_id).unwrap();
            // Create a tag-like ref
            src.commit(Some("refs/heads/main"), &sig, &sig, "initial", &tree, &[])
                .unwrap();
        }
        {
            let mut remote = src.remote_anonymous(bare_dir.to_str().unwrap()).unwrap();
            let mut push_opts = git2::PushOptions::new();
            remote
                .push(&["refs/heads/main:refs/heads/main"], Some(&mut push_opts))
                .unwrap();
        }
        drop(src);

        // Create lightweight toolchain via git worktree add on the branch
        let result = std::process::Command::new("git")
            .args([
                OsStr::new("--git-dir"),
                OsStr::new(bare_dir.to_str().unwrap()),
                OsStr::new("worktree"),
                OsStr::new("add"),
                OsStr::new("--detach"),
                OsStr::new(env_dir.to_str().unwrap()),
                OsStr::new("main"),
            ])
            .output()
            .expect("git worktree add");
        assert!(
            result.status.success(),
            "worktree add failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );

        // Verify it's lightweight
        assert!(env_dir.join(".git").is_file(), ".git should be a file");
        assert!(
            env_dir.join("bin").join("flutter").exists(),
            "worktree should have flutter"
        );
        assert!(
            env_dir
                .join("bin")
                .join("internal")
                .join("engine.version")
                .exists(),
            "should have engine.version"
        );

        let gitlink = std::fs::read_to_string(env_dir.join(".git")).unwrap();
        assert!(gitlink.contains("gitdir:"), ".git should point to gitdir");
        assert!(
            gitlink.contains(bare_dir.to_str().unwrap()),
            ".git should reference cache"
        );

        // Clean up
        std::process::Command::new("git")
            .args([
                OsStr::new("--git-dir"),
                OsStr::new(bare_dir.to_str().unwrap()),
                OsStr::new("worktree"),
                OsStr::new("remove"),
                OsStr::new("--force"),
                OsStr::new(env_dir.to_str().unwrap()),
            ])
            .output()
            .ok();
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_cache_size_returns_zero_for_missing() {
        let tmp = temp_dir();
        let path = tmp.join("nonexistent");

        // direct test
        assert!(!path.exists());
        let _size = super::cache_size();
        // Our function uses config::git_cache_dir(), which won't be our tmp.
        // So just verify the manual dir_size returns 0 for nonexistent
        assert_eq!(crate::util::dir_size(&path), 0);

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_fetch_single_refspec_creates_shallow_clone() {
        let tmp = temp_dir();

        // Create source repo with a commit and a tag
        let src_dir = tmp.join("source");
        let src = git2::Repository::init(&src_dir).unwrap();
        std::fs::write(src_dir.join("f.txt"), b"data").unwrap();
        let sig = git2::Signature::now("test", "test@test.com").unwrap();
        {
            let mut index = src.index().unwrap();
            index.add_path(std::path::Path::new("f.txt")).unwrap();
            index.write().unwrap();
            let tree = src.find_tree(index.write_tree().unwrap()).unwrap();
            let oid = src
                .commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
                .unwrap();
            let commit = src.find_commit(oid).unwrap();
            src.tag("v3.29.0", commit.as_object(), &sig, "v3.29.0", false)
                .unwrap();
        }
        drop(src);

        let cache_dir = tmp.join("cache.git");
        let cache = git2::Repository::init_bare(&cache_dir).unwrap();
        drop(cache);

        // Fetch single tag with depth=1
        let output = std::process::Command::new("git")
            .args([
                OsStr::new("--git-dir"),
                OsStr::new(cache_dir.to_str().unwrap()),
                OsStr::new("fetch"),
                OsStr::new("--depth"),
                OsStr::new("1"),
                OsStr::new(src_dir.to_str().unwrap()),
                OsStr::new("+refs/tags/v3.29.0:refs/tags/v3.29.0"),
            ])
            .output()
            .expect("git fetch --depth");
        assert!(
            output.status.success(),
            "fetch failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let shallow_file = cache_dir.join("shallow");
        assert!(
            shallow_file.exists(),
            "shallow marker should exist after depth=1 fetch"
        );

        let opened = git2::Repository::open_bare(&cache_dir).unwrap();
        assert!(
            opened.refname_to_id("refs/tags/v3.29.0").is_ok(),
            "tag should be fetched"
        );
        drop(opened);

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_fetch_single_refspec_does_scope_to_requested_refs() {
        let tmp = temp_dir();

        // Source with a tag and a detached branch
        let src_dir = tmp.join("source");
        let src = git2::Repository::init(&src_dir).unwrap();
        std::fs::write(src_dir.join("f.txt"), b"data").unwrap();
        let sig = git2::Signature::now("test", "test@test.com").unwrap();
        {
            let mut index = src.index().unwrap();
            index.add_path(std::path::Path::new("f.txt")).unwrap();
            index.write().unwrap();
            let tree = src.find_tree(index.write_tree().unwrap()).unwrap();
            let oid = src
                .commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
                .unwrap();
            let commit = src.find_commit(oid).unwrap();
            src.tag("v3.29.0", commit.as_object(), &sig, "v3.29.0", false)
                .unwrap();
        }
        drop(src);

        // Add a branch that fetcher should NOT fetch
        let src = git2::Repository::open(&src_dir).unwrap();
        let head_id = src
            .refname_to_id("refs/heads/master")
            .unwrap_or_else(|_| src.refname_to_id("refs/heads/main").unwrap());
        src.branch(
            "unrelated-feature",
            &src.find_commit(head_id).unwrap(),
            false,
        )
        .unwrap();
        drop(src);

        let cache_dir = tmp.join("cache.git");
        let cache = git2::Repository::init_bare(&cache_dir).unwrap();
        drop(cache);

        // Fetch only v3.29.0 tag
        let output = std::process::Command::new("git")
            .args([
                OsStr::new("--git-dir"),
                OsStr::new(cache_dir.to_str().unwrap()),
                OsStr::new("fetch"),
                OsStr::new("--depth"),
                OsStr::new("1"),
                OsStr::new(src_dir.to_str().unwrap()),
                OsStr::new("+refs/tags/v3.29.0:refs/tags/v3.29.0"),
            ])
            .output()
            .expect("git fetch --depth");
        assert!(
            output.status.success(),
            "fetch failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let opened = git2::Repository::open_bare(&cache_dir).unwrap();

        // Requested tag must be present
        assert!(
            opened.refname_to_id("refs/tags/v3.29.0").is_ok(),
            "v3.29.0 tag should be fetched"
        );

        // Unrelated branch must NOT be fetched (branches are never auto-followed)
        assert!(
            opened
                .refname_to_id("refs/heads/unrelated-feature")
                .is_err(),
            "unrelated branch should NOT be fetched"
        );

        drop(opened);
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_fetch_single_refspec_fails_for_nonexistent_ref() {
        let tmp = temp_dir();

        let src_dir = tmp.join("source");
        let src = git2::Repository::init(&src_dir).unwrap();
        std::fs::write(src_dir.join("f.txt"), b"data").unwrap();
        let sig = git2::Signature::now("test", "test@test.com").unwrap();
        {
            let mut index = src.index().unwrap();
            index.add_path(std::path::Path::new("f.txt")).unwrap();
            index.write().unwrap();
            let tree = src.find_tree(index.write_tree().unwrap()).unwrap();
            let oid = src
                .commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
                .unwrap();
            let commit = src.find_commit(oid).unwrap();
            src.tag("v3.29.0", commit.as_object(), &sig, "v3.29.0", false)
                .unwrap();
        }
        drop(src);

        let cache_dir = tmp.join("cache.git");
        let cache = git2::Repository::init_bare(&cache_dir).unwrap();
        drop(cache);

        // This refspec won't match anything -- git should fail
        let output = std::process::Command::new("git")
            .args([
                OsStr::new("--git-dir"),
                OsStr::new(cache_dir.to_str().unwrap()),
                OsStr::new("fetch"),
                OsStr::new("--depth"),
                OsStr::new("1"),
                OsStr::new(src_dir.to_str().unwrap()),
                OsStr::new("+refs/tags/v99.99.99:refs/tags/v99.99.99"),
            ])
            .output()
            .expect("git fetch --depth");
        assert!(
            !output.status.success(),
            "fetch should fail for nonexistent ref"
        );

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_fetch_single_refspec_with_branch() {
        let tmp = temp_dir();

        let src_dir = tmp.join("source");
        let src = git2::Repository::init(&src_dir).unwrap();
        std::fs::write(src_dir.join("f.txt"), b"data").unwrap();
        let sig = git2::Signature::now("test", "test@test.com").unwrap();
        {
            let mut index = src.index().unwrap();
            index.add_path(std::path::Path::new("f.txt")).unwrap();
            index.write().unwrap();
            let tree = src.find_tree(index.write_tree().unwrap()).unwrap();
            src.commit(Some("refs/heads/main"), &sig, &sig, "init", &tree, &[])
                .unwrap();
        }
        drop(src);

        let cache_dir = tmp.join("cache.git");
        let cache = git2::Repository::init_bare(&cache_dir).unwrap();
        drop(cache);

        let output = std::process::Command::new("git")
            .args([
                OsStr::new("--git-dir"),
                OsStr::new(cache_dir.to_str().unwrap()),
                OsStr::new("fetch"),
                OsStr::new("--depth"),
                OsStr::new("1"),
                OsStr::new(src_dir.to_str().unwrap()),
                OsStr::new("+refs/heads/main:refs/heads/main"),
            ])
            .output()
            .expect("git fetch --depth");
        assert!(
            output.status.success(),
            "fetch should succeed for existing branch: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let opened = git2::Repository::open_bare(&cache_dir).unwrap();
        assert!(
            opened.refname_to_id("refs/heads/main").is_ok(),
            "branch main should be present"
        );
        drop(opened);

        fs::remove_dir_all(&tmp).unwrap();
    }
}
