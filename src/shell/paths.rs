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

/// Parse the stdout of `nu -c "echo $nu.default-config-dir"` into a path.
///
/// Returns `Some(path)` if stdout contains a non-empty trimmed path, `None` otherwise.
fn parse_nu_config_output(stdout: &[u8]) -> Option<PathBuf> {
    let path_str = std::str::from_utf8(stdout).ok()?;
    let path = PathBuf::from(path_str.trim());
    (!path.as_os_str().is_empty()).then_some(path)
}

/// Query `nu` for its default config directory.
///
/// Returns `Some(path)` if the `nu` binary is in PATH and reports its config dir,
/// `None` otherwise (not installed, PATH issues, timeout, etc.).
fn query_nu_config_dir() -> Option<PathBuf> {
    let output = crate::shell_exec::Cmd::new("nu")
        .args(["-c", "echo $nu.default-config-dir"])
        .run()
        .ok()
        .filter(|o| o.status.success())?;
    parse_nu_config_output(&output.stdout)
}

/// Resolve the nushell config directory from a queried path or platform defaults.
///
/// If `queried` is `Some`, uses that directly. Otherwise falls back to etcetera's
/// platform config dir, then `home/.config`.
fn resolve_nushell_config_dir(home: &std::path::Path, queried: Option<PathBuf>) -> PathBuf {
    queried.unwrap_or_else(|| {
        choose_base_strategy()
            .map(|s| s.config_dir())
            .unwrap_or_else(|_| home.join(".config"))
            .join("nushell")
    })
}

/// Get Nushell's default config directory (single best path for writing).
///
/// Used by `completion_path()` to determine where to write completions.
/// Queries `nu` for `$nu.default-config-dir` to handle platform-specific paths.
/// On macOS, this is `~/Library/Application Support/nushell` rather than `~/.config/nushell`.
/// Falls back to etcetera's config_dir if the nu command fails.
fn nushell_config_dir(home: &std::path::Path) -> PathBuf {
    resolve_nushell_config_dir(home, query_nu_config_dir())
}

