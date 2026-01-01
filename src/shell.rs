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

/// Detect if a line contains shell integration for a specific command.
///
/// # Detection Goal
///
/// We need to answer: "Is shell integration configured for THIS binary?"
///
/// When running as `wt`, we should detect `wt` integration but NOT `git-wt` integration
/// (and vice versa). This prevents misleading "restart shell to activate" messages when
/// the user has integration for a different command name.
///
/// # Command Name Patterns
///
/// Users invoke worktrunk in several ways, each creating different command names:
///
/// | Invocation              | Binary name | Function created |
/// |-------------------------|-------------|------------------|
/// | `wt`                    | `wt`        | `wt()`           |
/// | `git wt` (subcommand)   | `git-wt`    | `git-wt()`       |
/// | `git-wt` (direct)       | `git-wt`    | `git-wt()`       |
///
/// Note: `git wt` dispatches to the `git-wt` binary, so both create the same function.
///
/// # Detection Strategy
///
/// We detect shell integration by looking for TWO types of patterns:
///
/// ## 1. Eval/source lines (user's shell config)
///
/// Lines like `eval "$(wt config shell init bash)"` in `.bashrc`/`.zshrc`.
///
/// **Challenge:** `wt config shell init` is a substring of `git wt config shell init`.
///
/// **Solution:** Use negative lookbehind to exclude `git ` and `git-` prefixes:
/// - For `wt`: match `wt config shell init` NOT preceded by `git ` or `git-`
/// - For `git-wt`: match `git-wt config shell init` OR `git wt config shell init`
///
/// ## 2. Generated function markers (sourced into shell)
///
/// The generated shell code contains unique patterns like `_wt_lazy_complete` and
/// `${WORKTRUNK_BIN:-wt}`. These are detected in Fish's `conf.d/{cmd}.fish` files
/// where we install the integration directly (not via eval).
///
/// # Pattern Details
///
/// **Eval line patterns** (for `wt`):
/// ```text
/// eval "$(wt config shell init bash)"           ✓ matches
/// eval "$(command wt config shell init bash)"   ✓ matches
/// eval "$(git wt config shell init bash)"       ✗ no match (git- prefix)
/// eval "$(git-wt config shell init bash)"       ✗ no match (git- prefix)
/// source <(wt config shell init zsh)            ✓ matches
/// ```
///
/// **Generated function markers** (for `wt`):
/// ```text
/// wt() {                                        ✓ matches (function definition)
/// _wt_lazy_complete()                           ✓ matches (completion helper)
/// ${WORKTRUNK_BIN:-wt}                          ✓ matches (fallback pattern)
/// git-wt() {                                    ✗ no match
/// _git-wt_lazy_complete()                       ✗ no match
/// ```
///
/// # Edge Cases Handled
///
/// - Quoted command names: `eval "$('wt' config shell init bash)"` - rare but matched
/// - Comment lines: `# eval "$(wt config shell init bash)"` - skipped
/// - Partial matches: `newt config shell init` - not matched (word boundary)
///
/// # Usage
///
/// Used by:
/// - `is_integration_configured()` - detect "configured but not restarted" state
/// - `uninstall` - identify lines to remove from shell config
pub fn is_shell_integration_line(line: &str, cmd: &str) -> bool {
    let trimmed = line.trim();

    // Skip comments (# for POSIX shells, <# #> for PowerShell)
    if trimmed.starts_with('#') {
        return false;
    }

    // Check for eval/source line pattern
    if has_init_invocation(trimmed, cmd) {
        return true;
    }

    // Check for generated function markers (installed integration files)
    if has_function_marker(trimmed, cmd) {
        return true;
    }

    false
}

/// Check if line contains `{cmd} config shell init` as a command invocation.
///
/// For `wt`: matches `wt config shell init` but NOT `git wt` or `git-wt`.
/// For `git-wt`: matches `git-wt config shell init` OR `git wt config shell init`.
fn has_init_invocation(line: &str, cmd: &str) -> bool {
    // For git-wt, we need to match both "git-wt config shell init" AND "git wt config shell init"
    // because users invoke it both ways (and git dispatches "git wt" to "git-wt")
    if cmd == "git-wt" {
        // Match either form
        return has_init_pattern(line, "git-wt") || has_init_pattern(line, "git wt");
    }

    // For other commands, use normal matching with prefix exclusion
    has_init_pattern_with_prefix_check(line, cmd)
}

/// Check if line has the init pattern for a specific command string (no prefix checks).
fn has_init_pattern(line: &str, cmd: &str) -> bool {
    let init_pattern = format!("{cmd} config shell init");
    if !line.contains(&init_pattern) {
        return false;
    }

    // Must be in an execution context
    line.contains("eval")
        || line.contains("source")
        || line.contains("Invoke-Expression")
        || line.contains("if ")
}

