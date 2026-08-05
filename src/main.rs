use anyhow::Result;
use clap::{CommandFactory, Parser};
use clap_complete::generate;
use cli::{Cli, Commands, ShellVariant};
use joy::cache;
use joy::cli;
use joy::completions;
use joy::config;
use joy::doctor;
use joy::engine_cache;
use joy::environment;
use joy::profile::Profile;
use joy::releases;
use joy::toolchain;
use joy::types::Version;
use joy::util;
use std::io;
use std::str::FromStr;

/// Parse a version string at the CLI boundary — the "Parse, don't validate" entry point.
/// Returns a nice error message on failure.
fn parse_version(s: &str) -> Result<Version> {
    // Normalize to lowercase: channel names are matched case-sensitively against
    // the release list, so "STABLE" must resolve to the "stable" channel and
    // install into envs/stable — not a stray envs/STABLE directory.
    let normalized = s.to_lowercase();
    Version::new(&normalized).map_err(|e| anyhow::anyhow!("Invalid version '{}': {}", s, e))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    joy::set_verbose(cli.verbose);

    if joy::is_verbose() {
        eprintln!("[debug] joy {} starting", env!("CARGO_PKG_VERSION"));
    }

    // Ensure directories exist on startup
    std::fs::create_dir_all(config::envs_dir()?)?;
    std::fs::create_dir_all(engine_cache::cache_dir()?)?;
    std::fs::create_dir_all(config::git_cache_dir()?)?;

    match cli.command {
        Commands::Releases { all, notes } => match notes {
            Some(notes_version) => {
                let active = toolchain::resolve_active_version()?;
                let target = parse_version(&notes_version)?;
                releases::show_release_notes_between(&active, &target)
            }
            None => releases::list_releases(all),
        },
        Commands::Gc { git, engines } => cache::run_gc(git, engines),
        Commands::Doctor => doctor::run_doctor(),
        Commands::Default { version } => match version {
            Some(v) => {
                let version = parse_version(&v)?;
                toolchain::set_default(&version)
            }
            None => {
                toolchain::show_default();
                Ok(())
            }
        },
        Commands::Override { command } => match command {
            cli::OverrideCommands::Set { version } => {
                let version = parse_version(&version)?;
                toolchain::set_override(&version)
            }
            cli::OverrideCommands::List => toolchain::list_overrides(),
        },
        Commands::Complete { kind } => match kind {
            cli::CompleteKind::InstalledVersions => {
                for v in completions::complete_installed_versions() {
                    println!("{v}");
                }
                Ok(())
            }
            cli::CompleteKind::ReleaseVersions => {
                for v in completions::complete_release_versions() {
                    println!("{v}");
                }
                Ok(())
            }
        },
        Commands::Completions { command } => match command {
            cli::CompletionsCommands::Generate { shell } => {
                generate(
                    Into::<clap_complete::Shell>::into(shell),
                    &mut Cli::command(),
                    "joy",
                    &mut io::stdout(),
                );
                Ok(())
            }
            cli::CompletionsCommands::Install { shell } => {
                let sv = shell
                    .unwrap_or_else(|| completions::current_shell().unwrap_or(ShellVariant::Bash));
                let com_shell = sv.into();
                let dir = completions::completion_dir_for_shell(sv);
                completions::install_completions(com_shell, &mut Cli::command(), dir.as_path())?;
                println!("Completions installed to {}", util::display_path(&dir));
                Ok(())
            }
        },
        Commands::Toolchain { command } => match command {
            None => environment::show_current(),
            Some(cli::ToolchainCommands::Install {
                version,
                force,
                git,
                repo,
                profile,
                skip_checksum,
            }) => {
                let version = parse_version(&version)?;
                // Fail loudly on a typo'd profile name instead of silently
                // installing the default profile.
                let profile = Profile::from_str(&profile)
                    .map_err(|e| anyhow::anyhow!("Invalid profile: {e}"))?;
                toolchain::install_with_opts(
                    &version,
                    force,
                    git,
                    repo.as_deref(),
                    &profile,
                    skip_checksum,
                )
            }
            Some(cli::ToolchainCommands::Remove { versions }) => {
                if versions.is_empty() {
                    anyhow::bail!(
                        "No versions specified. Usage: joy toolchain remove <version> [<version>...]"
                    );
                }
                let parsed: Result<Vec<Version>> =
                    versions.iter().map(|v| parse_version(v)).collect();
                toolchain::remove_many(&parsed?)
            }
            Some(cli::ToolchainCommands::Update { force }) => toolchain::update_active(force),
            Some(cli::ToolchainCommands::List) => toolchain::list(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_normalizes_uppercase_to_lowercase() {
        assert_eq!(parse_version("STABLE").unwrap().as_str(), "stable");
        assert_eq!(parse_version("Stable").unwrap().as_str(), "stable");
        assert_eq!(parse_version("BETA").unwrap().as_str(), "beta");
        assert_eq!(parse_version("DEV").unwrap().as_str(), "dev");
        // Concrete versions are unaffected.
        assert_eq!(parse_version("3.29.0").unwrap().as_str(), "3.29.0");
        assert_eq!(
            parse_version("3.29.0-BETA.1").unwrap().as_str(),
            "3.29.0-beta.1"
        );
    }

    #[test]
    fn parse_version_still_rejects_invalid_input() {
        let err = parse_version("bad version!").unwrap_err().to_string();
        assert!(err.contains("Invalid version"), "unexpected error: {err}");
    }
}
