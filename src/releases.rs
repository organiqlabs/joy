use crate::config::ReleaseInfo;
use anyhow::{Context, Result};
use colored::Colorize;
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;

/// How long a cached release list is considered fresh before re-fetching from the network.
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60); // 24 hours

/// The Flutter release API returns a JSON object with releases key.
#[derive(Deserialize)]
struct FlutterReleasesResponse {
    releases: Vec<FlutterRelease>,
    base_url: Option<String>,
}

#[derive(Deserialize)]
struct FlutterRelease {
    version: String,
    channel: String,
    archive: String,
    sha256: String,
    release_date: String,
}

/// Path to the cached release list for the current platform.
pub(crate) fn releases_cache_path() -> PathBuf {
    let os = std::env::consts::OS;
    crate::config::releases_cache_dir().join(format!("releases_{os}.json"))
}

/// Save a release list to the disk cache.
fn save_cache(releases: &[ReleaseInfo]) {
    if let Ok(json) = serde_json::to_string(releases) {
        let dir = crate::config::releases_cache_dir();
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(releases_cache_path(), &json);
    }
}

/// Load a release list from the disk cache.
fn load_cache() -> Option<Vec<ReleaseInfo>> {
    let path = releases_cache_path();
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Check whether the cached release list is fresh enough to use without a network call.
fn is_cache_fresh() -> bool {
    let path = releases_cache_path();
    if !path.exists() {
        return false;
    }
    std::fs::metadata(&path)
        .ok()
        .and_then(|m| m.modified().ok())
        .is_some_and(|mtime| mtime.elapsed().is_ok_and(|age| age < CACHE_TTL))
}

/// Fetch the list of Flutter releases from Google's storage API.
/// We pick the correct platform JSON (linux/macos/windows).
/// Uses the disk cache if it's fresh (< 24 hours old). Falls back to stale
/// cache on network failure.
pub fn fetch_releases() -> Result<Vec<ReleaseInfo>> {
    // Serve from cache if it's fresh enough — no network call needed
    if let Some(cached) = load_cache()
        && is_cache_fresh()
    {
        return Ok(cached);
    }

    let os = std::env::consts::OS;
    let url = match os {
        "linux" => {
            "https://storage.googleapis.com/flutter_infra_release/releases/releases_linux.json"
        }
        "macos" => {
            "https://storage.googleapis.com/flutter_infra_release/releases/releases_macos.json"
        }
        "windows" => {
            "https://storage.googleapis.com/flutter_infra_release/releases/releases_windows.json"
        }
        _ => anyhow::bail!("Unsupported OS: {os}"),
    };

    match fetch_releases_from_remote(url) {
        Ok(releases) => {
            save_cache(&releases);
            Ok(releases)
        }
        Err(remote_err) => {
            // Network failed — try the cache (even if stale)
            match load_cache() {
                Some(cached) => {
                    eprintln!(
                        "Warning: Could not fetch release list (offline?). Using cached data."
                    );
                    Ok(cached)
                }
                None => {
                    // No cache either — return the original error
                    Err(remote_err)
                }
            }
        }
    }
}

/// Fetch releases from the remote API, parsing the raw JSON response.
fn fetch_releases_from_remote(url: &str) -> Result<Vec<ReleaseInfo>> {
    let resp = reqwest::blocking::get(url).context("Failed to fetch Flutter releases list")?;
    let data: FlutterReleasesResponse = resp
        .json()
        .context("Failed to parse Flutter releases JSON")?;

    let releases: Vec<ReleaseInfo> = data
        .releases
        .into_iter()
        .map(|r| ReleaseInfo {
            version: r.version,
            channel: r.channel,
            archive_url: format!(
                "{}/{}",
                data.base_url
                    .as_deref()
                    .unwrap_or("https://storage.googleapis.com/flutter_infra_release/releases"),
                r.archive
            ),
            sha256: r.sha256,
            release_date: r.release_date,
        })
        .collect();

    Ok(releases)
}

/// Clear the cached release list.
pub fn clear_cache() -> Result<()> {
    let path = releases_cache_path();
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Return the size of the cached release list in bytes.
pub fn cache_size() -> u64 {
    let path = releases_cache_path();
    if path.exists() {
        std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    }
}

/// Display the releases list to stdout
pub fn list_releases(show_all: bool) -> Result<()> {
    let releases = fetch_releases()?;
    let max_display = if show_all { releases.len() } else { 20 };

    println!("{}", "Available Flutter releases:".bold());
    for release in releases.iter().take(max_display) {
        let channel_color = match release.channel.as_str() {
            "stable" => "green",
            "beta" => "yellow",
            _ => "cyan",
        };
        println!(
            "  {} ({}) [{}] {}",
            release.version.bold(),
            release.channel.color(channel_color),
            release.release_date,
            release.archive_url.dimmed()
        );
    }

    if !show_all && releases.len() > max_display {
        println!(
            "  ... and {} more (use --all to see all)",
            releases.len() - max_display
        );
    }

    Ok(())
}

/// Find a release by version string (exact match or channel name).
pub fn find_release(version: &str) -> Result<ReleaseInfo> {
    let releases = fetch_releases()?;

    // Try exact match first
    if let Some(r) = releases.iter().find(|r| r.version == version) {
        return Ok(r.clone());
    }

    // Try channel match (latest in that channel)
    if let Some(r) = releases.iter().rev().find(|r| r.channel == version) {
        return Ok(r.clone());
    }

    anyhow::bail!(
        "Could not find Flutter version '{}'. Run 'joy releases' to see available versions.",
        version
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT_ID: AtomicU32 = AtomicU32::new(10000);

    fn temp_dir() -> PathBuf {
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("joy_releases_test_{id}"));
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

    fn setup_xdg() -> XdgGuard {
        let tmp = temp_dir();
        let cache_home = tmp.join("xdg").join("cache");
        std::fs::create_dir_all(&cache_home).unwrap();
        unsafe {
            std::env::set_var("XDG_CACHE_HOME", &cache_home);
        }
        XdgGuard(tmp)
    }

    fn sample_releases() -> Vec<ReleaseInfo> {
        vec![
            ReleaseInfo {
                version: "3.29.0".to_string(),
                channel: "stable".to_string(),
                archive_url: "https://example.com/flutter_3.29.0.tar.xz".to_string(),
                sha256: "abc123".to_string(),
                release_date: "2025-01-15".to_string(),
            },
            ReleaseInfo {
                version: "3.28.0".to_string(),
                channel: "beta".to_string(),
                archive_url: "https://example.com/flutter_3.28.0.tar.xz".to_string(),
                sha256: "def456".to_string(),
                release_date: "2025-01-01".to_string(),
            },
        ]
    }

    // ---- Save + load roundtrip ----

    #[test]
    #[serial]
    fn test_save_and_load_cache_roundtrip() {
        let _guard = setup_xdg();

        // Cache should not exist yet
        assert!(load_cache().is_none());

        // Save and reload
        let releases = sample_releases();
        save_cache(&releases);
        let loaded = load_cache().expect("should load saved cache");

        assert!(
            releases_cache_path().exists(),
            "cache file should exist after save"
        );
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].version, "3.29.0");
        assert_eq!(loaded[1].version, "3.28.0");
        assert_eq!(
            loaded[0].archive_url,
            "https://example.com/flutter_3.29.0.tar.xz"
        );
        assert_eq!(loaded[1].sha256, "def456");
    }

    // ---- Load with no file ----

    #[test]
    #[serial]
    fn test_load_cache_returns_none_when_no_file() {
        let _guard = setup_xdg();
        assert!(load_cache().is_none(), "no cache file = None");
    }

    // ---- Load with corrupt file ----

    #[test]
    #[serial]
    fn test_load_cache_returns_none_for_corrupt_file() {
        let _guard = setup_xdg();
        let path = releases_cache_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"this is not valid json").unwrap();
        assert!(path.exists(), "corrupt file should exist");
        assert!(load_cache().is_none(), "corrupt file should return None");
    }

    // ---- Load with empty array ----

    #[test]
    #[serial]
    fn test_load_cache_with_empty_array() {
        let _guard = setup_xdg();
        let path = releases_cache_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"[]").unwrap();
        let loaded = load_cache().expect("empty array should load");
        assert!(loaded.is_empty(), "empty array should produce empty vec");
    }

    // ---- Clear cache ----

    #[test]
    #[serial]
    fn test_clear_cache_removes_file() {
        let _guard = setup_xdg();
        let releases = sample_releases();
        save_cache(&releases);
        assert!(
            releases_cache_path().exists(),
            "cache should exist after save"
        );

        clear_cache().unwrap();
        assert!(
            !releases_cache_path().exists(),
            "cache should be removed after clear"
        );
        assert!(load_cache().is_none(), "no cache after clear");
    }

    #[test]
    #[serial]
    fn test_clear_cache_is_idempotent() {
        let _guard = setup_xdg();
        // Clearing when no cache exists should not error
        assert!(clear_cache().is_ok());
    }

    // ---- Cache size ----

    #[test]
    #[serial]
    fn test_cache_size_zero_when_no_cache() {
        let _guard = setup_xdg();
        assert_eq!(cache_size(), 0, "no cache = size 0");
    }

    #[test]
    #[serial]
    fn test_cache_size_after_save_and_clear() {
        let _guard = setup_xdg();
        let releases = sample_releases();
        save_cache(&releases);
        assert!(cache_size() > 0, "size should be positive after save");

        clear_cache().unwrap();
        assert_eq!(cache_size(), 0, "size should be 0 after clear");
    }

    // ---- Cache freshness ----

    #[test]
    #[serial]
    fn test_is_cache_fresh_returns_false_when_no_file() {
        let _guard = setup_xdg();
        assert!(!is_cache_fresh(), "no cache file = not fresh");
    }

    #[test]
    #[serial]
    fn test_is_cache_fresh_returns_true_for_recently_saved() {
        let _guard = setup_xdg();
        let releases = sample_releases();
        save_cache(&releases);
        assert!(is_cache_fresh(), "recently saved cache should be fresh");
    }

    // ---- Fallback: the cache layer that fetch_releases uses on network failure ----
    // Covered by test_save_and_load_cache_roundtrip above: save_cache then load_cache
    // returns the data. This is the same path fetch_releases uses on network failure.

    // ---- ReleaseInfo serialization roundtrip (save uses JSON) ----

    #[test]
    #[serial]
    fn test_cache_json_roundtrip() {
        let _guard = setup_xdg();
        let releases = sample_releases();
        save_cache(&releases);

        // Read raw JSON from the cache file and verify it's valid
        let content = std::fs::read_to_string(releases_cache_path()).unwrap();
        let deserialized: Vec<ReleaseInfo> = serde_json::from_str(&content).unwrap();
        assert_eq!(deserialized.len(), 2);
        assert_eq!(deserialized[0].version, "3.29.0");
        assert_eq!(deserialized[1].version, "3.28.0");
    }
}
