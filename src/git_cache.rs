use crate::config;
use anyhow::{Context, Result};
use gix::bstr::ByteSlice;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// Whether a remote ref is a tag or a branch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RefKind {
    Tag,
    Branch,
}

/// Shared bare git repository — the central object cache for all worktrees.
pub struct GitCache {
    repo: gix::Repository,
    path: PathBuf,
}

impl GitCache {
    /// Open the existing bare repo at `{cache_root}/git`, or initialise one.
    pub fn open_or_init() -> Result<Self> {
        let path = config::git_cache_dir();
        let repo = if path.join("HEAD").exists() {
            gix::open(&path)
                .with_context(|| format!("Failed to open git cache at {}", path.display()))?
        } else {
            std::fs::create_dir_all(&path).with_context(|| {
                format!("Failed to create git cache directory at {}", path.display())
            })?;
            let ts_repo = gix::ThreadSafeRepository::init_opts(
                &path,
                gix::create::Kind::Bare,
                gix::create::Options::default(),
                gix::open::Options::default(),
            )
            .with_context(|| {
                format!("Failed to initialise bare git cache at {}", path.display())
            })?;
            let _ = std::fs::create_dir_all(path.join("objects").join("info"));
            ts_repo.into()
        };
        Ok(Self { repo, path })
    }

    /// Ask the remote which refs exist for `version`.
    /// Returns `RefKind::Tag` or `RefKind::Branch` — whichever matches first.
    pub fn discover_ref(&self, remote_url: &str, version: &str) -> Result<RefKind> {
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

        let tag_ref = format!("refs/tags/{version}");
        let branch_ref = format!("refs/heads/{version}");

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
                return Ok(RefKind::Tag);
            }
            if name == branch_ref.as_str() {
                return Ok(RefKind::Branch);
            }
        }

        anyhow::bail!("Could not find a remote tag or branch named '{version}' at {remote_url}")
    }

    /// Shallow-fetch (`--depth=1`) a single ref into the shared bare repository.
    pub fn fetch_shallow(&self, remote_url: &str, version: &str, kind: RefKind) -> Result<()> {
        let refspec = match kind {
            RefKind::Tag => format!("+refs/tags/{version}:refs/tags/{version}"),
            RefKind::Branch => format!("+refs/heads/{version}:refs/heads/{version}"),
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
            eprintln!("Fetched Flutter {version}");
        }

        Ok(())
    }

    /// Create a lightweight worktree (`.git` is a file, not a directory).
    /// The worktree references objects in the shared bare repo.
    pub fn create_worktree(&self, version: &str, env_dir: &Path) -> Result<()> {
        if let Some(parent) = env_dir.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| "Failed to create parent directory for worktree".to_string())?;
        }

        let commit = match self.resolve_ref(version) {
            Ok(id) => id,
            Err(e) => anyhow::bail!("Could not resolve ref for version '{version}': {e}"),
        };

        let commit_obj = self
            .repo
            .find_object(commit)
            .with_context(|| format!("Failed to find object for {commit}"))?;
        let tree = commit_obj
            .peel_to_tree()
            .with_context(|| "Failed to peel commit to tree".to_string())?;

        std::fs::create_dir_all(env_dir).with_context(|| {
            format!(
                "Failed to create worktree directory at {}",
                env_dir.display()
            )
        })?;

        checkout_tree(&tree, env_dir)
            .with_context(|| format!("Failed to checkout worktree at {}", env_dir.display()))?;

        let worktrees_dir = self.repo.common_dir().join("worktrees").join(version);
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

        let gitlink_content = format!("gitdir: {}/worktrees/{version}\n", self.path.display());
        std::fs::write(env_dir.join(".git"), &gitlink_content)
            .with_context(|| "Failed to write .git file for worktree".to_string())?;

        Ok(())
    }

    /// Remove a worktree and prune stale metadata.
    pub fn remove_worktree(&self, version: &str) {
        let env_dir = config::envs_dir().join(version);
        let worktrees_dir = self.repo.common_dir().join("worktrees").join(version);

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
}

/// Path to the central bare Git repository used as object cache.
pub fn git_cache_path() -> PathBuf {
    config::git_cache_dir()
}

/// Calculate total size of the git object cache on disk.
pub fn cache_size() -> u64 {
    let path = git_cache_path();
    if !path.exists() {
        return 0;
    }
    crate::util::dir_size(&path)
}

/// Remove all cached bare repo data and re-initialise.
pub fn clear_cache() -> Result<()> {
    let path = git_cache_path();
    if path.exists() {
        std::fs::remove_dir_all(&path).context("Failed to remove git cache")?;
    }
    GitCache::open_or_init()?;
    Ok(())
}

