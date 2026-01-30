//! Shell configuration and completion paths.
//!
//! This module handles locating shell configuration files (e.g., `.bashrc`, `.zshrc`)
//! and completion directories for different shells.

use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};
use std::path::PathBuf;

use crate::path::home_dir;

/// Get the user's home directory or return an error.
pub fn home_dir_required() -> Result<PathBuf, std::io::Error> {
    home_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Cannot determine home directory. Set $HOME (Unix) or $USERPROFILE (Windows)",
        )
    })
}

/// Get PowerShell profile paths in order of preference.
/// On Windows, returns both PowerShell Core (7+) and Windows PowerShell (5.1) paths.
/// On Unix, uses the conventional ~/.config/powershell location.
pub fn powershell_profile_paths(home: &std::path::Path) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        // Use platform-specific Documents path (handles non-English Windows)
        let docs = dirs::document_dir().unwrap_or_else(|| home.join("Documents"));
        vec![
            // PowerShell Core 6+ (pwsh.exe) - preferred
            docs.join("PowerShell")
                .join("Microsoft.PowerShell_profile.ps1"),
            // Windows PowerShell 5.1 (powershell.exe) - legacy but still common
            docs.join("WindowsPowerShell")
                .join("Microsoft.PowerShell_profile.ps1"),
        ]
    }
    #[cfg(not(windows))]
    {
        vec![
            home.join(".config")
                .join("powershell")
                .join("Microsoft.PowerShell_profile.ps1"),
        ]
    }
}

/// Returns the config file paths for a shell.
///
/// The `cmd` parameter affects the Fish functions filename (e.g., `wt.fish` or `git-wt.fish`).
/// Returns paths in order of preference. The first existing file should be used.
pub fn config_paths(shell: super::Shell, cmd: &str) -> Result<Vec<PathBuf>, std::io::Error> {
    let home = home_dir_required()?;

    Ok(match shell {
        super::Shell::Bash => {
            // Use .bashrc - sourced by interactive shells (login shells should source .bashrc)
            vec![home.join(".bashrc")]
        }
        super::Shell::Zsh => {
            let zdotdir = std::env::var("ZDOTDIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.clone());
            vec![zdotdir.join(".zshrc")]
        }
        super::Shell::Fish => {
            // For fish, we write to functions/ which is autoloaded on first use.
            // This ensures PATH is fully configured before our function loads,
            // fixing the issue where Homebrew PATH setup in config.fish runs
            // after conf.d/ files. See: https://github.com/max-sixty/worktrunk/issues/566
            vec![
                home.join(".config")
                    .join("fish")
                    .join("functions")
                    .join(format!("{}.fish", cmd)),
            ]
        }
        super::Shell::PowerShell => powershell_profile_paths(&home),
    })
}

/// Returns the legacy fish conf.d path for cleanup purposes.
///
/// Previously, fish shell integration was installed to `~/.config/fish/conf.d/{cmd}.fish`.
/// This caused issues with Homebrew PATH setup (see issue #566). We now install to
/// `functions/{cmd}.fish` instead. This method returns the legacy path so install/uninstall
/// can clean it up.
pub fn legacy_fish_conf_d_path(cmd: &str) -> Result<PathBuf, std::io::Error> {
    let home = home_dir_required()?;
    Ok(home
        .join(".config")
        .join("fish")
        .join("conf.d")
        .join(format!("{}.fish", cmd)))
}

/// Returns the path to the native completion directory for a shell.
///
/// The `cmd` parameter affects the completion filename (e.g., `wt.fish` or `git-wt.fish`).
///
/// Note: Bash and Zsh use inline lazy completions in the init script.
/// Only Fish uses a separate completion file at ~/.config/fish/completions/
/// (installed by `wt config shell install`) that uses $WORKTRUNK_BIN to bypass
/// the shell function wrapper.
pub fn completion_path(shell: super::Shell, cmd: &str) -> Result<PathBuf, std::io::Error> {
    let home = home_dir_required()?;

    // Use etcetera for XDG-compliant paths when available
    let strategy = choose_base_strategy().ok();

    Ok(match shell {
        super::Shell::Bash => {
            // XDG_DATA_HOME defaults to ~/.local/share
            let data_home = strategy
                .as_ref()
                .map(|s| s.data_dir())
                .unwrap_or_else(|| home.join(".local").join("share"));
            data_home
                .join("bash-completion")
                .join("completions")
                .join(cmd)
        }
        super::Shell::Zsh => home.join(".zfunc").join(format!("_{}", cmd)),
        super::Shell::Fish => {
            // XDG_CONFIG_HOME defaults to ~/.config
            let config_home = strategy
                .as_ref()
                .map(|s| s.config_dir())
                .unwrap_or_else(|| home.join(".config"));
            config_home
                .join("fish")
                .join("completions")
                .join(format!("{}.fish", cmd))
        }
        super::Shell::PowerShell => {
            // PowerShell doesn't use a separate completion file - completions are
            // registered inline in the profile using Register-ArgumentCompleter
            // Return a dummy path that won't be used
            home.join(format!(".{}-powershell-completions", cmd))
        }
    })
}
