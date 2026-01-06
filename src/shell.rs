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
/// We detect shell integration by looking for eval/source lines in shell config files.
///
/// Lines like `eval "$(wt config shell init bash)"` in `.bashrc`/`.zshrc`.
///
/// **Challenge:** `wt config shell init` is a substring of `git wt config shell init`.
///
/// **Solution:** Use negative lookbehind to exclude `git ` and `git-` prefixes:
/// - For `wt`: match `wt config shell init` NOT preceded by `git ` or `git-`
/// - For `git-wt`: match `git-wt config shell init` OR `git wt config shell init`
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
/// - `wt config show` - display shell integration status
///
/// # Impact of False Negatives
///
/// Detection is ONLY used when shell integration is NOT active (i.e., user ran
/// the binary directly without the shell wrapper). Once the shell wrapper is
/// active (after shell restart), `WORKTRUNK_DIRECTIVE_FILE` is set and no
/// detection is needed.
///
/// **When binary is run directly (wrapper not active):**
/// - If detection finds integration → "restart the shell to activate"
/// - If detection misses (false negative) → "shell integration not installed"
///
/// **When wrapper is active:** No warnings shown regardless of detection.
///
/// This means false negatives only cause incorrect messaging in `wt config show`
/// and when users run the binary directly before restarting their shell.
pub fn is_shell_integration_line(line: &str, cmd: &str) -> bool {
    let trimmed = line.trim();

    // Skip comments (# for POSIX shells, <# #> for PowerShell)
    if trimmed.starts_with('#') {
        return false;
    }

    // Check for eval/source line pattern
    has_init_invocation(trimmed, cmd)
}

/// Check if line contains `{cmd} config shell init` as a command invocation.
///
/// For `wt`: matches `wt config shell init` but NOT `git wt` or `git-wt`.
/// For `git-wt`: matches `git-wt config shell init` OR `git wt config shell init`.
fn has_init_invocation(line: &str, cmd: &str) -> bool {
    // For git-wt, we need to match both "git-wt config shell init" AND "git wt config shell init"
    // because users invoke it both ways (and git dispatches "git wt" to "git-wt")
    if cmd == "git-wt" {
        // Match either form, with boundary check for "git" in "git wt" form
        return has_init_pattern_with_prefix_check(line, "git-wt")
            || has_init_pattern_with_prefix_check(line, "git wt");
    }

    // For other commands, use normal matching with prefix exclusion
    has_init_pattern_with_prefix_check(line, cmd)
}