/// Check whether a worktree's `.git` pointer is still valid.
pub fn worktree_is_valid(version: &str) -> bool {
    let env_dir = config::envs_dir().join(version);
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

fn checkout_tree(tree: &gix::Tree<'_>, dest: &Path) -> Result<()> {
    for entry in tree.iter() {
        let entry = entry.with_context(|| "Failed to read tree entry")?;
        let name = entry.filename().to_str().unwrap_or_default().to_string();
        let entry_path = dest.join(&name);

        let mode = entry.mode();
        if mode.is_tree() {
            std::fs::create_dir_all(&entry_path)
                .with_context(|| format!("Failed to create directory {entry_path:?}"))?;
            let subtree = entry
                .object()
                .with_context(|| format!("Failed to get subtree object for {name}"))?;
            let subtree = subtree
                .peel_to_tree()
                .with_context(|| format!("Failed to peel subtree for {name}"))?;
            checkout_tree(&subtree, &entry_path)?;
        } else if mode.is_blob() {
            if let Some(parent) = entry_path.parent() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("Failed to create parent directory for {entry_path:?}")
                })?;
            }
            let blob = entry
                .object()
                .with_context(|| format!("Failed to get blob object for {name}"))?;
            let data = &blob.data;
            std::fs::write(&entry_path, data)
                .with_context(|| format!("Failed to write {entry_path:?}"))?;
            if mode.is_executable() {
                set_executable(&entry_path)?;
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .with_context(|| format!("Failed to read metadata for {path:?}"))?
        .permissions();
    perms.set_mode(perms.mode() | 0o111);
    std::fs::set_permissions(path, perms)
        .with_context(|| format!("Failed to set permissions for {path:?}"))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gix::refs::Target;
    use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};
    use serial_test::serial;
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

    fn init_bare(path: &Path) -> gix::Repository {
        let ts = gix::ThreadSafeRepository::init_opts(
            path,
            gix::create::Kind::Bare,
            gix::create::Options::default(),
            gix::open::Options::default(),
        )
        .unwrap();
        ts.into()
    }

    fn create_source_with_tag(source_dir: &Path, tag: &str, files: &[(&str, &[u8])]) {
        let ts = gix::ThreadSafeRepository::init_opts(
            source_dir,
            gix::create::Kind::WithWorktree,
            gix::create::Options::default(),
            gix::open::Options::default(),
        )
        .unwrap();
        let repo: gix::Repository = ts.into();

        for (path, content) in files {
            let full_path = source_dir.join(path);
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&full_path, content).unwrap();
        }

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

        let commit_id = repo
            .commit_as(
                sig,
                sig,
                format!("refs/tags/{tag}"),
                "initial",
                tree_id,
                [] as [gix::hash::ObjectId; 0],
            )
            .unwrap()
            .detach();
        repo.edit_references_as(
            Some(RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: "set head".into(),
                    },
                    expected: PreviousValue::Any,
                    new: Target::Object(commit_id),
                },
                name: "HEAD".try_into().unwrap(),
                deref: false,
            }),
            Some(sig),
        )
        .unwrap();
    }

    #[test]
    #[serial]
    fn test_open_or_init_creates_bare_repo() {
        let tmp = temp_dir();
        let cache_path = tmp.join("cache.git");

        // Create config for testing
        // We can't override config easily, so test the init logic directly
        let repo = init_bare(&cache_path);
        assert!(cache_path.join("HEAD").exists());
        assert!(cache_path.join("objects").exists());
        drop(repo);

        // open_or_init through the struct
        unsafe {
            std::env::set_var("XDG_DATA_HOME", tmp.join("data"));
            std::env::set_var("XDG_CACHE_HOME", tmp.join("cache"));
        }
        // Can't easily test GitCache::open_or_init() without overriding config,
        // so we test the core functionality directly
        let reopened = gix::open(&cache_path).unwrap();
        assert!(reopened.is_bare());

        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
            std::env::remove_var("XDG_CACHE_HOME");
        }
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_resolve_ref_after_creation() {
        let tmp = temp_dir();
        let bare = tmp.join("bare.git");

        let _repo = {
            let ts = gix::ThreadSafeRepository::init_opts(
                &bare,
                gix::create::Kind::Bare,
                gix::create::Options::default(),
                gix::open::Options::default(),
            )
            .unwrap();
            let repo: gix::Repository = ts.into();

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
                "refs/tags/v3.29.0",
                "initial",
                tree_id,
                [] as [gix::hash::ObjectId; 0],
            )
            .unwrap();
            repo
        };

        // Reopen and resolve
        let repo = gix::open(&bare).unwrap();
        let mut r = repo.find_reference("refs/tags/v3.29.0").unwrap();
        let oid = r.peel_to_id().unwrap().detach();
        assert!(!oid.is_null());

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_checkout_tree_writes_files() {
        let tmp = temp_dir();
        let source = tmp.join("source");
        let checkout = tmp.join("checkout");

        create_source_with_tag(
            &source,
            "test",
            &[
                ("bin/flutter", b"#!/bin/sh\necho hello"),
                ("bin/internal/engine.version", b"abc123"),
                ("README.md", b"# Flutter SDK"),
            ],
        );

        let repo = gix::open(&source).unwrap();
        let tree = repo.head_commit().unwrap().tree().unwrap();
        checkout_tree(&tree, &checkout).unwrap();

        assert!(checkout.join("bin").join("flutter").exists());
        assert!(
            checkout
                .join("bin")
                .join("internal")
                .join("engine.version")
                .exists()
        );
        assert!(checkout.join("README.md").exists());

        let content = fs::read_to_string(checkout.join("README.md")).unwrap();
        assert_eq!(content.trim(), "# Flutter SDK");

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_checkout_tree_preserves_executable_permissions() {
        let tmp = temp_dir();
        let source = tmp.join("source");
        let checkout = tmp.join("checkout");

        let ts = gix::ThreadSafeRepository::init_opts(
            &source,
            gix::create::Kind::WithWorktree,
            gix::create::Options::default(),
            gix::open::Options::default(),
        )
        .unwrap();
        let repo: gix::Repository = ts.into();

        for (path, content) in [
            ("run.sh", &b"#!/bin/sh\necho run"[..]),
            ("readme.txt", &b"hello"[..]),
        ] {
            let full_path = source.join(path);
            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&full_path, content).unwrap();
        }

        let blob_exec_id = repo.write_blob(b"#!/bin/sh\necho run").unwrap().detach();
        let blob_reg_id = repo.write_blob(b"hello").unwrap().detach();

        let mut tree_entries = vec![
            gix::objs::tree::Entry {
                mode: gix::objs::tree::EntryKind::BlobExecutable.into(),
                filename: "run.sh".as_bytes().into(),
                oid: blob_exec_id,
            },
            gix::objs::tree::Entry {
                mode: gix::objs::tree::EntryKind::Blob.into(),
                filename: "readme.txt".as_bytes().into(),
                oid: blob_reg_id,
            },
        ];
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
            "refs/heads/main",
            "initial",
            tree_id,
            [] as [gix::hash::ObjectId; 0],
        )
        .unwrap();
        repo.edit_references_as(
            Some(RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: "set head".into(),
                    },
                    expected: PreviousValue::Any,
                    new: Target::Object(repo.head_commit().unwrap().id().detach()),
                },
                name: "HEAD".try_into().unwrap(),
                deref: false,
            }),
            Some(sig),
        )
        .unwrap();

        let tree = repo.head_commit().unwrap().tree().unwrap();
        checkout_tree(&tree, &checkout).unwrap();

        assert!(checkout.join("run.sh").exists());
        assert!(checkout.join("readme.txt").exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script_perms = fs::metadata(checkout.join("run.sh")).unwrap().permissions();
            assert!(
                script_perms.mode() & 0o111 != 0,
                "run.sh should be executable"
            );
            let text_perms = fs::metadata(checkout.join("readme.txt"))
                .unwrap()
                .permissions();
            assert!(
                text_perms.mode() & 0o111 == 0,
                "readme.txt should NOT be executable"
            );
        }

        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_worktree_is_valid_checks_gitlink() {
        let tmp = temp_dir();
        unsafe {
            std::env::set_var("XDG_DATA_HOME", tmp.join("xdg").join("data"));
            std::env::set_var("XDG_CACHE_HOME", tmp.join("xdg").join("cache"));
        }
        let target_dir = tmp.join("gitdir_target").join("3.44.4");

        let work_dir = config::envs_dir().join("3.44.4");
        fs::create_dir_all(&work_dir).unwrap();
        fs::create_dir_all(&target_dir).unwrap();
        fs::write(target_dir.join("HEAD"), b"ref: refs/heads/main\n").unwrap();

        let gitlink_content = format!("gitdir: {}\n", target_dir.display());
        fs::write(work_dir.join(".git"), &gitlink_content).unwrap();

        assert!(
            worktree_is_valid("3.44.4"),
            "valid .git file pointing to existing HEAD should pass"
        );

        // Break it
        fs::remove_file(target_dir.join("HEAD")).unwrap();
        assert!(
            !worktree_is_valid("3.44.4"),
            "broken gitdir target should fail"
        );

        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
            std::env::remove_var("XDG_CACHE_HOME");
        }
        fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_worktree_is_valid_returns_false_for_missing() {
        assert!(!worktree_is_valid("definitely-not-installed-v99"));
    }

    #[test]
    fn test_cache_size_returns_zero_for_missing() {
        let tmp = temp_dir();
        let path = tmp.join("nonexistent");
        assert!(!path.exists());

        // Test the util function directly
        assert_eq!(crate::util::dir_size(&path), 0);
        fs::remove_dir_all(&tmp).unwrap();
    }
}
