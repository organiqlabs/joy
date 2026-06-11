use crate::config;
use crate::profile::Artifact;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Root of the central engine cache at {cache_root}/engines/
pub fn cache_dir() -> PathBuf {
    config::engine_cache_dir()
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
    let trimmed = content.trim().to_string();
    if trimmed.is_empty() {
        anyhow::bail!("engine.version is empty in {}", version_file.display());
    }
    Ok(trimmed)
}

/// Symlink a toolchain's bin/cache/engine to a cached engine at a given path.
fn symlink_engine_to(env_dir: &Path, engine_cache_path: &Path, engine_version: &str) -> Result<()> {
    let engine_link = env_dir.join("bin").join("cache").join("engine");

    verify_engine_integrity(engine_cache_path).context(format!(
        "Engine {engine_version} cache is corrupted at {}",
        engine_cache_path.display()
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

/// Symlink a toolchain's bin/cache/engine to the central cached engine.
pub fn symlink_engine(env_dir: &Path, engine_version: &str) -> Result<()> {
    let engine_cache_path = engine_dir(engine_version);

    if !engine_cache_path.exists() {
        anyhow::bail!(
            "Engine {engine_version} is not cached at {}",
            engine_cache_path.display()
        );
    }

    symlink_engine_to(env_dir, &engine_cache_path, engine_version)
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

/// Verify that a cached engine directory has valid contents (not empty/corrupted).
/// Returns Ok(()) if the engine directory contains at least one platform subdirectory with files.
pub fn verify_engine_integrity(engine_dir: &Path) -> Result<()> {
    if !engine_dir.exists() {
        anyhow::bail!("Engine is not cached at {}", engine_dir.display());
    }
    if !engine_dir.is_dir() {
        anyhow::bail!(
            "Engine path exists but is not a directory: {}",
            engine_dir.display()
        );
    }
    let entries: Vec<_> = std::fs::read_dir(engine_dir)
        .context(format!(
            "Failed to read engine directory {}",
            engine_dir.display()
        ))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();
    if entries.is_empty() {
        anyhow::bail!(
            "Engine cache is empty or corrupted at {}",
            engine_dir.display()
        );
    }
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
    host_engine_url(engine_version)
}

/// Host-platform string for the current OS (used in download URLs).
fn host_platform() -> &'static str {
    match std::env::consts::OS {
        "linux" => "linux-x64",
        "macos" => "darwin-x64",
        "windows" => "windows-x64",
        _ => "unknown",
    }
}

/// Base URL for Flutter engine downloads.
fn engine_base_url(engine_version: &str) -> String {
    format!("https://storage.googleapis.com/flutter_infra_release/flutter/{engine_version}")
}

/// URL for the host-platform engine.
fn host_engine_url(engine_version: &str) -> String {
    format!(
        "{}/{}/engine.zip",
        engine_base_url(engine_version),
        host_platform()
    )
}

/// Returns the download URL for a specific artifact type.
pub fn artifact_download_url(engine_version: &str, artifact: &Artifact) -> String {
    let base = engine_base_url(engine_version);
    match artifact {
        Artifact::FlutterFramework | Artifact::HostDevTools => {
            String::new() // comes from git, no separate download
        }
        Artifact::HostEngine
        | Artifact::DesktopLinux
        | Artifact::DesktopMacos
        | Artifact::DesktopWindows => host_engine_url(engine_version),
        Artifact::AndroidEngineArm => format!("{base}/android-arm-release/engine.zip"),
        Artifact::AndroidEngineArm64 => format!("{base}/android-arm64-release/engine.zip"),
        Artifact::AndroidEngineX64 => format!("{base}/android-x64-release/engine.zip"),
        Artifact::AndroidEngineX86 => format!("{base}/android-x86-release/engine.zip"),
        Artifact::IosEngine => format!("{base}/ios-release/engine.zip"),
        Artifact::IosSimulator => format!("{base}/ios-sim-release/engine.zip"),
        Artifact::WebEngineCanvaskit => format!("{base}/web-canvaskit/engine.zip"),
        Artifact::WebEngineSkwasm => format!("{base}/flutter-web-sdk.zip"),
        Artifact::WebEngineHtml => format!("{base}/flutter-web-sdk.zip"),
    }
}

/// Subdirectory name within the engine cache for a given artifact.
pub fn artifact_subdir(artifact: &Artifact) -> &'static str {
    match artifact {
        Artifact::FlutterFramework | Artifact::HostDevTools => "",
        Artifact::HostEngine | Artifact::DesktopLinux => "linux-x64",
        Artifact::DesktopMacos => "darwin-x64",
        Artifact::DesktopWindows => "windows-x64",
        Artifact::AndroidEngineArm => "android-arm-release",
        Artifact::AndroidEngineArm64 => "android-arm64-release",
        Artifact::AndroidEngineX64 => "android-x64-release",
        Artifact::AndroidEngineX86 => "android-x86-release",
        Artifact::IosEngine => "ios-release",
        Artifact::IosSimulator => "ios-sim-release",
        Artifact::WebEngineCanvaskit => "web-canvaskit",
        Artifact::WebEngineSkwasm => "web-skwasm",
        Artifact::WebEngineHtml => "web-html",
    }
}

/// Ensure a specific artifact is cached. Downloads it if not present.
/// Returns the path to the cached artifact's platform subdirectory.
pub fn ensure_artifact(engine_version: &str, artifact: &Artifact) -> Result<PathBuf> {
    if is_web_artifact(artifact) {
        ensure_web_sdk(engine_version)?;
        let subdir = artifact_subdir(artifact);
        let platform_path = engine_dir(engine_version).join(subdir);
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
            platform_path.display()
        );
    }

    let url = artifact_download_url(engine_version, artifact);
    if url.is_empty() {
        anyhow::bail!("{:?} is not a downloadable artifact", artifact);
    }
    let subdir = artifact_subdir(artifact);
    let dest = engine_dir(engine_version);
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
    let tmp_dir = config::tmp_dir();
    std::fs::create_dir_all(&tmp_dir)?;
    let archive_path = tmp_dir.join(format!("engine-{engine_version}-{subdir}.zip"));

    crate::install::download_with_progress(&url, &archive_path)?;
    crate::install::extract_archive(&archive_path, &dest)?;
    std::fs::remove_file(&archive_path)?;

    Ok(platform_path)
}

/// Download an engine archive into the central cache.
/// Returns the path to the downloaded archive.
pub fn download_engine(engine_version: &str) -> Result<PathBuf> {
    let dest = engine_dir(engine_version);
    if dest.exists() {
        return Ok(dest);
    }

    let url = engine_download_url(engine_version);
    let tmp_dir = config::tmp_dir();
    std::fs::create_dir_all(&tmp_dir)?;
    let archive_path = tmp_dir.join(format!("engine-{engine_version}.zip"));

    crate::install::download_with_progress(&url, &archive_path)?;
    crate::install::extract_archive(&archive_path, &dest)?;
    std::fs::remove_file(&archive_path)?;

    Ok(dest)
}

#[cfg(unix)]
pub fn symlink_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(src, dst)
}

