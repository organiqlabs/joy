use joy::config;
use joy::git_cache;
use joy::install;
use joy::profile::Profile;
use joy::types::Version;
use serial_test::serial;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

static NEXT_ID: AtomicU32 = AtomicU32::new(1000);

fn temp_dir() -> PathBuf {
    let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("joy_e2e_{id}"));
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

fn setup_xdg() -> (XdgGuard, PathBuf, PathBuf) {
    let tmp = temp_dir();
    let data_home = tmp.join("xdg").join("data");
    let cache_home = tmp.join("xdg").join("cache");
    std::fs::create_dir_all(&data_home).unwrap();
    std::fs::create_dir_all(&cache_home).unwrap();
    unsafe {
        std::env::set_var("XDG_DATA_HOME", &data_home);
        std::env::set_var("XDG_CACHE_HOME", &cache_home);
    }
    (XdgGuard(tmp), data_home, cache_home)
}

fn create_test_repo(dir: &Path, tag: &str, engine_ver: &str) {
    let ts = gix::ThreadSafeRepository::init_opts(
        dir,
        gix::create::Kind::WithWorktree,
        gix::create::Options::default(),
        gix::open::Options::default(),
    )
    .unwrap();
    let repo: gix::Repository = ts.into();

    std::fs::create_dir_all(dir.join("bin").join("internal")).unwrap();
    std::fs::write(dir.join("bin").join("flutter"), b"#!/bin/sh\necho fake").unwrap();
    std::fs::write(dir.join("bin").join("dart"), b"#!/bin/sh\necho fake dart").unwrap();
    std::fs::write(
        dir.join("bin").join("internal").join("engine.version"),
        engine_ver.as_bytes(),
    )
    .unwrap();

    let files: &[(&str, &[u8])] = &[
        ("bin/flutter", b"#!/bin/sh\necho fake"),
        ("bin/dart", b"#!/bin/sh\necho fake dart"),
        ("bin/internal/engine.version", engine_ver.as_bytes()),
    ];

    let mut tree_entries = Vec::new();
    for (path, content) in files {
        let blob_id = repo.write_blob(content).unwrap().detach();
        tree_entries.push(gix::objs::tree::Entry {
            mode: gix::objs::tree::EntryKind::Blob.into(),
            filename: path.as_bytes().into(),
            oid: blob_id,
        });
    }
    tree_entries.sort_by(|a, b| a.filename.cmp(&b.filename));
    let tree = gix::objs::Tree {
        entries: tree_entries,
    };
    let tree_id = repo.write_object(&tree).unwrap();

    let sig = gix::actor::SignatureRef {
        name: "test".into(),
        email: "test@test.com".into(),
        time: "0 +0000",
    };

    repo.commit_as(
        sig,
        sig,
        format!("refs/tags/{tag}"),
        "initial",
        tree_id,
        [] as [gix::hash::ObjectId; 0],
    )
    .unwrap();
}

fn pre_populate_engine(engine_ver: &str) {
    let engine_path = config::engine_cache_dir().unwrap().join(engine_ver);
    std::fs::create_dir_all(engine_path.join("bin")).unwrap();
    std::fs::write(
        engine_path.join("bin").join("flutter_engine"),
        b"fake engine",
    )
    .unwrap();
}

fn v(tag: &str) -> Version {
    Version::new(tag).unwrap()
}

#[test]
#[serial]
fn test_minimal_profile_skips_engine() {
    let tag = "minimal-test-v1";
    let engine_ver = "minimal-engine";

    let (_guard, _data_home, _cache_home) = setup_xdg();
    let remote_dir = temp_dir();
    create_test_repo(&remote_dir, tag, engine_ver);

    install::install_version_git_with_profile(
        &v(tag),
        Some(remote_dir.to_str().unwrap()),
        false,
        &Profile::Minimal,
        false,
    )
    .unwrap();

    let env_dir = config::envs_dir().unwrap().join(tag);
    assert!(
        env_dir.join("bin").join("flutter").exists(),
        "flutter binary should exist"
    );
    assert!(
        !env_dir.join("bin").join("cache").join("engine").exists(),
        "minimal profile should NOT create engine symlink"
    );
}

