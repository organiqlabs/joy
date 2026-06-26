use crate::cli::ShellVariant;
use anyhow::{Context, Result};
use clap::Command;
use clap_complete::{Shell, generate};
use std::fs;
use std::path::{Path, PathBuf};

/// Known zsh completion directories, ordered by priority.
pub fn zsh_completion_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Ok(custom) = std::env::var("ZSH_CUSTOM") {
        dirs.push(PathBuf::from(custom).join("completions"));
    }
    if let Ok(zsh) = std::env::var("ZSH") {
        dirs.push(PathBuf::from(zsh).join("completions"));
    }
    if let Ok(fpath_str) = std::env::var("FPATH") {
        for entry in fpath_str.split(':') {
            let p = PathBuf::from(entry);
            if p.is_dir() {
                dirs.push(p);
            }
        }
    }
    dirs.push(PathBuf::from("~/.zsh/completions"));
    dirs.push(PathBuf::from("~/.local/share/zsh/completions"));
    dirs.push(PathBuf::from("~/.zfunc"));

    dirs
}

/// Resolve a tilde path to absolute.
fn resolve_home(path: &Path) -> PathBuf {
    if path.starts_with("~")
        && let Ok(home) = std::env::var("HOME")
    {
        let rest = path.strip_prefix("~").unwrap_or(path);
        return PathBuf::from(home).join(rest.strip_prefix("/").unwrap_or(rest));
    }
    path.to_path_buf()
}

/// Get the appropriate completion directory for a shell.
/// For zsh, uses the first writable zsh completion directory.
/// For other shells, uses the shell's config directory.
pub fn completion_dir_for_shell(shell: ShellVariant) -> PathBuf {
    match shell {
        ShellVariant::Zsh => {
            for dir in &zsh_completion_dirs() {
                let abs = resolve_home(dir);
                if abs.is_dir() || abs.parent().is_some_and(|p| p.is_dir()) {
                    return abs;
                }
            }
            resolve_home(&PathBuf::from("~/.zsh/completions"))
        }
        ShellVariant::Bash => PathBuf::from("/etc/bash_completion.d"),
        ShellVariant::Fish => {
            if let Ok(home) = std::env::var("HOME") {
                PathBuf::from(home).join(".config/fish/completions")
            } else {
                PathBuf::from("/etc/fish/completions")
            }
        }
        ShellVariant::PowerShell => std::env::current_dir().unwrap_or_default(),
        ShellVariant::Elvish => {
            if let Ok(home) = std::env::var("HOME") {
                PathBuf::from(home).join(".elvish")
            } else {
                PathBuf::from("/etc/elvish")
            }
        }
    }
}

/// Write shell completion script to a directory.
/// Creates the directory if it doesn't exist.
/// Returns the path to the written file.
pub fn install_completions(shell: Shell, cmd: &mut Command, dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(dir).with_context(|| format!("Failed to create {dir:?}"))?;
    let filename = match shell {
        Shell::Bash => "joy.bash",
        Shell::Zsh => "_joy",
        Shell::Fish => "joy.fish",
        Shell::PowerShell => "_joy.ps1",
        Shell::Elvish => "joy.elv",
        _ => unreachable!(),
    };
    let dest = dir.join(filename);
    let mut file = fs::File::create(&dest).with_context(|| format!("Failed to create {dest:?}"))?;
    generate(shell, cmd, "joy", &mut file);
    Ok(dest)
}

/// Detect the user's current shell from `$SHELL`.
pub fn current_shell() -> Option<ShellVariant> {
    let shell = std::env::var("SHELL").ok()?;
    let name = Path::new(&shell).file_name()?.to_str()?;
    match name {
        "bash" => Some(ShellVariant::Bash),
        "zsh" => Some(ShellVariant::Zsh),
        "fish" => Some(ShellVariant::Fish),
        "powershell" | "pwsh" => Some(ShellVariant::PowerShell),
        _ => None,
    }
}

/// Check if completions are installed for a given shell.
pub fn is_completions_installed(shell: ShellVariant) -> bool {
    let filename = match shell {
        ShellVariant::Bash => "joy.bash",
        ShellVariant::Zsh => "_joy",
        ShellVariant::Fish => "joy.fish",
        ShellVariant::PowerShell => "_joy.ps1",
        ShellVariant::Elvish => "joy.elv",
    };

    if shell == ShellVariant::Zsh {
        return zsh_completion_dirs()
            .iter()
            .any(|dir| resolve_home(dir).join(filename).exists());
    }

    if let Ok(shell_env) = std::env::var("SHELL")
        && let Some(parent) = Path::new(&shell_env).parent()
        && parent.join(filename).exists()
    {
        return true;
    }

    false
}

/// Return a human-friendly installation hint for a shell.
pub fn install_hint(shell: ShellVariant) -> String {
    match shell {
        ShellVariant::Bash => r#"Add to ~/.bashrc:
  source <(joy completions generate bash)"#
            .to_string(),
        ShellVariant::Zsh => r#"Run:
  joy completions install zsh
Or manually:
  mkdir -p ~/.zsh/completions
  joy completions generate zsh > ~/.zsh/completions/_joy
Then add to ~/.zshrc:
  fpath=(~/.zsh/completions $fpath)
  autoload -U compinit && compinit"#
            .to_string(),
        ShellVariant::Fish => r#"Run:
  joy completions install fish"#
            .to_string(),
        ShellVariant::PowerShell => r#"Add to your profile:
  joy completions generate powershell | Out-String | Invoke-Expression"#
            .to_string(),
        ShellVariant::Elvish => r#"Add to ~/.elvish/rc.elv:
  eval (joy completions generate elvish | slurp)"#
            .to_string(),
    }
    .to_string()
}

