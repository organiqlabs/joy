use crate::config;
use crate::engine_cache;
use crate::git_cache;
use crate::util::human_size;
use anyhow::Result;
use colored::Colorize;

/// Run garbage collection on cached artifacts
pub fn run_gc(clean_git: bool, clean_engines: bool) -> Result<()> {
    println!("{}", "Running garbage collection...".bold());

    // Clean shared engine cache if requested
    if clean_engines {
        let engines_path = engine_cache::cache_dir();
        if engines_path.exists() {
            let eng_size = engine_cache::cache_size();
            engine_cache::clear_cache()?;
            println!(
                "  🗑️  Removed shared engine cache ({})",
                human_size(eng_size)
            );
            println!("✅ Freed {}", human_size(eng_size).green().bold());
        } else {
            println!("ℹ️  No shared engine cache to clean.");
        }
    } else {
        let engines_path = engine_cache::cache_dir();
        if engines_path.exists() {
            let eng_count = engine_cache::cached_versions().unwrap_or_default().len();
            let eng_size = engine_cache::cache_size();
            println!(
                "📦 Shared engine cache: {} ({} versions, use --engines to clean)",
                human_size(eng_size),
                eng_count
            );
        }
    }

    // Clean git object cache if requested
    if clean_git {
        let git_path = git_cache::git_cache_path();
        if git_path.exists() {
            let git_size = git_cache::cache_size().unwrap_or(0);
            git_cache::clear_cache()?;
            println!(
                "  🗑️  Removed shared Git object cache ({})",
                human_size(git_size)
            );
            println!("✅ Freed {}", human_size(git_size).green().bold());
        } else {
            println!("ℹ️  No Git object cache to clean.");
        }
    } else {
        let git_path = git_cache::git_cache_path();
        if git_path.exists() {
            let git_size = git_cache::cache_size().unwrap_or(0);
            println!(
                "📦 Git object cache: {} (use --git to clean)",
                human_size(git_size)
            );
        }
    }

    Ok(())
}

/// Find engine version strings referenced by installed Flutter versions
fn find_used_engine_versions() -> Result<Vec<String>> {
    let envs_dir = config::envs_dir();
    let mut used = Vec::new();

    if !envs_dir.exists() {
        return Ok(used);
    }

    for entry in std::fs::read_dir(&envs_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            // Flutter stores engine version in bin/internal/engine.version
            let engine_version_file = path.join("bin").join("internal").join("engine.version");
            if engine_version_file.exists()
                && let Ok(content) = std::fs::read_to_string(engine_version_file)
            {
                let ver = content.trim().to_string();
                if !ver.is_empty() && !used.contains(&ver) {
                    used.push(ver);
                }
            }
        }
    }

    Ok(used)
}