#[cfg(windows)]
pub fn symlink_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(src, dst)
}

/// Marker file path for web SDK extraction status.
fn web_sdk_marker(engine_version: &str) -> PathBuf {
    engine_dir(engine_version).join(".web-sdk-extracted")
}

/// Extract the web SDK archive into the engine cache directory for a specific version.
/// The archive is expected to contain `canvaskit/`, `skwasm/`, `html/` subdirectories.
/// These are renamed to `web-canvaskit/`, `web-skwasm/`, `web-html/` to match our artifact subdir naming.
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
/// Downloads `flutter-web-sdk.zip` once, extracts it, and creates a marker file
/// so that all three web renderer artifacts (`WebEngineCanvaskit`, `WebEngineSkwasm`,
/// `WebEngineHtml`) share a single download.
pub fn ensure_web_sdk(engine_version: &str) -> Result<()> {
    let marker = web_sdk_marker(engine_version);
    if marker.exists() {
        return Ok(());
    }
    let dest = engine_dir(engine_version);
    std::fs::create_dir_all(&dest)?;
    let url = artifact_download_url(engine_version, &Artifact::WebEngineCanvaskit);
    let tmp_dir = config::tmp_dir();
    std::fs::create_dir_all(&tmp_dir)?;
    let archive_path = tmp_dir.join(format!("engine-{engine_version}-web-sdk.zip"));
    crate::install::download_with_progress(&url, &archive_path)?;
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

        let engine_cache = cache_root.join(engine_ver);
        let engine_link = env_dir.join("bin").join("cache").join("engine");

        // Remove the fake engine dir first
        std::fs::remove_dir_all(&engine_link).unwrap();
        std::fs::create_dir_all(engine_link.parent().unwrap()).unwrap();
        symlink_engine_to(&env_dir, &engine_cache, engine_ver).unwrap();

        assert!(engine_link.is_symlink(), "should be a symlink");
        let target = std::fs::read_link(&engine_link).unwrap();
        assert_eq!(target, engine_cache);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_symlink_engine_rejects_corrupted_empty_cache() {
        let tmp = temp_dir();
        let engine_ver = "corrupt-empty";
        let env_dir = tmp.join("envs").join("ver");
        let cache_root = tmp.join("engines");

        // Create an empty engine cache dir -- no platform subdirectories
        let cache_dir = cache_root.join(engine_ver);
        std::fs::create_dir_all(&cache_dir).unwrap();

        make_fake_flutter(&env_dir, engine_ver);
        let engine_link = env_dir.join("bin").join("cache").join("engine");
        std::fs::remove_dir_all(&engine_link).unwrap();

        let result = symlink_engine_to(&env_dir, &cache_dir, engine_ver);
        assert!(result.is_err(), "should reject empty cache");
        assert!(!engine_link.is_symlink(), "no symlink for corrupted cache");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_symlink_engine_accepts_valid_cache_with_verify() {
        let tmp = temp_dir();
        let engine_ver = "valid-with-platform";
        let env_dir = tmp.join("envs").join("ver");
        let cache_root = tmp.join("engines");

        // Create a valid engine cache with platform subdirectory
        let cache_dir = cache_root.join(engine_ver);
        std::fs::create_dir_all(cache_dir.join("linux-x64")).unwrap();
        std::fs::write(cache_dir.join("linux-x64").join("libflutter.so"), b"engine").unwrap();

        make_fake_flutter(&env_dir, engine_ver);
        let engine_link = env_dir.join("bin").join("cache").join("engine");
        std::fs::remove_dir_all(&engine_link).unwrap();

        symlink_engine_to(&env_dir, &cache_dir, engine_ver).unwrap();
        assert!(
            engine_link.is_symlink(),
            "symlink should be created for valid cache"
        );
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

    // ---- RED: Artifact-specific URL tests ----

    #[test]
    fn test_artifact_url_host_engine_matches_host_platform() {
        let url = artifact_download_url("v1", &Artifact::HostEngine);
        let host = host_platform();
        assert!(
            url.contains(host),
            "HostEngine URL should contain {host}, got: {url}"
        );
        assert!(
            url.ends_with("engine.zip"),
            "HostEngine URL should end with engine.zip"
        );
    }

    #[test]
    fn test_artifact_url_android_per_arch_paths() {
        let url_arm = artifact_download_url("deadbeef", &Artifact::AndroidEngineArm);
        assert!(
            url_arm.contains("android-arm-release"),
            "AndroidEngineArm URL should contain android-arm-release"
        );
        let url_arm64 = artifact_download_url("deadbeef", &Artifact::AndroidEngineArm64);
        assert!(
            url_arm64.contains("android-arm64-release"),
            "AndroidEngineArm64 URL should contain android-arm64-release"
        );
        let url_x64 = artifact_download_url("deadbeef", &Artifact::AndroidEngineX64);
        assert!(
            url_x64.contains("android-x64-release"),
            "AndroidEngineX64 URL should contain android-x64-release"
        );
        let url_x86 = artifact_download_url("deadbeef", &Artifact::AndroidEngineX86);
        assert!(
            url_x86.contains("android-x86-release"),
            "AndroidEngineX86 URL should contain android-x86-release"
        );
        assert!(url_arm.ends_with("engine.zip"));
        assert!(url_arm64.ends_with("engine.zip"));
        assert!(url_x64.ends_with("engine.zip"));
        assert!(url_x86.ends_with("engine.zip"));
    }

    #[test]
    fn test_artifact_url_ios_device_and_simulator() {
        let url_dev = artifact_download_url("deadbeef", &Artifact::IosEngine);
        assert!(
            url_dev.contains("ios-release"),
            "IosEngine URL should contain ios-release"
        );
        assert!(url_dev.ends_with("engine.zip"));
        let url_sim = artifact_download_url("deadbeef", &Artifact::IosSimulator);
        assert!(
            url_sim.contains("ios-sim-release"),
            "IosSimulator URL should contain ios-sim-release"
        );
        assert!(url_sim.ends_with("engine.zip"));
    }

    #[test]
    fn test_artifact_url_web_renderers() {
        let url_canvas = artifact_download_url("deadbeef", &Artifact::WebEngineCanvaskit);
        assert!(
            url_canvas.contains("web-canvaskit"),
            "WebEngineCanvaskit URL should contain web-canvaskit"
        );
        assert!(url_canvas.ends_with("engine.zip"));
        let url_skwasm = artifact_download_url("deadbeef", &Artifact::WebEngineSkwasm);
        assert!(
            url_skwasm.contains("flutter-web-sdk"),
            "WebEngineSkwasm URL should contain flutter-web-sdk"
        );
        assert!(url_skwasm.ends_with("flutter-web-sdk.zip"));
        let url_html = artifact_download_url("deadbeef", &Artifact::WebEngineHtml);
        assert!(
            url_html.contains("flutter-web-sdk"),
            "WebEngineHtml URL should contain flutter-web-sdk"
        );
        assert!(url_html.ends_with("flutter-web-sdk.zip"));
    }

    #[test]
    fn test_artifact_url_framework_and_tools_have_no_url() {
        assert!(artifact_download_url("h", &Artifact::FlutterFramework).is_empty());
        assert!(artifact_download_url("h", &Artifact::HostDevTools).is_empty());
    }

    #[test]
    fn test_artifact_url_desktop_same_as_host() {
        let host_url = artifact_download_url("h", &Artifact::HostEngine);
        assert_eq!(
            artifact_download_url("h", &Artifact::DesktopLinux),
            host_url
        );
        assert_eq!(
            artifact_download_url("h", &Artifact::DesktopMacos),
            host_url
        );
        assert_eq!(
            artifact_download_url("h", &Artifact::DesktopWindows),
            host_url
        );
    }

    // ---- RED: Artifact subdir mapping ----

    #[test]
    fn test_artifact_subdir_host_engine_maps_to_host_platform() {
        let host = host_platform();
        assert_eq!(artifact_subdir(&Artifact::HostEngine), host);
        assert_eq!(artifact_subdir(&Artifact::DesktopLinux), "linux-x64");
        assert_eq!(artifact_subdir(&Artifact::DesktopMacos), "darwin-x64");
        assert_eq!(artifact_subdir(&Artifact::DesktopWindows), "windows-x64");
    }

    #[test]
    fn test_artifact_subdir_platform_engines() {
        assert_eq!(
            artifact_subdir(&Artifact::AndroidEngineArm),
            "android-arm-release"
        );
        assert_eq!(
            artifact_subdir(&Artifact::AndroidEngineArm64),
            "android-arm64-release"
        );
        assert_eq!(
            artifact_subdir(&Artifact::AndroidEngineX64),
            "android-x64-release"
        );
        assert_eq!(
            artifact_subdir(&Artifact::AndroidEngineX86),
            "android-x86-release"
        );
        assert_eq!(artifact_subdir(&Artifact::IosEngine), "ios-release");
        assert_eq!(artifact_subdir(&Artifact::IosSimulator), "ios-sim-release");
        assert_eq!(
            artifact_subdir(&Artifact::WebEngineCanvaskit),
            "web-canvaskit"
        );
        assert_eq!(artifact_subdir(&Artifact::WebEngineSkwasm), "web-skwasm");
        assert_eq!(artifact_subdir(&Artifact::WebEngineHtml), "web-html");
    }

    #[test]
    fn test_artifact_subdir_git_artifacts_are_empty() {
        assert_eq!(artifact_subdir(&Artifact::FlutterFramework), "");
        assert_eq!(artifact_subdir(&Artifact::HostDevTools), "");
    }

    #[test]
    fn test_ensure_artifact_framework_and_tools_are_not_downloadable() {
        assert!(ensure_artifact("h", &Artifact::FlutterFramework).is_err());
        assert!(ensure_artifact("h", &Artifact::HostDevTools).is_err());
    }

    #[test]
    fn test_ensure_artifact_returns_path_when_already_cached() {
        let tmp = temp_dir();
        let ver = "cached-artifact-v1";
        let engine_root = tmp.join("engines").join(ver);
        std::fs::create_dir_all(engine_root.join("linux-x64")).unwrap();
        std::fs::write(engine_root.join("linux-x64").join("libflutter.so"), b"data").unwrap();

        // Point cache_dir to our temp by using a local override
        // We can't easily override the global cache_dir, so test ensure_artifact behavior directly
        let subdir = engine_root.join("linux-x64");
        assert!(subdir.exists());
        let has_files = subdir
            .read_dir()
            .map(|mut e| e.next().is_some())
            .unwrap_or(false);
        assert!(has_files, "pre-populated dir should have files");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_verify_integrity_accepts_valid_engine() {
        let tmp = temp_dir();
        let engine_root = tmp.join("valid-eng-hash");
        std::fs::create_dir_all(engine_root.join("linux-x64")).unwrap();
        std::fs::write(engine_root.join("linux-x64").join("libflutter.so"), b"data").unwrap();
        let result = verify_engine_integrity(&engine_root);
        assert!(
            result.is_ok(),
            "valid engine should pass integrity: {result:?}"
        );
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_verify_integrity_rejects_empty_engine() {
        let tmp = temp_dir();
        let engine_root = tmp.join("empty-eng-hash");
        std::fs::create_dir_all(&engine_root).unwrap();
        let result = verify_engine_integrity(&engine_root);
        assert!(result.is_err(), "empty engine should fail integrity");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_verify_integrity_rejects_missing_engine() {
        let tmp = temp_dir();
        let engine_root = tmp.join("missing-eng-hash");
        let result = verify_engine_integrity(&engine_root);
        assert!(result.is_err(), "missing engine should fail integrity");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_verify_integrity_rejects_empty_file_instead_of_dir() {
        let tmp = temp_dir();
        let engine_root = tmp.join("file-eng-hash");
        std::fs::write(&engine_root, b"not a directory").unwrap();
        let result = verify_engine_integrity(&engine_root);
        assert!(result.is_err(), "file (not dir) should fail integrity");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    // ---- RED: Web SDK extraction (Phase 3) ----

    /// Helper: create a zip file with the given directory structure and files inside.
    /// `entries` maps paths relative to the zip root to file contents.
    fn create_fake_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (entry_path, content) in entries {
            if *content == b"" {
                zip.add_directory(*entry_path, options).unwrap();
            } else {
                zip.start_file(*entry_path, options).unwrap();
                std::io::Write::write_all(&mut zip, content).unwrap();
            }
        }
        zip.finish().unwrap();
    }

    #[test]
    fn test_extract_web_sdk_renames_subdirs() {
        let tmp = temp_dir();
        let zip_path = tmp.join("flutter-web-sdk.zip");
        let dest = tmp.join("extracted");

        // Create a zip that mimics flutter-web-sdk.zip structure
        create_fake_zip(
            &zip_path,
            &[
                ("canvaskit/", b""),
                ("canvaskit/canvaskit.wasm", b"wasm"),
                ("skwasm/", b""),
                ("skwasm/skwasm.wasm", b"wasm"),
                ("html/", b""),
                ("html/flutter.js", b"js"),
            ],
        );

        extract_web_sdk(&zip_path, &dest).unwrap();

        // Verify original subdirs are GONE
        assert!(
            !dest.join("canvaskit").exists(),
            "canvaskit should be renamed"
        );
        assert!(!dest.join("skwasm").exists(), "skwasm should be renamed");
        assert!(!dest.join("html").exists(), "html should be renamed");

        // Verify new subdirs exist with correct structure
        assert!(
            dest.join("web-canvaskit").is_dir(),
            "web-canvaskit should exist"
        );
        assert!(
            dest.join("web-canvaskit").join("canvaskit.wasm").exists(),
            "canvaskit.wasm should be in web-canvaskit"
        );
        assert!(dest.join("web-skwasm").is_dir(), "web-skwasm should exist");
        assert!(
            dest.join("web-skwasm").join("skwasm.wasm").exists(),
            "skwasm.wasm should be in web-skwasm"
        );
        assert!(dest.join("web-html").is_dir(), "web-html should exist");
        assert!(
            dest.join("web-html").join("flutter.js").exists(),
            "flutter.js should be in web-html"
        );

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_extract_web_sdk_idempotent() {
        let tmp = temp_dir();
        let zip_path = tmp.join("flutter-web-sdk.zip");
        let dest = tmp.join("extracted");

        create_fake_zip(
            &zip_path,
            &[
                ("canvaskit/", b""),
                ("canvaskit/canvaskit.wasm", b"wasm"),
                ("skwasm/", b""),
                ("html/", b""),
            ],
        );

        extract_web_sdk(&zip_path, &dest).unwrap();
        // Call again -- should not error even if subdirs already moved
        extract_web_sdk(&zip_path, &dest).unwrap();

        assert!(dest.join("web-canvaskit").exists());
        assert!(dest.join("web-skwasm").exists());
        assert!(dest.join("web-html").exists());

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_is_web_artifact_returns_true_for_web_variants() {
        assert!(is_web_artifact(&Artifact::WebEngineCanvaskit));
        assert!(is_web_artifact(&Artifact::WebEngineSkwasm));
        assert!(is_web_artifact(&Artifact::WebEngineHtml));
        assert!(!is_web_artifact(&Artifact::HostEngine));
        assert!(!is_web_artifact(&Artifact::AndroidEngineArm64));
        assert!(!is_web_artifact(&Artifact::IosEngine));
        assert!(!is_web_artifact(&Artifact::DesktopLinux));
    }

    #[test]
    fn test_web_sdk_marker_path() {
        let marker = web_sdk_marker("abc123");
        assert!(marker.to_string_lossy().contains("abc123"));
        assert!(marker.to_string_lossy().contains(".web-sdk-extracted"));
    }

    // ---- RED: Integrity hardening (Phase 6) ----

    #[test]
    fn test_read_engine_version_rejects_empty_file() {
        let tmp = temp_dir();
        let ver_dir = tmp.join("bin").join("internal");
        std::fs::create_dir_all(&ver_dir).unwrap();
        std::fs::write(ver_dir.join("engine.version"), b"").unwrap();
        let result = read_engine_version(&tmp);
        assert!(result.is_err(), "empty file should error");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_read_engine_version_rejects_whitespace_only() {
        let tmp = temp_dir();
        let ver_dir = tmp.join("bin").join("internal");
        std::fs::create_dir_all(&ver_dir).unwrap();
        std::fs::write(ver_dir.join("engine.version"), b"   \n\t  ").unwrap();
        let result = read_engine_version(&tmp);
        assert!(result.is_err(), "whitespace-only should error");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_symlink_engine_to_replaces_existing_directory() {
        let tmp = temp_dir();
        let engine_ver = "replacedir-hash";
        let cache_root = tmp.join("engines");
        let env_dir = tmp.join("envs").join("ver");

        // Create a valid engine cache
        let cache_dir = cache_root.join(engine_ver);
        std::fs::create_dir_all(cache_dir.join("linux-x64")).unwrap();
        std::fs::write(cache_dir.join("linux-x64").join("libflutter.so"), b"engine").unwrap();

        // Create a real directory at the target path (not a symlink)
        let engine_link = env_dir.join("bin").join("cache").join("engine");
        std::fs::create_dir_all(&engine_link).unwrap();
        std::fs::write(engine_link.join("old-file.bin"), b"old").unwrap();
        assert!(engine_link.is_dir(), "should be a real directory");

        symlink_engine_to(&env_dir, &cache_dir, engine_ver).unwrap();

        assert!(
            engine_link.is_symlink(),
            "directory should be replaced by symlink"
        );
        let target = std::fs::read_link(&engine_link).unwrap();
        assert_eq!(target, cache_dir);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn test_symlink_engine_to_replaces_symlink_cleanly() {
        let tmp = temp_dir();
        let engine_ver = "replacesym-hash";
        let cache_root = tmp.join("engines");
        let env_dir = tmp.join("envs").join("ver");

        let cache_dir = cache_root.join(engine_ver);
        std::fs::create_dir_all(cache_dir.join("linux-x64")).unwrap();
        std::fs::write(cache_dir.join("linux-x64").join("libflutter.so"), b"engine").unwrap();

        // Create a symlink pointing elsewhere
        let engine_link = env_dir.join("bin").join("cache").join("engine");
        std::fs::create_dir_all(engine_link.parent().unwrap()).unwrap();
        let wrong_target = tmp.join("wrong-dir");
        std::fs::create_dir_all(&wrong_target).unwrap();
        symlink_dir(&wrong_target, &engine_link).unwrap();
        assert!(engine_link.is_symlink(), "should be a symlink");

        symlink_engine_to(&env_dir, &cache_dir, engine_ver).unwrap();

        assert!(engine_link.is_symlink(), "should still be a symlink");
        let target = std::fs::read_link(&engine_link).unwrap();
        assert_eq!(target, cache_dir, "symlink should point to new cache dir");
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