/// Check if line has the init pattern, with prefix exclusion for non-git-wt commands.
///
/// Handles Windows `.exe` suffix: searches for both `{cmd} config shell init` and
/// `{cmd}.exe config shell init` to match lines like:
/// ```text
/// eval "$(git-wt.exe config shell init bash)"
/// ```
fn has_init_pattern_with_prefix_check(line: &str, cmd: &str) -> bool {
    // Search for both plain command and .exe variant (Windows Git Bash)
    let patterns = [
        format!("{cmd} config shell init"),
        format!("{cmd}.exe config shell init"),
    ];

    for init_pattern in &patterns {
        // Determine the command portion for position checking
        // For ".exe" pattern, the command in the line includes ".exe"
        let cmd_in_line = if init_pattern.contains(".exe") {
            format!("{cmd}.exe")
        } else {
            cmd.to_string()
        };

        let mut search_start = 0;
        while let Some(pos) = line[search_start..].find(init_pattern.as_str()) {
            let absolute_pos = search_start + pos;

            // Check what precedes the match
            if is_valid_command_position(line, absolute_pos, &cmd_in_line) {
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
    }

    false
}

/// Check if the command at `pos` is a valid standalone command, not part of another command.
///
/// For `wt` at position `pos`:
/// - Valid: start of line, after `$(`, after whitespace, after `command `
/// - Invalid: after `git ` (would be `git wt`), after `git-` (would be `git-wt`)
///
/// For `git-wt`: must not be preceded by alphanumeric, underscore, or hyphen
/// (e.g., `my-git-wt` should NOT match)
fn is_valid_command_position(line: &str, pos: usize, cmd: &str) -> bool {
    if pos == 0 {
        return true; // Start of line
    }

    let before = &line[..pos];

    // For git-wt (and git-wt.exe), just check it's not part of a longer identifier
    // e.g., `my-git-wt` should not match
    if cmd == "git-wt" || cmd == "git-wt.exe" {
        let last_char = before.chars().last().unwrap();
        return !last_char.is_alphanumeric() && last_char != '_' && last_char != '-';
    }

    // For other commands (like `wt`), check for git prefix
    // This handles: `git wt config...` and `git-wt config...`
    if before.ends_with("git ") || before.ends_with("git-") {
        return false;
    }

    // Valid if preceded by: whitespace, $(, (, ", ', or `command `
    let last_char = before.chars().last().unwrap();
    matches!(last_char, ' ' | '\t' | '$' | '(' | '"' | '\'' | '`')
}

/// Check if a line contains the command name at a word boundary.
///
/// Used to identify potential false negatives - lines that contain the command
/// but weren't detected as integration lines.
fn contains_cmd_at_word_boundary(line: &str, cmd: &str) -> bool {
    let mut search_start = 0;
    while let Some(pos) = line[search_start..].find(cmd) {
        let absolute_pos = search_start + pos;

        // Check character before (must be non-identifier or start of string)
        let before_ok = if absolute_pos == 0 {
            true
        } else {
            let prev_char = line[..absolute_pos].chars().last().unwrap();
            !prev_char.is_alphanumeric() && prev_char != '_' && prev_char != '-'
        };

        // Check character after (must be non-identifier or end of string)
        let after_pos = absolute_pos + cmd.len();
        let after_ok = if after_pos >= line.len() {
            true
        } else {
            let next_char = line[after_pos..].chars().next().unwrap();
            !next_char.is_alphanumeric() && next_char != '_' && next_char != '-'
        };

        if before_ok && after_ok {
            return true;
        }

        search_start = absolute_pos + 1;
    }
    false
}

/// A detected line with its 1-based line number.
#[derive(Debug, Clone)]
pub struct DetectedLine {
    pub line_number: usize,
    pub content: String,
}

/// Result of scanning a shell config file for integration detection.
#[derive(Debug, Clone)]
pub struct FileDetectionResult {
    /// Path to the config file that was scanned.
    pub path: PathBuf,
    /// Lines that matched as shell integration (detected).
    pub matched_lines: Vec<DetectedLine>,
    /// Lines containing the command at word boundary but NOT detected.
    /// These are potential false negatives.
    pub unmatched_candidates: Vec<DetectedLine>,
    /// Aliases that bypass shell integration by pointing to a binary path.
    /// e.g., `alias gwt="/usr/bin/wt"` or `alias wt="wt.exe"`
    pub bypass_aliases: Vec<BypassAlias>,
}

/// An alias that bypasses shell integration by pointing to a binary.
#[derive(Debug, Clone)]
pub struct BypassAlias {
    /// Line number in the config file (1-indexed).
    pub line_number: usize,
    /// The alias name (e.g., "gwt").
    pub alias_name: String,
    /// The target the alias points to (e.g., "/usr/bin/wt" or "wt.exe").
    pub target: String,
    /// The full line content.
    pub content: String,
}

/// Scan a single file for shell integration lines and potential false negatives.
fn scan_file(path: &std::path::Path, cmd: &str) -> Option<FileDetectionResult> {
    use std::fs;
    use std::io::{BufRead, BufReader};

    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut matched_lines = Vec::new();
    let mut unmatched_candidates = Vec::new();
    let mut bypass_aliases = Vec::new();

    for (line_number, line) in reader.lines().map_while(Result::ok).enumerate() {
        let line_number = line_number + 1; // 1-based
        let trimmed = line.trim();
        // Skip empty lines and comments
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if is_shell_integration_line(&line, cmd) {
            matched_lines.push(DetectedLine {
                line_number,
                content: line.clone(),
            });
        } else if contains_cmd_at_word_boundary(&line, cmd) {
            unmatched_candidates.push(DetectedLine {
                line_number,
                content: line.clone(),
            });
        }

        // Check for aliases that bypass shell integration
        if let Some(alias) = detect_bypass_alias(trimmed, cmd, line_number) {
            bypass_aliases.push(BypassAlias {
                content: line.clone(),
                ..alias
            });
        }
    }

    // Only return if we found something interesting
    if matched_lines.is_empty() && unmatched_candidates.is_empty() && bypass_aliases.is_empty() {
        return None;
    }

    Some(FileDetectionResult {
        path: path.to_path_buf(),
        matched_lines,
        unmatched_candidates,
        bypass_aliases,
    })
}

/// Detect if a line defines an alias that bypasses shell integration.
///
/// Returns `Some(BypassAlias)` if the line is an alias pointing to a binary path.
/// Binary paths are detected by: containing `/` or `\`, or ending with `.exe`.
///
/// Examples that bypass:
/// - `alias gwt="/usr/bin/wt"` — absolute path
/// - `alias wt="wt.exe"` — Windows binary
/// - `alias gwt='git-wt.exe'` — Windows binary with single quotes
///
/// Examples that don't bypass:
/// - `alias gwt="wt"` — points to function name (OK)
/// - `alias gwt="git-wt"` — points to function name (OK)
fn detect_bypass_alias(line: &str, cmd: &str, line_number: usize) -> Option<BypassAlias> {
    // Match patterns like: alias <name>="<target>" or alias <name>='<target>'
    // Also handle: alias <name>=<target> (no quotes, less common)
    let line = line.trim();

    // Must start with "alias "
    if !line.starts_with("alias ") {
        return None;
    }

    let after_alias = line[6..].trim_start();

    // Find the = sign
    let eq_pos = after_alias.find('=')?;
    let alias_name = after_alias[..eq_pos].trim();
    let target_part = after_alias[eq_pos + 1..].trim();

    // Extract target, handling quotes
    let target = if let Some(stripped) = target_part.strip_prefix('"') {
        // Double-quoted: find closing quote
        let end = stripped.find('"')?;
        &stripped[..end]
    } else if let Some(stripped) = target_part.strip_prefix('\'') {
        // Single-quoted: find closing quote
        let end = stripped.find('\'')?;
        &stripped[..end]
    } else {
        // Unquoted: take until whitespace or end
        target_part.split_whitespace().next()?
    };

    // Check if target looks like a binary path (contains path separators or .exe)
    let target_lower = target.to_ascii_lowercase();
    let is_binary_target =
        target.contains('/') || target.contains('\\') || target_lower.ends_with(".exe");

    if !is_binary_target {
        return None;
    }

    // Check if the target references our command (wt, git-wt, etc.)
    // We check if target contains the cmd name to catch:
    // - /usr/bin/wt
    // - wt.exe
    // - /path/to/git-wt
    // - git-wt.exe
    let target_lower = target.to_ascii_lowercase();
    let cmd_lower = cmd.to_ascii_lowercase();
    if !target_lower.contains(&cmd_lower) {
        return None;
    }

    Some(BypassAlias {
        line_number,
        alias_name: alias_name.to_string(),
        target: target.to_string(),
        content: String::new(), // Filled in by caller
    })
}

/// Scan shell config files for detailed detection results.
///
/// Returns information about:
/// - Which lines matched as shell integration
/// - Which lines contain the command but didn't match (potential false negatives)
///
/// Used by `wt config show` to provide debugging output.
pub fn scan_for_detection_details(cmd: &str) -> Result<Vec<FileDetectionResult>, std::io::Error> {
    use std::collections::HashSet;

    let home = home_dir_required()?;
    let mut results = Vec::new();

    // Collect all config file paths to scan
    // Use HashSet to deduplicate paths (e.g., when ZDOTDIR == $HOME)
    let mut config_files: Vec<PathBuf> = vec![
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
        // Fish conf.d
        home.join(".config/fish/conf.d").join(format!("{cmd}.fish")),
    ];

    // Add PowerShell profiles
    config_files.extend(powershell_profile_paths(&home));

    // Deduplicate and scan
    let mut seen = HashSet::new();
    for path in config_files {
        if !seen.insert(path.clone()) || !path.exists() {
            continue;
        }
        if let Some(result) = scan_file(&path, cmd) {
            results.push(result);
        }
    }

    Ok(results)
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
        // This prompt goes to /dev/tty (not stderr), bypassing our stderr redirect.
        //
        // Worktrunk does NOT cause this warning - our shell init script doesn't modify
        // fpath or call compinit. It only registers completions with `compdef` if the
        // user has already set up compinit themselves. The warning appears because:
        // 1. This probe runs `zsh -ic` which sources global configs like /etc/zsh/zshrc
        // 2. Some environments (notably Ubuntu CI) have global configs that call compinit
        // 3. Those environments may have insecure fpath directories
        //
        // Safe to suppress because we're only probing shell state, not doing anything
        // security-sensitive, and this only affects our subprocess.
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

/// Extract executable name from a path, stripping `.exe` on Windows.
///
/// Uses `std::path::Path` for platform-native path handling:
/// - Unix: `/usr/bin/bash` -> "bash"
/// - Windows: `C:\Program Files\Git\usr\bin\bash.exe` -> "bash"
///
/// Only strips `.exe` extension (not other extensions like `.9` in `zsh-5.9`).
pub fn extract_filename_from_path(path: &str) -> Option<&str> {
    let filename = std::path::Path::new(path).file_name()?.to_str()?;

    // Strip .exe extension (case-insensitive for Windows)
    // Don't use file_stem() because it would strip version numbers like ".9" from "zsh-5.9"
    // Handle all case variants: .exe, .EXE, .Exe, .eXe, etc.
    if filename.len() > 4 && filename[filename.len() - 4..].eq_ignore_ascii_case(".exe") {
        Some(&filename[..filename.len() - 4])
    } else {
        Some(filename)
    }
}

/// Determine Shell variant from a shell name (without path or extension).
///
/// Handles versioned/prefixed binaries like `zsh-5.9` or `bash5`
/// by checking if the name starts with a known shell.
fn shell_from_name(shell_name: &str) -> Option<Shell> {
    // Try exact match first
    if let Ok(shell) = shell_name.parse() {
        return Some(shell);
    }

    // Handle versioned/prefixed binaries (e.g., "zsh-5.9", "bash5")
    // Check if shell name starts with a known shell
    let name_lower = shell_name.to_lowercase();
    if name_lower.starts_with("zsh") {
        Some(Shell::Zsh)
    } else if name_lower.starts_with("bash") {
        Some(Shell::Bash)
    } else if name_lower.starts_with("fish") {
        Some(Shell::Fish)
    } else if name_lower.starts_with("pwsh") || name_lower.starts_with("powershell") {
        Some(Shell::PowerShell)
    } else {
        None
    }
}

/// Get the current shell from `$SHELL` environment variable.
///
/// Returns `None` if `$SHELL` is not set or doesn't match a known shell.
/// Handles versioned/prefixed binaries like `/nix/store/.../zsh-5.9` or `bash5`
/// by checking if the name starts with a known shell.
///
/// Works on both Unix and Windows:
/// - Unix: `/usr/bin/bash` -> Bash
/// - Windows Git Bash: `C:\Program Files\Git\usr\bin\bash.exe` -> Bash
pub fn current_shell() -> Option<Shell> {
    let shell_path = std::env::var("SHELL").ok()?;
    let shell_name = extract_filename_from_path(&shell_path)?;
    shell_from_name(shell_name)
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

    // ==========================================================================
    // Path extraction tests (Issue #348)
    // ==========================================================================

    #[rstest]
    #[case::just_name("bash", Some("bash"))]
    #[case::just_name_exe("bash.exe", Some("bash"))]
    #[case::mixed_case_exe_title("bash.Exe", Some("bash"))]
    #[case::mixed_case_exe_upper("bash.EXE", Some("bash"))]
    #[case::mixed_case_exe_camel("bash.eXe", Some("bash"))]
    #[case::empty("", None)]
    fn test_extract_filename_from_path_common(#[case] path: &str, #[case] expected: Option<&str>) {
        assert_eq!(extract_filename_from_path(path), expected);
    }

    #[cfg(unix)]
    #[rstest]
    #[case::unix_bash("/usr/bin/bash", Some("bash"))]
    #[case::unix_zsh("/bin/zsh", Some("zsh"))]
    #[case::unix_fish("/usr/local/bin/fish", Some("fish"))]
    #[case::nix_versioned("/nix/store/abc123/zsh-5.9", Some("zsh-5.9"))]
    fn test_extract_filename_from_path_unix(#[case] path: &str, #[case] expected: Option<&str>) {
        assert_eq!(extract_filename_from_path(path), expected);
    }

    #[cfg(windows)]
    #[rstest]
    #[case::windows_git_bash(r"C:\Program Files\Git\usr\bin\bash.exe", Some("bash"))]
    #[case::windows_powershell(
        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        Some("powershell")
    )]
    #[case::windows_pwsh(r"C:\Program Files\PowerShell\7\pwsh.exe", Some("pwsh"))]
    #[case::windows_zsh(r"C:\msys64\usr\bin\zsh.exe", Some("zsh"))]
    #[case::uppercase_exe(r"C:\WINDOWS\SYSTEM32\BASH.EXE", Some("BASH"))]
    fn test_extract_filename_from_path_windows(#[case] path: &str, #[case] expected: Option<&str>) {
        assert_eq!(extract_filename_from_path(path), expected);
    }

    /// Issue #348: Windows Git Bash shell detection
    ///
    /// Git Bash sets $SHELL to Windows-style paths like:
    /// `C:\Program Files\Git\usr\bin\bash.exe`
    ///
    /// This test verifies the full path-to-shell detection flow works on Windows.
    #[cfg(windows)]
    #[rstest]
    #[case::git_bash(r"C:\Program Files\Git\usr\bin\bash.exe", Shell::Bash)]
    #[case::msys2_zsh(r"C:\msys64\usr\bin\zsh.exe", Shell::Zsh)]
    #[case::powershell(
        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        Shell::PowerShell
    )]
    #[case::pwsh(r"C:\Program Files\PowerShell\7\pwsh.exe", Shell::PowerShell)]
    fn test_issue_348_windows_shell_detection(#[case] shell_path: &str, #[case] expected: Shell) {
        // This is the exact flow that failed before the fix:
        // 1. extract_filename_from_path() extracts "bash" from Windows path
        // 2. shell_from_name() maps "bash" to Shell::Bash
        let shell_name = extract_filename_from_path(shell_path)
            .expect("should extract filename from Windows path");
        let detected =
            shell_from_name(shell_name).expect("should detect shell from extracted name");
        assert_eq!(detected, expected);
    }

    #[rstest]
    #[case::bash("bash", Some(Shell::Bash))]
    #[case::bash_versioned("bash5", Some(Shell::Bash))]
    #[case::zsh("zsh", Some(Shell::Zsh))]
    #[case::zsh_versioned("zsh-5.9", Some(Shell::Zsh))]
    #[case::fish("fish", Some(Shell::Fish))]
    #[case::powershell("powershell", Some(Shell::PowerShell))]
    #[case::pwsh("pwsh", Some(Shell::PowerShell))]
    #[case::pwsh_preview("pwsh-preview", Some(Shell::PowerShell))]
    #[case::unknown("tcsh", None)]
    #[case::unknown_csh("csh", None)]
    fn test_shell_from_name(#[case] name: &str, #[case] expected: Option<Shell>) {
        assert_eq!(shell_from_name(name), expected);
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

    // ==========================================================================
    // Windows .exe suffix tests (Issue #348)
    // ==========================================================================

    /// Windows Git Bash users may have .exe in their config lines.
    /// Detection should match both `git-wt config shell init` and `git-wt.exe config shell init`.
    #[rstest]
    #[case::wt_exe_basic(r#"eval "$(wt.exe config shell init bash)""#, "wt", true)]
    #[case::wt_exe_with_command(r#"eval "$(command wt.exe config shell init bash)""#, "wt", true)]
    #[case::git_wt_exe_basic(r#"eval "$(git-wt.exe config shell init bash)""#, "git-wt", true)]
    #[case::git_wt_exe_with_command(
        r#"eval "$(command git-wt.exe config shell init bash)""#,
        "git-wt",
        true
    )]
    #[case::git_wt_exe_with_if(
        r#"if command -v git-wt.exe &> /dev/null; then eval "$(command git-wt.exe config shell init bash)"; fi"#,
        "git-wt",
        true
    )]
    // Issue #348: exact pattern from user's dotfiles
    #[case::issue_348_exact(
        r#"eval "$(command git-wt.exe config shell init bash)""#,
        "git-wt",
        true
    )]
    fn test_windows_exe_suffix(#[case] line: &str, #[case] cmd: &str, #[case] should_match: bool) {
        assert_eq!(
            is_shell_integration_line(line, cmd),
            should_match,
            "Windows .exe test failed\nLine: {line}\nCommand: {cmd}\nExpected: {should_match}"
        );
    }

    /// .exe should NOT cause false positives for different commands
    #[rstest]
    #[case::wt_exe_not_git_wt(r#"eval "$(wt.exe config shell init bash)""#, "git-wt", false)]
    #[case::git_wt_exe_not_wt(r#"eval "$(git-wt.exe config shell init bash)""#, "wt", false)]
    // Prefixed command with .exe should not match
    #[case::my_git_wt_exe_not_git_wt(
        r#"eval "$(my-git-wt.exe config shell init bash)""#,
        "git-wt",
        false
    )]
    fn test_windows_exe_no_false_positives(
        #[case] line: &str,
        #[case] cmd: &str,
        #[case] should_match: bool,
    ) {
        assert_eq!(
            is_shell_integration_line(line, cmd),
            should_match,
            "Windows .exe false positive check failed\nLine: {line}\nCommand: {cmd}\nExpected: {should_match}"
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

    // ==========================================================================
    // ADVERSARIAL FALSE NEGATIVE TESTS
    // These test cases attempt to find patterns that SHOULD be detected but ARE NOT
    // ==========================================================================

    /// Helper to test false negatives - if this panics, we found one
    fn assert_detects(line: &str, cmd: &str, description: &str) {
        assert!(
            is_shell_integration_line(line, cmd),
            "FALSE NEGATIVE: {} not detected for cmd={}\nLine: {}",
            description,
            cmd,
            line
        );
    }

    /// Helper to verify non-detection (expected behavior)
    fn assert_not_detects(line: &str, cmd: &str, description: &str) {
        assert!(
            !is_shell_integration_line(line, cmd),
            "UNEXPECTED MATCH: {} matched for cmd={}\nLine: {}",
            description,
            cmd,
            line
        );
    }

    // ------------------------------------------------------------------------
    // FALSE NEGATIVE: dot (.) command as source equivalent
    // ------------------------------------------------------------------------

    /// The `.` command is POSIX-equivalent to `source` but NOT detected
    #[test]
    fn test_fn_dot_command_process_substitution() {
        // . <(wt config shell init bash) is equivalent to source <(...)
        // This is a common POSIX pattern
        assert_not_detects(
            ". <(wt config shell init bash)",
            "wt",
            "CONFIRMED FALSE NEGATIVE: dot command with process substitution",
        );
    }

    #[test]
    fn test_fn_dot_command_zsh_equals() {
        // . =(wt config shell init zsh) is zsh-specific
        assert_not_detects(
            ". =(wt config shell init zsh)",
            "wt",
            "CONFIRMED FALSE NEGATIVE: dot command with zsh =() substitution",
        );
    }

    // ------------------------------------------------------------------------
    // FALSE NEGATIVE: PowerShell iex alias
    // ------------------------------------------------------------------------

    /// iex is PowerShell's alias for Invoke-Expression
    #[test]
    fn test_fn_powershell_iex_alias() {
        // Common in PowerShell profiles
        assert_not_detects(
            "iex (wt config shell init powershell)",
            "wt",
            "CONFIRMED FALSE NEGATIVE: PowerShell iex alias",
        );
    }

    #[test]
    fn test_fn_powershell_iex_with_ampersand() {
        assert_not_detects(
            "iex (& wt config shell init powershell)",
            "wt",
            "CONFIRMED FALSE NEGATIVE: PowerShell iex with &",
        );
    }

    // ------------------------------------------------------------------------
    // FALSE NEGATIVE: PowerShell block comments
    // Note: This is actually a FALSE POSITIVE risk (comments matching)
    // ------------------------------------------------------------------------

    #[test]
    fn test_fn_powershell_block_comment() {
        // PowerShell block comments <# #> should NOT match
        // But current code doesn't skip them
        let line = "<# Invoke-Expression (wt config shell init powershell) #>";
        let result = is_shell_integration_line(line, "wt");
        // This DOES match (false positive) - documenting the behavior
        assert!(
            result,
            "PowerShell block comment currently matches (false positive risk)"
        );
    }

    // ------------------------------------------------------------------------
    // FALSE NEGATIVE: zsh =() process substitution without source/eval
    // ------------------------------------------------------------------------

    /// Zsh allows sourcing with just =() which creates a temp file
    #[test]
    fn test_fn_zsh_bare_equals_substitution() {
        // Some zsh configs might use: . =(command)
        // Already covered above, but this is a variant
        assert_not_detects(
            ". =(command wt config shell init zsh)",
            "wt",
            "dot with command prefix",
        );
    }

    // ------------------------------------------------------------------------
    // EDGE CASE: Backtick command substitution
    // ------------------------------------------------------------------------

    /// Backticks (older syntax) should work - they DO
    #[test]
    fn test_backtick_substitution() {
        assert_detects(
            "eval \"`wt config shell init bash`\"",
            "wt",
            "backtick substitution",
        );
    }

    /// Backticks without quotes
    #[test]
    fn test_backtick_no_outer_quotes() {
        assert_detects(
            "eval `wt config shell init bash`",
            "wt",
            "backtick without outer quotes",
        );
    }

    // ------------------------------------------------------------------------
    // FALSE NEGATIVE: Path prefixes to binary
    // The detection checks for specific preceding characters (' ', '\t', '$', etc.)
    // but '/' is not included, so paths like /usr/local/bin/wt don't match
    // ------------------------------------------------------------------------

    #[test]
    fn test_fn_absolute_path() {
        // Path-prefixed binary invocation - NOT detected because '/' not in allowed chars
        assert_not_detects(
            r#"eval "$(/usr/local/bin/wt config shell init bash)""#,
            "wt",
            "CONFIRMED FALSE NEGATIVE: absolute path to binary",
        );
    }

    #[test]
    fn test_fn_home_path() {
        assert_not_detects(
            r#"eval "$(~/.cargo/bin/wt config shell init bash)""#,
            "wt",
            "CONFIRMED FALSE NEGATIVE: home-relative path",
        );
    }

    #[test]
    fn test_fn_env_var_path() {
        assert_not_detects(
            r#"eval "$($HOME/.cargo/bin/wt config shell init bash)""#,
            "wt",
            "CONFIRMED FALSE NEGATIVE: env var in path",
        );
    }

    // ------------------------------------------------------------------------
    // EDGE CASE: WORKTRUNK_BIN fallback variations
    // ------------------------------------------------------------------------

    #[test]
    fn test_worktrunk_bin_only() {
        // Using only WORKTRUNK_BIN without default
        assert_not_detects(
            r#"eval "$($WORKTRUNK_BIN config shell init bash)""#,
            "wt",
            "WORKTRUNK_BIN without default (expected: no match - cant tell which cmd)",
        );
    }

    // ------------------------------------------------------------------------
    // EDGE CASE: git wt spacing variations
    // ------------------------------------------------------------------------

    #[test]
    fn test_git_wt_double_space() {
        // Extra space between git and wt
        assert_not_detects(
            r#"eval "$(git  wt config shell init bash)""#,
            "git-wt",
            "double space (expected: no match due to pattern)",
        );
    }

    #[test]
    fn test_git_wt_tab_separator() {
        // Tab between git and wt
        let line = "eval \"$(git\twt config shell init bash)\"";
        assert_not_detects(
            line,
            "git-wt",
            "tab separator (expected: no match - only single space matched)",
        );
    }

    // ------------------------------------------------------------------------
    // FALSE NEGATIVE: fish without explicit source/eval keyword
    // The fish pattern wt config shell init fish | source works because "source" is detected
    // ------------------------------------------------------------------------

    #[test]
    fn test_fish_standard() {
        assert_detects(
            "wt config shell init fish | source",
            "wt",
            "standard fish pattern",
        );
    }

    #[test]
    fn test_fish_with_command() {
        assert_detects(
            "command wt config shell init fish | source",
            "wt",
            "fish with command prefix",
        );
    }

    // ------------------------------------------------------------------------
    // FALSE NEGATIVE: Nushell (unsupported but users might try)
    // ------------------------------------------------------------------------

    #[test]
    fn test_nushell_pattern() {
        // Nushell uses "source" so it might match
        let line = "wt config shell init nu | source";
        // This actually matches because it contains "source" and "wt config shell init"
        assert_detects(line, "wt", "nushell pattern (unexpectedly matches)");
    }

    // ------------------------------------------------------------------------
    // Verify comment handling edge cases
    // ------------------------------------------------------------------------

    #[test]
    fn test_inline_comment() {
        // The line starts with actual code, not a comment
        assert_detects(
            r#"eval "$(wt config shell init bash)" # setup wt"#,
            "wt",
            "inline comment after code",
        );
    }

    #[test]
    fn test_commented_in_middle() {
        // Line starts with #
        assert_not_detects(
            r#"#eval "$(wt config shell init bash)""#,
            "wt",
            "line starting with # (expected: no match)",
        );
    }

    // ------------------------------------------------------------------------
    // Multiple commands on one line
    // ------------------------------------------------------------------------

    #[test]
    fn test_multiple_evals() {
        // Both wt and git-wt on same line
        let line =
            r#"eval "$(wt config shell init bash)"; eval "$(git-wt config shell init bash)""#;
        assert_detects(line, "wt", "wt in multi-command line");
        assert_detects(line, "git-wt", "git-wt in multi-command line");
    }

    // ==========================================================================
    // WORD BOUNDARY TESTS - Bugs fixed in adversarial testing rounds 3-4
    // ==========================================================================

    /// Prefixed git-wt commands should NOT match git-wt
    #[rstest]
    #[case::my_git_wt(r#"eval "$(my-git-wt config shell init bash)""#)]
    #[case::test_git_wt(r#"eval "$(test-git-wt config shell init bash)""#)]
    #[case::underscore_git_wt(r#"eval "$(_git-wt config shell init bash)""#)]
    #[case::x_git_wt(r#"eval "$(x-git-wt config shell init bash)""#)]
    fn test_prefixed_git_wt_no_match(#[case] line: &str) {
        assert_not_detects(line, "git-wt", "prefixed git-wt command should NOT match");
    }

    /// Prefixed "git wt" (space form) should NOT match git-wt
    #[rstest]
    #[case::agit_wt(r#"eval "$(agit wt config shell init bash)""#)]
    #[case::xgit_wt(r#"eval "$(xgit wt config shell init bash)""#)]
    #[case::mygit_wt(r#"eval "$(mygit wt config shell init bash)""#)]
    fn test_prefixed_git_space_wt_no_match(#[case] line: &str) {
        assert_not_detects(line, "git-wt", "prefixed 'git wt' should NOT match git-wt");
    }

    // ------------------------------------------------------------------------
    // Summary of confirmed ACCEPTABLE FALSE NEGATIVES:
    // (These are documented limitations, not bugs to fix)
    //
    // 1. `. <(cmd ...)` - POSIX dot command (rare, users can use `source`)
    // 2. `. =(cmd ...)` - zsh =() substitution (rare)
    // 3. `iex (cmd ...)` - PowerShell iex alias (would need to add `iex` check)
    // 4. `/path/to/wt` - path-prefixed binary (would need path parsing)
    // 5. `~/path/to/wt` - home-relative path (would need path parsing)
    // 6. `$HOME/path/wt` - env var path (would need path parsing)
    // 7. Line continuations (`\` or backtick) - architectural limitation
    // 8. Heredoc context (`: <<'EOF'`) - architectural limitation
    //
    // Summary of ACCEPTABLE FALSE POSITIVE risks:
    // 9. PowerShell block comments `<# #>` - rare in shell configs
    // 10. Subshell `(eval ...)` - detected correctly but doesn't persist
    //
    // FIXED in this version (were bugs, now correct):
    // 11. `my-git-wt` no longer matches `git-wt`
    // 12. `agit wt` no longer matches `git wt`
    //
    // By design (not bugs):
    // 13. `git  wt` (double space) - only single space "git wt" is valid
    //
    // IMPACT OF FALSE NEGATIVES:
    // Detection is ONLY used when shell wrapper is NOT active. Once the user
    // restarts their shell, WORKTRUNK_DIRECTIVE_FILE is set and no detection
    // is needed. False negatives only affect:
    // - `wt config show` status display
    // - Warning message before shell restart ("not installed" vs "restart to activate")
    // - `wt config shell uninstall` (lines might not be found)
    // ------------------------------------------------------------------------

    // ==========================================================================
    // ALIAS BYPASS DETECTION TESTS (Issue #348)
    // ==========================================================================

    /// Aliases pointing to binary paths should be detected as bypassing shell integration
    #[rstest]
    #[case::absolute_path(r#"alias gwt="/usr/bin/wt""#, "wt", "gwt", "/usr/bin/wt")]
    #[case::exe_suffix(r#"alias gwt="wt.exe""#, "wt", "gwt", "wt.exe")]
    #[case::exe_with_path(r#"alias gwt="/path/to/wt.exe""#, "wt", "gwt", "/path/to/wt.exe")]
    #[case::single_quotes(r#"alias gwt='/usr/bin/wt'"#, "wt", "gwt", "/usr/bin/wt")]
    #[case::git_wt_exe(r#"alias gwt="git-wt.exe""#, "git-wt", "gwt", "git-wt.exe")]
    #[case::windows_path(
        r#"alias gwt="C:\Program Files\wt\wt.exe""#,
        "wt",
        "gwt",
        r"C:\Program Files\wt\wt.exe"
    )]
    fn test_bypass_alias_detected(
        #[case] line: &str,
        #[case] cmd: &str,
        #[case] expected_name: &str,
        #[case] expected_target: &str,
    ) {
        let result = detect_bypass_alias(line, cmd, 1);
        assert!(
            result.is_some(),
            "Expected bypass alias detection for: {line}"
        );
        let alias = result.unwrap();
        assert_eq!(alias.alias_name, expected_name);
        assert_eq!(alias.target, expected_target);
    }

    /// Aliases pointing to function names (not paths) should NOT be detected as bypassing
    #[rstest]
    #[case::function_name(r#"alias gwt="wt""#, "wt")]
    #[case::git_wt_function(r#"alias gwt="git-wt""#, "git-wt")]
    #[case::other_alias(r#"alias ll="ls -la""#, "wt")]
    #[case::not_an_alias("eval \"$(wt config shell init bash)\"", "wt")]
    #[case::commented_alias(r#"# alias gwt="/usr/bin/wt""#, "wt")]
    fn test_bypass_alias_not_detected(#[case] line: &str, #[case] cmd: &str) {
        // Note: commented lines are skipped in scan_file, but detect_bypass_alias
        // itself doesn't filter comments - that's done by the caller
        let result = detect_bypass_alias(line, cmd, 1);
        // For commented alias, we test the raw function behavior
        if !line.trim().starts_with('#') {
            assert!(
                result.is_none(),
                "Should NOT detect bypass for: {line}, got: {:?}",
                result
            );
        }
    }

    /// Unrelated aliases should not be detected
    #[test]
    fn test_unrelated_alias_not_detected() {
        let result = detect_bypass_alias(r#"alias vim="nvim""#, "wt", 1);
        assert!(result.is_none());
    }
}
