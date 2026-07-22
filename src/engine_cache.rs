pub mod checksum;
pub mod symlinks;
pub mod urls;

pub(crate) use checksum::verify_or_save_sha256;
pub use symlinks::{adopt_engine_dir, symlink_dir, symlink_engine, symlink_engine_to};
pub use urls::{artifact_download_url, artifact_subdir, engine_download_url};

use crate::config;
use crate::profile::Artifact;
use crate::types::Version;
use crate::util::display_path;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Root of the central engine cache at {cache_root}/engines/
pub fn cache_dir() -> Result<PathBuf> {
    config::engine_cache_dir()
}

/// Path to a specific engine version's cached binaries
pub fn engine_dir(engine_version: &str) -> Result<PathBuf> {
    Ok(cache_dir()?.join(engine_version))
}

/// Read the engine version string from an installed Flutter SDK
pub fn read_engine_version(env_dir: &Path) -> Result<Version> {
    let version_file = env_dir.join("bin").join("internal").join("engine.version");
    let content = std::fs::read_to_string(&version_file).context(format!(
        "Failed to read engine.version from {}",
        display_path(env_dir)
    ))?;
    let trimmed = content.trim().to_string();
    if trimmed.is_empty() {
        anyhow::bail!("engine.version is empty in {}", display_path(&version_file));
    }
    let version = Version::new(&trimmed).map_err(|e| {
        anyhow::anyhow!(
            "Invalid engine version in {}: {e}",
            display_path(&version_file)
        )
    })?;
    Ok(version)
}

