//! Shell integration for worktrunk.
//!
//! This module provides:
//! - Shell detection and configuration path discovery
//! - Shell integration line detection for config files
//! - Shell initialization code generation (bash, zsh, fish, powershell)

mod detection;
mod paths;
mod utils;

use askama::Template;

// Re-export public types and functions
pub use detection::{
    BypassAlias, DetectedLine, FileDetectionResult, is_shell_integration_line,
    scan_for_detection_details,
};
pub use paths::{completion_path, config_paths, legacy_fish_conf_d_path};
pub use utils::{current_shell, detect_zsh_compinit, extract_filename_from_path};

/// Supported shells
///
/// Currently supported: bash, fish, zsh, powershell
///
/// On Windows, Git Bash users should use `bash` for shell integration.
/// PowerShell integration is available for native Windows users without Git Bash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum, strum::Display, strum::EnumString)]
#[strum(serialize_all = "kebab-case", ascii_case_insensitive)]
pub enum Shell {
    Bash,
    Fish,
    Zsh,
    #[strum(serialize = "powershell")]
    #[clap(name = "powershell")]
    PowerShell,
}

impl Shell {
    /// Returns the config file paths for this shell.
    ///
    /// The `cmd` parameter affects the Fish functions filename (e.g., `wt.fish` or `git-wt.fish`).
    /// Returns paths in order of preference. The first existing file should be used.
    pub fn config_paths(&self, cmd: &str) -> Result<Vec<std::path::PathBuf>, std::io::Error> {
        paths::config_paths(*self, cmd)
    }

    /// Returns the legacy fish conf.d path for cleanup purposes.
    ///
    /// Previously, fish shell integration was installed to `~/.config/fish/conf.d/{cmd}.fish`.
    /// This caused issues with Homebrew PATH setup (see issue #566). We now install to
    /// `functions/{cmd}.fish` instead. This method returns the legacy path so install/uninstall
    /// can clean it up.
    pub fn legacy_fish_conf_d_path(cmd: &str) -> Result<std::path::PathBuf, std::io::Error> {
        paths::legacy_fish_conf_d_path(cmd)
    }

    /// Returns the path to the native completion directory for this shell.
    ///
    /// The `cmd` parameter affects the completion filename (e.g., `wt.fish` or `git-wt.fish`).
    ///
    /// Note: Bash and Zsh use inline lazy completions in the init script.
    /// Only Fish uses a separate completion file at ~/.config/fish/completions/
    /// (installed by `wt config shell install`) that uses $WORKTRUNK_BIN to bypass
    /// the shell function wrapper.
    pub fn completion_path(&self, cmd: &str) -> Result<std::path::PathBuf, std::io::Error> {
        paths::completion_path(*self, cmd)
    }

    /// Returns the line to add to the config file for shell integration.
    ///
    /// The `cmd` parameter specifies the command name (e.g., `wt` or `git-wt`).
    /// All shells use a conditional wrapper to avoid errors when the command doesn't exist.
    ///
    /// Note: The generated line does not include `--cmd` because `binary_name()` already
    /// detects the command name from argv\[0\] at runtime.
    pub fn config_line(&self, cmd: &str) -> String {
        match self {
            Self::Bash | Self::Zsh => {
                format!(
                    "if command -v {cmd} >/dev/null 2>&1; then eval \"$(command {cmd} config shell init {})\"; fi",
                    self
                )
            }
            Self::Fish => {
                format!(
                    "if type -q {cmd}; command {cmd} config shell init {} | source; end",
                    self
                )
            }
            Self::PowerShell => {
                format!(
                    "if (Get-Command {cmd} -ErrorAction SilentlyContinue) {{ Invoke-Expression (& {cmd} config shell init powershell) }}",
                )
            }
        }
    }

    /// Check if shell integration is configured for the given command name.
    ///
    /// Returns the path to the first config file with integration if found.
    /// This helps detect the "configured but not restarted shell" state.
    ///
    /// The `cmd` parameter specifies the command name to look for (e.g., "wt" or "git-wt").
    /// This ensures we only consider integration "configured" if it uses the same binary
    /// we're running as - prevents confusion when users have multiple installs.
    pub fn is_integration_configured(
        cmd: &str,
    ) -> Result<Option<std::path::PathBuf>, std::io::Error> {
        let results = scan_for_detection_details(cmd)?;
        Ok(results
            .into_iter()
            .find(|r| !r.matched_lines.is_empty())
            .map(|r| r.path))
    }
}