/// Check if line has the init pattern, with prefix exclusion for non-git-wt commands.
fn has_init_pattern_with_prefix_check(line: &str, cmd: &str) -> bool {
    let init_pattern = format!("{cmd} config shell init");

    // Find all occurrences of the pattern
    let mut search_start = 0;
    while let Some(pos) = line[search_start..].find(&init_pattern) {
        let absolute_pos = search_start + pos;

        // Check what precedes the match
        if is_valid_command_position(line, absolute_pos, cmd) {
            // Must be in an execution context
            if line.contains("eval")
                || line.contains("source")
                || line.contains("Invoke-Expression")
                || line.contains("if ")
            {
                return true;
            }
        }

        // Continue searching after this match
        search_start = absolute_pos + 1;
    }

    false
}

/// Check if the command at `pos` is a valid standalone command, not part of another command.
///
/// For `wt` at position `pos`:
/// - Valid: start of line, after `$(`, after whitespace, after `command `
/// - Invalid: after `git ` (would be `git wt`), after `git-` (would be `git-wt`)
///
/// For `git-wt`: always valid (it's the full command name)
fn is_valid_command_position(line: &str, pos: usize, cmd: &str) -> bool {
    // git-wt is unambiguous - it's the full command name
    if cmd == "git-wt" {
        return true;
    }

    // For other commands (like `wt`), check for git prefix
    if pos == 0 {
        return true; // Start of line
    }

    let before = &line[..pos];

    // Check for `git ` or `git-` immediately before the command
    // This handles: `git wt config...` and `git-wt config...`
    if before.ends_with("git ") || before.ends_with("git-") {
        return false;
    }

    // Valid if preceded by: whitespace, $(, (, ", ', or `command `
    let last_char = before.chars().last().unwrap();
    matches!(last_char, ' ' | '\t' | '$' | '(' | '"' | '\'' | '`')
}

/// Check if line contains markers from generated shell integration code.
///
/// These patterns appear in the shell code itself (e.g., Fish's conf.d files),
/// not in the eval line. They're unique to each command.
fn has_function_marker(line: &str, cmd: &str) -> bool {
    // Function definition patterns need word boundary checks to avoid:
    // - "git-wt()" matching when looking for "wt()"
    // - "newt()" matching when looking for "wt()"

    // Bash/Zsh: `wt() {` or `wt () {`
    if has_function_def_bash(line, cmd) {
        return true;
    }

    // Fish: `function wt` (must be at word boundary)
    if has_function_def_fish(line, cmd) {
        return true;
    }

    // Completion helper function: `_wt_lazy_complete`
    // This is unique enough that simple contains is fine
    let completion_helper = format!("_{cmd}_lazy_complete");
    if line.contains(&completion_helper) {
        return true;
    }

    // Fallback pattern: `WORKTRUNK_BIN:-wt}` (unique per command)
    let fallback = format!("WORKTRUNK_BIN:-{cmd}}}");
    if line.contains(&fallback) {
        return true;
    }

    false
}

/// Check for bash/zsh function definition: `cmd()` or `cmd ()`
/// Must have word boundary before the command name.
fn has_function_def_bash(line: &str, cmd: &str) -> bool {
    let func_def = format!("{cmd}()");
    let func_def_space = format!("{cmd} ()");

    for pattern in [&func_def, &func_def_space] {
        if let Some(pos) = line.find(pattern) {
            // Check for word boundary before the command
            if pos == 0
                || !line[..pos].ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == '-')
            {
                // Must be a function definition (has `{` on same line)
                if line.contains('{') {
                    return true;
                }
            }
        }
    }
    false
}

