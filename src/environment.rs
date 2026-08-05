use crate::config;
use crate::types::Version;
use crate::util::display_path;
use anyhow::{Context, Result};
use colored::Colorize;
use std::path::PathBuf;

/// List all installed Flutter versions
pub fn list_versions() -> Result<()> {
    let envs_dir = config::envs_dir()?;
    if !envs_dir.exists() {
        println!("No Flutter versions installed yet.");
        return Ok(());
    }

    let current = get_current_symlink_target()?;

    println!("{}", "Installed Flutter versions:".bold());
    let mut found = false;

    for entry in std::fs::read_dir(&envs_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            let current_name = get_current_version_name();
            let is_active = current
                .as_ref()
                .is_some_and(|c| path == *c || (!current_name.is_empty() && name == current_name));

            if is_active {
                println!("  {} {}", name.green().bold(), "(active)".green());
            } else {
                println!("  {}", name);
            }
            found = true;
        }
    }

    if !found {
        println!("  (no versions installed)");
    }

    Ok(())
}

/// Get the current version name from the global symlink
fn get_current_version_name() -> String {
    let global_path = match config::global_default_path() {
        Ok(p) => p,
        Err(_) => return String::new(),
    };
    if global_path.is_symlink()
        && let Ok(target) = std::fs::read_link(&global_path)
        && let Some(name) = target.file_name()
    {
        return name.to_string_lossy().to_string();
    }
    String::new()
}

/// Get the path the global symlink points to
fn get_current_symlink_target() -> Result<Option<PathBuf>> {
    let global_path = config::global_default_path()?;
    if global_path.is_symlink() {
        let target = std::fs::read_link(&global_path)?;
        Ok(Some(target))
    } else {
        Ok(None)
    }
}

/// Show the currently active Flutter version and how it was resolved.
///
/// The active version resolves with the same precedence as
/// `toolchain::resolve_active_version`: nearest `.joy/override` → `.joy.json` →
/// global default. The override source is reported here too, so the "current
/// version" shown matches what the rest of joy resolves.
pub fn show_current() -> Result<()> {
    // Directory override — nearest .joy/override wins.
    let cwd = std::env::current_dir()?;
    let overrides = crate::toolchain::find_overrides(&cwd);

    // Report the effective active version first.
    match crate::toolchain::resolve_active_version() {
        Ok(active) => println!("{} {}", "Active:".bold(), active.to_string().green().bold()),
        Err(_) => println!("{}", "No active toolchain configured.".dimmed()),
    }

    if let Some((dir, version)) = overrides.first() {
        println!(
            "  Override: {} (in {})",
            version.to_string().green().bold(),
            display_path(dir)
        );
    }

    // Project config (.joy.json)
    if let Some(project_version) = crate::project::read_project_version()? {
        println!(
            "  Project: {} (from .joy.json)",
            project_version.to_string().green().bold()
        );
    }

    // Global default
    let global_path = config::global_default_path()?;
    if global_path.is_symlink()
        && let Ok(target) = std::fs::read_link(&global_path)
        && let Some(name) = target.file_name()
    {
        println!(
            "  Global: {} -> {}",
            name.to_string_lossy().green().bold(),
            display_path(&target)
        );
    } else {
        println!("  Global: (none)");
    }

    Ok(())
}

/// Set the global default version.
pub fn set_global(version: &Version) -> Result<()> {
    let env_dir = config::envs_dir()?.join(version.as_str());
    crate::util::check_path_traversal(&env_dir, &config::envs_dir()?)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if !env_dir.join("bin").join("flutter").exists()
        && !env_dir.join("bin").join("flutter.bat").exists()
    {
        anyhow::bail!(
            "Flutter {version} is not installed. Run 'joy toolchain install {version}' first."
        );
    }

    let global_path = config::global_default_path()?;

    // Remove existing symlink if any
    if global_path.exists() || global_path.is_symlink() {
        std::fs::remove_file(&global_path)?;
    }

    // Create new symlink
    #[cfg(unix)]
    std::os::unix::fs::symlink(&env_dir, &global_path)
        .context("Failed to create global symlink")?;

    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&env_dir, &global_path)
        .context("Failed to create global symlink")?;

    println!(
        "Global default set to Flutter {}.",
        version.to_string().green().bold()
    );
    println!(
        "   Add {} to your PATH to use 'joy flutter'",
        display_path(config::envs_dir()?.join(version.as_str()).join("bin"))
    );
    Ok(())
}

/// Remove an installed version
pub fn remove_version(version: &Version) -> Result<()> {
    let env_dir = config::envs_dir()?.join(version.as_str());
    crate::util::check_path_traversal(&env_dir, &config::envs_dir()?)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if !env_dir.exists() {
        anyhow::bail!("Flutter {version} is not installed.");
    }

    // Check it's not the active global version
    if let Some(target) = get_current_symlink_target()?
        && target == env_dir
    {
        anyhow::bail!("Cannot remove the active global version. Switch to another version first.");
    }

    let cache = crate::git_cache::GitCache::<crate::git_cache::Fresh>::open_or_init().ok();
    if let Some(cache) = cache {
        cache.remove_worktree(version);
    }
    std::fs::remove_dir_all(&env_dir)?;
    println!("Removed Flutter {version}.");
    println!("   (Cached engine artifacts remain. Run 'joy gc' to free disk space.)");
    Ok(())
}
