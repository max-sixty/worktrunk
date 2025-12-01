use askama::Template;
use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};
use std::path::PathBuf;

use crate::path::home_dir;

/// Get the user's home directory or return an error
fn home_dir_required() -> Result<PathBuf, std::io::Error> {
    home_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Cannot determine home directory. Set $HOME environment variable",
        )
    })
}

/// Supported shells
///
/// Currently supported: bash, fish, zsh
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, strum::Display, strum::EnumString)]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
pub enum Shell {
    Bash,
    Fish,
    Zsh,
}

impl Shell {
    /// Returns the standard config file paths for this shell
    ///
    /// Returns paths in order of preference. The first existing file should be used.
    pub fn config_paths(&self) -> Result<Vec<PathBuf>, std::io::Error> {
        let home = home_dir_required()?;

        Ok(match self {
            Self::Bash => {
                // Use .bashrc - sourced by interactive shells (login shells should source .bashrc)
                vec![home.join(".bashrc")]
            }
            Self::Zsh => {
                let zdotdir = std::env::var("ZDOTDIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| home.clone());
                vec![zdotdir.join(".zshrc")]
            }
            Self::Fish => {
                // For fish, we write to conf.d/ which is auto-sourced
                vec![
                    home.join(".config")
                        .join("fish")
                        .join("conf.d")
                        .join("wt.fish"),
                ]
            }
        })
    }

    /// Returns the path to the native completion directory for this shell
    ///
    /// Note: Bash and Zsh use inline lazy completions in the init script.
    /// Only Fish uses a separate completion file at ~/.config/fish/completions/wt.fish
    /// (installed by `wt config shell install`) that uses $WORKTRUNK_BIN to bypass
    /// the shell function wrapper.
    pub fn completion_path(&self) -> Result<PathBuf, std::io::Error> {
        let home = home_dir_required()?;

        // Use etcetera for XDG-compliant paths when available
        let strategy = choose_base_strategy().ok();

        Ok(match self {
            Self::Bash => {
                // XDG_DATA_HOME defaults to ~/.local/share
                let data_home = strategy
                    .as_ref()
                    .map(|s| s.data_dir())
                    .unwrap_or_else(|| home.join(".local").join("share"));
                data_home
                    .join("bash-completion")
                    .join("completions")
                    .join("wt")
            }
            Self::Zsh => home.join(".zfunc").join("_wt"),
            Self::Fish => {
                // XDG_CONFIG_HOME defaults to ~/.config
                let config_home = strategy
                    .as_ref()
                    .map(|s| s.config_dir())
                    .unwrap_or_else(|| home.join(".config"));
                config_home.join("fish").join("completions").join("wt.fish")
            }
        })
    }

    /// Returns the line to add to the config file for shell integration
    ///
    /// All shells use a conditional wrapper to avoid errors when the command doesn't exist.
    pub fn config_line(&self) -> String {
        match self {
            Self::Bash | Self::Zsh => {
                format!(
                    "if command -v wt >/dev/null 2>&1; then eval \"$(command wt config shell init {})\"; fi",
                    self
                )
            }
            Self::Fish => {
                format!(
                    "if type -q wt; command wt config shell init {} | source; end",
                    self
                )
            }
        }
    }

    /// Check if shell integration is configured in any shell's config file
    ///
    /// Returns the path to the first config file with integration if found.
    /// This helps detect the "configured but not restarted shell" state.
    ///
    /// This function is prefix-agnostic - it detects integration patterns regardless
    /// of what cmd_prefix was used during configuration (wt, worktree, etc).
    pub fn is_integration_configured() -> Result<Option<PathBuf>, std::io::Error> {
        use std::fs;
        use std::io::{BufRead, BufReader};

        let home = home_dir_required()?;

        // Check common shell config files for integration patterns
        let config_files = vec![
            // Bash
            home.join(".bashrc"),
            home.join(".bash_profile"),
            home.join(".profile"),
            // Zsh
            home.join(".zshrc"),
            std::env::var("ZDOTDIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.clone())
                .join(".zshrc"),
        ];

        // Check standard config files for eval pattern (any prefix)
        for path in config_files {
            if !path.exists() {
                continue;
            }

            if let Ok(file) = fs::File::open(&path) {
                let reader = BufReader::new(file);
                for line in reader.lines().map_while(Result::ok) {
                    let trimmed = line.trim();
                    // Skip comments
                    if trimmed.starts_with('#') {
                        continue;
                    }
                    // Match lines containing: eval "$(... init ...)" or eval '$(... init ...)'
                    // This catches both the direct pattern and the guarded pattern:
                    //   eval "$(wt config shell init bash)"
                    //   if command -v wt ...; then eval "$(command wt config shell init zsh)"; fi
                    if (trimmed.contains("eval \"$(") || trimmed.contains("eval '$("))
                        && trimmed.contains(" init ")
                    {
                        return Ok(Some(path));
                    }
                }
            }
        }

        // Check Fish conf.d directory for any .fish files (Fish integration)
        let fish_conf_d = home.join(".config/fish/conf.d");
        if fish_conf_d.exists()
            && let Ok(entries) = fs::read_dir(&fish_conf_d)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("fish")
                    && let Ok(content) = fs::read_to_string(&path)
                {
                    // Look for wt shell integration (new protocol uses wt_exec + eval)
                    if content.contains("function wt_exec")
                        && content.contains("--internal")
                        && content.contains("eval")
                    {
                        return Ok(Some(path));
                    }
                }
            }
        }

        Ok(None)
    }

