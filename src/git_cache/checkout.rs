use anyhow::{Context, Result};
use gix::bstr::ByteSlice;
use std::path::Path;

pub fn checkout_tree(tree: &gix::Tree<'_>, dest: &Path) -> Result<()> {
    for entry in tree.iter() {
        let entry = entry.with_context(|| "Failed to read tree entry")?;
        // Git filenames are byte strings. A non-UTF-8 name cannot be
        // represented as a `Path` on this platform; refuse loudly rather than
        // silently mangling it (an empty-string fallback could write to the
        // wrong path entirely).
        let name = entry.filename().to_str().with_context(|| {
            format!(
                "Tree entry has a non-UTF-8 filename ({:?}); cannot write it to the worktree",
                entry.filename()
            )
        })?;
        let entry_path = dest.join(name);

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
        } else if mode.is_link() {
            // Git stores a symlink as a blob whose content is the target path
            // (no trailing newline). Restore it as a real symlink where the
            // platform allows it; see `create_symlink`.
            if let Some(parent) = entry_path.parent() {
                std::fs::create_dir_all(parent).with_context(|| {
                    format!("Failed to create parent directory for {entry_path:?}")
                })?;
            }
            let blob = entry
                .object()
                .with_context(|| format!("Failed to get symlink target object for {name}"))?;
            create_symlink(&blob.data, &entry_path)
                .with_context(|| format!("Failed to create symlink {entry_path:?}"))?;
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

/// Create a symlink at `link_path` pointing to the target bytes Git stores for
/// a symlink entry.
#[cfg(unix)]
fn create_symlink(target: &[u8], link_path: &Path) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let target = std::ffi::OsStr::from_bytes(target);
    std::os::unix::fs::symlink(target, link_path)?;
    Ok(())
}

/// On platforms without first-class symlink support (e.g. Windows without
/// Developer Mode), fall back to Git's `core.symlinks=false` behaviour: write
/// the link target as a regular file so no data is lost.
#[cfg(not(unix))]
fn create_symlink(target: &[u8], link_path: &Path) -> Result<()> {
    std::fs::write(link_path, target)?;
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
    use gix::objs::Tree;
    use gix::objs::tree::{Entry, EntryKind, EntryMode};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT_ID: AtomicU32 = AtomicU32::new(60000);

    fn temp_dir() -> PathBuf {
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("joy_checkout_test_{id}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Build a git tree object from `(mode, name, content)` triples and return
    /// it as a `gix::Tree` for `checkout_tree`. Symlink entries carry their
    /// target path as the blob content, exactly as Git stores them.
    fn build_tree<'a>(
        repo: &'a gix::Repository,
        entries: &[(EntryKind, &str, &[u8])],
    ) -> gix::Tree<'a> {
        let mut tree_entries: Vec<Entry> = entries
            .iter()
            .map(|(kind, name, content)| {
                let oid = repo.write_blob(*content).unwrap().detach();
                Entry {
                    mode: EntryMode::from(*kind),
                    filename: gix::bstr::BString::from(*name),
                    oid,
                }
            })
            .collect();
        // gix requires tree entries sorted per git's ordering rules.
        tree_entries.sort();
        let tree = Tree {
            entries: tree_entries,
        };
        let oid = repo.write_object(tree).unwrap().detach();
        repo.find_object(oid).unwrap().peel_to_tree().unwrap()
    }

    #[test]
    fn checkout_preserves_symlink_entries() {
        let tmp = temp_dir();
        let repo = gix::init_bare(tmp.join("bare")).unwrap();
        let tree = build_tree(
            &repo,
            &[
                (EntryKind::Blob, "hello.txt", b"hello world"),
                (EntryKind::Link, "hello-link", b"hello.txt"),
            ],
        );

        let dest = tmp.join("out");
        checkout_tree(&tree, &dest).expect("checkout should succeed");

        assert_eq!(
            std::fs::read(dest.join("hello.txt")).unwrap(),
            b"hello world",
            "plain blob must be written verbatim"
        );

        #[cfg(unix)]
        {
            use std::fs::symlink_metadata;
            let meta = symlink_metadata(dest.join("hello-link")).unwrap();
            assert!(
                meta.file_type().is_symlink(),
                "a Link-mode entry must become a real symlink"
            );
            assert_eq!(
                std::fs::read_link(dest.join("hello-link")).unwrap(),
                PathBuf::from("hello.txt"),
                "symlink must point at the git-stored target"
            );
        }
        #[cfg(not(unix))]
        {
            // core.symlinks=false fallback: target written as a regular file.
            assert_eq!(
                std::fs::read(dest.join("hello-link")).unwrap(),
                b"hello.txt",
                "link target must be preserved as file content"
            );
        }
    }

    #[test]
    fn checkout_rejects_non_utf8_filenames() {
        let tmp = temp_dir();
        let repo = gix::init_bare(tmp.join("bare")).unwrap();
        // A valid entry followed by an entry with an invalid UTF-8 name must
        // abort the whole checkout with a contextual error instead of being
        // silently rewritten to an empty string.
        let mut entries = Tree {
            entries: vec![
                Entry {
                    mode: EntryMode::from(EntryKind::Blob),
                    filename: gix::bstr::BString::from("ok.txt"),
                    oid: repo.write_blob(b"fine").unwrap().detach(),
                },
                Entry {
                    mode: EntryMode::from(EntryKind::Blob),
                    filename: gix::bstr::BString::from(vec![0xff, 0xfe]),
                    oid: repo.write_blob(b"mangled").unwrap().detach(),
                },
            ],
        };
        entries.entries.sort();
        let oid = repo.write_object(entries).unwrap().detach();
        let tree = repo.find_object(oid).unwrap().peel_to_tree().unwrap();

        let dest = tmp.join("out");
        let err = checkout_tree(&tree, &dest).expect_err("non-UTF-8 name must fail the checkout");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("UTF-8"),
            "error should explain the UTF-8 problem, got: {msg}"
        );
        assert_eq!(
            std::fs::read(dest.join("ok.txt")).unwrap(),
            b"fine",
            "entries before the invalid one are written eagerly (existing behaviour)"
        );
    }
}
