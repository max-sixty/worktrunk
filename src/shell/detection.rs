//! Shell integration detection logic.
//!
//! This module detects whether shell integration is configured by scanning
//! shell config files (`.bashrc`, `.zshrc`, etc.) for eval/source lines
//! that invoke `wt config shell init`.

use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use super::paths::{home_dir_required, powershell_profile_paths};

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
/// - `Shell::is_shell_configured()` - detect "configured but not restarted" state
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

    // Skip comments (# for POSIX shells, <# #> for PowerShell block comments)
    if trimmed.starts_with('#') || trimmed.starts_with("<#") {
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
                // Must be in an execution context (eval, source, dot command, PowerShell, etc.)
                if line.contains("eval")
                    || line.contains("source")
                    || line.contains(". <(")  // POSIX dot command with process substitution
                    || line.contains(". =(")  // zsh dot command with =() substitution
                    || line.contains("Invoke-Expression")
                    || line.contains("iex")
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
/// For `git-wt`: must not be preceded by alphanumeric (Unicode-aware), underscore, or hyphen
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

    // Valid if preceded by: whitespace, $(, (, ", ', `, or / (for absolute paths)
    let last_char = before.chars().last().unwrap();
    matches!(last_char, ' ' | '\t' | '$' | '(' | '"' | '\'' | '`' | '/')
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
        // Fish functions/ (current location)
        home.join(".config/fish/functions")
            .join(format!("{cmd}.fish")),
        // Fish conf.d (legacy location - for detecting existing installs)
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

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

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

    /// The `.` command is POSIX-equivalent to `source` - now detected
    #[test]
    fn test_dot_command_process_substitution() {
        // . <(wt config shell init bash) is equivalent to source <(...)
        // This is a common POSIX pattern
        assert_detects(
            ". <(wt config shell init bash)",
            "wt",
            "dot command with process substitution",
        );
    }

    #[test]
    fn test_dot_command_zsh_equals() {
        // . =(wt config shell init zsh) is zsh-specific
        assert_detects(
            ". =(wt config shell init zsh)",
            "wt",
            "dot command with zsh =() substitution",
        );
    }

    // ------------------------------------------------------------------------
    // FALSE NEGATIVE: PowerShell iex alias
    // ------------------------------------------------------------------------

    /// iex is PowerShell's alias for Invoke-Expression - now detected
    #[test]
    fn test_powershell_iex_alias() {
        // Common in PowerShell profiles
        assert_detects(
            "iex (wt config shell init powershell)",
            "wt",
            "PowerShell iex alias",
        );
    }

    #[test]
    fn test_powershell_iex_with_ampersand() {
        assert_detects(
            "iex (& wt config shell init powershell)",
            "wt",
            "PowerShell iex with &",
        );
    }

    // ------------------------------------------------------------------------
    // FALSE NEGATIVE: PowerShell block comments
    // Note: This is actually a FALSE POSITIVE risk (comments matching)
    // ------------------------------------------------------------------------

    #[test]
    fn test_powershell_block_comment() {
        // PowerShell block comments <# #> should NOT match - now correctly skipped
        let line = "<# Invoke-Expression (wt config shell init powershell) #>";
        assert_not_detects(line, "wt", "PowerShell block comment should not match");
    }

    // ------------------------------------------------------------------------
    // FALSE NEGATIVE: zsh =() process substitution without source/eval
    // ------------------------------------------------------------------------

    /// Zsh allows sourcing with just =() which creates a temp file - now detected
    #[test]
    fn test_zsh_bare_equals_substitution() {
        // Some zsh configs might use: . =(command)
        // Already covered above, but this is a variant
        assert_detects(
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
    fn test_absolute_path() {
        // Path-prefixed binary invocation - now detected with '/' in allowed chars
        assert_detects(
            r#"eval "$(/usr/local/bin/wt config shell init bash)""#,
            "wt",
            "absolute path to binary",
        );
    }

    #[test]
    fn test_home_path() {
        assert_detects(
            r#"eval "$(~/.cargo/bin/wt config shell init bash)""#,
            "wt",
            "home-relative path",
        );
    }

    #[test]
    fn test_env_var_path() {
        assert_detects(
            r#"eval "$($HOME/.cargo/bin/wt config shell init bash)""#,
            "wt",
            "env var in path",
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

    /// Unicode alphanumerics before command should NOT match (is_alphanumeric is Unicode-aware)
    #[rstest]
    #[case::greek(r#"eval "$(αgit-wt config shell init bash)""#, "git-wt")]
    #[case::cyrillic(r#"eval "$(яwt config shell init bash)""#, "wt")]
    fn test_unicode_alphanumerics_no_match(#[case] line: &str, #[case] cmd: &str) {
        assert_not_detects(line, cmd, "Unicode alphanumeric before command");
    }

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
