use askama::Template;
use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};
use std::path::PathBuf;

use crate::path::home_dir;

/// Get PowerShell profile paths in order of preference.
/// On Windows, returns both PowerShell Core (7+) and Windows PowerShell (5.1) paths.
/// On Unix, uses the conventional ~/.config/powershell location.
fn powershell_profile_paths(home: &std::path::Path) -> Vec<PathBuf> {
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

/// Get the user's home directory or return an error
fn home_dir_required() -> Result<PathBuf, std::io::Error> {
    home_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "Cannot determine home directory. Set $HOME (Unix) or $USERPROFILE (Windows)",
        )
    })
}

/// Check if a line contains shell integration for the given command.
///
/// Returns true if the line:
/// - Is not a comment (doesn't start with #)
/// - Contains `{cmd} config shell init`
/// - Contains an execution context (eval, source, Invoke-Expression, or conditional)
///
/// Used by both detection (is shell integration configured?) and
/// uninstall (which lines to remove?).
pub fn is_shell_integration_line(line: &str, cmd: &str) -> bool {
    let trimmed = line.trim();

    // Skip comments (# for POSIX shells, <# #> for PowerShell)
    if trimmed.starts_with('#') {
        return false;
    }

    // Must contain the init pattern for this command
    let init_pattern = format!("{cmd} config shell init");
    if !trimmed.contains(&init_pattern) {
        return false;
    }

    // Must be in an execution context
    trimmed.contains("eval")
        || trimmed.contains("source")
        || trimmed.contains("Invoke-Expression")
        || trimmed.contains("if ")
}

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
    /// Returns the standard config file paths for this shell
    ///
    /// Returns paths in order of preference. The first existing file should be used.
    pub fn config_paths(&self) -> Result<Vec<PathBuf>, std::io::Error> {
        self.config_paths_with_prefix("wt")
    }

    /// Returns config paths with custom prefix (affects fish conf.d filename)
    pub fn config_paths_with_prefix(&self, cmd: &str) -> Result<Vec<PathBuf>, std::io::Error> {
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
                // Filename includes prefix to avoid conflicts (e.g., wt.fish, git-wt.fish)
                vec![
                    home.join(".config")
                        .join("fish")
                        .join("conf.d")
                        .join(format!("{}.fish", cmd)),
                ]
            }
            Self::PowerShell => powershell_profile_paths(&home),
        })
    }

    /// Returns the path to the native completion directory for this shell
    ///
    /// Note: Bash and Zsh use inline lazy completions in the init script.
    /// Only Fish uses a separate completion file at ~/.config/fish/completions/wt.fish
    /// (installed by `wt config shell install`) that uses $WORKTRUNK_BIN to bypass
    /// the shell function wrapper.
    pub fn completion_path(&self) -> Result<PathBuf, std::io::Error> {
        self.completion_path_with_prefix("wt")
    }

    /// Returns completion path with custom prefix (affects fish completion filename)
    pub fn completion_path_with_prefix(&self, cmd: &str) -> Result<PathBuf, std::io::Error> {
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
                    .join(cmd)
            }
            Self::Zsh => home.join(".zfunc").join(format!("_{}", cmd)),
            Self::Fish => {
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
            Self::PowerShell => {
                // PowerShell doesn't use a separate completion file - completions are
                // registered inline in the profile using Register-ArgumentCompleter
                // Return a dummy path that won't be used
                home.join(format!(".{}-powershell-completions", cmd))
            }
        })
    }

    /// Returns the line to add to the config file for shell integration
    ///
    /// All shells use a conditional wrapper to avoid errors when the command doesn't exist.
    pub fn config_line(&self) -> String {
        self.config_line_with_prefix("wt")
    }

    /// Returns the line to add to the config file for shell integration with custom prefix
    pub fn config_line_with_prefix(&self, cmd: &str) -> String {
        // For non-default prefixes, include --cmd in the init command
        let prefix_arg = if cmd == "wt" {
            String::new()
        } else {
            format!(" --cmd={}", cmd)
        };

        match self {
            Self::Bash | Self::Zsh => {
                format!(
                    "if command -v {cmd} >/dev/null 2>&1; then eval \"$(command {cmd} config shell init {}{prefix_arg})\"; fi",
                    self
                )
            }
            Self::Fish => {
                format!(
                    "if type -q {cmd}; command {cmd} config shell init {}{prefix_arg} | source; end",
                    self
                )
            }
            Self::PowerShell => {
                format!(
                    "if (Get-Command {cmd} -ErrorAction SilentlyContinue) {{ Invoke-Expression (& {cmd} config shell init powershell{prefix_arg}) }}",
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
    pub fn is_integration_configured(cmd: &str) -> Result<Option<PathBuf>, std::io::Error> {
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

        for path in config_files {
            if !path.exists() {
                continue;
            }

            if let Ok(file) = fs::File::open(&path) {
                let reader = BufReader::new(file);
                for line in reader.lines().map_while(Result::ok) {
                    if is_shell_integration_line(&line, cmd) {
                        return Ok(Some(path));
                    }
                }
            }
        }

        // Check Fish conf.d directory - look for {cmd}.fish file specifically
        let fish_conf_d = home.join(".config/fish/conf.d");
        let fish_config = fish_conf_d.join(format!("{cmd}.fish"));
        if fish_config.exists()
            && let Ok(file) = fs::File::open(&fish_config)
        {
            let reader = BufReader::new(file);
            for line in reader.lines().map_while(Result::ok) {
                if is_shell_integration_line(&line, cmd) {
                    return Ok(Some(fish_config));
                }
            }
        }

        // Check PowerShell profiles for integration (both Core and 5.1)
        for profile_path in powershell_profile_paths(&home) {
            if !profile_path.exists() {
                continue;
            }

            if let Ok(file) = fs::File::open(&profile_path) {
                let reader = BufReader::new(file);
                for line in reader.lines().map_while(Result::ok) {
                    if is_shell_integration_line(&line, cmd) {
                        return Ok(Some(profile_path));
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

    /// Returns a summary with custom prefix for display in confirmation
    pub fn integration_summary_with_prefix(&self, cmd: &str) -> String {
        self.config_line_with_prefix(cmd)
    }
}

/// Shell integration configuration
pub struct ShellInit {
    pub shell: Shell,
    pub cmd: String,
}

impl ShellInit {
    pub fn new(shell: Shell) -> Self {
        Self::with_prefix(shell, "wt".to_string())
    }

    pub fn with_prefix(shell: Shell, cmd: String) -> Self {
        Self { shell, cmd }
    }

    /// Generate shell integration code
    pub fn generate(&self) -> Result<String, askama::Error> {
        match self.shell {
            Shell::Bash => {
                let posix_shim = PosixDirectivesTemplate { cmd: &self.cmd }.render()?;
                let template = BashTemplate {
                    shell_name: self.shell.to_string(),
                    cmd: &self.cmd,
                    posix_shim: &posix_shim,
                };
                template.render()
            }
            Shell::Zsh => {
                let posix_shim = PosixDirectivesTemplate { cmd: &self.cmd }.render()?;
                let template = ZshTemplate {
                    cmd: &self.cmd,
                    posix_shim: &posix_shim,
                };
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
}

/// Custom Askama filters for shell templates
mod filters {
    /// Convert command name to a safe shell function identifier (hyphens → underscores)
    /// Used in templates as: {{ cmd|safe_fn }}
    pub fn safe_fn(cmd: &str, _: &dyn askama::Values) -> askama::Result<String> {
        Ok(cmd.replace('-', "_"))
    }
}

/// POSIX directive shim template (shared by bash, zsh, oil)
#[derive(Template)]
#[template(path = "posix_directives.sh", escape = "none")]
struct PosixDirectivesTemplate<'a> {
    cmd: &'a str,
}

/// Bash shell template
#[derive(Template)]
#[template(path = "bash.sh", escape = "none")]
struct BashTemplate<'a> {
    shell_name: String,
    cmd: &'a str,
    posix_shim: &'a str,
}

/// Zsh shell template
#[derive(Template)]
#[template(path = "zsh.zsh", escape = "none")]
struct ZshTemplate<'a> {
    cmd: &'a str,
    posix_shim: &'a str,
}

/// Fish shell template
#[derive(Template)]
#[template(path = "fish.fish", escape = "none")]
struct FishTemplate<'a> {
    cmd: &'a str,
}

/// PowerShell template
#[derive(Template)]
#[template(path = "powershell.ps1", escape = "none")]
struct PowerShellTemplate<'a> {
    cmd: &'a str,
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
    if std::env::var("WORKTRUNK_TEST_COMPINIT_CONFIGURED").is_ok() {
        return Some(true); // Assume compinit is configured
    }

    // Force compinit to be missing (for tests that expect the warning)
    if std::env::var("WORKTRUNK_TEST_COMPINIT_MISSING").is_ok() {
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
        // Prevent subprocesses from writing to the directive file
        .env_remove(crate::shell_exec::DIRECTIVE_FILE_ENV_VAR)
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
    fn test_shell_config_line_bash() {
        let line = Shell::Bash.config_line();
        assert!(line.contains("eval"));
        assert!(line.contains("wt config shell init bash"));
        assert!(line.contains("command -v wt"));
    }

    #[test]
    fn test_shell_config_line_zsh() {
        let line = Shell::Zsh.config_line();
        assert!(line.contains("eval"));
        assert!(line.contains("wt config shell init zsh"));
    }

    #[test]
    fn test_shell_config_line_fish() {
        let line = Shell::Fish.config_line();
        assert!(line.contains("type -q wt"));
        assert!(line.contains("wt config shell init fish"));
        assert!(line.contains("source"));
    }

    #[test]
    fn test_shell_config_line_powershell() {
        let line = Shell::PowerShell.config_line();
        assert!(line.contains("Invoke-Expression"));
        assert!(line.contains("wt config shell init powershell"));
    }

    #[test]
    fn test_config_line_uses_custom_prefix() {
        // When using a custom prefix, the generated shell config line must use that prefix
        // throughout - both in the command check AND the command invocation.
        // This prevents the bug where we check for `git-wt` but then call `wt`.
        let prefix = "git-wt";

        // Bash/Zsh
        let bash_line = Shell::Bash.config_line_with_prefix(prefix);
        assert!(
            bash_line.contains("command -v git-wt"),
            "bash should check for git-wt"
        );
        assert!(
            bash_line.contains("command git-wt config shell init"),
            "bash should call git-wt, not wt"
        );

        // Fish
        let fish_line = Shell::Fish.config_line_with_prefix(prefix);
        assert!(
            fish_line.contains("type -q git-wt"),
            "fish should check for git-wt"
        );
        assert!(
            fish_line.contains("command git-wt config shell init"),
            "fish should call git-wt, not wt"
        );

        // PowerShell
        let ps_line = Shell::PowerShell.config_line_with_prefix(prefix);
        assert!(
            ps_line.contains("Get-Command git-wt"),
            "powershell should check for git-wt"
        );
        assert!(
            ps_line.contains("& git-wt config shell init"),
            "powershell should call git-wt, not wt"
        );
    }

    #[test]
    fn test_shell_init_generate() {
        // Test that shell init generates valid output for each shell
        let shells = [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::PowerShell];
        for shell in shells {
            let init = ShellInit::new(shell);
            let result = init.generate();
            assert!(result.is_ok(), "Failed to generate for {:?}", shell);
            let output = result.unwrap();
            assert!(!output.is_empty(), "Empty output for {:?}", shell);
        }
    }

    #[test]
    fn test_shell_config_paths_returns_paths() {
        // All shells should return at least one config path
        let shells = [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::PowerShell];
        for shell in shells {
            let result = shell.config_paths();
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
            let result = shell.completion_path();
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
        let fish_paths = Shell::Fish.config_paths_with_prefix(prefix).unwrap();
        assert!(
            fish_paths[0].to_string_lossy().contains("custom-wt.fish"),
            "Fish config should include prefix in filename"
        );

        // Bash and Zsh config paths are fixed (not affected by prefix)
        let bash_paths = Shell::Bash.config_paths_with_prefix(prefix).unwrap();
        assert!(
            bash_paths[0].to_string_lossy().contains(".bashrc"),
            "Bash config should be .bashrc"
        );

        let zsh_paths = Shell::Zsh.config_paths_with_prefix(prefix).unwrap();
        assert!(
            zsh_paths[0].to_string_lossy().contains(".zshrc"),
            "Zsh config should be .zshrc"
        );
    }

    #[test]
    fn test_shell_completion_path_with_custom_prefix() {
        let prefix = "my-prefix";

        // Bash completion should include prefix in path
        let bash_path = Shell::Bash.completion_path_with_prefix(prefix).unwrap();
        assert!(
            bash_path.to_string_lossy().contains("my-prefix"),
            "Bash completion should include prefix"
        );

        // Fish completion should include prefix in filename
        let fish_path = Shell::Fish.completion_path_with_prefix(prefix).unwrap();
        assert!(
            fish_path.to_string_lossy().contains("my-prefix.fish"),
            "Fish completion should include prefix in filename"
        );

        // Zsh completion should include prefix
        let zsh_path = Shell::Zsh.completion_path_with_prefix(prefix).unwrap();
        assert!(
            zsh_path.to_string_lossy().contains("_my-prefix"),
            "Zsh completion should include underscore prefix"
        );
    }

    #[test]
    fn test_config_line_prefix_arg_handling() {
        // Default prefix should not include --cmd
        let bash_default = Shell::Bash.config_line_with_prefix("wt");
        assert!(
            !bash_default.contains("--cmd"),
            "Default prefix should not include --cmd"
        );

        // Custom prefix should include --cmd
        let bash_custom = Shell::Bash.config_line_with_prefix("custom");
        assert!(
            bash_custom.contains("--cmd=custom"),
            "Custom prefix should include --cmd=custom"
        );
    }

    #[test]
    fn test_shell_init_with_custom_prefix() {
        let init = ShellInit::with_prefix(Shell::Bash, "custom".to_string());
        let result = init.generate();
        assert!(result.is_ok(), "Should generate with custom prefix");
        let output = result.unwrap();
        assert!(
            output.contains("custom"),
            "Output should contain custom prefix"
        );
    }

    #[test]
    fn test_shell_init_unique_function_names() {
        // Verify that commands with hyphens get safe function names
        // This prevents conflicts when multiple commands are loaded (e.g., wt and git-wt)
        let init = ShellInit::with_prefix(Shell::Bash, "git-wt".to_string());
        let output = init.generate().unwrap();

        // Should use _git_wt_exec (underscores), not _git-wt_exec (hyphens)
        assert!(
            output.contains("_git_wt_exec"),
            "Function name should use underscores: {output}"
        );
        assert!(
            !output.contains("git-wt_exec"),
            "Function name should not contain hyphens"
        );
    }

    /// Verify that `config_line_with_prefix()` generates lines that
    /// `is_shell_integration_line()` can detect.
    ///
    /// This prevents install and detection from drifting out of sync.
    #[rstest]
    fn test_config_line_detected_by_is_shell_integration_line(
        #[values(Shell::Bash, Shell::Zsh, Shell::Fish, Shell::PowerShell)] shell: Shell,
        #[values("wt", "git-wt")] prefix: &str,
    ) {
        let line = shell.config_line_with_prefix(prefix);
        assert!(
            is_shell_integration_line(&line, prefix),
            "{shell} config_line_with_prefix({prefix:?}) not detected:\n  {line}"
        );
    }
}
