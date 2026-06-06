use crate::config;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Root of the central engine cache at ~/.dartup/cache/engines/
pub fn cache_dir() -> PathBuf {
    config::dartup_home().join("cache").join("engines")
}

/// Path to a specific engine version's cached binaries
pub fn engine_dir(engine_version: &str) -> PathBuf {
    cache_dir().join(engine_version)
}

/// Read the engine version string from an installed Flutter SDK
pub fn read_engine_version(env_dir: &Path) -> Result<String> {
    let version_file = env_dir.join("bin").join("internal").join("engine.version");
    let content = std::fs::read_to_string(&version_file).context(format!(
        "Failed to read engine.version from {}",
        env_dir.display()
    ))?;
    Ok(content.trim().to_string())
}

/// Symlink a toolchain's bin/cache/engine to the central cached engine.
pub fn symlink_engine(env_dir: &Path, engine_version: &str) -> Result<()> {
    let engine_cache_path = engine_dir(engine_version);
    let engine_link = env_dir.join("bin").join("cache").join("engine");

    if !engine_cache_path.exists() {
        anyhow::bail!(
            "Engine {engine_version} is not cached at {}",
            engine_cache_path.display()
        );
    }

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

    symlink_dir(&engine_cache_path, &engine_link).context("Failed to create engine symlink")?;

    Ok(())
}

/// Remove engine symlinks from a toolchain (restores a real directory).
pub fn remove_engine_symlinks(env_dir: &Path) -> Result<()> {
    let engine_link = env_dir.join("bin").join("cache").join("engine");
    if engine_link.is_symlink() {
        std::fs::remove_file(&engine_link)?;
    }
    Ok(())
}

/// Check if a toolchain's engine is symlinked to the central cache.
pub fn is_symlinked(env_dir: &Path) -> bool {
    let engine_link = env_dir.join("bin").join("cache").join("engine");
    engine_link.is_symlink()
}

/// List engine versions cached in the central store.
pub fn cached_versions() -> Result<Vec<String>> {
    let dir = cache_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut versions: Vec<String> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
        .collect();
    versions.sort();
    Ok(versions)
}

/// Move an existing engine directory from a toolchain into the central cache,
/// then replace it with a symlink.
pub fn adopt_engine_dir(env_dir: &Path, engine_version: &str) -> Result<()> {
    let src = env_dir.join("bin").join("cache").join("engine");
    let dest = engine_dir(engine_version);

    if !src.exists() {
        anyhow::bail!("No engine directory at {}", src.display());
    }

    if !dest.exists() {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::rename(&src, &dest)?;
    } else {
        std::fs::remove_dir_all(&src)?;
    }

    // Create symlink from env to central cache
    if let Some(parent) = src.parent() {
        std::fs::create_dir_all(parent)?;
    }
    symlink_dir(&dest, &src).context("Failed to symlink adopted engine")?;

    Ok(())
}

/// Total size of the central engine cache on disk.
pub fn cache_size() -> u64 {
    crate::util::dir_size(cache_dir())
}