    /// Returns a summary of what the shell integration does for display in confirmation
    ///
    /// This just returns the same as config_line since we want to show the exact wrapper
    pub fn integration_summary(&self) -> String {
        self.config_line()
    }
}

/// Shell integration configuration
pub struct ShellInit {
    pub shell: Shell,
}

impl ShellInit {
    pub fn new(shell: Shell) -> Self {
        Self { shell }
    }

    /// Generate shell integration code
    pub fn generate(&self) -> Result<String, askama::Error> {
        match self.shell {
            Shell::Bash => {
                let posix_shim = PosixDirectivesTemplate { cmd_prefix: "wt" }.render()?;
                let template = BashTemplate {
                    shell_name: self.shell.to_string(),
                    cmd_prefix: "wt",
                    posix_shim: &posix_shim,
                };
                template.render()
            }
            Shell::Zsh => {
                let posix_shim = PosixDirectivesTemplate { cmd_prefix: "wt" }.render()?;
                let template = ZshTemplate {
                    cmd_prefix: "wt",
                    posix_shim: &posix_shim,
                };
                template.render()
            }
            Shell::Fish => {
                let template = FishTemplate { cmd_prefix: "wt" };
                template.render()
            }
        }
    }
}

/// POSIX directive shim template (shared by bash, zsh, oil)
#[derive(Template)]
#[template(path = "posix_directives.sh", escape = "none")]
struct PosixDirectivesTemplate<'a> {
    cmd_prefix: &'a str,
}

/// Bash shell template
#[derive(Template)]
#[template(path = "bash.sh", escape = "none")]
struct BashTemplate<'a> {
    shell_name: String,
    cmd_prefix: &'a str,
    posix_shim: &'a str,
}

/// Zsh shell template
#[derive(Template)]
#[template(path = "zsh.zsh", escape = "none")]
struct ZshTemplate<'a> {
    cmd_prefix: &'a str,
    posix_shim: &'a str,
}

/// Fish shell template
#[derive(Template)]
#[template(path = "fish.fish", escape = "none")]
struct FishTemplate<'a> {
    cmd_prefix: &'a str,
}

/// Detect if user's zsh has compinit enabled by probing for the compdef function.
///
/// Zsh's completion system (compinit) must be explicitly enabled - it's not on by default.
/// When compinit runs, it defines the `compdef` function. We probe for this function
/// by spawning an interactive zsh that sources the user's config, then checking if
/// compdef exists.
///
/// This approach matches what other CLI tools (hugo, podman, dvc) recommend: detect
/// the state and advise users, rather than trying to auto-enable compinit.
///
/// Returns:
/// - `Some(true)` if compinit is enabled (compdef function exists)
/// - `Some(false)` if compinit is NOT enabled
/// - `None` if detection failed (zsh not installed, timeout, error)
pub fn detect_zsh_compinit() -> Option<bool> {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    // Allow tests to bypass this check since zsh subprocess behavior varies across CI envs
    if std::env::var("WT_ASSUME_COMPINIT").is_ok() {
        return Some(true); // Assume compinit is configured
    }

    // Force compinit to be missing (for tests that expect the warning)
    if std::env::var("WT_ASSUME_NO_COMPINIT").is_ok() {
        return Some(false); // Force warning to appear
    }

    // Probe command: check if compdef function exists (proof compinit ran).
    // We use unique markers (__WT_COMPINIT_*) to avoid false matches from any
    // output the user's zshrc might produce during startup.
    let probe_cmd =
        r#"(( $+functions[compdef] )) && echo __WT_COMPINIT_YES__ || echo __WT_COMPINIT_NO__"#;

    let mut child = Command::new("zsh")
        .arg("-ic")
        .arg(probe_cmd)
        .stdout(Stdio::piped())
        .stderr(Stdio::null()) // Suppress user's zsh startup messages
        .spawn()
        .ok()?;

    let start = Instant::now();
    let timeout = Duration::from_secs(2);

    loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                // Process finished (exit status is always 0 due to || fallback in probe)
                // wait_with_output() collects remaining stdout even after try_wait() succeeds
                let output = child.wait_with_output().ok()?;
                let stdout = String::from_utf8_lossy(&output.stdout);
                return Some(stdout.contains("__WT_COMPINIT_YES__"));
            }
            Ok(None) => {
                // Still running - check timeout
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait(); // Reap zombie process
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
}

/// Check if the current shell is zsh (based on $SHELL environment variable).
///
/// Used to determine if the user's primary shell is zsh when running `install`
/// without a specific shell argument. If they're a zsh user, we show compinit
/// hints; if they're using bash/fish, we skip the hint since zsh isn't their
/// daily driver.
pub fn is_current_shell_zsh() -> bool {
    std::env::var("SHELL")
        .map(|s| s.ends_with("/zsh") || s.ends_with("/zsh-"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_from_str() {
        assert!(matches!("bash".parse::<Shell>(), Ok(Shell::Bash)));
        assert!(matches!("BASH".parse::<Shell>(), Ok(Shell::Bash)));
        assert!(matches!("fish".parse::<Shell>(), Ok(Shell::Fish)));
        assert!(matches!("zsh".parse::<Shell>(), Ok(Shell::Zsh)));
        assert!("invalid".parse::<Shell>().is_err());
    }
}
