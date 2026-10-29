//! Shell integration detection logic.
//!
//! This module detects whether shell integration is configured by scanning
//! shell config files (`.bashrc`, `.zshrc`, etc.) for eval/source lines
//! that invoke `wt config shell init`.

use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use super::paths::{home_dir_required, powershell_profile_paths, zsh_config_dir};

/// Recognize complete lines written by Worktrunk or shown in manual setup.
///
/// This is a template match, not a shell parser. Customized expressions remain
/// user-managed: install may append its standard line, and uninstall leaves
/// them alone rather than deleting other commands sharing the line.
/// Returns the shell so installation need not parse the matched text again.
pub fn shell_integration(line: &str, cmd: &str) -> Option<super::Shell> {
    let integration = match_integration_line(line)?;
    let name = integration
        .cmd
        .strip_suffix(".exe")
        .unwrap_or(integration.cmd);
    (integration.active
        && (integration.cmd == cmd || name == cmd || (cmd == "git-wt" && name == "git wt")))
        .then_some(integration.shell)
}

/// Uninstall also recognizes old PowerShell lines without `Out-String`, which
/// no longer activate integration, and installations under other binary names.
pub fn is_shell_integration_line_for_uninstall_any_cmd(line: &str) -> bool {
    match_integration_line(line).is_some()
}

struct IntegrationLine<'a> {
    shell: super::Shell,
    cmd: &'a str,
    active: bool,
}

fn match_integration_line(line: &str) -> Option<IntegrationLine<'_>> {
    use super::Shell;

    // None of our templates contains '#'. Accept an appended comment without
    // interpreting quoting or comment rules in arbitrary shell expressions.
    let line = line.trim();
    if !line.contains(" config shell init ") {
        return None;
    }
    let line = line
        .split_once(" #")
        .map_or(line, |(code, _)| code)
        .trim_end();
    for shell in Shell::all() {
        let generated = shell.config_line(COMMAND_PLACEHOLDER);
        let manual = match shell {
            Shell::Bash | Shell::Zsh => {
                format!("eval \"$({COMMAND_PLACEHOLDER} config shell init {shell})\"")
            }
            Shell::Fish => format!("{COMMAND_PLACEHOLDER} config shell init fish | source"),
            Shell::Nushell => format!(
                "{COMMAND_PLACEHOLDER} config shell init nu | save -f ($nu.vendor-autoload-dirs | last | path join {COMMAND_PLACEHOLDER}.nu)"
            ),
            Shell::PowerShell => format!(
                "Invoke-Expression (& {COMMAND_PLACEHOLDER} config shell init powershell | Out-String)"
            ),
        };
        let mut templates = vec![generated];
        if matches!(shell, Shell::Bash | Shell::Zsh | Shell::Fish) {
            templates.push(manual.replace(
                COMMAND_PLACEHOLDER,
                &format!("command {COMMAND_PLACEHOLDER}"),
            ));
        }
        templates.push(manual);
        for template in templates {
            if let Some(cmd) = template_command(line, &template) {
                return Some(IntegrationLine {
                    shell,
                    cmd,
                    active: true,
                });
            }
            // The installer used to pass --cmd explicitly. PowerShell also
            // used to omit Out-String (the broken form repaired in #888).
            let old = if shell == Shell::PowerShell {
                template.replace(" | Out-String", "")
            } else {
                template.clone()
            };
            let with_cmd = old.replace(
                &format!("config shell init {shell}"),
                &format!("config shell init {shell} --cmd={COMMAND_PLACEHOLDER}"),
            );
            for template in [old, with_cmd] {
                if let Some(cmd) = template_command(line, &template) {
                    return Some(IntegrationLine {
                        shell,
                        cmd,
                        active: shell != Shell::PowerShell,
                    });
                }
            }
        }
    }
    None
}

const COMMAND_PLACEHOLDER: &str = "WORKTRUNK_COMMAND";

