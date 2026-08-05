use crate::types::{Channel, Version};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

fn xdg() -> Result<xdg::BaseDirectories> {
    if std::env::var_os("HOME").is_none() {
        anyhow::bail!(
            "$HOME must be set to use joy. Set the HOME environment variable or use a container with HOME defined."
        );
    }
    Ok(xdg::BaseDirectories::with_prefix("joy"))
}

pub fn data_root() -> Result<PathBuf> {
    xdg()?
        .get_data_home()
        .context("failed to determine XDG data home directory")
}

pub fn cache_root() -> Result<PathBuf> {
    xdg()?
        .get_cache_home()
        .context("failed to determine XDG cache home directory")
}

pub fn envs_dir() -> Result<PathBuf> {
    Ok(data_root()?.join("envs"))
}

pub fn engine_cache_dir() -> Result<PathBuf> {
    Ok(cache_root()?.join("engines"))
}

pub fn git_cache_dir() -> Result<PathBuf> {
    Ok(cache_root()?.join("git"))
}

/// Lock file guarding git cache mutations. Lives *beside* the cache directory
/// so clearing the cache never unlinks a lock another process is waiting on.
pub fn git_cache_lock_path() -> Result<PathBuf> {
    Ok(cache_root()?.join(".git.lock"))
}

/// Lock file guarding engine cache mutations. Lives *beside* the cache directory
/// so clearing the cache never unlinks a lock another process is waiting on.
pub fn engine_cache_lock_path() -> Result<PathBuf> {
    Ok(cache_root()?.join(".engines.lock"))
}

pub fn global_default_path() -> Result<PathBuf> {
    Ok(data_root()?.join("default"))
}

pub fn tmp_dir() -> Result<PathBuf> {
    Ok(cache_root()?.join("tmp"))
}

pub fn releases_cache_dir() -> Result<PathBuf> {
    Ok(cache_root()?.join("releases"))
}

pub const PROJECT_CONFIG_FILE: &str = ".joy.json";
pub const OVERRIDE_DIR: &str = ".joy";
pub const OVERRIDE_FILE: &str = "override";

pub fn override_path(project_root: &std::path::Path) -> std::path::PathBuf {
    project_root.join(OVERRIDE_DIR).join(OVERRIDE_FILE)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub version: Version,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReleaseInfo {
    pub version: Version,
    pub channel: Channel,
    pub archive_url: String,
    pub sha256: String,
    /// Release date as an ISO-8601 string — full timestamps from the API (e.g.
    /// `2026-07-30T21:19:19.727334Z`), bare dates like `2025-01-15` on the CLI.
    /// Kept as a string on purpose: ISO-8601 sorts lexicographically, so all
    /// date comparisons use plain string ordering. Do not migrate to a DateTime
    /// type without updating those comparisons.
    pub release_date: String,
}
