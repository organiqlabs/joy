use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "dartup", about = "A fast Flutter version manager", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Show currently active Flutter version
    Current,
    /// List available Flutter releases from the official channel
    Releases {
        /// Show all releases (not just recent)
        #[arg(long)]
        all: bool,
    },
    /// Run garbage collection on unused cached artifacts
    Gc {
        /// Also clean the shared Git object cache
        #[arg(long)]
        git: bool,
        /// Also clean the shared engine cache
        #[arg(long)]
        engines: bool,
    },
    /// Check that dartup is set up correctly
    Doctor,
    /// Set the global default Flutter toolchain (e.g., "3.29.0", "stable")
    Default { version: Option<String> },
    /// Set or list directory-specific overrides
    Override {
        #[command(subcommand)]
        command: OverrideCommands,
    },
    /// Manage Flutter SDK toolchains (versions/channels)
    Toolchain {
        #[command(subcommand)]
        command: ToolchainCommands,
    },
}

#[derive(Subcommand)]
pub enum OverrideCommands {
    /// Set a directory-specific override (stored in .dartup/override)
    Set { version: String },
    /// List active overrides in parent directories
    List,
}

#[derive(Subcommand)]
pub enum ToolchainCommands {
    /// Install a Flutter SDK toolchain (e.g., "3.29.0", "stable", "beta")
    Install {
        version: String,
        /// Re-download even if cached
        #[arg(short, long)]
        force: bool,
        /// Clone from Git repo using the shared object cache
        #[arg(long)]
        git: bool,
        /// Git remote URL (required with --git, defaults to flutter/flutter)
        #[arg(long)]
        repo: Option<String>,
    },
    /// Remove an installed Flutter toolchain
    Remove { version: String },
    /// List installed Flutter toolchains
    List,
}
