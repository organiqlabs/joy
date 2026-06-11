mod cache;
mod cli;
mod config;
mod engine_cache;
mod environment;
mod git_cache;
mod install;
mod profile;
mod project;
mod releases;
mod toolchain;
mod toolchain_meta;
mod util;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};
use profile::Profile;
use std::str::FromStr;

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Ensure directories exist on startup
    std::fs::create_dir_all(config::envs_dir())?;
    std::fs::create_dir_all(config::engine_cache_dir())?;
    std::fs::create_dir_all(config::git_cache_dir())?;

    match cli.command {
        Commands::Current => environment::show_current(),
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
        Commands::Toolchain { command } => match command {
            cli::ToolchainCommands::Install {
                version,
                force,
                git,
                repo,
                profile,
            } => {
                let profile = Profile::from_str(&profile).unwrap_or_else(|_| Profile::Default);
                toolchain::install_with_opts(&version, force, git, repo.as_deref(), &profile)
            }
            cli::ToolchainCommands::Remove { version } => toolchain::remove(&version),
            cli::ToolchainCommands::List => toolchain::list(),
        },
    }
}
