use anyhow::Result;
use clap::{CommandFactory, Parser};
use clap_complete::generate;
use cli::{Cli, Commands, ShellVariant};
use joy::cache;
use joy::cli;
use joy::completions;
use joy::config;
use joy::engine_cache;
use joy::environment;
use joy::profile::Profile;
use joy::releases;
use joy::toolchain;
use joy::util;
use std::io;
use std::str::FromStr;

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Ensure directories exist on startup
    std::fs::create_dir_all(config::envs_dir())?;
    std::fs::create_dir_all(engine_cache::cache_dir())?;
    std::fs::create_dir_all(config::git_cache_dir())?;

    match cli.command {
        Commands::Releases { all } => releases::list_releases(all),
        Commands::Gc { git, engines } => cache::run_gc(git, engines),
        Commands::Doctor => environment::run_doctor(),
        Commands::Default { version } => match version {
            Some(v) => toolchain::set_default(&v),
            None => {
                toolchain::show_default();
                Ok(())
            }
        },
        Commands::Override { command } => match command {
            cli::OverrideCommands::Set { version } => toolchain::set_override(&version),
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
        Commands::Update { force } => toolchain::update_active(force),
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
                let profile = Profile::from_str(&profile).unwrap_or_else(|_| Profile::Default);
                toolchain::install_with_opts(
                    &version,
                    force,
                    git,
                    repo.as_deref(),
                    &profile,
                    skip_checksum,
                )
            }
            Some(cli::ToolchainCommands::Remove { versions }) => toolchain::remove_many(&versions),
            Some(cli::ToolchainCommands::List) => toolchain::list(),
        },
    }
}
