use crate::config;
use crate::profile::Profile;
use crate::releases;
use crate::toolchain_meta;
use crate::util::display_path;
use anyhow::{Context, Result};
use colored::Colorize;
use std::path::PathBuf;

/// Install a Flutter SDK toolchain, optionally via Git clone with shared object cache
pub fn install_with_opts(
    version: &str,
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
pub fn remove_many(versions: &[String]) -> Result<()> {
    for version in versions {
        crate::environment::remove_version(version)?;
    }
    Ok(())
}

pub fn list() -> Result<()> {
    crate::environment::list_versions()
}

/// Set the global default toolchain (delegates to environment::set_global)
pub fn set_default(version: &str) -> Result<()> {
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
pub fn resolve_active_version() -> Result<String> {
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
        return Ok(name.to_string_lossy().to_string());
    }

    anyhow::bail!("No active toolchain found. Install one with 'joy toolchain install <version>'.")
}

/// Update the currently active toolchain — upgrade to the latest on the same
/// channel, or reinstall the current version with --force.
pub fn update_active(force: bool) -> Result<()> {
    let version = resolve_active_version()?;

    let profile = toolchain_meta::load_profile(&version).unwrap_or(Profile::Default);
    let is_git = config::envs_dir()?.join(&version).join(".git").exists();

    let all_releases = releases::fetch_releases()?;

    // If the version string IS a channel name (e.g. "stable"), just install
    // the latest — install_version resolves channels to their newest release.
    if all_releases.iter().any(|r| r.channel == version) {
        println!("Upgrading Flutter {} to latest...", version);
        install_with_opts(&version, true, is_git, None, &profile, false)?;
        crate::environment::set_global(&version)?;
        return Ok(());
    }

    // Concrete version — look for a newer release on the same channel
    let current_release = releases::find_release(&version)?;
    let channel = &current_release.channel;

    let latest = all_releases
        .iter()
        .filter(|r| r.channel == *channel)
        .max_by_key(|r| &r.release_date);

    if let Some(latest) = latest
        && latest.version != version
    {
        println!(
            "Upgrading Flutter {} -> {}...",
            version,
            latest.version.green().bold()
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
fn update_active_reference(old: &str, new: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;

    // 1. Local directory override
    let local_override = config::override_path(&cwd);
    if local_override.exists()
        && let Ok(content) = std::fs::read_to_string(&local_override)
        && content.trim() == old
    {
        std::fs::write(&local_override, new)?;
        println!("   Override updated to Flutter {}", new.green().bold());
        return Ok(());
    }

    // 2. Parent-directory overrides
    for (dir, ver) in find_overrides(&cwd) {
        if ver == old {
            let op = config::override_path(&dir);
            std::fs::write(&op, new)?;
            println!("   Override updated to Flutter {}", new.green().bold());
            return Ok(());
        }
    }

    // 3. Global default symlink
    let global_path = config::global_default_path()?;
    if global_path.is_symlink()
        && let Ok(target) = std::fs::read_link(&global_path)
        && let Some(name) = target.file_name()
        && name == old
    {
        crate::environment::set_global(new)?;
        return Ok(());
    }

    Ok(())
}

/// Walk up from cwd to find all .joy/override files
fn find_overrides(cwd: &std::path::Path) -> Vec<(PathBuf, String)> {
    let mut results = Vec::new();
    let mut dir = Some(cwd);

    while let Some(current) = dir {
        let override_path = config::override_path(current);
        if override_path.exists()
            && let Ok(content) = std::fs::read_to_string(&override_path)
        {
            let version = content.trim().to_string();
            if !version.is_empty() {
                results.push((current.to_path_buf(), version));
            }
        }
        dir = current.parent();
    }

    results
}

/// Set a directory-specific override (stored in .joy/override)
pub fn set_override(version: &str) -> Result<()> {
    crate::util::validate_version(version).map_err(|e| anyhow::anyhow!("{}", e))?;
    // Verify the version is installed
    let env_dir = config::envs_dir()?.join(version);
    crate::util::check_path_traversal(&env_dir, &config::envs_dir()?)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if !env_dir.join("bin").join("flutter").exists()
        && !env_dir.join("bin").join("flutter.bat").exists()
    {
        anyhow::bail!("Flutter {version} is not installed. Run 'joy install {version}' first.");
    }

    let cwd = std::env::current_dir()?;
    let override_path = config::override_path(&cwd);

    // Create .joy directory if needed
    if let Some(parent) = override_path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create .joy directory for override")?;
    }

    std::fs::write(&override_path, version).context("Failed to write .joy/override")?;

    println!(
        "Override set: Flutter {} for {}",
        version.green().bold(),
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
                version.green().bold(),
                "(current)".green()
            );
        } else {
            println!("  {} -> {}", display_path(path), version.bold());
        }
    }
    println!(
        "\nNearest override: {} -> {}",
        display_path(&overrides[0].0),
        overrides[0].1.green().bold()
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

    fn make_fake_installation_in(envs: &Path, version: &str) {
        let env_dir = envs.join(version).join("bin");
        std::fs::create_dir_all(&env_dir).unwrap();
        std::fs::write(env_dir.join("flutter"), b"#!/bin/sh\necho fake").unwrap();
    }

    #[test]
    #[serial]
    fn test_remove_multiple_versions() {
        let (_guard, _data, _cache) = setup_xdg();
        let envs = config::envs_dir().unwrap();

        make_fake_installation_in(&envs, "v1");
        make_fake_installation_in(&envs, "v2");
        assert!(envs.join("v1").exists());
        assert!(envs.join("v2").exists());

        remove_many(&["v1".to_string(), "v2".to_string()]).unwrap();

        assert!(!envs.join("v1").exists());
        assert!(!envs.join("v2").exists());
    }
}