/// Recover the template's command slot, then compare the entire line including
/// repeated slots. A command-looking substring alone never establishes ownership.
fn template_command<'a>(line: &'a str, template: &str) -> Option<&'a str> {
    let (prefix, rest) = template.split_once(COMMAND_PLACEHOLDER)?;
    let delimiter = rest.split(COMMAND_PLACEHOLDER).next()?;
    let (cmd, _) = line.strip_prefix(prefix)?.split_once(delimiter)?;
    if cmd != "git wt" && super::validate_shell_command_name(cmd).is_err() {
        return None;
    }
    (line == template.replace(COMMAND_PLACEHOLDER, cmd)).then_some(cmd)
}

/// Check if a line contains the command name at a word boundary.
///
/// Used to show custom setup lines that are not recognized as integration.
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
    /// These may be customized integration or unrelated uses of the command.
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

/// Scan a file for recognized setup lines and other references to the command.
fn scan_file(path: &std::path::Path, cmd: &str) -> Option<FileDetectionResult> {
    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut matched_lines = Vec::new();
    let mut unmatched_candidates = Vec::new();
    let mut bypass_aliases = Vec::new();

    // Enumerate *before* filtering so a skipped line doesn't shift the numbers
    // of later lines, and use `filter_map` (not `map_while`) so a single
    // unreadable line — e.g. non-UTF-8 bytes in a hand-edited config — skips
    // only that line instead of truncating the scan at the first error.
    for (line_number, line) in reader
        .lines()
        .enumerate()
        .filter_map(|(index, line)| Some((index, line.ok()?)))
    {
        let line_number = line_number + 1; // 1-based
        let trimmed = line.trim();
        // Skip empty lines and comments
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if shell_integration(&line, cmd).is_some() {
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
    // Extract the filename from the path and check if it matches exactly
    // (with optional .exe suffix). This avoids false positives like
    // "/path/to/newton.bin" matching "wt" as a substring.
    let filename = target.rsplit(['/', '\\']).next().unwrap_or(target);
    let filename_lower = filename.to_ascii_lowercase();
    let cmd_lower = cmd.to_ascii_lowercase();

    // Match: filename is exactly cmd, or cmd.exe
    let matches = filename_lower == cmd_lower || filename_lower == format!("{cmd_lower}.exe");

    if !matches {
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
/// - Which lines contain the command but are not recognized setup lines
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
        zsh_config_dir(&home).join(".zshrc"),
        // Fish functions/ (current location) — fish's own config dir, so
        // detection reads the file install wrote.
        super::paths::fish_config_dir(&home)
            .join("functions")
            .join(format!("{cmd}.fish")),
        // Fish conf.d (legacy location - for detecting existing installs).
        // Pinned to `~/.config`, matching `legacy_fish_conf_d_path`: that is
        // where worktrunk wrote it, and `wt config show` compares the two.
        home.join(".config/fish/conf.d").join(format!("{cmd}.fish")),
    ];

    // Add Nushell vendor autoload paths (check all candidate locations)
    config_files.extend(super::Shell::Nushell.config_paths(cmd).unwrap_or_default());

    // Add PowerShell profiles
    config_files.extend(powershell_profile_paths(&home)?);

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

    #[rstest]
    #[case(r#"eval "$(wt config shell init bash)""#, crate::shell::Shell::Bash)]
    #[case(
        r#"eval "$(command wt config shell init zsh)""#,
        crate::shell::Shell::Zsh
    )]
    #[case(
        r#"  eval "$(wt config shell init zsh)" # worktrunk"#,
        crate::shell::Shell::Zsh
    )]
    #[case("wt config shell init fish | source", crate::shell::Shell::Fish)]
    #[case(
        "wt config shell init nu | save -f ($nu.vendor-autoload-dirs | last | path join wt.nu)",
        crate::shell::Shell::Nushell
    )]
    #[case(
        "Invoke-Expression (& wt config shell init powershell | Out-String)",
        crate::shell::Shell::PowerShell
    )]
    fn documented_lines_match(#[case] line: &str, #[case] shell: crate::shell::Shell) {
        assert_eq!(shell_integration(line, "wt"), Some(shell));
        assert!(is_shell_integration_line_for_uninstall_any_cmd(line));
    }

    #[test]
    fn historical_installations_match() {
        for shell in [
            crate::shell::Shell::Bash,
            crate::shell::Shell::Zsh,
            crate::shell::Shell::Fish,
            crate::shell::Shell::PowerShell,
        ] {
            let line = shell
                .config_line("git-wt")
                .replace(" | Out-String", "")
                .replace(
                    &format!("config shell init {shell}"),
                    &format!("config shell init {shell} --cmd=git-wt"),
                );
            assert!(is_shell_integration_line_for_uninstall_any_cmd(&line));
            assert_eq!(
                shell_integration(&line, "git-wt").is_some(),
                shell != crate::shell::Shell::PowerShell
            );
        }
        let old_powershell = "Invoke-Expression (& wt config shell init powershell)";
        assert!(is_shell_integration_line_for_uninstall_any_cmd(
            old_powershell
        ));
        assert_eq!(shell_integration(old_powershell, "wt"), None);
        let dispatch = r#"eval "$(git wt config shell init zsh)""#;
        assert_eq!(
            shell_integration(dispatch, "git-wt"),
            Some(crate::shell::Shell::Zsh)
        );
        assert_eq!(shell_integration(dispatch, "wt"), None);
        assert!(is_shell_integration_line_for_uninstall_any_cmd(dispatch));
    }

    #[rstest]
    #[case(r#"# eval "$(wt config shell init bash)""#)]
    #[case(r#"alias setup='eval "$(wt config shell init bash)"'"#)]
    #[case(r#"eval "$(wt config shell init bash)"; echo keep-me"#)]
    #[case(r#"eval "$(direnv hook bash)" # wt config shell init bash"#)]
    #[case(r#"echo "wt config shell init fish | source""#)]
    #[case(r#"eval "$($WORKTRUNK_BIN config shell init bash)""#)]
    #[case(r#"eval "$(/opt/bin/wt config shell init bash)""#)]
    #[case(r#"eval "$(wt config shell init bash-extra)""#)]
    #[case(r#"if command -v other >/dev/null 2>&1; then eval "$(command wt config shell init bash)"; fi"#)]
    fn custom_lines_are_not_claimed(#[case] line: &str) {
        assert_eq!(shell_integration(line, "wt"), None);
        assert!(!is_shell_integration_line_for_uninstall_any_cmd(line));
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
    #[case::substring_in_path(r#"alias gravity=/path/to/newton.bin"#, "wt")]
    #[case::substring_in_path_quoted(r#"alias gravity="/path/to/newton.bin""#, "wt")]
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

    /// A non-UTF-8 line earlier in the file must not hide integration lines
    /// below it. `scan_file` reads line-by-line; a `map_while(Result::ok)`
    /// would stop at the first unreadable line, so an accented comment stored
    /// as Latin-1 (or any stray non-UTF-8 byte) above the `eval` line would
    /// make detection report "not installed" for a config that is installed.
    #[test]
    fn test_scan_file_skips_non_utf8_line_before_integration() {
        use std::io::Write;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".bashrc");
        let mut file = fs::File::create(&path).unwrap();
        // 0xE9 is `é` in Latin-1 — an invalid UTF-8 byte on its own.
        file.write_all(b"# comment with a Latin-1 accent: \xE9\n")
            .unwrap();
        file.write_all(b"eval \"$(wt config shell init bash)\"\n")
            .unwrap();
        drop(file);

        let result = scan_file(&path, "wt").expect("integration line should be detected");
        assert_eq!(
            result.matched_lines.len(),
            1,
            "the eval line below the non-UTF-8 line should still be matched"
        );
        // Enumeration happens before filtering, so the surviving line keeps its
        // true 1-based position even though line 1 was skipped.
        assert_eq!(result.matched_lines[0].line_number, 2);
    }
}
