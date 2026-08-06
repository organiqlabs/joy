use crate::config;
use crate::profile::Profile;
use crate::releases;
use crate::toolchain_meta;
use crate::types::Version;
use crate::util::display_path;
use anyhow::{Context, Result};
use colored::Colorize;
use std::path::PathBuf;

/// Install a Flutter SDK toolchain, optionally via Git clone with shared object cache
pub fn install_with_opts(
    version: &Version,
    force: bool,
    git: bool,
    repo: Option<&str>,
    profile: &Profile,
    skip_checksum: bool,
) -> Result<()> {
    if git {
        crate::install::install_version_git_with_profile(
            version,
            repo,
            force,
            profile,
            skip_checksum,
        )
    } else {
        crate::install::install_version(version, force, profile, skip_checksum)
    }
}

/// Remove one or more installed Flutter toolchains
pub fn remove_many(versions: &[Version]) -> Result<()> {
    for version in versions {
        crate::environment::remove_version(version)?;
    }
    Ok(())
}

pub fn list() -> Result<()> {
    crate::environment::list_versions()
}

/// Set the global default toolchain (delegates to environment::set_global)
pub fn set_default(version: &Version) -> Result<()> {
    crate::environment::set_global(version)
}

/// Show the current global default
pub fn show_default() {
    let global_path = match config::global_default_path() {
        Ok(p) => p,
        Err(_) => {
            println!("No global default set. Use 'joy default <version>' to set one.");
            return;
        }
    };
    if global_path.is_symlink()
        && let Ok(target) = std::fs::read_link(&global_path)
        && let Some(name) = target.file_name()
    {
        println!(
            "{} {} -> {}",
            "default:".bold(),
            name.to_string_lossy().green().bold(),
            display_path(&target)
        );
        return;
    }
    println!("No global default set. Use 'joy default <version>' to set one.");
}

/// Resolve the currently active version from override → project config → global default.
pub fn resolve_active_version() -> Result<Version> {
    let cwd = std::env::current_dir()?;

    // 1. Directory override (.joy/override in cwd or parent dirs)
    let overrides = find_overrides(&cwd);
    if let Some((_, version)) = overrides.first() {
        return Ok(version.clone());
    }

    // 2. Project config (.joy.json)
    if let Some(project_version) = crate::project::read_project_version()? {
        return Ok(project_version);
    }

    // 3. Global default symlink target name
    let global_path = config::global_default_path()?;
    if global_path.is_symlink()
        && let Ok(target) = std::fs::read_link(&global_path)
        && let Some(name) = target.file_name()
    {
        let name_str = name.to_string_lossy();
        if let Ok(v) = Version::new(name_str.as_ref()) {
            return Ok(v);
        }
    }

    anyhow::bail!("No active toolchain found. Install one with 'joy toolchain install <version>'.")
}

/// Update the currently active toolchain — upgrade to the latest on the same
/// channel, or reinstall the current version with --force.
pub fn update_active(force: bool) -> Result<()> {
    let version = resolve_active_version()?;

    let profile = toolchain_meta::load_profile(&version).unwrap_or(Profile::Default);
    let is_git = config::envs_dir()?
        .join(version.as_str())
        .join(".git")
        .exists();

    let all_releases = releases::fetch_releases()?;

    // If the version string IS a channel name (e.g. "stable"), just install
    // the latest — install_version resolves channels to their newest release.
    if all_releases
        .iter()
        .any(|r| r.channel.as_str() == version.as_str())
    {
        println!("Upgrading Flutter {} to latest...", version);
        install_with_opts(&version, true, is_git, None, &profile, false)?;
        crate::environment::set_global(&version)?;
        return Ok(());
    }

    // Concrete version — look for a newer release on the same channel
    let current_release = releases::find_release(version.as_str())?;
    let channel = current_release.channel.clone();

    let latest = all_releases
        .iter()
        .filter(|r| r.channel == channel)
        .max_by_key(|r| &r.release_date);

    if let Some(latest) = latest
        && latest.version.as_str() != version.as_str()
    {
        println!(
            "Upgrading Flutter {} -> {}...",
            version,
            latest.version.to_string().green().bold()
        );
        install_with_opts(&latest.version, true, is_git, None, &profile, false)?;
        update_active_reference(&version, &latest.version)?;
        return Ok(());
    }

    // Already the latest — reinstall only if --force was passed
    if force {
        install_with_opts(&version, true, is_git, None, &profile, false)
    } else {
        // Still ensure the global default symlink is in place
        let symlink = config::global_default_path()?;
        if !symlink.is_symlink() || !symlink.exists() {
            // set_global prints the PATH hint when creating the symlink
            crate::environment::set_global(&version).ok();
        }
        println!(
            "Flutter {} is already the latest on the {} channel. Use --force to reinstall.",
            version, channel
        );
        println!(
            "   Add {} to your PATH to use 'flutter' and 'dart'.",
            display_path(symlink.join("bin"))
        );
        Ok(())
    }
}

