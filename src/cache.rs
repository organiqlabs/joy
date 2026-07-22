use crate::engine_cache;
use crate::git_cache;
use crate::releases;
use crate::util::human_size;
use anyhow::Result;
use colored::Colorize;

pub fn run_gc(clean_git: bool, clean_engines: bool) -> Result<()> {
    println!("{}", "Running garbage collection...".bold());

    let engines_path = engine_cache::cache_dir()?;
    if clean_engines {
        if engines_path.exists() {
            let eng_size = engine_cache::cache_size();
            engine_cache::clear_cache()?;
            println!("  Removed shared engine cache ({})", human_size(eng_size));
            println!("Freed {}", human_size(eng_size).green().bold());
        } else {
            println!("No shared engine cache to clean.");
        }
    } else if engines_path.exists() {
        let eng_count = engine_cache::cached_versions().unwrap_or_default().len();
        let eng_size = engine_cache::cache_size();
        println!(
            "Shared engine cache: {} ({} versions, use --engines to clean)",
            human_size(eng_size),
            eng_count
        );
    }

    let git_path = git_cache::git_cache_path()?;
    if clean_git {
        if git_path.exists() {
            let git_size = git_cache::cache_size();
            git_cache::clear_cache()?;
            println!(
                "  Removed shared Git object cache ({})",
                human_size(git_size)
            );
            println!("Freed {}", human_size(git_size).green().bold());
        } else {
            println!("No Git object cache to clean.");
        }
    } else if git_path.exists() {
        let git_size = git_cache::cache_size();
        println!(
            "Git object cache: {} (use --git to clean)",
            human_size(git_size)
        );
    }

    let releases_size = releases::cache_size();
    if releases_size > 0 {
        releases::clear_cache().ok();
        println!(
            "  Removed release list cache ({})",
            human_size(releases_size)
        );
        println!("Freed {}", human_size(releases_size).green().bold());
    } else {
        println!("No release list cache to clean.");
    }

    Ok(())
}