#[test]
#[serial]
fn test_default_profile_includes_engine() {
    let tag = "default-test-v1";
    let engine_ver = "default-engine";

    let (_guard, _data_home, _cache_home) = setup_xdg();
    let remote_dir = temp_dir();
    create_test_repo(&remote_dir, tag, engine_ver);
    pre_populate_engine(engine_ver);

    install::install_version_git_with_profile(
        &v(tag),
        Some(remote_dir.to_str().unwrap()),
        false,
        &Profile::Default,
        false,
    )
    .unwrap();

    let env_dir = config::envs_dir().unwrap().join(tag);
    assert!(
        env_dir
            .join("bin")
            .join("cache")
            .join("engine")
            .is_symlink(),
        "default profile should create engine symlink"
    );
}

#[test]
#[serial]
fn test_minimal_profile_and_no_engine_version_works() {
    let tag = "minimal-noeng-v1";
    let (_guard, _data_home, _cache_home) = setup_xdg();
    let remote_dir = temp_dir();
    let _repo = {
        let ts = gix::ThreadSafeRepository::init_opts(
            &remote_dir,
            gix::create::Kind::WithWorktree,
            gix::create::Options::default(),
            gix::open::Options::default(),
        )
        .unwrap();
        let repo: gix::Repository = ts.into();
        std::fs::create_dir_all(remote_dir.join("bin")).unwrap();
        std::fs::write(remote_dir.join("bin").join("flutter"), b"#!/bin/sh").unwrap();

        let blob_id = repo.write_blob(b"#!/bin/sh").unwrap().detach();
        let tree = gix::objs::Tree {
            entries: vec![gix::objs::tree::Entry {
                mode: gix::objs::tree::EntryKind::Blob.into(),
                filename: b"bin/flutter".into(),
                oid: blob_id,
            }],
        };
        let tree_id = repo.write_object(&tree).unwrap();

        let sig = gix::actor::SignatureRef {
            name: "test".into(),
            email: "test@test.com".into(),
            time: "0 +0000",
        };

        repo.commit_as(
            sig,
            sig,
            format!("refs/tags/{tag}"),
            "no engine",
            tree_id,
            [] as [gix::hash::ObjectId; 0],
        )
        .unwrap();
        repo
    };

    install::install_version_git_with_profile(
        &v(tag),
        Some(remote_dir.to_str().unwrap()),
        false,
        &Profile::Minimal,
        false,
    )
    .unwrap();

    let env_dir = config::envs_dir().unwrap().join(tag);
    assert!(
        env_dir.join("bin").join("flutter").exists(),
        "flutter binary should exist even with minimal profile"
    );
}

#[test]
#[serial]
fn test_auto_repair_broken_worktree() {
    let tag = "repair-test-v1";
    let engine_ver = "repair-engine";

    let (_guard, _data_home, _cache_home) = setup_xdg();
    let remote_dir = temp_dir();
    create_test_repo(&remote_dir, tag, engine_ver);

    install::install_version_git_with_profile(
        &v(tag),
        Some(remote_dir.to_str().unwrap()),
        false,
        &Profile::Minimal,
        false,
    )
    .unwrap();

    let env_dir = config::envs_dir().unwrap().join(tag);
    assert!(
        git_cache::worktree_is_valid(tag),
        "worktree should be valid after fresh install"
    );

    let cache_path = config::git_cache_dir().unwrap();
    assert!(cache_path.exists(), "cache should exist after install");
    // `clear_cache()` now refuses while git-based toolchains are linked (the
    // gc guard), so simulate the "cache cleared" state directly: removing the
    // worktree's registration in the bare repo is exactly what an orphaned
    // worktree looks like after a cache clear.
    let wt_dir = cache_path.join("worktrees").join(tag);
    assert!(
        wt_dir.exists(),
        "worktree registration should exist after install"
    );
    std::fs::remove_dir_all(&wt_dir).unwrap();

    assert!(
        !git_cache::worktree_is_valid(tag),
        "worktree should be broken after the registration is removed"
    );
    assert!(
        env_dir.join("bin").join("flutter").exists(),
        "SDK files should still exist even with broken .git"
    );

    install::install_version_git_with_profile(
        &v(tag),
        Some(remote_dir.to_str().unwrap()),
        false,
        &Profile::Minimal,
        false,
    )
    .unwrap();

    assert!(
        git_cache::worktree_is_valid(tag),
        "worktree should be valid after auto-repair"
    );
    assert!(
        env_dir.join("bin").join("flutter").exists(),
        "flutter binary should exist after repair"
    );

    let git_link = env_dir.join(".git");
    let content = std::fs::read_to_string(&git_link).unwrap();
    let gitdir_path = content.strip_prefix("gitdir: ").unwrap().trim().to_string();
    assert!(
        std::path::Path::new(&gitdir_path).join("HEAD").exists(),
        "repaired .git should point to valid gitdir"
    );
}

