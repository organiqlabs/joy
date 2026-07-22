use anyhow::{Context, Result};
use gix::bstr::ByteSlice;
use std::path::Path;

pub fn checkout_tree(tree: &gix::Tree<'_>, dest: &Path) -> Result<()> {
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
