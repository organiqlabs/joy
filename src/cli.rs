use clap::{Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

#[derive(Parser)]
#[command(name = "joy", about = "A fast Flutter version manager", version)]
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
    /// Check that joy is set up correctly
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
    /// Shell completion management
    Completions {
        #[command(subcommand)]
        command: CompletionsCommands,
    },
    /// (hidden) Dynamic completion source for shells
    #[command(hide = true)]
    Complete { kind: CompleteKind },
}

/// Completion source kind for dynamic arg completions
#[derive(ValueEnum, Clone, Copy, PartialEq)]
pub enum CompleteKind {
    /// List installed toolchain versions
    InstalledVersions,
    /// List available release versions
    ReleaseVersions,
}

#[derive(Subcommand)]
pub enum CompletionsCommands {
    /// Generate and print completion script for the given shell
    Generate { shell: ShellVariant },
    /// Install shell completions to the system
    Install {
        /// Shell type (auto-detected from $SHELL if not specified)
        shell: Option<ShellVariant>,
    },
}

#[derive(Debug, ValueEnum, Clone, Copy, PartialEq)]
pub enum ShellVariant {
    Bash,
    Zsh,
    Fish,
    PowerShell,
    Elvish,
}

impl From<ShellVariant> for Shell {
    fn from(v: ShellVariant) -> Self {
        match v {
            ShellVariant::Bash => Shell::Bash,
            ShellVariant::Zsh => Shell::Zsh,
            ShellVariant::Fish => Shell::Fish,
            ShellVariant::PowerShell => Shell::PowerShell,
            ShellVariant::Elvish => Shell::Elvish,
        }
    }
}

#[derive(Subcommand)]
pub enum OverrideCommands {
    /// Set a directory-specific override (stored in .joy/override)
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
        /// Installation profile: minimal (SDK only), default (SDK+engine), full (all platforms)
        #[arg(long, default_value = "default")]
        profile: String,
        /// Skip SHA256 checksum verification after download
        #[arg(long)]
        skip_checksum: bool,
    },
    /// Remove one or more installed Flutter toolchains
    Remove { versions: Vec<String> },
    /// List installed Flutter toolchains
    List,
}