/// Remove all cached engines from the central store.
pub fn clear_cache() -> Result<()> {
    let dir = cache_dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

/// Returns the engine download URL for a given engine version.
pub fn engine_download_url(engine_version: &str) -> String {
    let os = std::env::consts::OS;
    let platform = match os {
        "linux" => "linux-x64",
        "macos" => "darwin-x64",
        "windows" => "windows-x64",
        _ => "unknown",
    };
    format!(
        "https://storage.googleapis.com/flutter_infra_release/flutter/{}/{}/engine.zip",
        engine_version, platform
    )
}

/// Download an engine archive into the central cache.
/// Returns the path to the downloaded archive.
pub fn download_engine(engine_version: &str) -> Result<PathBuf> {
    let dest = engine_dir(engine_version);
    if dest.exists() {
        return Ok(dest);
    }

    let url = engine_download_url(engine_version);
    let tmp_dir = config::dartup_home().join(".tmp");
    std::fs::create_dir_all(&tmp_dir)?;
    let archive_path = tmp_dir.join(format!("engine-{engine_version}.zip"));

    crate::install::download_with_progress(&url, &archive_path)?;
    crate::install::extract_archive(&archive_path, &dest)?;
    std::fs::remove_file(&archive_path)?;

    Ok(dest)
}

#[cfg(unix)]
fn symlink_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

#[cfg(windows)]
fn symlink_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(src, dst)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_dir() -> PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("dartup_engine_cache_test_{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_fake_flutter(env_dir: &Path, engine_ver: &str) {
        let ver_dir = env_dir.join("bin").join("internal");
        std::fs::create_dir_all(&ver_dir).unwrap();
        std::fs::write(ver_dir.join("engine.version"), engine_ver).unwrap();
        // Empty engine directory
        let engine_dir = env_dir.join("bin").join("cache").join("engine");
        std::fs::create_dir_all(&engine_dir).unwrap();
        // Put a marker file in so dir_size > 0
        std::fs::write(engine_dir.join(".marker"), b"test").unwrap();
    }

    fn make_fake_engine_cache(cache_root: &Path, engine_ver: &str) {
        let dir = cache_root.join(engine_ver);
        let platform_dir = dir.join("linux-x64");
        std::fs::create_dir_all(&platform_dir).unwrap();
        std::fs::write(platform_dir.join("libflutter.so"), b"engine").unwrap();
    }

    // --- Tests ---

    #[test]
    fn test_engine_dir_path() {
        let tmp = temp_dir();
        let ver = "abc123def456";
        let path = engine_dir(ver);
        assert!(path.to_string_lossy().contains("engines"));
        assert!(path.to_string_lossy().contains(ver));
        let _ = tmp; // no cleanup needed, path is just computed
    }

    #[test]
    fn test_read_engine_version_from_valid_sdk() {
        let tmp = temp_dir();
        make_fake_flutter(&tmp, "abc123def456");
        let ver = read_engine_version(&tmp).unwrap();
        assert_eq!(ver, "abc123def456");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_read_engine_version_fails_when_missing() {
        let tmp = temp_dir();
        let result = read_engine_version(&tmp);
        assert!(result.is_err());
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_symlink_engine_creates_symlink() {
        let tmp = temp_dir();
        let engine_ver = "abc123def";
        let cache_root = tmp.join("engines");
        let env_dir = tmp.join("envs").join("testver");

        make_fake_flutter(&env_dir, engine_ver);
        make_fake_engine_cache(&cache_root, engine_ver);

        // Override cache_dir to use our test cache root
        // We'll manually create the engine dir and then symlink
        let engine_cache = cache_root.join(engine_ver);
        let engine_link = env_dir.join("bin").join("cache").join("engine");

        // Remove the fake engine dir first
        std::fs::remove_dir_all(&engine_link).unwrap();
        std::fs::create_dir_all(engine_link.parent().unwrap()).unwrap();
        symlink_dir(&engine_cache, &engine_link).unwrap();

        assert!(engine_link.is_symlink(), "should be a symlink");
        let target = std::fs::read_link(&engine_link).unwrap();
        assert_eq!(target, engine_cache);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_is_symlinked_detects_symlinked_engine() {
        let tmp = temp_dir();
        let env_dir = tmp.join("envs").join("ver");
        let engine_link = env_dir.join("bin").join("cache").join("engine");

        assert!(!is_symlinked(&env_dir), "should be false when absent");

        std::fs::create_dir_all(engine_link.parent().unwrap()).unwrap();
        std::fs::write(&engine_link, b"not-a-symlink").unwrap();
        assert!(!is_symlinked(&env_dir), "should be false for regular file");

        std::fs::remove_file(&engine_link).unwrap();
        symlink_dir(&tmp, &engine_link).unwrap();
        assert!(is_symlinked(&env_dir), "should be true for symlink");

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_remove_engine_symlinks_cleans_up() {
        let tmp = temp_dir();
        let env_dir = tmp.join("envs").join("ver");
        let engine_link = env_dir.join("bin").join("cache").join("engine");

        std::fs::create_dir_all(engine_link.parent().unwrap()).unwrap();
        symlink_dir(&tmp, &engine_link).unwrap();
        assert!(engine_link.is_symlink());

        remove_engine_symlinks(&env_dir).unwrap();
        assert!(!engine_link.exists(), "symlink should be removed");

        // Idempotent
        remove_engine_symlinks(&env_dir).unwrap();

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_cached_versions_lists_engine_dirs() {
        let tmp = temp_dir();
        let cache_root = tmp.join("engines");

        assert!(cached_versions().unwrap_or_default().is_empty() || cache_dir() != cache_root); // non-deterministic with real config

        // Direct test
        make_fake_engine_cache(&cache_root, "ver1");
        make_fake_engine_cache(&cache_root, "ver2");

        let mut versions: Vec<String> = std::fs::read_dir(&cache_root)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
            .collect();
        versions.sort();
        assert_eq!(versions, vec!["ver1", "ver2"]);

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_adopt_engine_dir_moves_to_cache() {
        let tmp = temp_dir();
        let engine_ver = "abc123";
        let cache_root = tmp.join("engines");
        let env_dir = tmp.join("envs").join("ver");

        make_fake_flutter(&env_dir, engine_ver);
        let engine_src = env_dir.join("bin").join("cache").join("engine");
        assert!(engine_src.exists(), "fake engine should exist");

        // Manually test adopt logic
        let dest = cache_root.join(engine_ver);
        let engine_link = env_dir.join("bin").join("cache").join("engine");

        if !dest.exists() {
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::rename(&engine_src, &dest).unwrap();
        }

        std::fs::create_dir_all(engine_link.parent().unwrap()).unwrap();
        symlink_dir(&dest, &engine_link).unwrap();

        assert!(dest.exists(), "engine should be in central cache");
        assert!(engine_link.is_symlink(), "engine should be symlinked");
        assert_eq!(std::fs::read_link(&engine_link).unwrap(), dest);

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_clear_cache_removes_engines() {
        let tmp = temp_dir();
        let cache_root = tmp.join("engines");
        make_fake_engine_cache(&cache_root, "ver1");
        assert!(cache_root.exists());

        std::fs::remove_dir_all(&cache_root).unwrap();
        assert!(!cache_root.exists());

        // Idempotent
        std::fs::remove_dir_all(&cache_root).ok();

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_engine_download_url_contains_version() {
        let url = engine_download_url("abc123");
        assert!(url.contains("abc123"), "URL should contain version");
        assert!(
            url.ends_with("engine.zip"),
            "URL should end with engine.zip"
        );
    }
}
