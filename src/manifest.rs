use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A single release entry from the Flutter manifest.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CachedRelease {
    pub version: String,
    pub channel: String,
    pub archive_url: String,
    pub sha256: String,
    pub release_date: String,
}

/// Resolution of a user-supplied version string.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedTarget {
    /// Found in the official release manifest (archive install).
    Release(CachedRelease),
    /// Git reference (tag/branch) from a remote repo.
    GitRef { version: String, repo_url: String },
}

/// Structure stored on disk.
#[derive(Serialize, Deserialize)]
struct ManifestFile {
    fetched_at_epoch: u64,
    releases: Vec<CachedRelease>,
}

const MANIFEST_TTL_SECS: u64 = 3600;

/// Path to the cached manifest file inside dartup home.
pub fn manifest_cache_path() -> PathBuf {
    crate::config::dartup_home()
        .join("cache")
        .join("manifest.json")
}

/// Load the manifest from a JSON file at `path`. Returns `None` if the
/// file doesn't exist or is older than `MANIFEST_TTL_SECS`.
pub fn load_cached_manifest(path: &Path) -> Result<Option<Vec<CachedRelease>>> {
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path).context("Failed to read cached manifest")?;
    let mf: ManifestFile =
        serde_json::from_str(&content).context("Failed to parse cached manifest")?;

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    if now.saturating_sub(mf.fetched_at_epoch) > MANIFEST_TTL_SECS {
        return Ok(None); // expired
    }
    Ok(Some(mf.releases))
}

/// Save a release list as a cached manifest at `path`.
pub fn save_manifest(path: &Path, releases: &[CachedRelease]) -> Result<()> {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let mf = ManifestFile {
        fetched_at_epoch: now,
        releases: releases.to_vec(),
    };

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&mf)?;
    std::fs::write(path, &json).context("Failed to write manifest cache")
}

/// Fetch the release manifest — from a local file URL for testability.
/// In production, this fetches from the Flutter release API.
pub fn fetch_or_load_manifest(cache_path: &Path) -> Result<Vec<CachedRelease>> {
    // Try cache first
    if let Some(cached) = load_cached_manifest(cache_path)? {
        return Ok(cached);
    }

    // Fall back to fetching from Flutter's release API
    let releases = fetch_releases_from_api()?;

    // Cache the result
    save_manifest(cache_path, &releases)?;

    Ok(releases)
}

/// Fetch releases from the official Flutter API and convert to CachedRelease.
fn fetch_releases_from_api() -> Result<Vec<CachedRelease>> {
    let raw = crate::releases::fetch_releases()?;
    Ok(raw
        .into_iter()
        .map(|r| CachedRelease {
            version: r.version,
            channel: r.channel,
            archive_url: r.archive_url,
            sha256: r.sha256,
            release_date: r.release_date,
        })
        .collect())
}

/// Resolve a version string to a concrete target.
///
/// Resolution order:
/// 1. Exact version match in release manifest → Release
/// 2. Channel name match (latest in channel) → Release
/// 3. Otherwise → GitRef (assume a tag/branch)
pub fn resolve_version(releases: &[CachedRelease], version_str: &str) -> ResolvedTarget {
    // Exact version match
    if let Some(r) = releases.iter().find(|r| r.version == version_str) {
        return ResolvedTarget::Release(r.clone());
    }

    // Channel match — latest in that channel
    if let Some(r) = releases.iter().rev().find(|r| r.channel == version_str) {
        return ResolvedTarget::Release(r.clone());
    }

    // Assume it's a git ref
    ResolvedTarget::GitRef {
        version: version_str.to_string(),
        repo_url: String::new(),
    }
}

