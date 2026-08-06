use crate::config;
use crate::types::Version;
use crate::util::display_path;
use anyhow::{Context, Result};
use colored::Colorize;
use std::path::{Path, PathBuf};

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
            // Skip internal staging/backup directories (e.g.
            // .joy-staging-3.29.0-1234) that a concurrent or interrupted
            // install may leave behind — they are not installed versions.
            if name.starts_with('.') {
                continue;
            }
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
/// Whether a version has an actual SDK installation on disk — the same
/// `bin/flutter` presence check installs use (see
/// [`crate::install::has_flutter_binary`]).
pub fn version_is_installed(version: &Version) -> bool {
    let env_dir = match config::envs_dir() {
        Ok(d) => d.join(version.as_str()),
        Err(_) => return false,
    };
    crate::install::has_flutter_binary(&env_dir)
}

/// The resolved active toolchain, distinguishing a configured-but-missing
/// install from one that actually works.
///
/// Precedence is unchanged from [`crate::toolchain::resolve_active_version`]
/// (override → `.joy.json` → global default): a stale project config still
/// wins over the global default, but it is reported as **Configured** rather
/// than **Active** so the user immediately sees that commands will fail until
/// the version is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveStatus {
    /// Resolved to a version that is actually installed — commands will work.
    Active(Version),
    /// Resolved from configuration (e.g. a stale `.joy.json`) but the version
    /// is not installed.
    ConfiguredNotInstalled(Version),
    /// Nothing is configured — no override, project config, or global default.
    None,
}

