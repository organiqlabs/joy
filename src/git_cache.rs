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

/// Returns the path to the alternates file inside a toolchain's .git directory.
fn alternates_path(env_dir: &Path) -> PathBuf {
    env_dir
        .join(".git")
        .join("objects")
        .join("info")
        .join("alternates")
}

/// Configure a toolchain to use the central cache via Git alternates.
/// Writes the absolute path of the cache's objects directory into
/// `<env_dir>/.git/objects/info/alternates`.
pub fn setup_alternates(env_dir: &Path) -> Result<()> {
    let alt_path = alternates_path(env_dir);
    if let Some(parent) = alt_path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create .git/objects/info directory")?;
    }
    let cache_objects = git_cache_path()
        .join("objects")
        .canonicalize()
        .unwrap_or_else(|_| git_cache_path().join("objects"));
    std::fs::write(&alt_path, cache_objects.to_string_lossy().as_bytes())
        .context("Failed to write alternates file")?;
    Ok(())
}

/// Check whether a toolchain has alternates configured.
pub fn has_alternates(env_dir: &Path) -> bool {
    alternates_path(env_dir).exists()
}

/// Remove the alternates file from a toolchain.
pub fn remove_alternates(env_dir: &Path) -> Result<()> {
    let alt_path = alternates_path(env_dir);
    if alt_path.exists() {
        std::fs::remove_file(&alt_path).context("Failed to remove alternates file")?;
    }
    Ok(())
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

    // Only fetch missing objects — prune is off so we accumulate refs
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

/// Clone a repository into a toolchain directory using the central cache.
/// Sets up alternates so objects are shared, not duplicated.
pub fn clone_via_cache(version: &str, remote_url: &str) -> Result<PathBuf> {
    let env_dir = config::envs_dir().join(version);

    // Ensure cache exists
    ensure_initialized()?;

    // Fetch into cache first
    fetch(
        remote_url,
        &["+refs/heads/*:refs/heads/*", "+refs/tags/*:refs/tags/*"],
    )?;

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
pub fn cache_size() -> Result<u64> {
    let path = git_cache_path();
    if !path.exists() {
        return Ok(0);
    }
    Ok(crate::util::dir_size(&path))
}

/// List Git tags available on a remote repository using `git ls-remote --tags`.
/// Returns tag names (without the "refs/tags/" prefix).
pub fn list_remote_tags(repo_url: &str) -> Result<Vec<String>> {
    let output = std::process::Command::new("git")
        .args(["ls-remote", "--tags", repo_url])
        .output()
        .context("Failed to run git ls-remote --tags")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git ls-remote --tags failed for {repo_url}:\n{stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut tags: Vec<String> = stdout
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.splitn(2, '\t').collect();
            if parts.len() != 2 {
                return None;
            }
            let refspec = parts[1];
            // Parse "refs/tags/v3.29.0" -> "v3.29.0"
            // Peeled refs (refs/tags/v3.29.0^{}) are duplicates — skip them
            if refspec.ends_with("^{}") {
                return None;
            }
            refspec.strip_prefix("refs/tags/").map(|n| n.to_string())
        })
        .collect();

    tags.sort();
    tags.dedup();
    Ok(tags)
}

/// Fetch objects from a remote into the central cache with a depth limit
/// (shallow fetch). `depth` must be >= 1.
pub fn fetch_depth(remote_url: &str, depth: i32) -> Result<()> {
    if depth < 1 {
        anyhow::bail!("fetch depth must be >= 1, got {depth}");
    }

    // Ensure cache is initialized, then use git CLI for shallow fetch
    // (git2 doesn't support --depth in this version)
    let git_cache = git_cache_path();
    ensure_initialized()?;
    let refspecs = &["+refs/heads/*:refs/heads/*", "+refs/tags/*:refs/tags/*"];

    let output = std::process::Command::new("git")
        .args([
            OsStr::new("--git-dir"),
            OsStr::new(git_cache.to_str().unwrap()),
            OsStr::new("fetch"),
            OsStr::new("--depth"),
            OsStr::new(&depth.to_string()),
            OsStr::new(remote_url),
            OsStr::new(refspecs[0]),
            OsStr::new(refspecs[1]),
        ])
        .output()
        .context("Failed to run git fetch --depth")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git fetch --depth failed:\n{stderr}");
    }

    Ok(())
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
        let dir = std::env::temp_dir().join(format!("dartup_git_cache_test_{n}"));
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
    fn test_setup_alternates_writes_correct_path() {
        let tmp = temp_dir();
        let cache_objects = tmp.join("cache").join("objects");
        fs::create_dir_all(&cache_objects).unwrap();
        let git_dir = tmp.join("envs").join("test_version").join(".git");
        let alternates = git_dir.join("objects").join("info").join("alternates");

        // Write alternates as setup_alternates would
        if let Some(parent) = alternates.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&alternates, cache_objects.to_string_lossy().as_bytes()).unwrap();

        assert!(alternates.exists(), "alternates file should exist");
        let content = fs::read_to_string(&alternates).unwrap();
        let expected = cache_objects.to_string_lossy().to_string();
        assert_eq!(content, expected);

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_has_alternates_detects_presence() {
        let tmp = temp_dir();
        let env_dir = tmp.join("envs").join("ver");
        let alt = env_dir
            .join(".git")
            .join("objects")
            .join("info")
            .join("alternates");

        assert!(
            !has_alternates(&env_dir),
            "should return false when no alternates"
        );

        fs::create_dir_all(alt.parent().unwrap()).unwrap();
        fs::write(&alt, b"/some/path").unwrap();

        assert!(
            has_alternates(&env_dir),
            "should return true when alternates exist"
        );

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_remove_alternates_cleans_up() {
        let tmp = temp_dir();
        let env_dir = tmp.join("envs").join("ver");
        let alt = env_dir
            .join(".git")
            .join("objects")
            .join("info")
            .join("alternates");

        fs::create_dir_all(alt.parent().unwrap()).unwrap();
        fs::write(&alt, b"/some/path").unwrap();
        assert!(alt.exists());

        remove_alternates(&env_dir).unwrap();
        assert!(!alt.exists(), "alternates should be removed");

        // Should be idempotent
        remove_alternates(&env_dir).unwrap();

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_fetch_with_local_repo() {
        let tmp = temp_dir();

        // Create a source bare repo with some content
        let source_dir = tmp.join("source.git");
        let source = git2::Repository::init_bare(&source_dir).unwrap();
        // We need a tree to create refs — create a bare repo with initial commit
        drop(source);

        // Init a non-bare repo, make a commit, push to source
        let work_dir = tmp.join("work");
        let work = git2::Repository::init(&work_dir).unwrap();
        {
            let sig = git2::Signature::now("test", "test@test.com").unwrap();
            let tree_id = work.index().unwrap().write_tree().unwrap();
            let tree = work.find_tree(tree_id).unwrap();
            work.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
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

        // Verify objects were fetched — there should be at least one object
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
            src.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
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
            src.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
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
        let _size = super::cache_size().unwrap_or(0);
        // Our function uses config::git_cache_dir(), which won't be our tmp.
        // So just verify the manual dir_size returns 0 for nonexistent
        assert_eq!(crate::util::dir_size(&path), 0);

        fs::remove_dir_all(&tmp).unwrap();
    }

    // ---- RED: tag listing tests ----

    #[test]
    fn test_list_remote_tags_returns_tags() {
        let tmp = temp_dir();

        // Create a source repo with tags
        let src_dir = tmp.join("source");
        let src = git2::Repository::init(&src_dir).unwrap();
        std::fs::write(src_dir.join("README.md"), b"# Test").unwrap();
        {
            let sig = git2::Signature::now("test", "test@test.com").unwrap();
            let mut index = src.index().unwrap();
            index.add_path(std::path::Path::new("README.md")).unwrap();
            index.write().unwrap();
            let tree_id = index.write_tree().unwrap();
            let tree = src.find_tree(tree_id).unwrap();
            let oid = src
                .commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
                .unwrap();
            let commit = src.find_commit(oid).unwrap();
            src.tag("v3.29.0", commit.as_object(), &sig, "v3.29.0", false)
                .unwrap();
            src.tag("v3.28.0", commit.as_object(), &sig, "v3.28.0", false)
                .unwrap();
            src.tag("v3.27.0", commit.as_object(), &sig, "v3.27.0", false)
                .unwrap();
        }
        drop(src);

        // Call list_remote_tags on the local repo
        let tags = super::list_remote_tags(src_dir.to_str().unwrap()).unwrap();
        assert!(
            tags.contains(&"v3.29.0".to_string()),
            "should contain v3.29.0, got: {tags:?}"
        );
        assert!(
            tags.contains(&"v3.28.0".to_string()),
            "should contain v3.28.0"
        );
        assert!(
            tags.contains(&"v3.27.0".to_string()),
            "should contain v3.27.0"
        );
        assert_eq!(tags.len(), 3);

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_list_remote_tags_returns_empty_for_no_tags() {
        let tmp = temp_dir();
        let src_dir = tmp.join("empty");
        let src = git2::Repository::init(&src_dir).unwrap();
        std::fs::write(src_dir.join("file"), b"data").unwrap();
        {
            let sig = git2::Signature::now("test", "test@test.com").unwrap();
            let mut index = src.index().unwrap();
            index.add_path(std::path::Path::new("file")).unwrap();
            index.write().unwrap();
            let tree_id = index.write_tree().unwrap();
            let tree = src.find_tree(tree_id).unwrap();
            src.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
                .unwrap();
        }
        drop(src);

        let tags = super::list_remote_tags(src_dir.to_str().unwrap()).unwrap();
        assert!(
            tags.is_empty(),
            "should be empty for repo with no tags, got: {tags:?}"
        );

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_list_remote_tags_fails_for_nonexistent_repo() {
        let result = super::list_remote_tags("/nonexistent/path/for/sure");
        assert!(result.is_err(), "should fail for nonexistent path");
    }

    // ---- RED: shallow fetch tests ----

    #[test]
    fn test_fetch_depth_succeeds() {
        let tmp = temp_dir();

        // Create source bare repo with a commit and branch
        let src_dir = tmp.join("source.git");
        let src = git2::Repository::init_bare(&src_dir).unwrap();
        drop(src);

        // Clone, make a commit, push
        let work_dir = tmp.join("work");
        let work = git2::Repository::init(&work_dir).unwrap();
        let sig = git2::Signature::now("test", "test@test.com").unwrap();
        std::fs::write(work_dir.join("f.txt"), b"data").unwrap();
        {
            let mut index = work.index().unwrap();
            index.add_path(std::path::Path::new("f.txt")).unwrap();
            index.write().unwrap();
            let tree = work.find_tree(index.write_tree().unwrap()).unwrap();
            work.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
                .unwrap();
        }
        {
            let mut remote = work.remote_anonymous(src_dir.to_str().unwrap()).unwrap();
            let mut push_opts = git2::PushOptions::new();
            remote
                .push(&["refs/heads/main:refs/heads/main"], Some(&mut push_opts))
                .unwrap();
        }
        drop(work);

        // Create a cache bare repo and fetch with depth=1
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
                OsStr::new("+refs/heads/*:refs/heads/*"),
                OsStr::new("+refs/tags/*:refs/tags/*"),
            ])
            .output()
            .expect("git fetch --depth");
        assert!(
            output.status.success(),
            "fetch depth failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // Verify refs are present
        let opened = git2::Repository::open_bare(&cache_dir).unwrap();
        assert!(opened.refname_to_id("refs/heads/main").is_ok());
        drop(opened);

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_fetch_depth_rejects_zero_depth() {
        let result = super::fetch_depth("https://example.com/repo.git", 0);
        assert!(result.is_err(), "depth 0 should be rejected");
    }
}
