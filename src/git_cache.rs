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
// Fresh ──discover_ref──▶ RemoteDiscovered ──fetch_shallow──▶ Fresh ──create_worktree──▶ ()
// ```

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
    pub fn open_or_init() -> Result<Self> {
        let path = config::git_cache_dir()?;
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
        Ok(Self {
            repo,
            path,
            state: Fresh,
        })
    }

    /// Transition **Fresh → RemoteDiscovered** by asking the remote which ref
    /// exists for `version`.
    pub fn discover_ref(
        self,
        remote_url: &str,
        version: &Version,
    ) -> Result<GitCache<RemoteDiscovered>> {
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

        let tag_ref = format!("refs/tags/{}", version.as_str());
        let branch_ref = format!("refs/heads/{}", version.as_str());

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

// Discovered state: must call fetch_shallow before create_worktree

impl GitCache<RemoteDiscovered> {
    /// Transition **RemoteDiscovered → Fresh** by shallow-fetching the
    /// previously-discovered ref into the shared bare repository.
    pub fn fetch_shallow(self, remote_url: &str, version: &Version) -> Result<GitCache<Fresh>> {
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
            eprintln!("Fetched Flutter {version}");
        }

        Ok(GitCache {
            repo: self.repo,
            path: self.path,
            state: Fresh,
        })
    }
}

// Worktree creation: callable on Fresh (after fetch)

impl GitCache<Fresh> {
    /// Create a lightweight worktree (`.git` is a file, not a directory).
    /// The worktree references objects in the shared bare repo.
    pub fn create_worktree(&self, version: &Version, env_dir: &Path) -> Result<()> {
        if let Some(parent) = env_dir.parent() {
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

        std::fs::create_dir_all(env_dir).with_context(|| {
            format!(
                "Failed to create worktree directory at {}",
                env_dir.display()
            )
        })?;

        checkout_tree(&tree, env_dir)
            .with_context(|| format!("Failed to checkout worktree at {}", env_dir.display()))?;

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
}

/// Path to the central bare Git repository used as object cache.
pub fn git_cache_path() -> Result<PathBuf> {
    config::git_cache_dir()
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

/// Remove all cached bare repo data and re-initialise.
pub fn clear_cache() -> Result<()> {
    let path = git_cache_path()?;
    if path.exists() {
        std::fs::remove_dir_all(&path).context("Failed to remove git cache")?;
    }
    GitCache::<Fresh>::open_or_init()?;
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
