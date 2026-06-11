use crate::config;
use crate::profile::Profile;
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
) -> Result<()> {
    if git {
        crate::install::install_version_git_with_profile(version, repo, force, profile)
    } else {
        crate::install::install_version(version, force, profile)
    }
}

/// Remove an installed Flutter toolchain (version/channel)
pub fn remove(version: &str) -> Result<()> {
    crate::environment::remove_version(version)
}

/// List installed Flutter toolchains
pub fn list() -> Result<()> {
    crate::environment::list_versions()
}

/// Set the global default toolchain (delegates to environment::set_global)
pub fn set_default(version: &str) -> Result<()> {
    crate::environment::set_global(version)
}

/// Show the current global default
pub fn show_default() {
    let global_path = config::global_default_path();
    if global_path.is_symlink()
        && let Ok(target) = std::fs::read_link(&global_path)
        && let Some(name) = target.file_name()
    {
        println!(
            "{} {} → {}",
            "default:".bold(),
            name.to_string_lossy().green().bold(),
            target.display()
        );
        return;
    }
    println!(
        "{} No global default set. Use 'dartup default <version>' to set one.",
        "ℹ️".bold()
    );
}

/// Walk up from cwd to find all .dartup/override files
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

/// Set a directory-specific override (stored in .dartup/override)
pub fn set_override(version: &str) -> Result<()> {
    // Verify the version is installed
    let env_dir = config::envs_dir().join(version);
    if !env_dir.join("bin").join("flutter").exists()
        && !env_dir.join("bin").join("flutter.bat").exists()
    {
        anyhow::bail!("Flutter {version} is not installed. Run 'dartup install {version}' first.");
    }

    let cwd = std::env::current_dir()?;
    let override_path = config::override_path(&cwd);

    // Create .dartup directory if needed
    if let Some(parent) = override_path.parent() {
        std::fs::create_dir_all(parent)
            .context("Failed to create .dartup directory for override")?;
    }

    std::fs::write(&override_path, version).context("Failed to write .dartup/override")?;

    println!(
        "✅ Override set: Flutter {} for {}",
        version.green().bold(),
        cwd.display()
    );
    println!("   (stored in {})", override_path.display());

    Ok(())
}

/// List active overrides found by walking up from cwd
pub fn list_overrides() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let overrides = find_overrides(&cwd);

    if overrides.is_empty() {
        println!(
            "{} No overrides found in current or parent directories.",
            "ℹ️".bold()
        );
        return Ok(());
    }

    println!("{}", "Active overrides:".bold());
    for (path, version) in &overrides {
        let is_active = path == &cwd;
        if is_active {
            println!(
                "  {} → {} {}",
                path.display(),
                version.green().bold(),
                "(current)".green()
            );
        } else {
            println!("  {} → {}", path.display(), version.bold());
        }
    }
    println!(
        "\nNearest override: {} → {}",
        overrides[0].0.display(),
        overrides[0].1.green().bold()
    );

    Ok(())
}
