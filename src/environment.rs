use crate::completions;
use crate::config;
use crate::engine_cache;
use crate::git_cache;
use crate::releases;
use crate::types::Version;
use crate::util::{dir_size, display_path, human_size};
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

/// Show currently active Flutter version
pub fn show_current() -> Result<()> {
    // Check project config first
    if let Some(project_version) = crate::project::read_project_version()? {
        println!(
            "Project: {} (from .joy.json)",
            project_version.to_string().green().bold()
        );
    }

    let global_path = config::global_default_path()?;
    if global_path.is_symlink() {
        let target = std::fs::read_link(&global_path)?;
        if let Some(name) = target.file_name() {
            println!(
                "Global:  {} -> {}",
                name.to_string_lossy().green().bold(),
                display_path(&target)
            );
        }
    } else {
        println!("No global default set. Use 'joy use -g <version>' to set one.");
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
        anyhow::bail!("Flutter {version} is not installed. Run 'joy install {version}' first.");
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

/// Run doctor -- verify installation
pub fn run_doctor() -> Result<()> {
    println!("{}", "joy Doctor".bold());
    println!();

    // Check joy data and cache directories
    let data_dir = config::data_root()?;
    if data_dir.exists() {
        println!("Data directory: {}", display_path(&data_dir));
    } else {
        println!("Data directory missing: {}", display_path(&data_dir));
    }

    let cache_dir = config::cache_root()?;
    if cache_dir.exists() {
        println!("Cache directory: {}", display_path(&cache_dir));
    } else {
        println!("Cache directory missing: {}", display_path(&cache_dir));
    }

    // Check installed versions
    let envs = std::fs::read_dir(config::envs_dir()?)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .count();
    println!("Installed versions: {}", envs);

    // Check global default
    let global = config::global_default_path()?;
    if global.is_symlink() {
        if let Ok(target) = std::fs::read_link(&global) {
            println!("Global default -> {}", display_path(&target));
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
    let engines_path = engine_cache::cache_dir()?;
    if engines_path.exists() {
        let engines_count = engine_cache::cached_versions().unwrap_or_default().len();
        let engines_size = engine_cache::cache_size();
        println!(
            "Shared engine cache: {} ({} versions) at {}",
            human_size(engines_size),
            engines_count,
            display_path(&engines_path)
        );
    } else {
        println!("No shared engine cache. Engines will be adopted on install.");
    }

    // Git object cache info
    let git_path = git_cache::git_cache_path()?;
    if git_path.exists() {
        let git_objects_size = git_cache::cache_size();
        println!(
            "Git object cache: {} at {}",
            human_size(git_objects_size),
            display_path(&git_path)
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
            "No global Git object cache. Create one with 'joy toolchain install --git <version>'"
        );
    }

    // Release list cache
    let releases_cache_path = releases::releases_cache_path()?;
    if releases_cache_path.exists() {
        let releases_size = releases::cache_size();
        let modified = std::fs::metadata(&releases_cache_path)
            .and_then(|m| m.modified())
            .ok();
        let age = modified.and_then(|t| t.elapsed().ok());
        let age_str = age
            .map(|d| {
                let hours = d.as_secs_f64() / 3600.0;
                if hours < 1.0 {
                    format!("{:.0} min", d.as_secs_f64() / 60.0)
                } else {
                    format!("{:.1} hours", hours)
                }
            })
            .unwrap_or_else(|| "unknown".to_string());
        println!(
            "Release list cache: {} ({}, {} ago)",
            crate::util::human_size(releases_size),
            display_path(&releases_cache_path),
            age_str
        );
    } else {
        println!("Release list cache: {}", "empty".dimmed());
    }

    // Shell completions
    if let Some(shell) = completions::current_shell() {
        if completions::is_completions_installed(shell) {
            println!("Shell completions: {}", "installed".green().bold());
        } else {
            println!(
                "Shell completions: {} ({})",
                "not installed".yellow().bold(),
                completions::install_hint(shell)
            );
        }
    }

    // Check for disk usage
    let envs_size = dir_size(config::envs_dir()?);
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