/// Get the latest version string for a given channel from a release list.
pub fn latest_for_channel(releases: &[CachedRelease], channel: &str) -> Option<String> {
    releases
        .iter()
        .rev()
        .find(|r| r.channel == channel)
        .map(|r| r.version.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_releases() -> Vec<CachedRelease> {
        vec![
            CachedRelease {
                version: "3.27.0".into(),
                channel: "stable".into(),
                archive_url: "https://storage.googleapis.com/...".into(),
                sha256: "aaa".into(),
                release_date: "2025-01-15".into(),
            },
            CachedRelease {
                version: "3.28.0".into(),
                channel: "stable".into(),
                archive_url: "https://storage.googleapis.com/...".into(),
                sha256: "bbb".into(),
                release_date: "2025-03-01".into(),
            },
            CachedRelease {
                version: "3.29.0".into(),
                channel: "stable".into(),
                archive_url: "https://storage.googleapis.com/...".into(),
                sha256: "ccc".into(),
                release_date: "2025-06-01".into(),
            },
            CachedRelease {
                version: "3.30.0-pre.1".into(),
                channel: "beta".into(),
                archive_url: "https://storage.googleapis.com/...".into(),
                sha256: "ddd".into(),
                release_date: "2025-07-01".into(),
            },
        ]
    }

    // ---- RED: tests that will fail until we implement ----

    #[test]
    fn test_load_cached_manifest_returns_none_when_missing() {
        let tmp = std::env::temp_dir().join("dartup_manifest_test_nonexistent");
        let _ = std::fs::remove_file(&tmp);
        let result = load_cached_manifest(&tmp).unwrap();
        assert!(result.is_none(), "should return None for missing file");
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_save_and_load_manifest_roundtrip() {
        let tmp = std::env::temp_dir().join("dartup_manifest_test_roundtrip.json");
        let _ = std::fs::remove_file(&tmp);

        let releases = sample_releases();
        save_manifest(&tmp, &releases).unwrap();

        let loaded = load_cached_manifest(&tmp).unwrap();
        assert!(loaded.is_some(), "should load cached manifest");
        assert_eq!(loaded.unwrap(), releases);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_expired_manifest_returns_none() {
        let tmp = std::env::temp_dir().join("dartup_manifest_test_expired.json");
        let _ = std::fs::remove_file(&tmp);

        // Save manifest with an ancient timestamp
        let mf = ManifestFile {
            fetched_at_epoch: 100_000, // way in the past
            releases: sample_releases(),
        };
        let json = serde_json::to_string(&mf).unwrap();
        std::fs::write(&tmp, &json).unwrap();

        let loaded = load_cached_manifest(&tmp).unwrap();
        assert!(loaded.is_none(), "expired manifest should return None");

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_resolve_exact_version() {
        let releases = sample_releases();
        match resolve_version(&releases, "3.29.0") {
            ResolvedTarget::Release(r) => assert_eq!(r.version, "3.29.0"),
            other => panic!("expected Release, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_channel_latest() {
        let releases = sample_releases();
        match resolve_version(&releases, "stable") {
            ResolvedTarget::Release(r) => {
                assert_eq!(r.version, "3.29.0", "should resolve to latest stable");
            }
            other => panic!("expected Release, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_beta_channel() {
        let releases = sample_releases();
        match resolve_version(&releases, "beta") {
            ResolvedTarget::Release(r) => {
                assert_eq!(r.version, "3.30.0-pre.1");
            }
            other => panic!("expected Release, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_unknown_as_git_ref() {
        let releases = sample_releases();
        match resolve_version(&releases, "feature/my-branch") {
            ResolvedTarget::GitRef { version, repo_url } => {
                assert_eq!(version, "feature/my-branch");
                assert!(repo_url.is_empty(), "repo_url should be empty when unknown");
            }
            other => panic!("expected GitRef, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_version_not_found_falls_back_to_git() {
        let releases = sample_releases();
        let result = resolve_version(&releases, "9.99.99");
        assert!(
            matches!(result, ResolvedTarget::GitRef { .. }),
            "nonexistent version should fall back to GitRef"
        );
    }
}
