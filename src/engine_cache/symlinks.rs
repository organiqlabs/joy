use crate::engine_cache::{engine_dir, verify_engine_integrity};
use crate::util::display_path;
use anyhow::{Context, Result};
use std::path::Path;

pub fn symlink_engine_to(
    env_dir: &Path,
    engine_cache_path: &Path,
    engine_version: &str,
) -> Result<()> {
    let engine_link = env_dir.join("bin").join("cache").join("engine");

    verify_engine_integrity(engine_cache_path).context(format!(
        "Engine {engine_version} cache is corrupted at {}",
        display_path(engine_cache_path)
    ))?;

    if engine_link.exists() || engine_link.is_symlink() {
        if engine_link.is_symlink() || engine_link.is_file() {
            std::fs::remove_file(&engine_link)?;
        } else {
            std::fs::remove_dir_all(&engine_link)?;
        }
    }

    if let Some(parent) = engine_link.parent() {
        std::fs::create_dir_all(parent)?;
    }

    symlink_dir(engine_cache_path, &engine_link).context("Failed to create engine symlink")?;

    Ok(())
}

pub fn symlink_engine(env_dir: &Path, engine_version: &str) -> Result<()> {
    let engine_cache_path = engine_dir(engine_version)?;

    if !engine_cache_path.exists() {
        anyhow::bail!(
            "Engine {engine_version} is not cached at {}",
            display_path(&engine_cache_path)
        );
    }

    symlink_engine_to(env_dir, &engine_cache_path, engine_version)
}

pub fn adopt_engine_dir(env_dir: &Path, engine_version: &str) -> Result<()> {
    let src = env_dir.join("bin").join("cache").join("engine");
    let dest = engine_dir(engine_version)?;

    if !src.exists() {
        anyhow::bail!("No engine directory at {}", display_path(&src));
    }

    if !dest.exists() {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&src, &dest)?;
    } else {
        std::fs::remove_dir_all(&src)?;
    }

    if let Some(parent) = src.parent() {
        std::fs::create_dir_all(parent)?;
    }
    symlink_dir(&dest, &src).context("Failed to symlink adopted engine")?;

    Ok(())
}

#[cfg(unix)]
pub fn symlink_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

#[cfg(windows)]
pub fn symlink_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(src, dst)
}
