use crate::engine_cache;
use crate::git_cache;
use crate::releases;
use crate::util::human_size;
use anyhow::Result;
use colored::Colorize;

/// Run garbage collection on cached artifacts.
///
/// Nothing is deleted unless its corresponding flag is set: `--engines`, `--git`,
/// or `--releases`. Without flags, `joy gc` only reports what is cached and how
/// to clean it.
pub fn run_gc(clean_git: bool, clean_engines: bool, clean_releases: bool) -> Result<()> {
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
    let mut git_gc_failed = false;
    if clean_git {
        if git_path.exists() {
            let git_size = git_cache::cache_size();
            match git_cache::clear_cache() {
                Ok(()) => {
                    println!(
                        "  Removed shared Git object cache ({})",
                        human_size(git_size)
                    );
                    println!("Freed {}", human_size(git_size).green().bold());
                }
                Err(e) => {
                    eprintln!("{e}");
                    git_gc_failed = true;
                }
            }
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
    if clean_releases {
        if releases_size > 0 {
            match releases::clear_cache() {
                Ok(()) => {
                    println!(
                        "  Removed release list cache ({})",
                        human_size(releases_size)
                    );
                    println!("Freed {}", human_size(releases_size).green().bold());
                }
                Err(e) => {
                    eprintln!("Warning: could not clear release list cache: {e}");
                }
            }
        } else {
            println!("No release list cache to clean.");
        }
    } else if releases_size > 0 {
        println!(
            "Release list cache: {} (use --releases to clean)",
            human_size(releases_size)
        );
    }

    if git_gc_failed {
        anyhow::bail!(
            "Git cache was not cleaned — see the message above. Remove the linked \
            toolchains first, then retry."
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT_ID: AtomicU32 = AtomicU32::new(50000);

    fn temp_dir() -> PathBuf {
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("joy_cache_test_{id}"));
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

    fn setup_xdg() -> XdgGuard {
        let tmp = temp_dir();
        let cache_home = tmp.join("xdg").join("cache");
        std::fs::create_dir_all(&cache_home).unwrap();
        unsafe {
            std::env::set_var("XDG_CACHE_HOME", &cache_home);
        }
        XdgGuard(tmp)
    }

    /// Seed a fake release-list cache file so `releases::cache_size()` reports
    /// something non-zero.
    fn seed_release_cache() {
        let path = releases::releases_cache_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"[]").unwrap();
    }

    /// `joy gc` without --releases must NOT delete the cached release list — it
    /// is preserved unless explicitly requested, matching --git/--engines.
    #[test]
    #[serial]
    fn gc_without_releases_flag_preserves_release_cache() {
        let _guard = setup_xdg();
        seed_release_cache();
        assert!(releases::cache_size() > 0, "release cache should be seeded");

        run_gc(false, false, false).expect("plain gc should succeed");

        assert!(
            releases::releases_cache_path().unwrap().exists(),
            "release list cache must survive a plain 'joy gc'"
        );
        assert!(
            releases::cache_size() > 0,
            "release cache must still be present after plain gc"
        );
    }

    /// `joy gc --releases` must delete the cached release list.
    #[test]
    #[serial]
    fn gc_with_releases_flag_clears_release_cache() {
        let _guard = setup_xdg();
        seed_release_cache();
        assert!(releases::cache_size() > 0, "release cache should be seeded");

        run_gc(false, false, true).expect("gc --releases should succeed");

        assert_eq!(
            releases::cache_size(),
            0,
            "release cache must be cleared by --releases"
        );
    }
}
