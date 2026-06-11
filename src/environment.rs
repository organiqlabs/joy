use crate::config;
use crate::engine_cache;
use crate::git_cache;
use crate::util::{dir_size, human_size};
use anyhow::{Context, Result};
use colored::Colorize;
use std::path::PathBuf;

/// List all installed Flutter versions
pub fn list_versions() -> Result<()> {
    let envs_dir = config::envs_dir();
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
            let is_active = current
                .as_ref()
                .is_some_and(|c| path == *c || name == get_current_version_name());

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
    let global_path = config::global_default_path();
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
    let global_path = config::global_default_path();
    if global_path.is_symlink() {
        let target = std::fs::read_link(&global_path)?;
        Ok(Some(target))
    } else {
        Ok(None)
    }
}

/// Show currently active Flutter version
pub fn show_current() -> Result<()> {
    // Check project config first
    if let Some(project_version) = crate::project::read_project_version()? {
        println!(
            "Project: {} (from .dartup.json)",
            project_version.green().bold()
        );
    }

    let global_path = config::global_default_path();
    if global_path.is_symlink() {
        let target = std::fs::read_link(&global_path)?;
        if let Some(name) = target.file_name() {
            println!(
                "Global:  {} -> {}",
                name.to_string_lossy().green().bold(),
                target.display()
            );
        }
    } else {
        println!("No global default set. Use 'dartup use -g <version>' to set one.");
    }

    Ok(())
}

/// Set the global default version
pub fn set_global(version: &str) -> Result<()> {
    let env_dir = config::envs_dir().join(version);
    if !env_dir.join("bin").join("flutter").exists()
        && !env_dir.join("bin").join("flutter.bat").exists()
    {
        anyhow::bail!("Flutter {version} is not installed. Run 'dartup install {version}' first.");
    }

    let global_path = config::global_default_path();

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

    println!("Global default set to Flutter {}.", version.green().bold());
    println!(
        "   Add {} to your PATH to use 'dartup flutter'",
        config::envs_dir().join(version).join("bin").display()
    );
    Ok(())
}

/// Remove an installed version
pub fn remove_version(version: &str) -> Result<()> {
    let env_dir = config::envs_dir().join(version);
    if !env_dir.exists() {
        anyhow::bail!("Flutter {version} is not installed.");
    }

    // Check it's not the active global version
    let current = get_current_version_name();
    if current == version {
        anyhow::bail!("Cannot remove the active global version. Switch to another version first.");
    }

    std::fs::remove_dir_all(&env_dir)?;
    println!("Removed Flutter {version}.");
    println!("   (Cached engine artifacts remain. Run 'dartup gc' to free disk space.)");
    Ok(())
}

/// Run doctor -- verify installation
pub fn run_doctor() -> Result<()> {
    println!("{}", "dartup Doctor".bold());
    println!();

    // Check dartup data and cache directories
    let data_dir = config::data_root();
    if data_dir.exists() {
        println!("Data directory: {}", data_dir.display());
    } else {
        println!("Data directory missing: {}", data_dir.display());
    }

    let cache_dir = config::cache_root();
    if cache_dir.exists() {
        println!("Cache directory: {}", cache_dir.display());
    } else {
        println!("Cache directory missing: {}", cache_dir.display());
    }

    // Check installed versions
    let envs = std::fs::read_dir(config::envs_dir())?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .count();
    println!("Installed versions: {}", envs);

    // Check global default
    let global = config::global_default_path();
    if global.is_symlink() {
        if let Ok(target) = std::fs::read_link(&global) {
            println!("Global default -> {}", target.display());
            // Verify the symlink target still exists
            if target.exists() {
                println!("Global symlink target exists");
            } else {
                println!("Global symlink target is broken!");
            }
        }
    } else {
        println!("No global default set");
    }

    // Engine cache info
    let engines_path = engine_cache::cache_dir();
    if engines_path.exists() {
        let engines_count = engine_cache::cached_versions().unwrap_or_default().len();
        let engines_size = engine_cache::cache_size();
        println!(
            "Shared engine cache: {} ({} versions) at {}",
            human_size(engines_size),
            engines_count,
            engines_path.display()
        );
    } else {
        println!("No shared engine cache. Engines will be adopted on install.");
    }

    // Git object cache info
    let git_path = git_cache::git_cache_path();
    if git_path.exists() {
        let git_objects_size = git_cache::cache_size();
        println!(
            "Git object cache: {} at {}",
            human_size(git_objects_size),
            git_path.display()
        );
        if std::fs::read_dir(git_path.join("objects").join("pack"))
            .ok()
            .map_or(0, |d| d.filter_map(|e| e.ok()).count())
            > 0
        {
            println!("Shared object store has packed objects");
        }
    } else {
        println!(
            "No global Git object cache. Create one with 'dartup toolchain install --git <version>'"
        );
    }

    // Check for disk usage
    let envs_size = dir_size(config::envs_dir());
    let engine_cache_size = engine_cache::cache_size();
    let git_cache_disk = dir_size(&git_path);
    println!("Disk usage:");
    println!("   Environments: {}", human_size(envs_size));
    println!("   Engine cache: {}", human_size(engine_cache_size));
    println!("   Git cache:    {}", human_size(git_cache_disk));
    println!(
        "   Total:        {}",
        human_size(envs_size + engine_cache_size + git_cache_disk)
    );

    Ok(())
}
