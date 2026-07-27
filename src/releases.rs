use crate::config::ReleaseInfo;
use crate::types::{Channel, Version};
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
pub(crate) fn releases_cache_path() -> Result<PathBuf> {
    let os = std::env::consts::OS;
    Ok(crate::config::releases_cache_dir()?.join(format!("releases_{os}.json")))
}

/// Save a release list to the disk cache.
/// Failures are logged via eprintln! but do not propagate — the network fetch
/// already succeeded, so the cache is optional.
fn save_cache(releases: &[ReleaseInfo]) {
    let json = match serde_json::to_string(releases) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("Warning: could not serialize release cache: {e}");
            return;
        }
    };
    let dir = match crate::config::releases_cache_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Warning: could not determine cache directory: {e}");
            return;
        }
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("Warning: could not create cache directory {dir:?}: {e}");
        return;
    }
    let path = match releases_cache_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Warning: could not determine cache path: {e}");
            return;
        }
    };
    if let Err(e) = std::fs::write(&path, &json) {
        eprintln!(
            "Warning: could not write release cache to {}: {e}",
            path.display()
        );
    }
}

/// Load a release list from the disk cache.
fn load_cache() -> Option<Vec<ReleaseInfo>> {
    let path = releases_cache_path().ok()?;
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Check whether the cached release list is fresh enough to use without a network call.
fn is_cache_fresh() -> bool {
    let path = match releases_cache_path() {
        Ok(p) => p,
        Err(_) => return false,
    };
    if !path.exists() {
        return false;
    }
    std::fs::metadata(&path)
        .ok()
        .and_then(|m| m.modified().ok())
        .is_some_and(|mtime| mtime.elapsed().is_ok_and(|age| age < CACHE_TTL))
}

/// Validate that a download URL points to a trusted Flutter storage domain.
/// Returns an error message if the URL is suspicious.
fn validate_download_url(raw: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(raw).map_err(|e| format!("Invalid download URL: {e}"))?;
    if parsed.scheme() != "https" {
        return Err(format!(
            "Download URL must use HTTPS, got scheme '{}'",
            parsed.scheme()
        ));
    }
    let host = parsed.host_str().unwrap_or("");
    if !host.ends_with(".googleapis.com") {
        return Err(format!(
            "Download URL host '{host}' is not a trusted Flutter storage domain"
        ));
    }
    Ok(())
}

/// Convert a raw Flutter API release into a typed `ReleaseInfo`.
/// Returns `None` if the version, channel, or download URL cannot be parsed/validated.
fn convert_release(r: FlutterRelease, base_url: &str) -> Option<ReleaseInfo> {
    let version = match Version::new(&r.version) {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "Warning: Skipping release with unparseable version '{}': {e}",
                r.version
            );
            return None;
        }
    };
    let channel = match Channel::new(&r.channel) {
        Ok(c) => c,
        Err(e) => {
            eprintln!(
                "Warning: Skipping release '{}' with unparseable channel '{}': {e}",
                r.version, r.channel
            );
            return None;
        }
    };
    let archive_url = format!("{}/{}", base_url, r.archive);
    if let Err(e) = validate_download_url(&archive_url) {
        eprintln!(
            "Warning: Skipping release '{}' with invalid download URL: {e}",
            r.version
        );
        return None;
    }
    Some(ReleaseInfo {
        version,
        channel,
        archive_url,
        sha256: r.sha256,
        release_date: r.release_date,
    })
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
    if crate::is_verbose() {
        eprintln!("[debug] Fetching release list from {url}");
    }
    let resp = crate::http_client()
        .get(url)
        .send()
        .context("Failed to fetch Flutter releases list")?;
    let data: FlutterReleasesResponse = resp
        .json()
        .context("Failed to parse Flutter releases JSON")?;

    let base_url = data
        .base_url
        .as_deref()
        .unwrap_or("https://storage.googleapis.com/flutter_infra_release/releases");

    let releases: Vec<ReleaseInfo> = data
        .releases
        .into_iter()
        .filter_map(|r| convert_release(r, base_url))
        .collect();

    Ok(releases)
}

/// Clear the cached release list.
pub fn clear_cache() -> Result<()> {
    let path = releases_cache_path()?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Return the size of the cached release list in bytes.
pub fn cache_size() -> u64 {
    let path = match releases_cache_path() {
        Ok(p) => p,
        Err(_) => return 0,
    };
    if path.exists() {
        std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    }
}

/// Filter releases within an inclusive date range.
/// Returns references to releases whose `release_date` falls between `from` and `to`
/// (both inclusive). Dates are compared as ISO 8601 strings, so `"2025-01-15" <= "2025-06-01"`
/// compares lexicographically correctly.
pub fn get_releases_between<'a>(
    releases: &'a [ReleaseInfo],
    from: &str,
    to: &str,
) -> Vec<&'a ReleaseInfo> {
    releases
        .iter()
        .filter(|r| r.release_date.as_str() >= from && r.release_date.as_str() <= to)
        .collect()
}

