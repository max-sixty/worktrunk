use askama::Template;
use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};
use std::path::PathBuf;

/// Get the user's home directory or return an error
fn home_dir() -> Result<PathBuf, std::io::Error> {
    home::home_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Could not determine home directory",
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
    // Disabled shells (elvish, nushell, oil, powershell, xonsh)
    // Uncomment when ready to support additional shells
    // /// TODO: Elvish support - currently disabled due to test failures
    // #[value(skip)]
    // Elvish,
    Fish,
    // /// TODO: Nushell support - currently disabled due to test failures
    // #[value(skip)]
    // Nushell,
    // /// TODO: Oil support - currently disabled due to test failures
    // #[value(skip)]
    // Oil,
    // /// TODO: PowerShell support - currently disabled due to test failures
    // #[value(skip)]
    // Powershell,
    // /// TODO: Xonsh support - currently disabled due to test failures
    // #[value(skip)]
    // Xonsh,
    Zsh,
}

impl Shell {
    /// Returns the standard config file paths for this shell
    ///
    /// Returns paths in order of preference. The first existing file should be used.
    pub fn config_paths(&self) -> Result<Vec<PathBuf>, std::io::Error> {
        let home = home_dir()?;

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
            } // Disabled shells - uncomment when ready to support
              // Self::Nushell => {
              //     vec![home.join(".config").join("nushell").join("config.nu")]
              // }
              // Self::Powershell => {
              //     if cfg!(target_os = "windows") {
              //         let userprofile = PathBuf::from(
              //             std::env::var("USERPROFILE").unwrap_or_else(|_| ".".to_string()),
              //         );
              //         vec![
              //             userprofile
              //                 .join("Documents")
              //                 .join("PowerShell")
              //                 .join("Microsoft.PowerShell_profile.ps1"),
              //         ]
              //     } else {
              //         vec![
              //             home.join(".config")
              //                 .join("powershell")
              //                 .join("Microsoft.PowerShell_profile.ps1"),
              //         ]
              //     }
              // }
              // Self::Oil => {
              //     vec![home.join(".config").join("oil").join("oshrc")]
              // }
              // Self::Elvish => {
              //     vec![home.join(".config").join("elvish").join("rc.elv")]
              // }
              // Self::Xonsh => {
              //     vec![home.join(".xonshrc")]
              // }
        })
    }

    /// Returns the path to the native completion directory for this shell
    ///
    /// These are the standard locations where shells look for lazy-loaded completions:
    /// - Bash: `~/.local/share/bash-completion/completions/`
    /// - Zsh: `~/.zfunc/` (for manual setup only; install uses lazy compdef)
    /// - Fish: `~/.config/fish/completions/`
    pub fn completion_path(&self) -> Result<PathBuf, std::io::Error> {
        let home = home_dir()?;

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
            } // Disabled shells - uncomment when ready to support
              // Self::Oil => {
              //     format!(
              //         "if command -v {} >/dev/null 2>&1; then eval \"$(command {} init {})\"; fi",
              //         cmd_prefix, cmd_prefix, self
              //     )
              // }
              // Self::Nushell => {
              //     // Use user's home directory cache instead of shared /tmp for security
              //     format!(
              //         "if (which {} | is-not-empty) {{ let tmpfile = ($env.HOME | path join \".cache\" \"nushell-{}-init.nu\"); ^{} init {} | save --force $tmpfile; source $tmpfile }}",
              //         cmd_prefix, cmd_prefix, cmd_prefix, self
              //     )
              // }
              // Self::Powershell => {
              //     format!(
              //         "if (Get-Command {} -ErrorAction SilentlyContinue) {{ Invoke-Expression (& {} init {}) }}",
              //         cmd_prefix, cmd_prefix, self
              //     )
              // }
              // Self::Elvish => {
              //     format!(
              //         "if (has-external {}) {{ eval (e:{} init {}) }}",
              //         cmd_prefix, cmd_prefix, self
              //     )
              // }
              // Self::Xonsh => {
              //     format!(
              //         "import shutil; exec(shutil.which('{}') and $({} init {}).strip() or '')",
              //         cmd_prefix, cmd_prefix, self
              //     )
              // }
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

        let home = home_dir()?;

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
            // Disabled shells
            // // Nushell
            // home.join(".config/nushell/config.nu"),
            // // Elvish
            // home.join(".config/elvish/rc.elv"),
            // // Xonsh
            // home.join(".xonshrc"),
            // // Powershell
            // home.join(".config/powershell/Microsoft.PowerShell_profile.ps1"),
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
                    // Match: eval "$(anything init bash/zsh/etc)"
                    // This catches both wt and custom prefixes
                    if (trimmed.starts_with("eval \"$(") || trimmed.starts_with("eval '$("))
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
                    // Look for function definitions with switch/remove/merge handling
                    if content.contains("function ")
                        && content.contains("switch")
                        && content.contains("__WORKTRUNK_CD__")
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
            } // Disabled shells - uncomment when ready to support
              // Shell::Oil => {
              //     let posix_shim = PosixDirectivesTemplate {
              //         cmd_prefix: &self.cmd_prefix,
              //     }
              //     .render()?;
              //     let template = BashTemplate {
              //         shell_name: self.shell.to_string(),
              //         cmd_prefix: &self.cmd_prefix,
              //         posix_shim: &posix_shim,
              //     };
              //     template.render()
              // }
              // Shell::Nushell => {
              //     let template = NushellTemplate {
              //         cmd_prefix: &self.cmd_prefix,
              //     };
              //     template.render()
              // }
              // Shell::Powershell => {
              //     let template = PowershellTemplate {
              //         cmd_prefix: &self.cmd_prefix,
              //     };
              //     template.render()
              // }
              // Shell::Elvish => {
              //     let template = ElvishTemplate {
              //         cmd_prefix: &self.cmd_prefix,
              //     };
              //     template.render()
              // }
              // Shell::Xonsh => {
              //     let template = XonshTemplate {
              //         cmd_prefix: &self.cmd_prefix,
              //     };
              //     template.render()
              // }
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

// Disabled shell templates - uncomment when ready to support
// /// Nushell shell template
// #[derive(Template)]
// #[template(path = "nushell.nu", escape = "none")]
// struct NushellTemplate<'a> {
//     cmd_prefix: &'a str,
// }
//
// /// PowerShell template
// #[derive(Template)]
// #[template(path = "powershell.ps1", escape = "none")]
// struct PowershellTemplate<'a> {
//     cmd_prefix: &'a str,
// }
//
// /// Elvish shell template
// #[derive(Template)]
// #[template(path = "elvish.elv", escape = "none")]
// struct ElvishTemplate<'a> {
//     cmd_prefix: &'a str,
// }
//
// /// Xonsh shell template
// #[derive(Template)]
// #[template(path = "xonsh.xsh", escape = "none")]
// struct XonshTemplate<'a> {
//     cmd_prefix: &'a str,
// }

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