/// Shell integration configuration
pub struct ShellInit {
    pub shell: Shell,
    pub cmd: String,
}

impl ShellInit {
    pub fn with_prefix(shell: Shell, cmd: String) -> Self {
        Self { shell, cmd }
    }

    /// Generate shell integration code (for `wt config shell init`)
    pub fn generate(&self) -> Result<String, askama::Error> {
        match self.shell {
            Shell::Bash => {
                let template = BashTemplate {
                    shell_name: self.shell.to_string(),
                    cmd: &self.cmd,
                };
                template.render()
            }
            Shell::Zsh => {
                let template = ZshTemplate { cmd: &self.cmd };
                template.render()
            }
            Shell::Fish => {
                let template = FishTemplate { cmd: &self.cmd };
                template.render()
            }
            Shell::PowerShell => {
                let template = PowerShellTemplate { cmd: &self.cmd };
                template.render()
            }
        }
    }

    /// Generate fish wrapper code (for `wt config shell install fish`)
    ///
    /// This generates a minimal wrapper that sources the full function from the binary.
    /// The wrapper file itself is static, but it loads the init output at runtime,
    /// so users get updated behavior without reinstalling.
    pub fn generate_fish_wrapper(&self) -> Result<String, askama::Error> {
        let template = FishWrapperTemplate { cmd: &self.cmd };
        template.render()
    }
}

/// Bash shell template
#[derive(Template)]
#[template(path = "bash.sh", escape = "none")]
struct BashTemplate<'a> {
    shell_name: String,
    cmd: &'a str,
}

/// Zsh shell template
#[derive(Template)]
#[template(path = "zsh.zsh", escape = "none")]
struct ZshTemplate<'a> {
    cmd: &'a str,
}

/// Fish shell template (full function for `wt config shell init fish`)
#[derive(Template)]
#[template(path = "fish.fish", escape = "none")]
struct FishTemplate<'a> {
    cmd: &'a str,
}

/// Fish wrapper template (minimal wrapper for `functions/wt.fish`)
///
/// This wrapper is autoloaded by fish and sources the full function from the binary.
/// Unlike the full FishTemplate, this allows updates to worktrunk to automatically
/// provide the latest wrapper logic without requiring `wt config shell install`.
#[derive(Template)]
#[template(path = "fish_wrapper.fish", escape = "none")]
struct FishWrapperTemplate<'a> {
    cmd: &'a str,
}

