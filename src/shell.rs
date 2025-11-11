use askama::Template;
use std::path::PathBuf;

const POSIX_DIRECTIVE_SHIM: &str = include_str!("../templates/posix_directives.sh");

/// Supported shells
#[derive(Debug, Clone, Copy, clap::ValueEnum, strum::Display, strum::EnumString)]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
pub enum Shell {
    Bash,
    Elvish,
    Fish,
    Nushell,
    Oil,
    Powershell,
    Xonsh,
    Zsh,
}

impl Shell {
    /// Returns true if this shell supports completion generation
    pub fn supports_completion(&self) -> bool {
        matches!(self, Self::Bash | Self::Fish | Self::Zsh | Self::Oil)
    }

    /// Returns the standard config file paths for this shell
    ///
    /// Returns paths in order of preference. The first existing file should be used.
    /// For Fish, the cmd_prefix is used to name the conf.d file.
    pub fn config_paths(&self, cmd_prefix: &str) -> Vec<PathBuf> {
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()));

        match self {
            Self::Bash => {
                // macOS uses .bash_profile, Linux typically uses .bashrc
                if cfg!(target_os = "macos") {
                    vec![home.join(".bash_profile"), home.join(".profile")]
                } else {
                    vec![home.join(".bashrc"), home.join(".bash_profile")]
                }
            }
            Self::Zsh => {
                let zdotdir = std::env::var("ZDOTDIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| home.clone());
                vec![zdotdir.join(".zshrc")]
            }
            Self::Fish => {
                // For fish, we write to conf.d/ which is auto-sourced
                // Use cmd_prefix in the filename
                vec![
                    home.join(".config")
                        .join("fish")
                        .join("conf.d")
                        .join(format!("{}.fish", cmd_prefix)),
                ]
            }
            Self::Nushell => {
                vec![home.join(".config").join("nushell").join("config.nu")]
            }
            Self::Powershell => {
                if cfg!(target_os = "windows") {
                    let userprofile = PathBuf::from(
                        std::env::var("USERPROFILE").unwrap_or_else(|_| ".".to_string()),
                    );
                    vec![
                        userprofile
                            .join("Documents")
                            .join("PowerShell")
                            .join("Microsoft.PowerShell_profile.ps1"),
                    ]
                } else {
                    vec![
                        home.join(".config")
                            .join("powershell")
                            .join("Microsoft.PowerShell_profile.ps1"),
                    ]
                }
            }
            Self::Oil => {
                vec![home.join(".config").join("oil").join("oshrc")]
            }
            Self::Elvish => {
                vec![home.join(".config").join("elvish").join("rc.elv")]
            }
            Self::Xonsh => {
                vec![home.join(".xonshrc")]
            }
        }
    }

    /// Returns the line to add to the config file for shell integration
    ///
    /// All shells use a conditional wrapper to avoid errors when the command doesn't exist.
    pub fn config_line(&self, cmd_prefix: &str) -> String {
        match self {
            Self::Bash | Self::Zsh | Self::Oil => {
                format!(
                    "if command -v {} >/dev/null 2>&1; then eval \"$(command {} init {})\"; fi",
                    cmd_prefix, cmd_prefix, self
                )
            }
            Self::Fish => {
                format!(
                    "if type -q {}; command {} init {} | source; end",
                    cmd_prefix, cmd_prefix, self
                )
            }
            Self::Nushell => {
                // Use user's home directory cache instead of shared /tmp for security
                format!(
                    "if (which {} | is-not-empty) {{ let tmpfile = ($env.HOME | path join \".cache\" \"nushell-{}-init.nu\"); ^{} init {} | save --force $tmpfile; source $tmpfile }}",
                    cmd_prefix, cmd_prefix, cmd_prefix, self
                )
            }
            Self::Powershell => {
                format!(
                    "if (Get-Command {} -ErrorAction SilentlyContinue) {{ Invoke-Expression (& {} init {}) }}",
                    cmd_prefix, cmd_prefix, self
                )
            }
            Self::Elvish => {
                format!(
                    "if (has-external {}) {{ eval (e:{} init {}) }}",
                    cmd_prefix, cmd_prefix, self
                )
            }
            Self::Xonsh => {
                format!(
                    "import shutil; exec(shutil.which('{}') and $({} init {}).strip() or '')",
                    cmd_prefix, cmd_prefix, self
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
    pub fn is_integration_configured() -> Option<PathBuf> {
        use std::fs;
        use std::io::{BufRead, BufReader};

        let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()));

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
            // Nushell
            home.join(".config/nushell/config.nu"),
            // Elvish
            home.join(".config/elvish/rc.elv"),
            // Xonsh
            home.join(".xonshrc"),
            // Powershell
            home.join(".config/powershell/Microsoft.PowerShell_profile.ps1"),
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
                        return Some(path);
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
                        return Some(path);
                    }
                }
            }
        }

        None
    }

    /// Returns a summary of what the shell integration does for display in confirmation
    ///
    /// This just returns the same as config_line since we want to show the exact wrapper
    pub fn integration_summary(&self, cmd_prefix: &str) -> String {
        self.config_line(cmd_prefix)
    }
}