/// List engine versions cached in the central store.
pub fn cached_versions() -> Result<Vec<String>> {
    let dir = cache_dir()?;
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

/// Verify that a cached engine directory has valid contents (not empty/corrupted).
/// Returns Ok(()) if the engine directory contains at least one platform subdirectory with files.
pub fn verify_engine_integrity(engine_dir: &Path) -> Result<()> {
    if !engine_dir.exists() {
        anyhow::bail!("Engine is not cached at {}", display_path(engine_dir));
    }
    if !engine_dir.is_dir() {
        anyhow::bail!(
            "Engine path exists but is not a directory: {}",
            display_path(engine_dir)
        );
    }
    let entries: Vec<_> = std::fs::read_dir(engine_dir)
        .context(format!(
            "Failed to read engine directory {}",
            display_path(engine_dir)
        ))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    if entries.is_empty() {
        anyhow::bail!(
            "Engine cache is empty or corrupted at {}",
            display_path(engine_dir)
        );
    }
    Ok(())
}

/// Total size of the central engine cache on disk.
pub fn cache_size() -> u64 {
    let dir = match cache_dir() {
        Ok(d) => d,
        Err(_) => return 0,
    };
    crate::util::dir_size(&dir)
}

/// Remove all cached engines from the central store.
pub fn clear_cache() -> Result<()> {
    let dir = cache_dir()?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

/// Ensure a specific artifact is cached. Downloads it if not present.
/// Returns the path to the cached artifact's platform subdirectory.
pub fn ensure_artifact(
    engine_version: &str,
    artifact: &Artifact,
    skip_checksum: bool,
) -> Result<PathBuf> {
    if is_web_artifact(artifact) {
        ensure_web_sdk(engine_version, skip_checksum)?;
        let subdir = artifact_subdir(artifact);
        let platform_path = engine_dir(engine_version)?.join(subdir);
        if platform_path.exists() {
            let has_files = platform_path
                .read_dir()
                .map(|mut e| e.next().is_some())
                .unwrap_or(false);
            if has_files {
                return Ok(platform_path);
            }
        }
        anyhow::bail!(
            "Web SDK was extracted but {:?} subdirectory is missing at {}",
            artifact,
            display_path(&platform_path)
        );
    }

    let url = artifact_download_url(engine_version, artifact);
    if url.is_empty() {
        anyhow::bail!("{:?} is not a downloadable artifact", artifact);
    }
    let subdir = artifact_subdir(artifact);
    let dest = engine_dir(engine_version)?;
    let platform_path = dest.join(subdir);

    if platform_path.exists() {
        let has_files = platform_path
            .read_dir()
            .map(|mut e| e.next().is_some())
            .unwrap_or(false);
        if has_files {
            return Ok(platform_path);
        }
    }

    std::fs::create_dir_all(&dest)?;
    let tmp_dir = config::tmp_dir()?;
    std::fs::create_dir_all(&tmp_dir)?;
    let archive_path = tmp_dir.join(format!("engine-{engine_version}-{subdir}.zip"));

    crate::install::download_with_progress(&url, &archive_path)?;

    // Verify against saved SHA256, or save for future verification
    let sidecar = engine_dir(engine_version)?.join(format!(".artifact-{subdir}.sha256"));
    verify_or_save_sha256(
        &archive_path,
        &sidecar,
        &format!("{engine_version}/{subdir}"),
        skip_checksum,
    )?;

    crate::install::extract_archive(&archive_path, &dest)?;
    std::fs::remove_file(&archive_path)?;

    Ok(platform_path)
}

/// Download an engine archive into the central cache.
/// Returns the path to the downloaded archive.
pub fn download_engine(engine_version: &str, skip_checksum: bool) -> Result<PathBuf> {
    let dest = engine_dir(engine_version)?;
    if dest.exists() {
        return Ok(dest);
    }

    let url = engine_download_url(engine_version);
    let tmp_dir = config::tmp_dir()?;
    std::fs::create_dir_all(&tmp_dir)?;
    let archive_path = tmp_dir.join(format!("engine-{engine_version}.zip"));

    crate::install::download_with_progress(&url, &archive_path)?;

    let sidecar = engine_dir(engine_version)?.join(".engine.sha256");
    verify_or_save_sha256(&archive_path, &sidecar, engine_version, skip_checksum)?;

    crate::install::extract_archive(&archive_path, &dest)?;
    std::fs::remove_file(&archive_path)?;

    Ok(dest)
}

/// Marker file path for web SDK extraction status.
fn web_sdk_marker(engine_version: &str) -> Result<PathBuf> {
    Ok(engine_dir(engine_version)?.join(".web-sdk-extracted"))
}

/// Extract the web SDK archive into the engine cache directory for a specific version.
fn extract_web_sdk(archive: &Path, dest: &Path) -> Result<()> {
    crate::install::extract_archive(archive, dest)?;
    for (old, new) in [
        ("canvaskit", "web-canvaskit"),
        ("skwasm", "web-skwasm"),
        ("html", "web-html"),
    ] {
        let from = dest.join(old);
        let to = dest.join(new);
        if from.exists() && !to.exists() {
            std::fs::rename(&from, &to)?;
        }
    }
    Ok(())
}

/// Ensure the shared Flutter web SDK is cached for a given engine version.
pub fn ensure_web_sdk(engine_version: &str, skip_checksum: bool) -> Result<()> {
    let marker = web_sdk_marker(engine_version)?;
    if marker.exists() {
        return Ok(());
    }
    let dest = engine_dir(engine_version)?;
    std::fs::create_dir_all(&dest)?;
    let url = artifact_download_url(engine_version, &Artifact::WebEngineCanvaskit);
    let tmp_dir = config::tmp_dir()?;
    std::fs::create_dir_all(&tmp_dir)?;
    let archive_path = tmp_dir.join(format!("engine-{engine_version}-web-sdk.zip"));
    crate::install::download_with_progress(&url, &archive_path)?;

    let sidecar = engine_dir(engine_version)?.join(".web-sdk.sha256");
    verify_or_save_sha256(
        &archive_path,
        &sidecar,
        &format!("{engine_version}/web-sdk"),
        skip_checksum,
    )?;

    extract_web_sdk(&archive_path, &dest)?;
    std::fs::remove_file(&archive_path)?;
    std::fs::write(&marker, b"1")?;
    Ok(())
}

/// Returns true if the artifact is a web renderer that shares the web SDK download.
fn is_web_artifact(artifact: &Artifact) -> bool {
    matches!(
        artifact,
        Artifact::WebEngineCanvaskit | Artifact::WebEngineSkwasm | Artifact::WebEngineHtml
    )
}