/// After an upgrade, update the active reference (override or global default)
/// to point to the new version.
fn update_active_reference(old: &Version, new: &Version) -> Result<()> {
    let cwd = std::env::current_dir()?;

    // 1. Local directory override
    let local_override = config::override_path(&cwd);
    if local_override.exists()
        && let Ok(content) = std::fs::read_to_string(&local_override)
        && content.trim() == old.as_str()
    {
        std::fs::write(&local_override, new.as_str())?;
        println!(
            "   Override updated to Flutter {}",
            new.to_string().green().bold()
        );
        return Ok(());
    }

    // 2. Parent-directory overrides
    for (dir, ver) in find_overrides(&cwd) {
        if ver.as_str() == old.as_str() {
            let op = config::override_path(&dir);
            std::fs::write(&op, new.as_str())?;
            println!(
                "   Override updated to Flutter {}",
                new.to_string().green().bold()
            );
            return Ok(());
        }
    }

    // 3. Global default symlink
    let global_path = config::global_default_path()?;
    if global_path.is_symlink()
        && let Ok(target) = std::fs::read_link(&global_path)
        && let Some(name) = target.file_name()
        && name == old.as_str()
    {
        crate::environment::set_global(new)?;
        return Ok(());
    }

    Ok(())
}

/// Walk up from cwd to find all .joy/override files
pub(crate) fn find_overrides(cwd: &std::path::Path) -> Vec<(PathBuf, Version)> {
    let mut results = Vec::new();
    let mut dir = Some(cwd);

    while let Some(current) = dir {
        let override_path = config::override_path(current);
        if override_path.exists()
            && let Ok(content) = std::fs::read_to_string(&override_path)
        {
            let trimmed = content.trim().to_string();
            if !trimmed.is_empty()
                && let Ok(version) = Version::new(trimmed)
            {
                results.push((current.to_path_buf(), version));
            }
        }
        dir = current.parent();
    }

    results
}

/// Set a directory-specific override (stored in .joy/override)
pub fn set_override(version: &Version) -> Result<()> {
    let env_dir = config::envs_dir()?.join(version.as_str());
    crate::util::check_path_traversal(&env_dir, &config::envs_dir()?)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if !crate::environment::version_is_installed(version) {
        anyhow::bail!(
            "Flutter {version} is not installed. Run 'joy toolchain install {version}' first."
        );
    }

    let cwd = std::env::current_dir()?;
    let override_path = config::override_path(&cwd);

    // Create .joy directory if needed
    if let Some(parent) = override_path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create .joy directory for override")?;
    }

    std::fs::write(&override_path, version.as_str()).context("Failed to write .joy/override")?;

    println!(
        "Override set: Flutter {} for {}",
        version.to_string().green().bold(),
        display_path(&cwd)
    );
    println!("   (stored in {})", display_path(&override_path));

    Ok(())
}