/// Generate dynamic completion registrations for Fish shell by introspecting CLI structure
///
/// This generates completion registrations for all subcommands with positional arguments.
/// The actual filtering of what completions to show is handled by the `wt complete` command,
/// which checks the command context and only returns relevant completions.
///
/// This approach ensures that when new commands are added with positional arguments,
/// they automatically get registered for completion - you only need to update the
/// completion logic in `src/commands/completion.rs` to define what completions to show.
fn generate_fish_dynamic_completions(cli_cmd: &clap::Command, cmd_prefix: &str) -> String {
    fn generate_for_subcommands(
        cmd: &clap::Command,
        cmd_prefix: &str,
        parent_chain: &[&str],
    ) -> Vec<String> {
        let mut completions = Vec::new();

        for subcmd in cmd.get_subcommands() {
            let name = subcmd.get_name();
            let current_chain: Vec<&str> = parent_chain.iter().copied().chain([name]).collect();

            // Check if this subcommand has positional arguments
            let has_positional = subcmd.get_positionals().next().is_some();

            if has_positional {
                // Generate condition based on command chain
                let condition = if current_chain.len() == 1 {
                    // Top-level: just check for the subcommand
                    format!("__fish_seen_subcommand_from {}", name)
                } else {
                    // Nested: check for parent chain in sequence
                    // e.g., for "dev run-hook": "__fish_seen_subcommand_from dev; and __fish_seen_subcommand_from run-hook"
                    current_chain
                        .iter()
                        .map(|part| format!("__fish_seen_subcommand_from {}", part))
                        .collect::<Vec<_>>()
                        .join("; and ")
                };

                // Get description from the first positional argument's help text
                let desc = subcmd
                    .get_positionals()
                    .next()
                    .and_then(|arg| arg.get_help())
                    .map(|h| h.to_string())
                    .unwrap_or_else(|| format!("{} argument", current_chain.join(" ")));

                completions.push(format!(
                    "    complete -c {} -n '{}' -f -a '(__{}_complete)' -d '{}'",
                    cmd_prefix, condition, cmd_prefix, desc
                ));
            }

            // Recurse into nested subcommands
            completions.extend(generate_for_subcommands(subcmd, cmd_prefix, &current_chain));
        }

        completions
    }

    generate_for_subcommands(cli_cmd, cmd_prefix, &[]).join("\n")
}

/// Shell integration configuration
pub struct ShellInit {
    pub shell: Shell,
    pub cmd_prefix: String,
}

impl ShellInit {
    pub fn new(shell: Shell, cmd_prefix: String) -> Self {
        Self { shell, cmd_prefix }
    }

