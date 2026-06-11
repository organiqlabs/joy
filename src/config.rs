use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Return an XDG base-directories instance scoped to "dartup".
/// Panics if $HOME is not set.
fn xdg() -> xdg::BaseDirectories {
    xdg::BaseDirectories::with_prefix("dartup")
}

/// Root of user-specific data (installed SDKs, profiles, default symlink).
/// `$XDG_DATA_HOME/dartup` or `~/.local/share/dartup`.
pub fn data_root() -> PathBuf {
    xdg()
        .get_data_home()
        .expect("$HOME must be set to use dartup")
}

/// Root of user-specific cache (engine artifacts, git objects, temp downloads).
/// `$XDG_CACHE_HOME/dartup` or `~/.cache/dartup`.
pub fn cache_root() -> PathBuf {
    xdg()
        .get_cache_home()
        .expect("$HOME must be set to use dartup")
}

/// Directory where Flutter SDK versions are installed: `{data_root}/envs`
pub fn envs_dir() -> PathBuf {
    data_root().join("envs")
}

/// Directory for shared engine artifact cache: `{cache_root}/engines`
pub fn engine_cache_dir() -> PathBuf {
    cache_root().join("engines")
}

/// Directory for shared git data (bare repo cache): `{cache_root}/git`
pub fn git_cache_dir() -> PathBuf {
    cache_root().join("git")
}

/// Path to the global default symlink: `{data_root}/default`
pub fn global_default_path() -> PathBuf {
    data_root().join("default")
}

/// Temporary download directory: `{cache_root}/tmp`
pub fn tmp_dir() -> PathBuf {
    cache_root().join("tmp")
}

/// Per-project config file name
pub const PROJECT_CONFIG_FILE: &str = ".dartup.json";

/// Directory name for override storage
pub const OVERRIDE_DIR: &str = ".dartup";

/// Override file name inside .dartup/
pub const OVERRIDE_FILE: &str = "override";

/// Path to the override file for a given project directory
pub fn override_path(project_root: &std::path::Path) -> std::path::PathBuf {
    project_root.join(OVERRIDE_DIR).join(OVERRIDE_FILE)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReleaseInfo {
    pub version: String,
    pub channel: String,
    pub archive_url: String,
    pub sha256: String,
    pub release_date: String,
}