/// PowerShell template
#[derive(Template)]
#[template(path = "powershell.ps1", escape = "none")]
struct PowerShellTemplate<'a> {
    cmd: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[test]
    fn test_shell_from_str() {
        assert!(matches!("bash".parse::<Shell>(), Ok(Shell::Bash)));
        assert!(matches!("BASH".parse::<Shell>(), Ok(Shell::Bash)));
        assert!(matches!("fish".parse::<Shell>(), Ok(Shell::Fish)));
        assert!(matches!("zsh".parse::<Shell>(), Ok(Shell::Zsh)));
        assert!(matches!(
            "powershell".parse::<Shell>(),
            Ok(Shell::PowerShell)
        ));
        assert!(matches!(
            "POWERSHELL".parse::<Shell>(),
            Ok(Shell::PowerShell)
        ));
        assert!("invalid".parse::<Shell>().is_err());
    }

    #[test]
    fn test_shell_display() {
        assert_eq!(Shell::Bash.to_string(), "bash");
        assert_eq!(Shell::Fish.to_string(), "fish");
        assert_eq!(Shell::Zsh.to_string(), "zsh");
        assert_eq!(Shell::PowerShell.to_string(), "powershell");
    }

    #[test]
    fn test_shell_config_line() {
        insta::assert_snapshot!("config_line_bash", Shell::Bash.config_line("wt"));
        insta::assert_snapshot!("config_line_zsh", Shell::Zsh.config_line("wt"));
        insta::assert_snapshot!("config_line_fish", Shell::Fish.config_line("wt"));
        insta::assert_snapshot!(
            "config_line_powershell",
            Shell::PowerShell.config_line("wt")
        );
    }

    #[test]
    fn test_config_line_uses_custom_prefix() {
        // When using a custom prefix, the generated shell config line must use that prefix
        // throughout - both in the command check AND the command invocation.
        // This prevents the bug where we check for `git-wt` but then call `wt`.
        insta::assert_snapshot!("config_line_bash_custom", Shell::Bash.config_line("git-wt"));
        insta::assert_snapshot!("config_line_zsh_custom", Shell::Zsh.config_line("git-wt"));
        insta::assert_snapshot!("config_line_fish_custom", Shell::Fish.config_line("git-wt"));
        insta::assert_snapshot!(
            "config_line_powershell_custom",
            Shell::PowerShell.config_line("git-wt")
        );
    }

    #[test]
    fn test_shell_init_generate() {
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::PowerShell] {
            let init = ShellInit::with_prefix(shell, "wt".to_string());
            let output = init.generate().expect("Failed to generate");
            insta::assert_snapshot!(format!("init_{shell}"), output);
        }
    }

    #[test]
    fn test_shell_config_paths_returns_paths() {
        // All shells should return at least one config path
        let shells = [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::PowerShell];
        for shell in shells {
            let result = shell.config_paths("wt");
            assert!(result.is_ok(), "Failed to get config paths for {:?}", shell);
            let paths = result.unwrap();
            assert!(
                !paths.is_empty(),
                "No config paths returned for {:?}",
                shell
            );
        }
    }

    #[test]
    fn test_shell_completion_path_returns_path() {
        // All shells should return a completion path
        let shells = [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::PowerShell];
        for shell in shells {
            let result = shell.completion_path("wt");
            assert!(
                result.is_ok(),
                "Failed to get completion path for {:?}",
                shell
            );
            let path = result.unwrap();
            assert!(
                !path.as_os_str().is_empty(),
                "Empty completion path for {:?}",
                shell
            );
        }
    }

    #[test]
    fn test_shell_config_paths_with_custom_prefix() {
        // Test that custom prefix affects the paths where appropriate
        let prefix = "custom-wt";

        // Fish config path should include prefix in filename
        let fish_paths = Shell::Fish.config_paths(prefix).unwrap();
        assert!(
            fish_paths[0]
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains("custom-wt.fish")),
            "Fish config should include prefix in filename"
        );

        // Bash and Zsh config paths are fixed (not affected by prefix)
        let bash_paths = Shell::Bash.config_paths(prefix).unwrap();
        assert!(
            bash_paths[0]
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains(".bashrc")),
            "Bash config should be .bashrc"
        );

        let zsh_paths = Shell::Zsh.config_paths(prefix).unwrap();
        assert!(
            zsh_paths[0]
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains(".zshrc")),
            "Zsh config should be .zshrc"
        );
    }

    #[test]
    fn test_shell_completion_path_with_custom_prefix() {
        let prefix = "my-prefix";

        // Bash completion should include prefix in path
        let bash_path = Shell::Bash.completion_path(prefix).unwrap();
        assert!(
            bash_path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains("my-prefix")),
            "Bash completion should include prefix"
        );

        // Fish completion should include prefix in filename
        let fish_path = Shell::Fish.completion_path(prefix).unwrap();
        assert!(
            fish_path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains("my-prefix.fish")),
            "Fish completion should include prefix in filename"
        );

        // Zsh completion should include prefix
        let zsh_path = Shell::Zsh.completion_path(prefix).unwrap();
        assert!(
            zsh_path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains("_my-prefix")),
            "Zsh completion should include underscore prefix"
        );
    }

    #[test]
    fn test_shell_init_with_custom_prefix() {
        let init = ShellInit::with_prefix(Shell::Bash, "custom".to_string());
        insta::assert_snapshot!(init.generate().expect("Should generate with custom prefix"));
    }

    /// Verify that `config_line()` generates lines that
    /// `is_shell_integration_line()` can detect.
    ///
    /// This prevents install and detection from drifting out of sync.
    /// Note: .exe variants are not included because `binary_name()` strips
    /// the .exe suffix on Windows (MSYS2/Git Bash handles the resolution).
    #[rstest]
    fn test_config_line_detected_by_is_shell_integration_line(
        #[values(Shell::Bash, Shell::Zsh, Shell::Fish, Shell::PowerShell)] shell: Shell,
        #[values("wt", "git-wt")] prefix: &str,
    ) {
        let line = shell.config_line(prefix);
        assert!(
            is_shell_integration_line(&line, prefix),
            "{shell} config_line({prefix:?}) not detected:\n  {line}"
        );
    }
}