/// List active overrides found by walking up from cwd
pub fn list_overrides() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let overrides = find_overrides(&cwd);

    if overrides.is_empty() {
        println!("No overrides found in current or parent directories.");
        return Ok(());
    }

    println!("{}", "Active overrides:".bold());
    for (path, version) in &overrides {
        let is_active = path == &cwd;
        if is_active {
            println!(
                "  {} -> {} {}",
                display_path(path),
                version.to_string().green().bold(),
                "(current)".green()
            );
        } else {
            println!("  {} -> {}", display_path(path), version.to_string().bold());
        }
    }
    println!(
        "\nNearest override: {} -> {}",
        display_path(&overrides[0].0),
        overrides[0].1.to_string().green().bold()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::path::Path;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT_ID: AtomicU32 = AtomicU32::new(100);

    fn temp_dir() -> PathBuf {
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("joy_toolchain_test_{id}"));
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

    fn setup_xdg() -> (XdgGuard, PathBuf, PathBuf) {
        let tmp = temp_dir();
        let data_home = tmp.join("xdg").join("data");
        let cache_home = tmp.join("xdg").join("cache");
        std::fs::create_dir_all(&data_home).unwrap();
        std::fs::create_dir_all(&cache_home).unwrap();
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &data_home);
            std::env::set_var("XDG_CACHE_HOME", &cache_home);
        }
        (XdgGuard(tmp), data_home, cache_home)
    }

    fn make_fake_installation_in(envs: &Path, version: &Version) {
        let env_dir = envs.join(version.as_str()).join("bin");
        std::fs::create_dir_all(&env_dir).unwrap();
        std::fs::write(env_dir.join("flutter"), b"#!/bin/sh\necho fake").unwrap();
    }

    #[test]
    #[serial]
    fn test_remove_multiple_versions() {
        let (_guard, _data, _cache) = setup_xdg();
        let envs = config::envs_dir().unwrap();

        let v1 = Version::new("v1").unwrap();
        let v2 = Version::new("v2").unwrap();

        make_fake_installation_in(&envs, &v1);
        make_fake_installation_in(&envs, &v2);
        assert!(envs.join("v1").exists());
        assert!(envs.join("v2").exists());

        remove_many(&[v1.clone(), v2.clone()]).unwrap();

        assert!(!envs.join("v1").exists());
        assert!(!envs.join("v2").exists());
    }

    #[test]
    #[serial]
    fn test_find_overrides_empty_when_no_files() {
        let (_guard, _data, _cache) = setup_xdg();
        let tmp = temp_dir();
        std::fs::create_dir_all(&tmp).unwrap();
        let overrides = find_overrides(&tmp);
        assert!(overrides.is_empty(), "no .joy/override files = empty list");
    }

    #[test]
    #[serial]
    fn test_find_overrides_current_dir() {
        let (_guard, _data, _cache) = setup_xdg();
        let tmp = temp_dir();
        let version = Version::new("3.29.0").unwrap();

        let op = config::override_path(&tmp);
        std::fs::create_dir_all(op.parent().unwrap()).unwrap();
        std::fs::write(&op, "3.29.0").unwrap();

        let overrides = find_overrides(&tmp);
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].1, version);
    }

    #[test]
    #[serial]
    fn test_find_overrides_parent_dir() {
        let (_guard, _data, _cache) = setup_xdg();
        let parent = temp_dir();
        let child = parent.join("subproject");
        std::fs::create_dir_all(&child).unwrap();

        // Only parent has the override
        let op = config::override_path(&parent);
        std::fs::create_dir_all(op.parent().unwrap()).unwrap();
        std::fs::write(&op, "3.28.0").unwrap();

        // Searching from child should find it
        let overrides = find_overrides(&child);
        assert_eq!(overrides.len(), 1, "should find parent override from child");
        assert_eq!(overrides[0].1.as_str(), "3.28.0");
    }

    #[test]
    #[serial]
    fn test_find_overrides_current_and_parent() {
        let (_guard, _data, _cache) = setup_xdg();
        let parent = temp_dir();
        let child = parent.join("sub");
        std::fs::create_dir_all(&child).unwrap();

        // Override in parent
        let parent_op = config::override_path(&parent);
        std::fs::create_dir_all(parent_op.parent().unwrap()).unwrap();
        std::fs::write(&parent_op, "3.28.0").unwrap();

        // Override in child
        let child_op = config::override_path(&child);
        std::fs::create_dir_all(child_op.parent().unwrap()).unwrap();
        std::fs::write(&child_op, "3.29.0").unwrap();

        let overrides = find_overrides(&child);
        assert_eq!(overrides.len(), 2, "should find both current and parent");
        assert_eq!(
            overrides[0].1.as_str(),
            "3.29.0",
            "current dir should be first"
        );
        assert_eq!(
            overrides[1].1.as_str(),
            "3.28.0",
            "parent dir should be second"
        );
    }

    #[test]
    #[serial]
    fn test_find_overrides_skips_empty_file() {
        let (_guard, _data, _cache) = setup_xdg();
        let tmp = temp_dir();
        std::fs::create_dir_all(&tmp).unwrap();

        let op = config::override_path(&tmp);
        std::fs::create_dir_all(op.parent().unwrap()).unwrap();
        std::fs::write(&op, "").unwrap();

        let overrides = find_overrides(&tmp);
        assert!(
            overrides.is_empty(),
            "empty override file should be skipped"
        );
    }

    #[test]
    #[serial]
    fn test_find_overrides_skips_invalid_versions() {
        let (_guard, _data, _cache) = setup_xdg();
        let tmp = temp_dir();
        std::fs::create_dir_all(&tmp).unwrap();

        let op = config::override_path(&tmp);
        std::fs::create_dir_all(op.parent().unwrap()).unwrap();
        std::fs::write(&op, "../../../etc").unwrap();

        let overrides = find_overrides(&tmp);
        assert!(
            overrides.is_empty(),
            "invalid version strings should be skipped"
        );
    }

    #[test]
    #[serial]
    fn test_find_overrides_respects_whitespace_trim() {
        let (_guard, _data, _cache) = setup_xdg();
        let tmp = temp_dir();
        let version = Version::new("3.29.0").unwrap();

        let op = config::override_path(&tmp);
        std::fs::create_dir_all(op.parent().unwrap()).unwrap();
        std::fs::write(&op, "  3.29.0  \n").unwrap();

        let overrides = find_overrides(&tmp);
        assert_eq!(
            overrides.len(),
            1,
            "whitespace-trimmed version should be valid"
        );
        assert_eq!(overrides[0].1, version);
    }

    #[test]
    #[serial]
    fn test_resolve_active_version_fails_with_nothing() {
        let (_guard, _data, _cache) = setup_xdg();
        let tmp = temp_dir();
        std::fs::create_dir_all(&tmp).unwrap();

        let orig = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();
        let result = resolve_active_version();
        std::env::set_current_dir(&orig).unwrap();

        assert!(
            result.is_err(),
            "should fail with no override/project/global"
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No active toolchain"),
            "error should mention no active toolchain"
        );
    }

    #[test]
    #[serial]
    fn test_resolve_active_version_from_override() {
        let (_guard, _data, _cache) = setup_xdg();
        let tmp = temp_dir();
        let version = Version::new("3.29.0").unwrap();

        let op = config::override_path(&tmp);
        std::fs::create_dir_all(op.parent().unwrap()).unwrap();
        std::fs::write(&op, "3.29.0").unwrap();

        let orig = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();
        let result = resolve_active_version();
        std::env::set_current_dir(&orig).unwrap();

        let active = result.expect("should resolve from override");
        assert_eq!(active, version);
    }

    #[test]
    #[serial]
    fn test_resolve_active_version_from_global_symlink() {
        let (_guard, _data, _cache) = setup_xdg();
        let envs = config::envs_dir().unwrap();
        let version = Version::new("3.29.0").unwrap();

        // Create a fake installation so the symlink target exists
        let env_dir = envs.join("3.29.0");
        let bin_dir = env_dir.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::write(bin_dir.join("flutter"), b"#!/bin/sh\necho flutter").unwrap();

        // Create the global symlink
        let global_path = config::global_default_path().unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&env_dir, &global_path).unwrap();

        // From a clean temp dir (no override, no .joy.json)
        let tmp = temp_dir();
        std::fs::create_dir_all(&tmp).unwrap();

        let orig = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();
        let result = resolve_active_version();
        std::env::set_current_dir(&orig).unwrap();

        let active = result.expect("should resolve from global symlink");
        assert_eq!(active, version);
    }

    #[test]
    #[serial]
    fn test_resolve_active_version_override_takes_precedence() {
        let (_guard, _data, _cache) = setup_xdg();
        let envs = config::envs_dir().unwrap();
        let _global_ver = Version::new("3.28.0").unwrap();
        let override_ver = Version::new("3.29.0").unwrap();

        // Create both installations
        let global_dir = envs.join("3.28.0");
        std::fs::create_dir_all(global_dir.join("bin")).unwrap();
        std::fs::write(global_dir.join("bin").join("flutter"), b"#!/bin/sh").unwrap();

        let global_path = config::global_default_path().unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&global_dir, &global_path).unwrap();

        // Override says 3.29.0
        let tmp = temp_dir();
        std::fs::create_dir_all(&tmp).unwrap();
        let op = config::override_path(&tmp);
        std::fs::create_dir_all(op.parent().unwrap()).unwrap();
        std::fs::write(&op, "3.29.0").unwrap();

        let orig = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();
        let result = resolve_active_version();
        std::env::set_current_dir(&orig).unwrap();

        let active = result.expect("should resolve from override");
        assert_eq!(
            active, override_ver,
            "override should take precedence over global default"
        );
    }

    #[test]
    #[serial]
    fn test_update_active_reference_local_override() {
        let (_guard, _data, _cache) = setup_xdg();
        let tmp = temp_dir();
        let old_ver = Version::new("3.28.0").unwrap();
        let new_ver = Version::new("3.29.0").unwrap();

        // Create old override
        let op = config::override_path(&tmp);
        std::fs::create_dir_all(op.parent().unwrap()).unwrap();
        std::fs::write(&op, "3.28.0").unwrap();

        let orig = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();
        let result = update_active_reference(&old_ver, &new_ver);
        std::env::set_current_dir(&orig).unwrap();

        assert!(result.is_ok(), "should update local override");
        let content = std::fs::read_to_string(&op).unwrap();
        assert_eq!(
            content.trim(),
            "3.29.0",
            "override file should contain new version"
        );
    }

    #[test]
    #[serial]
    fn test_update_active_reference_no_match_is_noop() {
        let (_guard, _data, _cache) = setup_xdg();
        let tmp = temp_dir();
        let old_ver = Version::new("3.27.0").unwrap();
        let new_ver = Version::new("3.29.0").unwrap();

        // Create override with a different version
        let op = config::override_path(&tmp);
        std::fs::create_dir_all(op.parent().unwrap()).unwrap();
        std::fs::write(&op, "3.28.0").unwrap();

        let orig = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();
        let result = update_active_reference(&old_ver, &new_ver);
        std::env::set_current_dir(&orig).unwrap();

        assert!(result.is_ok(), "no match should still return Ok");
        let content = std::fs::read_to_string(&op).unwrap();
        assert_eq!(
            content.trim(),
            "3.28.0",
            "override should remain unchanged when old version doesn't match"
        );
    }

    #[test]
    #[serial]
    fn test_set_override_creates_file() {
        let (_guard, _data, _cache) = setup_xdg();
        let envs = config::envs_dir().unwrap();
        let version = Version::new("3.29.0").unwrap();

        // Need a fake installation
        make_fake_installation_in(&envs, &version);

        let tmp = temp_dir();
        std::fs::create_dir_all(&tmp).unwrap();

        let orig = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();
        let result = set_override(&version);
        std::env::set_current_dir(&orig).unwrap();

        assert!(result.is_ok(), "set_override should succeed");

        let op = config::override_path(&tmp);
        assert!(op.exists(), "override file should be created");
        let content = std::fs::read_to_string(&op).unwrap();
        assert_eq!(content.trim(), "3.29.0");
    }

    #[test]
    #[serial]
    fn test_set_override_fails_for_uninstalled_version() {
        let (_guard, _data, _cache) = setup_xdg();
        let version = Version::new("3.99.0").unwrap();

        let tmp = temp_dir();
        std::fs::create_dir_all(&tmp).unwrap();

        let orig = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();
        let result = set_override(&version);
        std::env::set_current_dir(&orig).unwrap();

        assert!(result.is_err(), "should fail for uninstalled version");
        assert!(
            result.unwrap_err().to_string().contains("not installed"),
            "error should mention version is not installed"
        );
    }

    #[test]
    #[serial]
    fn test_list_overrides_empty() {
        let (_guard, _data, _cache) = setup_xdg();
        let tmp = temp_dir();
        std::fs::create_dir_all(&tmp).unwrap();

        let orig = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();
        let result = list_overrides();
        std::env::set_current_dir(&orig).unwrap();

        assert!(result.is_ok(), "empty overrides should be Ok");
    }

    #[test]
    #[serial]
    fn test_list_overrides_with_override() {
        let (_guard, _data, _cache) = setup_xdg();
        let tmp = temp_dir();

        let op = config::override_path(&tmp);
        std::fs::create_dir_all(op.parent().unwrap()).unwrap();
        std::fs::write(&op, "3.29.0").unwrap();

        let orig = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();
        let result = list_overrides();
        std::env::set_current_dir(&orig).unwrap();

        assert!(result.is_ok(), "listing overrides should succeed");
    }

    #[test]
    #[serial]
    fn test_set_default_fails_for_uninstalled_version() {
        let (_guard, _data, _cache) = setup_xdg();
        let version = Version::new("3.99.0").unwrap();
        let result = set_default(&version);
        assert!(result.is_err(), "should fail for uninstalled version");
        assert!(
            result.unwrap_err().to_string().contains("not installed"),
            "error should mention not installed"
        );
    }

    #[test]
    #[serial]
    fn test_set_default_creates_symlink() {
        let (_guard, _data, _cache) = setup_xdg();
        let envs = config::envs_dir().unwrap();
        let version = Version::new("3.29.0").unwrap();

        make_fake_installation_in(&envs, &version);

        let result = set_default(&version);
        assert!(result.is_ok(), "set_default should succeed");

        let global_path = config::global_default_path().unwrap();
        assert!(
            global_path.is_symlink(),
            "global default should be a symlink"
        );

        let target = std::fs::read_link(&global_path).unwrap();
        assert_eq!(target, envs.join("3.29.0"));
    }
}