/// Check for fish function definition: `function cmd`
/// Must be followed by end-of-line or whitespace (not more identifier chars).
fn has_function_def_fish(line: &str, cmd: &str) -> bool {
    let func_keyword = format!("function {cmd}");
    if let Some(pos) = line.find(&func_keyword) {
        let after_pos = pos + func_keyword.len();
        // Check what follows: must be end of line, whitespace, or newline
        if after_pos >= line.len() {
            return true; // End of line
        }
        let next_char = line[after_pos..].chars().next().unwrap();
        if next_char.is_whitespace() {
            return true;
        }
    }
    false
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
    /// Returns the config file paths for this shell.
    ///
    /// The `cmd` parameter affects the Fish conf.d filename (e.g., `wt.fish` or `git-wt.fish`).
    /// Returns paths in order of preference. The first existing file should be used.
    pub fn config_paths(&self, cmd: &str) -> Result<Vec<PathBuf>, std::io::Error> {
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

    /// Returns the path to the native completion directory for this shell.
    ///
    /// The `cmd` parameter affects the completion filename (e.g., `wt.fish` or `git-wt.fish`).
    ///
    /// Note: Bash and Zsh use inline lazy completions in the init script.
    /// Only Fish uses a separate completion file at ~/.config/fish/completions/
    /// (installed by `wt config shell install`) that uses $WORKTRUNK_BIN to bypass
    /// the shell function wrapper.
    pub fn completion_path(&self, cmd: &str) -> Result<PathBuf, std::io::Error> {
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

    /// Generate shell integration code
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
        .stdin(Stdio::null()) // Prevent compinit from prompting interactively
        .stdout(Stdio::piped())
        .stderr(Stdio::null()) // Suppress user's zsh startup messages
        // Suppress zsh's "insecure directories" warning from compinit.
        //
        // When fpath contains directories with insecure permissions, compinit prompts:
        //   "zsh compinit: insecure directories, run compaudit for list."
        //   "Ignore insecure directories and continue [y] or abort compinit [n]?"
        //
        // This prompt goes to the TTY (not stderr), so it leaks through even with
        // stderr redirected. It's safe to suppress because:
        // 1. Users with this issue already see the warning every time they open a terminal
        // 2. We're only probing zsh's state, not doing anything security-sensitive
        // 3. This only affects our subprocess, not the user's actual shell
        .env("ZSH_DISABLE_COMPFIX", "true")
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
            fish_paths[0].to_string_lossy().contains("custom-wt.fish"),
            "Fish config should include prefix in filename"
        );

        // Bash and Zsh config paths are fixed (not affected by prefix)
        let bash_paths = Shell::Bash.config_paths(prefix).unwrap();
        assert!(
            bash_paths[0].to_string_lossy().contains(".bashrc"),
            "Bash config should be .bashrc"
        );

        let zsh_paths = Shell::Zsh.config_paths(prefix).unwrap();
        assert!(
            zsh_paths[0].to_string_lossy().contains(".zshrc"),
            "Zsh config should be .zshrc"
        );
    }

    #[test]
    fn test_shell_completion_path_with_custom_prefix() {
        let prefix = "my-prefix";

        // Bash completion should include prefix in path
        let bash_path = Shell::Bash.completion_path(prefix).unwrap();
        assert!(
            bash_path.to_string_lossy().contains("my-prefix"),
            "Bash completion should include prefix"
        );

        // Fish completion should include prefix in filename
        let fish_path = Shell::Fish.completion_path(prefix).unwrap();
        assert!(
            fish_path.to_string_lossy().contains("my-prefix.fish"),
            "Fish completion should include prefix in filename"
        );

        // Zsh completion should include prefix
        let zsh_path = Shell::Zsh.completion_path(prefix).unwrap();
        assert!(
            zsh_path.to_string_lossy().contains("_my-prefix"),
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

    // ==========================================================================
    // Detection tests: eval/source lines
    // ==========================================================================

    /// Basic eval patterns that SHOULD match for `wt`
    #[rstest]
    #[case::basic_eval(r#"eval "$(wt config shell init bash)""#)]
    #[case::with_command(r#"eval "$(command wt config shell init bash)""#)]
    #[case::source_process_sub(r#"source <(wt config shell init zsh)"#)]
    #[case::fish_source(r#"wt config shell init fish | source"#)]
    #[case::with_if_check(
        r#"if command -v wt >/dev/null; then eval "$(wt config shell init bash)"; fi"#
    )]
    #[case::single_quotes(r#"eval '$( wt config shell init bash )'"#)]
    fn test_wt_eval_patterns_match(#[case] line: &str) {
        assert!(
            is_shell_integration_line(line, "wt"),
            "Should match for 'wt': {line}"
        );
    }

    /// Patterns that should NOT match for `wt` (they're for git-wt)
    #[rstest]
    #[case::git_space_wt(r#"eval "$(git wt config shell init bash)""#)]
    #[case::git_hyphen_wt(r#"eval "$(git-wt config shell init bash)""#)]
    #[case::command_git_wt(r#"eval "$(command git wt config shell init bash)""#)]
    #[case::command_git_hyphen_wt(r#"eval "$(command git-wt config shell init bash)""#)]
    fn test_git_wt_patterns_dont_match_wt(#[case] line: &str) {
        assert!(
            !is_shell_integration_line(line, "wt"),
            "Should NOT match for 'wt' (this is git-wt integration): {line}"
        );
    }

    /// Patterns that SHOULD match for `git-wt`
    #[rstest]
    #[case::git_hyphen_wt(r#"eval "$(git-wt config shell init bash)""#)]
    #[case::git_space_wt(r#"eval "$(git wt config shell init bash)""#)]
    #[case::command_git_wt(r#"eval "$(command git wt config shell init bash)""#)]
    fn test_git_wt_eval_patterns_match(#[case] line: &str) {
        assert!(
            is_shell_integration_line(line, "git-wt"),
            "Should match for 'git-wt': {line}"
        );
    }

    /// Comment lines should never match
    #[rstest]
    #[case::bash_comment(r#"# eval "$(wt config shell init bash)""#)]
    #[case::indented_comment(r#"  # eval "$(wt config shell init bash)""#)]
    fn test_comments_dont_match(#[case] line: &str) {
        assert!(
            !is_shell_integration_line(line, "wt"),
            "Comment should not match: {line}"
        );
    }

    /// Lines without execution context should not match
    #[rstest]
    #[case::just_command("wt config shell init bash")]
    #[case::echo(r#"echo "wt config shell init bash""#)]
    fn test_no_execution_context_doesnt_match(#[case] line: &str) {
        assert!(
            !is_shell_integration_line(line, "wt"),
            "Without eval/source should not match: {line}"
        );
    }

    // ==========================================================================
    // Detection tests: function markers (for installed integration files)
    // ==========================================================================

    /// Function definition patterns that SHOULD match
    #[rstest]
    #[case::bash_func_def("wt() {")]
    #[case::bash_func_def_space("wt () {")]
    #[case::fish_func_def("function wt")]
    #[case::completion_helper("_wt_lazy_complete() {")]
    #[case::fallback_pattern(r#"command "${WORKTRUNK_BIN:-wt}" "$@""#)]
    fn test_function_markers_match(#[case] line: &str) {
        assert!(
            is_shell_integration_line(line, "wt"),
            "Function marker should match for 'wt': {line}"
        );
    }

    /// Function markers for git-wt should NOT match wt
    #[rstest]
    #[case::git_wt_func("git-wt() {")]
    #[case::git_wt_completion("_git-wt_lazy_complete() {")]
    #[case::git_wt_fallback(r#"command "${WORKTRUNK_BIN:-git-wt}" "$@""#)]
    fn test_git_wt_markers_dont_match_wt(#[case] line: &str) {
        assert!(
            !is_shell_integration_line(line, "wt"),
            "git-wt marker should NOT match 'wt': {line}"
        );
    }

    /// Function markers for git-wt SHOULD match git-wt
    #[rstest]
    #[case::git_wt_func("git-wt() {")]
    #[case::git_wt_completion("_git-wt_lazy_complete() {")]
    #[case::git_wt_fallback(r#"command "${WORKTRUNK_BIN:-git-wt}" "$@""#)]
    fn test_git_wt_markers_match_git_wt(#[case] line: &str) {
        assert!(
            is_shell_integration_line(line, "git-wt"),
            "git-wt marker should match 'git-wt': {line}"
        );
    }

    // ==========================================================================
    // Edge cases and real-world patterns
    // ==========================================================================

    /// Real-world patterns from user dotfiles
    #[rstest]
    #[case::chezmoi_style(
        r#"if command -v wt &>/dev/null; then eval "$(wt config shell init bash)"; fi"#,
        "wt",
        true
    )]
    #[case::nikiforov_style(r#"eval "$(command git wt config shell init bash)""#, "git-wt", true)]
    #[case::nikiforov_not_wt(r#"eval "$(command git wt config shell init bash)""#, "wt", false)]
    fn test_real_world_patterns(#[case] line: &str, #[case] cmd: &str, #[case] should_match: bool) {
        assert_eq!(
            is_shell_integration_line(line, cmd),
            should_match,
            "Line: {line}\nCommand: {cmd}\nExpected: {should_match}"
        );
    }

    /// Word boundary: `newt` should not match `wt`
    #[test]
    fn test_word_boundary_newt() {
        let line = r#"eval "$(newt config shell init bash)""#;
        // This line contains "wt config shell init" as a substring
        // but the command is "newt", not "wt"
        assert!(
            !is_shell_integration_line(line, "wt"),
            "newt should not match wt"
        );
    }

    /// Partial command names should not match
    #[test]
    fn test_partial_command_no_match() {
        // "swt" contains "wt" but is not "wt"
        let line = r#"eval "$(swt config shell init bash)""#;
        assert!(
            !is_shell_integration_line(line, "wt"),
            "swt should not match wt"
        );
    }
}