/// Get candidate nushell config directories for checking if integration is installed.
///
/// Returns multiple paths to check because:
/// - Installation might use the path from `nu -c "echo $nu.default-config-dir"`
/// - Runtime detection might fail the `nu` command (PATH issues, timeout, etc.)
/// - We need to find the config file regardless of which path was used
///
/// Returns paths in priority order: queried path first, then fallbacks.
/// Callers that pick `first()` to write get the same path as `nushell_config_dir()`.
fn nushell_config_candidates(home: &std::path::Path) -> Vec<PathBuf> {
    let mut candidates = vec![];

    // Best path: query nu directly (same source of truth as nushell_config_dir)
    if let Some(queried) = query_nu_config_dir() {
        candidates.push(queried);
    }

    // Fallbacks for when nu query fails at runtime but succeeded during install:

    // etcetera's platform config dir (matches nushell_config_dir fallback)
    if let Ok(strategy) = choose_base_strategy() {
        candidates.push(strategy.config_dir().join("nushell"));
    }

    // XDG_CONFIG_HOME/nushell if set
    if let Ok(xdg_config) = std::env::var("XDG_CONFIG_HOME") {
        candidates.push(PathBuf::from(xdg_config).join("nushell"));
    }

    // ~/.config/nushell (XDG default)
    candidates.push(home.join(".config").join("nushell"));

    // On macOS, add ~/Library/Application Support/nushell
    #[cfg(target_os = "macos")]
    {
        candidates.push(
            home.join("Library")
                .join("Application Support")
                .join("nushell"),
        );
    }

    candidates
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
        super::Shell::Nushell => {
            // Nushell vendor autoload directory - check multiple candidate locations because:
            // - Installation might use the path from `nu -c "echo $nu.default-config-dir"`
            // - Runtime detection might fail the `nu` command (PATH issues, timeout, etc.)
            // - We need to find the config file regardless of which path was used during install
            nushell_config_candidates(&home)
                .into_iter()
                .map(|config_dir| {
                    config_dir
                        .join("vendor")
                        .join("autoload")
                        .join(format!("{}.nu", cmd))
                })
                .collect()
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
            let config_home = strategy
                .as_ref()
                .map(|s| s.config_dir())
                .unwrap_or_else(|| home.join(".config"));
            config_home
                .join("fish")
                .join("completions")
                .join(format!("{}.fish", cmd))
        }
        super::Shell::Nushell => {
            // Nushell completions are defined inline in the init script
            // Return a path in the vendor autoload directory (same as config)
            let config_dir = nushell_config_dir(&home);
            config_dir
                .join("vendor")
                .join("autoload")
                .join(format!("{}.nu", cmd))
        }
        super::Shell::PowerShell => {
            // PowerShell doesn't use a separate completion file - completions are
            // registered inline in the profile using Register-ArgumentCompleter
            // Return a dummy path that won't be used
            home.join(format!(".{}-powershell-completions", cmd))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_nu_config_output_valid_path() {
        let stdout = b"/home/user/.config/nushell\n";
        assert_eq!(
            parse_nu_config_output(stdout),
            Some(PathBuf::from("/home/user/.config/nushell"))
        );
    }

    #[test]
    fn test_parse_nu_config_output_trims_whitespace() {
        let stdout = b"  /home/user/.config/nushell  \n";
        assert_eq!(
            parse_nu_config_output(stdout),
            Some(PathBuf::from("/home/user/.config/nushell"))
        );
    }

    #[test]
    fn test_parse_nu_config_output_empty() {
        assert_eq!(parse_nu_config_output(b""), None);
        assert_eq!(parse_nu_config_output(b"  \n"), None);
    }

    #[test]
    fn test_parse_nu_config_output_invalid_utf8() {
        assert_eq!(parse_nu_config_output(&[0xFF, 0xFE]), None);
    }

    #[test]
    fn test_nushell_config_candidates_includes_xdg_and_defaults() {
        let home = PathBuf::from("/home/user");

        let candidates = nushell_config_candidates(&home);

        // Should include default XDG path
        assert!(
            candidates
                .iter()
                .any(|p| p == &home.join(".config").join("nushell")),
            "Should include ~/.config/nushell in candidates"
        );

        // On macOS, should include ~/Library/Application Support/nushell
        #[cfg(target_os = "macos")]
        {
            assert!(
                candidates.iter().any(|p| p
                    == &home
                        .join("Library")
                        .join("Application Support")
                        .join("nushell")),
                "Should include ~/Library/Application Support/nushell in candidates on macOS"
            );
        }

        // All candidates should be nushell config dirs
        assert!(
            candidates.iter().all(|p| p.ends_with("nushell")),
            "All candidates should end with 'nushell'"
        );
    }

    #[test]
    fn test_nushell_config_candidates_returns_at_least_two() {
        let home = PathBuf::from("/home/user");
        let candidates = nushell_config_candidates(&home);

        // Even without `nu` in PATH, we should get fallback candidates
        // (etcetera config dir + ~/.config/nushell, plus macOS path on macOS)
        assert!(
            candidates.len() >= 2,
            "Should return at least 2 candidate paths, got: {candidates:?}"
        );
    }

    #[test]
    fn test_resolve_nushell_config_dir_with_queried_path() {
        let home = PathBuf::from("/home/user");
        let queried = PathBuf::from("/custom/nushell");
        assert_eq!(
            resolve_nushell_config_dir(&home, Some(queried.clone())),
            queried
        );
    }

    #[test]
    fn test_resolve_nushell_config_dir_without_queried_path() {
        let home = PathBuf::from("/home/user");
        let result = resolve_nushell_config_dir(&home, None);
        // Should fall back to a platform config dir ending in "nushell"
        assert!(
            result.ends_with("nushell"),
            "Fallback should end with 'nushell': {result:?}"
        );
    }
}