#[test]
#[serial]
fn test_valid_worktree_does_not_auto_repair() {
    let tag = "valid-v1";
    let engine_ver = "valid-engine";

    let (_guard, _data_home, _cache_home) = setup_xdg();
    let remote_dir = temp_dir();
    create_test_repo(&remote_dir, tag, engine_ver);

    install::install_version_git_with_profile(
        &v(tag),
        Some(remote_dir.to_str().unwrap()),
        false,
        &Profile::Minimal,
        false,
    )
    .unwrap();

    let result = install::install_version_git_with_profile(
        &v(tag),
        Some(remote_dir.to_str().unwrap()),
        false,
        &Profile::Minimal,
        false,
    );
    assert!(
        result.is_ok(),
        "valid worktree should report already installed"
    );
}

#[test]
#[serial]
fn test_missing_gitlink_is_not_valid() {
    let (_guard, _data_home, _cache_home) = setup_xdg();
    assert!(!git_cache::worktree_is_valid("nonexistent-version"));
}

/// Names of dot-prefixed internal dirs (staging/backup) in `envs_dir`, if any.
fn internal_leftovers() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(config::envs_dir().unwrap())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with('.'))
        .collect();
    names.sort();
    names
}

#[test]
#[serial]
fn failed_force_git_reinstall_preserves_previous_sdk() {
    let tag = "preserve-failed-v1";
    let engine_ver = "preserve-engine";

    let (_guard, _data_home, _cache_home) = setup_xdg();
    let remote_dir = temp_dir();
    create_test_repo(&remote_dir, tag, engine_ver);

    install::install_version_git_with_profile(
        &v(tag),
        Some(remote_dir.to_str().unwrap()),
        false,
        &Profile::Minimal,
        false,
    )
    .unwrap();

    let env_dir = config::envs_dir().unwrap().join(tag);
    let flutter = env_dir.join("bin").join("flutter");
    assert!(flutter.exists(), "initial install should exist");
    // Mark the working SDK so we can prove it survives a failed reinstall.
    std::fs::write(&flutter, "#!/bin/sh\necho ORIGINAL-SDK").unwrap();
    assert!(
        git_cache::worktree_is_valid(tag),
        "worktree should be valid before reinstall"
    );

    // Force reinstall from a bogus remote: the fetch phase fails before any
    // local mutation, so the previous SDK must be preserved verbatim.
    let result = install::install_version_git_with_profile(
        &v(tag),
        Some("/nonexistent/joy-e2e-remote"),
        true,
        &Profile::Minimal,
        false,
    );
    assert!(result.is_err(), "bogus remote must fail the reinstall");

    assert_eq!(
        std::fs::read_to_string(&flutter).unwrap(),
        "#!/bin/sh\necho ORIGINAL-SDK",
        "failed --force reinstall must preserve the previous SDK"
    );
    assert!(
        git_cache::worktree_is_valid(tag),
        "failed reinstall must leave the previous worktree linked"
    );
    assert!(
        internal_leftovers().is_empty(),
        "no staging/backup dirs may remain after a failed reinstall"
    );
}

#[test]
#[serial]
fn force_git_reinstall_replaces_sdk_and_leaves_no_leftovers() {
    let tag = "force-replace-v1";
    let engine_ver = "force-engine";

    let (_guard, _data_home, _cache_home) = setup_xdg();
    let remote_dir = temp_dir();
    create_test_repo(&remote_dir, tag, engine_ver);

    install::install_version_git_with_profile(
        &v(tag),
        Some(remote_dir.to_str().unwrap()),
        false,
        &Profile::Minimal,
        false,
    )
    .unwrap();

    let env_dir = config::envs_dir().unwrap().join(tag);
    let flutter = env_dir.join("bin").join("flutter");
    std::fs::write(&flutter, "#!/bin/sh\necho STALE").unwrap();

    // A successful --force reinstall replaces the SDK with a fresh checkout.
    install::install_version_git_with_profile(
        &v(tag),
        Some(remote_dir.to_str().unwrap()),
        true,
        &Profile::Minimal,
        false,
    )
    .unwrap();

    let content = std::fs::read_to_string(&flutter).unwrap();
    assert!(
        !content.contains("STALE"),
        "force reinstall should replace the SDK, got: {content}"
    );
    assert!(
        git_cache::worktree_is_valid(tag),
        "worktree must be valid after a force reinstall"
    );
    assert!(
        internal_leftovers().is_empty(),
        "no staging/backup dirs may remain after a successful reinstall"
    );
}