    /// Generate shell integration code
    pub fn generate(&self, cli_cmd: &clap::Command) -> Result<String, askama::Error> {
        match self.shell {
            Shell::Bash | Shell::Oil => {
                let template = BashTemplate {
                    shell_name: self.shell.to_string(),
                    cmd_prefix: &self.cmd_prefix,
                    posix_shim: POSIX_DIRECTIVE_SHIM,
                };
                template.render()
            }
            Shell::Zsh => {
                let template = ZshTemplate {
                    cmd_prefix: &self.cmd_prefix,
                    posix_shim: POSIX_DIRECTIVE_SHIM,
                };
                template.render()
            }
            Shell::Fish => {
                let dynamic_completions =
                    generate_fish_dynamic_completions(cli_cmd, &self.cmd_prefix);
                let template = FishTemplate {
                    cmd_prefix: &self.cmd_prefix,
                    dynamic_completions,
                };
                template.render()
            }
            Shell::Nushell => {
                let template = NushellTemplate {
                    cmd_prefix: &self.cmd_prefix,
                };
                template.render()
            }
            Shell::Powershell => {
                let template = PowershellTemplate {
                    cmd_prefix: &self.cmd_prefix,
                };
                template.render()
            }
            Shell::Elvish => {
                let template = ElvishTemplate {
                    cmd_prefix: &self.cmd_prefix,
                };
                template.render()
            }
            Shell::Xonsh => {
                let template = XonshTemplate {
                    cmd_prefix: &self.cmd_prefix,
                };
                template.render()
            }
        }
    }
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
    dynamic_completions: String,
}

/// Nushell shell template
#[derive(Template)]
#[template(path = "nushell.nu", escape = "none")]
struct NushellTemplate<'a> {
    cmd_prefix: &'a str,
}

/// PowerShell template
#[derive(Template)]
#[template(path = "powershell.ps1", escape = "none")]
struct PowershellTemplate<'a> {
    cmd_prefix: &'a str,
}

/// Elvish shell template
#[derive(Template)]
#[template(path = "elvish.elv", escape = "none")]
struct ElvishTemplate<'a> {
    cmd_prefix: &'a str,
}

/// Xonsh shell template
#[derive(Template)]
#[template(path = "xonsh.xsh", escape = "none")]
struct XonshTemplate<'a> {
    cmd_prefix: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Command;

    #[test]
    fn test_shell_from_str() {
        assert!(matches!("bash".parse::<Shell>(), Ok(Shell::Bash)));
        assert!(matches!("BASH".parse::<Shell>(), Ok(Shell::Bash)));
        assert!(matches!("fish".parse::<Shell>(), Ok(Shell::Fish)));
        assert!(matches!("zsh".parse::<Shell>(), Ok(Shell::Zsh)));
        assert!("invalid".parse::<Shell>().is_err());
    }

    #[test]
    fn test_fish_dynamic_completions_for_nested_subcommands() {
        // Test that we generate completions for nested subcommands like "dev run-hook"
        let cli = Command::new("test")
            .subcommand(Command::new("parent").subcommand(
                Command::new("nested").arg(clap::Arg::new("hook_type").help("Hook to run")),
            ))
            .subcommand(Command::new("top-level").arg(clap::Arg::new("arg").help("Some arg")));

        let completions = generate_fish_dynamic_completions(&cli, "test");

        // Should include completion for nested command (parent nested)
        assert!(
            completions.contains(
                "__fish_seen_subcommand_from parent; and __fish_seen_subcommand_from nested"
            ),
            "Should generate nested completion for parent nested, got: {}",
            completions
        );

        // Should include completion for top-level command
        assert!(
            completions.contains("__fish_seen_subcommand_from top-level"),
            "Should generate top-level completion, got: {}",
            completions
        );
    }

    #[test]
    fn test_fish_dynamic_completions_deeply_nested() {
        // Test 3-level nesting to verify recursion handles arbitrary depth
        let cli = Command::new("test").subcommand(
            Command::new("a").subcommand(
                Command::new("b")
                    .subcommand(Command::new("c").arg(clap::Arg::new("arg").help("deep arg"))),
            ),
        );

        let completions = generate_fish_dynamic_completions(&cli, "test");

        assert!(
            completions.contains("__fish_seen_subcommand_from a; and __fish_seen_subcommand_from b; and __fish_seen_subcommand_from c"),
            "Should handle 3+ level nesting, got: {}",
            completions
        );
    }
}