/// Display the releases between the active version and a target version.
pub fn show_release_notes_between(
    active_version: &Version,
    target_version: &Version,
) -> Result<()> {
    let releases = fetch_releases()?;

    // Find release info for both versions
    let active = find_release(active_version.as_str())?;
    let target = find_release(target_version.as_str())?;

    // Determine which is earlier/later by release_date
    let (earlier_date, later_date, earlier_ver, later_ver, direction) =
        if active.release_date <= target.release_date {
            (
                &active.release_date,
                &target.release_date,
                active_version,
                target_version,
                "upgrading",
            )
        } else {
            (
                &target.release_date,
                &active.release_date,
                target_version,
                active_version,
                "rolling back",
            )
        };

    println!(
        "{}  {}",
        "Release notes:".bold(),
        format!("{earlier_ver} -> {later_ver} ({direction})").dimmed()
    );
    println!();

    let in_range = get_releases_between(&releases, earlier_date, later_date);

    if in_range.is_empty() {
        println!("  (no releases found in this range)");
        return Ok(());
    }

    for release in in_range.iter() {
        let channel_color = match release.channel.as_str() {
            "stable" => "green",
            "beta" => "yellow",
            _ => "cyan",
        };
        let is_active = release.version == *active_version;
        let is_target = release.version == *target_version;
        let marker = if is_active {
            " <- active"
        } else if is_target {
            " <- target"
        } else {
            ""
        };
        println!(
            "  {} ({}) [{}]{}",
            release.version.to_string().bold(),
            release.channel.to_string().color(channel_color),
            release.release_date,
            marker,
        );
    }

    println!();
    println!(
        "{} release{} shown",
        in_range.len().to_string().bold(),
        if in_range.len() == 1 { "" } else { "s" }
    );
    println!("   Full details at https://docs.flutter.dev/release/release-notes");

    Ok(())
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
            release.version.to_string().bold(),
            release.channel.to_string().color(channel_color),
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
///
/// **Note:** The channel fallback uses `.rev()` to find the "latest" release on
/// a given channel. This assumes the release list from the API is in descending
/// order (newest first). If the API changes this ordering, the wrong version
/// could be selected. Consider using `.max_by_key` on `release_date` instead
/// if ordering assumptions change.
pub fn find_release(version: &str) -> Result<ReleaseInfo> {
    let releases = fetch_releases()?;

    // Try exact match first
    if let Some(r) = releases.iter().find(|r| r.version.as_str() == version) {
        return Ok(r.clone());
    }

    // Try channel match (latest in that channel)
    if let Some(r) = releases
        .iter()
        .rev()
        .find(|r| r.channel.as_str() == version)
    {
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
                version: Version::new("3.29.0").unwrap(),
                channel: Channel::new("stable").unwrap(),
                archive_url: "https://example.com/flutter_3.29.0.tar.xz".to_string(),
                sha256: "abc123".to_string(),
                release_date: "2025-01-15".to_string(),
            },
            ReleaseInfo {
                version: Version::new("3.28.0").unwrap(),
                channel: Channel::new("beta").unwrap(),
                archive_url: "https://example.com/flutter_3.28.0.tar.xz".to_string(),
                sha256: "def456".to_string(),
                release_date: "2025-01-01".to_string(),
            },
        ]
    }

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
            releases_cache_path().unwrap().exists(),
            "cache file should exist after save"
        );
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].version.as_str(), "3.29.0");
        assert_eq!(loaded[1].version.as_str(), "3.28.0");
        assert_eq!(
            loaded[0].archive_url,
            "https://example.com/flutter_3.29.0.tar.xz"
        );
        assert_eq!(loaded[1].sha256, "def456");
    }

    #[test]
    #[serial]
    fn test_load_cache_returns_none_when_no_file() {
        let _guard = setup_xdg();
        assert!(load_cache().is_none(), "no cache file = None");
    }

    #[test]
    #[serial]
    fn test_load_cache_returns_none_for_corrupt_file() {
        let _guard = setup_xdg();
        let path = releases_cache_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"this is not valid json").unwrap();
        assert!(path.exists(), "corrupt file should exist");
        assert!(load_cache().is_none(), "corrupt file should return None");
    }

    #[test]
    #[serial]
    fn test_load_cache_with_empty_array() {
        let _guard = setup_xdg();
        let path = releases_cache_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"[]").unwrap();
        let loaded = load_cache().expect("empty array should load");
        assert!(loaded.is_empty(), "empty array should produce empty vec");
    }

    #[test]
    #[serial]
    fn test_clear_cache_removes_file() {
        let _guard = setup_xdg();
        let releases = sample_releases();
        save_cache(&releases);
        assert!(
            releases_cache_path().unwrap().exists(),
            "cache should exist after save"
        );

        clear_cache().unwrap();
        assert!(
            !releases_cache_path().unwrap().exists(),
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

    #[test]
    #[serial]
    fn test_cache_json_roundtrip() {
        let _guard = setup_xdg();
        let releases = sample_releases();
        save_cache(&releases);

        // Read raw JSON from the cache file and verify it's valid
        let content = std::fs::read_to_string(releases_cache_path().unwrap()).unwrap();
        let deserialized: Vec<ReleaseInfo> = serde_json::from_str(&content).unwrap();
        assert_eq!(deserialized.len(), 2);
        assert_eq!(deserialized[0].version.as_str(), "3.29.0");
        assert_eq!(deserialized[1].version.as_str(), "3.28.0");
    }

    // --- pure get_releases_between tests (no cache/XDG setup needed) ---

    fn releases_for_filtering() -> Vec<ReleaseInfo> {
        vec![
            ReleaseInfo {
                version: Version::new("3.30.0").unwrap(),
                channel: Channel::new("stable").unwrap(),
                archive_url: "https://example.com/r1.tar.xz".to_string(),
                sha256: "a".to_string(),
                release_date: "2025-06-01".to_string(),
            },
            ReleaseInfo {
                version: Version::new("3.29.0").unwrap(),
                channel: Channel::new("stable").unwrap(),
                archive_url: "https://example.com/r2.tar.xz".to_string(),
                sha256: "b".to_string(),
                release_date: "2025-04-15".to_string(),
            },
            ReleaseInfo {
                version: Version::new("3.28.0").unwrap(),
                channel: Channel::new("beta").unwrap(),
                archive_url: "https://example.com/r3.tar.xz".to_string(),
                sha256: "c".to_string(),
                release_date: "2025-03-01".to_string(),
            },
            ReleaseInfo {
                version: Version::new("3.27.0").unwrap(),
                channel: Channel::new("beta").unwrap(),
                archive_url: "https://example.com/r4.tar.xz".to_string(),
                sha256: "d".to_string(),
                release_date: "2025-01-15".to_string(),
            },
            ReleaseInfo {
                version: Version::new("3.26.0").unwrap(),
                channel: Channel::new("stable").unwrap(),
                archive_url: "https://example.com/r5.tar.xz".to_string(),
                sha256: "e".to_string(),
                release_date: "2024-12-01".to_string(),
            },
        ]
    }

    #[test]
    fn test_get_releases_between_returns_all_in_range() {
        let releases = releases_for_filtering();
        let result = get_releases_between(&releases, "2025-03-01", "2025-06-01");
        let versions: Vec<&str> = result.iter().map(|r| r.version.as_str()).collect();
        assert_eq!(versions, vec!["3.30.0", "3.29.0", "3.28.0"]);
    }

    #[test]
    fn test_get_releases_between_single_date() {
        let releases = releases_for_filtering();
        let result = get_releases_between(&releases, "2025-04-15", "2025-04-15");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].version.as_str(), "3.29.0");
    }

    #[test]
    fn test_get_releases_between_empty_range_after_last() {
        let releases = releases_for_filtering();
        // All releases are before 2026
        let result = get_releases_between(&releases, "2026-01-01", "2026-06-01");
        assert!(result.is_empty(), "no releases in 2026");
    }

    #[test]
    fn test_get_releases_between_empty_range_before_first() {
        let releases = releases_for_filtering();
        // All releases are after 2020
        let result = get_releases_between(&releases, "2020-01-01", "2020-06-01");
        assert!(result.is_empty(), "no releases in 2020");
    }

    #[test]
    fn test_get_releases_between_all_releases() {
        let releases = releases_for_filtering();
        let result = get_releases_between(&releases, "2024-01-01", "2025-12-31");
        assert_eq!(result.len(), 5, "all releases");
    }

    #[test]
    fn test_get_releases_between_exclusive_upper_bound() {
        let releases = releases_for_filtering();
        // "2025-06-01" is the date of 3.30.0. Using a date before it should exclude 3.30.0
        let result = get_releases_between(&releases, "2025-03-01", "2025-05-31");
        let versions: Vec<&str> = result.iter().map(|r| r.version.as_str()).collect();
        assert_eq!(versions, vec!["3.29.0", "3.28.0"]);
    }

    #[test]
    fn test_get_releases_between_empty_slice() {
        let result = get_releases_between(&[], "2025-01-01", "2025-12-31");
        assert!(result.is_empty(), "empty slice yields empty result");
    }

    #[test]
    fn test_get_releases_between_inverted_range() {
        let releases = releases_for_filtering();
        // If from > to, the range is empty — no dates can satisfy both >= from and <= to
        let result = get_releases_between(&releases, "2025-06-01", "2025-03-01");
        assert!(result.is_empty(), "inverted range yields empty result");
    }

    #[test]
    fn test_get_releases_between_preserves_order() {
        let releases = releases_for_filtering();
        let result = get_releases_between(&releases, "2024-12-01", "2025-06-01");
        let versions: Vec<&str> = result.iter().map(|r| r.version.as_str()).collect();
        // Should preserve the original ordering of the releases slice
        assert_eq!(
            versions,
            vec!["3.30.0", "3.29.0", "3.28.0", "3.27.0", "3.26.0"]
        );
    }
}
