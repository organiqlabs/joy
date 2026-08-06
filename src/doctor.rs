use crate::completions;
use crate::config;
use crate::engine_cache;
use crate::environment;
use crate::git_cache;
use crate::releases;
use crate::util::{dir_size, display_path, human_size};
use anyhow::Result;
use colored::Colorize;

/// Run doctor — verify installation and display system status.
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
    println!("Installed versions: {envs}");

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

    // Check the effective version resolution (override → .joy.json → global)
    // and whether the resolved version is actually installed. A stale project
    // config still wins precedence but must be flagged instead of reported as
    // a working "Active" toolchain.
    match environment::resolve_active_status() {
        environment::ActiveStatus::Active(v) => {
            println!("Active version: {} {}", v, "(installed)".green().bold());
        }
        environment::ActiveStatus::ConfiguredNotInstalled(v) => {
            println!(
                "Configured version: {} {}",
                v,
                "(not installed!)".yellow().bold()
            );
            println!("   Install it with 'joy toolchain install {v}'");
        }
        environment::ActiveStatus::None => {
            println!("No active version configured");
        }
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