/// Resolve the effective version and its installation status.
pub fn resolve_active_status() -> ActiveStatus {
    match crate::toolchain::resolve_active_version() {
        Ok(v) if version_is_installed(&v) => ActiveStatus::Active(v),
        Ok(v) => ActiveStatus::ConfiguredNotInstalled(v),
        Err(_) => ActiveStatus::None,
    }
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

    // Report the effective version first, distinguishing a configured-but-missing
    // install from a genuinely active one.
    match resolve_active_status() {
        ActiveStatus::Active(active) => {
            println!("{} {}", "Active:".bold(), active.to_string().green().bold())
        }
        ActiveStatus::ConfiguredNotInstalled(configured) => {
            println!(
                "{} {}",
                "Configured:".bold(),
                configured.to_string().yellow().bold()
            );
            println!(
                "   {} {}",
                "Status:".bold(),
                "not installed".yellow().bold()
            );
            println!("   Install it with 'joy toolchain install {configured}'.");
        }
        ActiveStatus::None => println!("{}", "No active toolchain configured.".dimmed()),
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
        // Color the configured version by install status so it agrees with the
        // "Configured:" / "Active:" header above.
        let colored = if version_is_installed(&project_version) {
            project_version.to_string().green().bold()
        } else {
            project_version.to_string().yellow().bold()
        };
        println!("  Project: {} (from .joy.json)", colored);
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

/// Replace the global default symlink so it points at `target`.
///
/// Atomicity: the new link is created **beside** the existing one (never over
/// it) and then renamed into place with `std::fs::rename`. On Unix,
/// `rename(2)` swaps the destination entry in a single atomic operation, so at
/// every instant a reader sees either the old default or the new one — never
/// none. This removes the previous remove-then-create window where an
/// interruption left the user without a global default at all.
///
/// **Windows fallback:** `std::fs::rename` (MoveFileEx + `MOVEFILE_REPLACE_EXISTING`)
/// cannot reliably replace an existing *directory* symlink in place — Windows
/// treats it as a directory, and the OS can refuse the atomic replace. When
/// the atomic rename fails, we fall back to remove-then-rename, which is **not**
/// atomic: a crash between the removal and the rename would leave no default.
/// That is the narrowest window Windows allows for directory-symlink
/// replacement without admin privileges / Developer Mode (replacing reparse
/// points in place is documented as unreliable there). On non-Windows
/// platforms a failed rename is surfaced as an error instead — no fallback is
/// needed because the atomic path never fails for an existing symlink.
fn replace_global_symlink(global_path: &Path, target: &Path) -> Result<()> {
    let parent = global_path
        .parent()
        .with_context(|| format!("{} has no parent directory", global_path.display()))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create directory {}", parent.display()))?;

    let file_name = global_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("default");
    // Unique per process so two concurrent `joy default` runs cannot collide,
    // and lives beside the real link so `rename` stays on one filesystem.
    let tmp = parent.join(format!(".{file_name}.tmp.{}", std::process::id()));
    // A stale temp link from a previously interrupted run must not block us.
    let _ = std::fs::remove_file(&tmp);

    crate::engine_cache::symlink_dir(target, &tmp)
        .with_context(|| format!("Failed to create global symlink at {}", tmp.display()))?;

    match std::fs::rename(&tmp, global_path) {
        Ok(()) => Ok(()),
        Err(e) => {
            #[cfg(windows)]
            {
                // Atomic replacement of an existing directory symlink is not
                // guaranteed on Windows — fall back to remove-then-rename.
                let outcome = (|| -> std::io::Result<()> {
                    if global_path.exists() || global_path.is_symlink() {
                        std::fs::remove_file(&global_path)?;
                    }
                    std::fs::rename(&tmp, global_path)
                })();
                // The temp link is only reclaimed after the fallback finishes.
                let _ = std::fs::remove_file(&tmp);
                outcome.with_context(|| {
                    format!(
                        "Failed to replace global symlink at {} (atomic rename failed: {e})",
                        global_path.display()
                    )
                })
            }
            #[cfg(not(windows))]
            {
                let _ = std::fs::remove_file(&tmp);
                Err(e).with_context(|| {
                    format!(
                        "Failed to atomically replace global symlink at {}",
                        global_path.display()
                    )
                })
            }
        }
    }
}

/// Set the global default version.
pub fn set_global(version: &Version) -> Result<()> {
    let env_dir = config::envs_dir()?.join(version.as_str());
    crate::util::check_path_traversal(&env_dir, &config::envs_dir()?)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if !version_is_installed(version) {
        anyhow::bail!(
            "Flutter {version} is not installed. Run 'joy toolchain install {version}' first."
        );
    }

    let global_path = config::global_default_path()?;
    replace_global_symlink(&global_path, &env_dir)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT_ID: AtomicU32 = AtomicU32::new(90000);

    fn temp_dir() -> PathBuf {
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("joy_environment_test_{id}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Redirect XDG dirs to a throwaway location and restore them on drop, so
    /// the global-default tests never touch the real user config.
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
        let data_home = tmp.join("xdg").join("data");
        let cache_home = tmp.join("xdg").join("cache");
        std::fs::create_dir_all(&data_home).unwrap();
        std::fs::create_dir_all(&cache_home).unwrap();
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &data_home);
            std::env::set_var("XDG_CACHE_HOME", &cache_home);
        }
        XdgGuard(tmp)
    }

    fn make_fake_installation(version: &Version) -> PathBuf {
        let env_dir = config::envs_dir().unwrap().join(version.as_str());
        std::fs::create_dir_all(env_dir.join("bin")).unwrap();
        std::fs::write(env_dir.join("bin").join("flutter"), b"#!/bin/sh\necho fake").unwrap();
        env_dir
    }

    #[test]
    #[serial]
    fn set_global_replaces_existing_default() {
        let _guard = setup_xdg();
        let v1 = Version::new("3.28.0").unwrap();
        let v2 = Version::new("3.29.0").unwrap();
        let env1 = make_fake_installation(&v1);
        let env2 = make_fake_installation(&v2);

        set_global(&v1).unwrap();
        let global_path = config::global_default_path().unwrap();
        assert!(
            global_path.is_symlink(),
            "first default should be a symlink"
        );
        assert_eq!(std::fs::read_link(&global_path).unwrap(), env1);

        // Replacing an existing default must keep a valid symlink at every
        // observable step and end up pointing at the new version.
        set_global(&v2).unwrap();
        assert!(
            global_path.is_symlink(),
            "replacement must leave a valid symlink (not a removed gap)"
        );
        assert_eq!(std::fs::read_link(&global_path).unwrap(), env2);

        // No stale temp link may remain beside the real default.
        let parent = global_path.parent().unwrap();
        let leftovers: Vec<String> = std::fs::read_dir(parent)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".tmp."))
            .collect();
        assert!(
            leftovers.is_empty(),
            "no temp symlinks may remain after a successful swap: {leftovers:?}"
        );
    }

    #[test]
    #[serial]
    fn set_global_from_no_default() {
        let _guard = setup_xdg();
        let v = Version::new("3.29.0").unwrap();
        make_fake_installation(&v);

        set_global(&v).unwrap();

        let global_path = config::global_default_path().unwrap();
        assert!(
            global_path.is_symlink(),
            "default should be created as a symlink"
        );
        assert_eq!(
            std::fs::read_link(&global_path).unwrap(),
            config::envs_dir().unwrap().join("3.29.0")
        );
    }

    #[test]
    #[serial]
    fn set_global_fails_for_uninstalled_version() {
        let _guard = setup_xdg();
        let v = Version::new("3.99.0").unwrap();
        let err = set_global(&v).unwrap_err();
        assert!(
            err.to_string().contains("not installed"),
            "error should mention the version is not installed, got: {err}"
        );
    }

    #[test]
    #[serial]
    fn version_is_installed_reflects_disk_state() {
        let _guard = setup_xdg();
        let v = Version::new("3.29.0").unwrap();
        assert!(
            !version_is_installed(&v),
            "fresh XDG home must have nothing installed"
        );
        make_fake_installation(&v);
        assert!(
            version_is_installed(&v),
            "after install it must be installed"
        );
    }

    /// A stale .joy.json pointing at an uninstalled version must resolve but be
    /// reported as `ConfiguredNotInstalled`, not `Active` — otherwise commands
    /// fail later with no diagnostic hinting at the real cause.
    #[test]
    #[serial]
    fn resolve_active_status_reports_stale_project_config_as_configured() {
        let _guard = setup_xdg();
        let tmp = temp_dir();
        std::fs::create_dir_all(&tmp).unwrap();

        // Project config pins a version that is NOT installed.
        let configured = Version::new("3.99.0").unwrap();
        std::fs::write(
            tmp.join(crate::config::PROJECT_CONFIG_FILE),
            format!("{{\"version\": \"{}\"}}", configured.as_str()),
        )
        .unwrap();

        let orig = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();
        let status = resolve_active_status();
        std::env::set_current_dir(&orig).unwrap();

        assert_eq!(
            status,
            ActiveStatus::ConfiguredNotInstalled(configured),
            "stale project config must be reported as configured, not active"
        );
    }

    #[test]
    #[serial]
    fn resolve_active_status_is_active_when_installed() {
        let _guard = setup_xdg();
        let v = Version::new("3.29.0").unwrap();
        make_fake_installation(&v);
        let tmp = temp_dir();
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join(crate::config::PROJECT_CONFIG_FILE),
            format!("{{\"version\": \"{}\"}}", v.as_str()),
        )
        .unwrap();

        let orig = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();
        let status = resolve_active_status();
        std::env::set_current_dir(&orig).unwrap();

        assert_eq!(
            status,
            ActiveStatus::Active(v),
            "an installed configured version must be Active"
        );
    }

    #[test]
    #[serial]
    fn resolve_active_status_none_when_nothing_configured() {
        let _guard = setup_xdg();
        let tmp = temp_dir();
        std::fs::create_dir_all(&tmp).unwrap();

        let orig = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();
        let status = resolve_active_status();
        std::env::set_current_dir(&orig).unwrap();

        assert_eq!(status, ActiveStatus::None);
    }
}