/// Generate dynamic completions for available release versions.
pub fn complete_release_versions() -> Vec<String> {
    let mut versions: Vec<String> = crate::releases::fetch_releases()
        .ok()
        .into_iter()
        .flat_map(|releases| releases.into_iter())
        .map(|r| r.version)
        .collect();
    versions.sort();
    versions.dedup();
    versions
}

/// Generate dynamic completions for installed toolchain versions.
pub fn complete_installed_versions() -> Vec<String> {
    let envs_dir = crate::config::envs_dir();
    if !envs_dir.is_dir() {
        return vec![];
    }
    let mut versions: Vec<String> = fs::read_dir(&envs_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    versions.sort();
    versions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::CommandFactory;
    use serial_test::serial;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT_ID: AtomicU32 = AtomicU32::new(500);

    fn temp_dir() -> PathBuf {
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("joy_completions_test_{id}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    struct EnvGuard;

    impl EnvGuard {
        fn set_zsh_env() -> Self {
            unsafe {
                std::env::set_var("SHELL", "/usr/bin/zsh");
            }
            EnvGuard
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                std::env::remove_var("SHELL");
            }
        }
    }

    // --- install_completions ---

    #[test]
    fn installs_zsh_completions_to_temp_dir() {
        let tmp = temp_dir();
        let mut cmd = Cli::command();
        let dest = install_completions(Shell::Zsh, &mut cmd, &tmp).unwrap();

        assert_eq!(dest.file_name().unwrap(), "_joy");
        assert!(dest.exists());

        let content = std::fs::read_to_string(&dest).unwrap();
        assert!(
            content.starts_with("#compdef joy"),
            "Expected #compdef header, got: {content:.50}"
        );
    }

    #[test]
    fn installs_bash_completions_to_temp_dir() {
        let tmp = temp_dir();
        let mut cmd = Cli::command();
        let dest = install_completions(Shell::Bash, &mut cmd, &tmp).unwrap();

        assert_eq!(dest.file_name().unwrap(), "joy.bash");
        assert!(dest.exists());

        let content = std::fs::read_to_string(&dest).unwrap();
        assert!(
            content.contains("_joy"),
            "Expected bash function _joy, got: {content:.50}"
        );
    }

    #[test]
    fn installs_fish_completions_to_temp_dir() {
        let tmp = temp_dir();
        let mut cmd = Cli::command();
        let dest = install_completions(Shell::Fish, &mut cmd, &tmp).unwrap();

        assert_eq!(dest.file_name().unwrap(), "joy.fish");
        assert!(dest.exists());

        let content = std::fs::read_to_string(&dest).unwrap();
        assert!(content.contains("__fish_joy"), "Expected fish completion");
    }

    #[test]
    fn install_overwrites_existing_file() {
        let tmp = temp_dir();
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("_joy"), b"stale").unwrap();

        let mut cmd = Cli::command();
        let dest = install_completions(Shell::Zsh, &mut cmd, &tmp).unwrap();

        let content = std::fs::read_to_string(&dest).unwrap();
        assert!(
            content.starts_with("#compdef joy"),
            "Expected fresh completion"
        );
    }

    // --- current_shell ---

    #[test]
    #[serial]
    fn detects_zsh_from_env() {
        let _g = EnvGuard::set_zsh_env();
        let shell = current_shell();
        assert!(matches!(shell, Some(ShellVariant::Zsh)));
    }

    #[test]
    #[serial]
    fn returns_none_when_shell_unset() {
        unsafe {
            std::env::remove_var("SHELL");
        }
        assert!(current_shell().is_none());
    }

    // --- is_completions_installed ---

    #[test]
    #[serial]
    fn reports_installed_when_file_present() {
        let _g = EnvGuard::set_zsh_env();
        let tmp = temp_dir();
        let completions_dir = tmp.join("completions");
        let mut cmd = Cli::command();

        unsafe {
            std::env::set_var("ZSH_CUSTOM", &tmp);
        }
        install_completions(Shell::Zsh, &mut cmd, &completions_dir).unwrap();

        assert!(is_completions_installed(ShellVariant::Zsh));
    }

    #[test]
    #[serial]
    fn reports_not_installed_when_missing() {
        let _g = EnvGuard::set_zsh_env();
        let tmp = temp_dir();
        unsafe {
            std::env::set_var("ZSH_CUSTOM", &tmp);
            std::env::remove_var("ZSH");
        }
        assert!(!is_completions_installed(ShellVariant::Zsh));
        unsafe {
            std::env::remove_var("ZSH_CUSTOM");
        }
    }

    // --- install_hint ---

    #[test]
    fn returns_hint_for_each_shell() {
        for shell in &[
            ShellVariant::Zsh,
            ShellVariant::Bash,
            ShellVariant::Fish,
            ShellVariant::PowerShell,
            ShellVariant::Elvish,
        ] {
            let hint = install_hint(*shell);
            assert!(!hint.is_empty(), "hint for {shell:?} should not be empty");
        }
    }

    // --- complete_installed_versions ---

    #[test]
    #[serial]
    fn lists_nothing_when_no_envs_dir() {
        let tmp = temp_dir();
        unsafe {
            std::env::set_var("XDG_DATA_HOME", tmp.join("data"));
            std::env::set_var("XDG_CACHE_HOME", tmp.join("cache"));
        }
        let versions = complete_installed_versions();
        assert!(versions.is_empty());
    }
}
