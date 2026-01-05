//! Consolidated styling module for terminal output.
//!
//! This module uses the anstyle ecosystem:
//! - anstream for auto-detecting color support
//! - anstyle for composable styling
//! - Semantic style constants for domain-specific use
//!
//! ## stdout vs stderr principle
//!
//! - **stdout**: Primary data output (table data, JSON, statusline)
//! - **stderr**: Status messages (progress, success, errors, hints, warnings)
//!
//! This separation allows piping (`wt list | grep foo`) without status messages interfering.
//! Use `output::stdout()` for primary output, `output::print()` for status messages.

mod constants;
mod format;
mod highlighting;
mod hyperlink;
mod line;
mod suggest;

// Re-exports from anstream (auto-detecting output)
pub use anstream::{eprint, eprintln, print, println, stderr, stdout};

// Re-exports from anstyle (for composition)
pub use anstyle::Style as AnstyleStyle;

// Re-export our public types
pub use constants::*;
#[cfg(all(test, feature = "syntax-highlighting"))]
pub(crate) use format::format_bash_with_gutter_at_width;
pub use format::{GUTTER_OVERHEAD, format_bash_with_gutter, format_with_gutter, wrap_styled_text};
pub use highlighting::format_toml;
pub use hyperlink::{Stream, hyperlink_stdout, supports_hyperlinks};
pub use line::{StyledLine, StyledString, truncate_visible};
pub use suggest::suggest_command;

/// Default terminal width fallback if detection fails
const DEFAULT_TERMINAL_WIDTH: usize = 80;

/// Get terminal width, defaulting to 80 if detection fails
///
/// Prefers direct terminal size detection over COLUMNS environment variable,
/// because tools like cargo may set COLUMNS incorrectly.
///
/// Checks stderr first (for status messages), then stdout (for table output).
pub fn get_terminal_width() -> usize {
    // Prefer direct terminal detection (more accurate than COLUMNS which may be stale/wrong)
    // Check stderr first (status messages), then stdout (table output)
    if let Some((terminal_size::Width(w), _)) =
        terminal_size::terminal_size_of(std::io::stderr()).or_else(terminal_size::terminal_size)
    {
        return w as usize;
    }

    // Fall back to COLUMNS env var (for scripts, piped contexts, or when detection fails)
    if let Ok(cols) = std::env::var("COLUMNS")
        && let Ok(width) = cols.parse::<usize>()
    {
        return width;
    }

    DEFAULT_TERMINAL_WIDTH
}

/// Calculate visual width of a string, ignoring ANSI escape codes
///
/// Uses unicode-width for proper handling of wide characters (CJK, emoji).
pub fn visual_width(s: &str) -> usize {
    use ansi_str::AnsiStr;
    use unicode_width::UnicodeWidthStr;
    s.ansi_strip().width()
}

/// Fix dim rendering for terminals that don't handle \e[2m after \e[39m.
///
/// Claude Code's terminal doesn't render dim (\e[2m) correctly when it follows
/// a foreground color reset (\e[39m). This function replaces that sequence with
/// a full reset (\e[0m) before dim, which works correctly.
pub fn fix_dim_after_color_reset(s: &str) -> String {
    s.replace("\x1b[39m\x1b[2m", "\x1b[0m\x1b[2m")
}

// Re-export for tests
#[cfg(test)]
use format::wrap_text_at_width;
// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use anstyle::Style;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn test_toml_formatting() {
        let toml_content = r#"worktree-path = "../{{ repo }}.{{ branch }}"

[llm]
args = []

# This is a comment
[[approved-commands]]
project = "github.com/user/repo"
command = "npm install"
"#;

        let output = format_toml(toml_content);

        // Check that output contains ANSI escape codes
        assert!(
            output.contains("\x1b["),
            "Output should contain ANSI escape codes"
        );

        // Check that strings are highlighted (green = 32)
        assert!(
            output.contains("\x1b[32m"),
            "Should contain green color for strings"
        );

        // Check that comments are dimmed (dim = 2)
        assert!(
            output.contains("\x1b[2m"),
            "Should contain dim style for comments"
        );

        // Check that table headers are highlighted (cyan = 36, bold = 1)
        assert!(
            output.contains("\x1b[36m") || output.contains("\x1b[1m"),
            "Should contain cyan or bold for tables"
        );

        // Check that gutter is present (BrightWhite background = 107)
        assert!(
            output.contains("\x1b[107m"),
            "Should contain gutter (BrightWhite background)"
        );

        // Check that lines have content (not just gutter)
        assert!(
            output.lines().any(|line| line.len() > 20),
            "Should have lines with actual content beyond gutter and indent"
        );
    }

    // StyledString tests
    #[test]
    fn test_styled_string_width() {
        // ASCII strings
        let s = StyledString::raw("hello");
        assert_eq!(s.width(), 5);

        // Unicode arrows
        let s = StyledString::raw("↑3 ↓2");
        assert_eq!(
            s.width(),
            5,
            "↑3 ↓2 should have width 5, not {}",
            s.text.len()
        );

        // Mixed Unicode
        let s = StyledString::raw("日本語");
        assert_eq!(s.width(), 6); // CJK characters are typically width 2

        // Emoji
        let s = StyledString::raw("🎉");
        assert_eq!(s.width(), 2); // Emoji are typically width 2
    }

    // StyledLine tests
    #[test]
    fn test_styled_line_width() {
        let mut line = StyledLine::new();
        line.push_raw("Branch");
        line.push_raw("  ");
        line.push_raw("↑3 ↓2");

        // "Branch" (6) + "  " (2) + "↑3 ↓2" (5) = 13
        assert_eq!(line.width(), 13, "Line width should be 13");
    }

    #[test]
    fn test_styled_line_padding() {
        let mut line = StyledLine::new();
        line.push_raw("test");
        assert_eq!(line.width(), 4);

        line.pad_to(10);
        assert_eq!(line.width(), 10, "After padding to 10, width should be 10");

        // Padding when already at target should not change width
        line.pad_to(10);
        assert_eq!(line.width(), 10, "Padding again should not change width");
    }

    #[test]
    fn test_sparse_column_padding() {
        // Build simplified lines to test sparse column padding
        let mut line1 = StyledLine::new();
        line1.push_raw(format!("{:8}", "branch-a"));
        line1.push_raw("  ");
        // Has ahead/behind
        line1.push_raw(format!("{:5}", "↑3 ↓2"));
        line1.push_raw("  ");

        let mut line2 = StyledLine::new();
        line2.push_raw(format!("{:8}", "branch-b"));
        line2.push_raw("  ");
        // No ahead/behind, should pad with spaces
        line2.push_raw(" ".repeat(5));
        line2.push_raw("  ");

        // Both lines should have same width up to this point
        assert_eq!(
            line1.width(),
            line2.width(),
            "Rows with and without sparse column data should have same width"
        );
    }

    // Word-wrapping tests
    #[test]
    fn test_wrap_text_no_wrapping_needed() {
        let result = super::wrap_text_at_width("short line", 50);
        assert_eq!(result, vec!["short line"]);
    }

    #[test]
    fn test_wrap_text_at_word_boundary() {
        let text = "This is a very long line that needs to be wrapped at word boundaries";
        let result = super::wrap_text_at_width(text, 30);

        // Should wrap at word boundaries
        assert!(result.len() > 1, "Should wrap into multiple lines");

        // Each line should be within the width limit (or be a single long word)
        for line in &result {
            assert!(
                line.width() <= 30 || !line.contains(' '),
                "Line '{}' has width {} which exceeds 30 and contains spaces",
                line,
                line.width()
            );
        }

        // Joining should recover most of the original text (whitespace may differ)
        let rejoined = result.join(" ");
        assert_eq!(
            rejoined.split_whitespace().collect::<Vec<_>>(),
            text.split_whitespace().collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_wrap_text_single_long_word() {
        // A single word longer than max_width should still be included
        let result = super::wrap_text_at_width("verylongwordthatcannotbewrapped", 10);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "verylongwordthatcannotbewrapped");
    }

    #[test]
    fn test_wrap_text_empty_input() {
        let result = super::wrap_text_at_width("", 50);
        assert_eq!(result, vec![""]);
    }

    #[test]
    fn test_wrap_text_unicode() {
        // Unicode characters should be handled correctly by width
        let text = "This line has emoji 🎉 and should wrap correctly when needed";
        let result = super::wrap_text_at_width(text, 30);

        // Should wrap
        assert!(result.len() > 1);

        // Should preserve the emoji
        let rejoined = result.join(" ");
        assert!(rejoined.contains("🎉"));
    }

    #[test]
    fn test_format_with_gutter_wrapping() {
        // Create a very long line that would overflow a narrow terminal
        let long_text = "This is a very long commit message that would normally overflow the terminal width and break the gutter formatting, but now it should wrap nicely at word boundaries.";

        // Use fixed width for consistent testing (80 columns)
        let result = format_with_gutter(long_text, Some(80));

        // Should contain multiple lines (wrapped)
        let line_count = result.lines().count();
        assert!(
            line_count > 1,
            "Long text should wrap to multiple lines, got {} lines",
            line_count
        );

        // Each line should have the gutter
        for line in result.lines() {
            assert!(
                line.contains("\x1b[107m"),
                "Each line should contain gutter (BrightWhite background)"
            );
        }
    }

    #[test]
    fn test_format_with_gutter_preserves_newlines() {
        let multi_line = "Line 1\nLine 2\nLine 3";
        let result = format_with_gutter(multi_line, None);

        // Should have at least 3 lines (one for each input line)
        assert!(result.lines().count() >= 3);

        // Each original line should be present
        assert!(result.contains("Line 1"));
        assert!(result.contains("Line 2"));
        assert!(result.contains("Line 3"));
    }

    #[test]
    fn test_format_with_gutter_long_paragraph() {
        // Realistic commit message scenario - a long unbroken paragraph
        let commit_msg = "This commit refactors the authentication system to use a more secure token-based approach instead of the previous session-based system which had several security vulnerabilities that were identified during the security audit last month. The new implementation follows industry best practices and includes proper token rotation and expiration handling.";

        // Use fixed width for consistent testing (80 columns)
        let result = format_with_gutter(commit_msg, Some(80));

        insta::assert_snapshot!(result, @"
        [107m [0m This commit refactors the authentication system to use a more secure
        [107m [0m token-based approach instead of the previous session-based system which had
        [107m [0m several security vulnerabilities that were identified during the security
        [107m [0m audit last month. The new implementation follows industry best practices and
        [107m [0m includes proper token rotation and expiration handling.
        ");
    }

    #[test]
    fn test_bash_gutter_formatting_ends_with_reset() {
        // Test that bash gutter formatting properly resets colors at the end of each line
        // to prevent color bleeding into subsequent output (like child process output)
        let command = "pre-commit run --all-files";
        let result = format_bash_with_gutter(command);

        // The output should end with ANSI reset code (no trailing newline - caller adds it)
        // ANSI reset is \x1b[0m (ESC[0m)
        assert!(
            result.ends_with("\x1b[0m"),
            "Bash gutter formatting should end with ANSI reset code, got: {:?}",
            result.chars().rev().take(20).collect::<String>()
        );
        assert!(
            !result.ends_with('\n'),
            "Bash gutter formatting should not have trailing newline"
        );

        // Verify the reset appears at the end of EVERY line (for multi-line commands)
        let multi_line_command = "npm install && \\\n    npm run build";
        let multi_result = format_bash_with_gutter(multi_line_command);

        // Each line should end with reset code
        for line in multi_result.lines() {
            if !line.is_empty() {
                // Check that line contains a reset code somewhere
                // (The actual position depends on the highlighting, but it should be present)
                assert!(
                    line.contains("\x1b[0m"),
                    "Each line should contain ANSI reset code, line: {:?}",
                    line
                );
            }
        }

        // Most importantly: the final output should end with reset (no trailing newline)
        assert!(
            multi_result.ends_with("\x1b[0m"),
            "Multi-line bash gutter formatting should end with ANSI reset"
        );
    }

    #[test]
    fn test_reset_code_behavior() {
        // IMPORTANT: {:#} on Style::new() produces an EMPTY STRING, not a reset!
        // This is the root cause of color bleeding bugs.
        let style_reset = format!("{:#}", Style::new());
        assert_eq!(
            style_reset, "",
            "Style::new() with {{:#}} produces empty string (this is why we had color leaking!)"
        );

        // The correct way to get a reset code is anstyle::Reset
        let anstyle_reset = format!("{}", anstyle::Reset);
        assert_eq!(
            anstyle_reset, "\x1b[0m",
            "anstyle::Reset produces proper ESC[0m reset code"
        );

        // Document the fix: always use anstyle::Reset, never {:#} on Style::new()
        assert_ne!(
            style_reset, anstyle_reset,
            "Style::new() and anstyle::Reset are NOT equivalent - always use anstyle::Reset"
        );
    }

    #[test]
    fn test_wrap_text_with_ansi_codes() {
        use super::format::wrap_text_at_width;

        // Simulate a git log line with ANSI color codes
        // Visual content: "* 9452817 Clarify wt merge worktree removal behavior" (52 chars)
        // But with ANSI codes, the raw string is much longer
        let colored_text = "* \x1b[33m9452817\x1b[m Clarify wt merge worktree removal behavior";

        // Without ANSI stripping, this would wrap prematurely because the raw string
        // (with escape codes) is ~70 chars. With proper ANSI stripping, the visual
        // width is only ~52 chars, so it should NOT wrap at width 60.
        let result = wrap_text_at_width(colored_text, 60);

        assert_eq!(
            result.len(),
            1,
            "Colored text should NOT wrap when visual width (52) < max_width (60)"
        );
        assert_eq!(
            result[0], colored_text,
            "Should return original text with ANSI codes intact"
        );

        // Now test that it DOES wrap when visual width exceeds max_width
        let result = wrap_text_at_width(colored_text, 30);
        assert!(
            result.len() > 1,
            "Should wrap into multiple lines when visual width (52) > max_width (30)"
        );
    }

    // wrap_styled_text tests
    #[test]
    fn test_wrap_styled_text_no_wrapping_needed() {
        let result = wrap_styled_text("short line", 50);
        assert_eq!(result, vec!["short line"]);
    }

    #[test]
    fn test_wrap_styled_text_zero_width() {
        let result = wrap_styled_text("some text", 0);
        assert_eq!(result, vec!["some text"]);
    }

    #[test]
    fn test_wrap_styled_text_at_word_boundary() {
        let text = "This is a very long line that needs wrapping";
        let result = wrap_styled_text(text, 20);

        // Should wrap into multiple lines
        assert!(result.len() > 1, "Should wrap into multiple lines");

        // Each line should be within width limit (using visual_width to ignore ANSI codes)
        for line in &result {
            let visual = visual_width(line);
            assert!(
                visual <= 20 || !line.contains(' '),
                "Line '{}' has visual width {} which exceeds 20",
                line,
                visual
            );
        }
    }

    #[test]
    fn test_wrap_styled_text_preserves_styles_across_breaks() {
        // Create styled text: bold text that spans multiple words
        let bold = Style::new().bold();
        let input = format!("{bold}This is bold text that will wrap{bold:#}");
        let result = wrap_styled_text(&input, 15);

        // Should wrap
        assert!(result.len() > 1, "Should wrap into multiple lines");

        // First line should start with bold code
        assert!(
            result[0].contains("\x1b[1m"),
            "First line should have bold code"
        );
    }

    #[test]
    fn test_wrap_styled_text_empty_input() {
        let result = wrap_styled_text("", 50);
        assert_eq!(result, vec![""]);
    }

    #[test]
    fn test_wrap_styled_text_single_long_word() {
        // A single word longer than max_width should still be included
        let result = wrap_styled_text("verylongwordthatcannotbewrapped", 10);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "verylongwordthatcannotbewrapped");
    }

    #[test]
    fn test_wrap_styled_text_preserves_dim_across_wrap_points() {
        // This test verifies that wrap_ansi preserves dim styling across wrap points.
        // This was a bug where wrap_ansi lost track of ANSI state at line breaks,
        // causing text after the wrap to lose its dim styling.
        //
        // Simulate what format_bash_with_gutter produces:
        // - Starts with dim
        // - Highlighted tokens have styles like bold+dim+color
        // - After each highlight, we reset then restore dim: [0m[2m
        // - Unhighlighted text stays dimmed

        let dim = Style::new().dimmed();
        let reset = anstyle::Reset;
        let cmd_style = Style::new()
            .fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Blue)))
            .bold()
            .dimmed();

        // Create a styled line exactly like the highlighter produces:
        // [dim][cmd_style]cp[reset][dim] -cR {{ repo_root }}/target/debug/build {{ worktree }}/target/debug/
        let styled = format!(
            "{dim}{cmd_style}cp{reset}{dim} -cR {{{{ repo_root }}}}/target/debug/build {{{{ worktree }}}}/target/debug/"
        );

        // Wrap at a width that forces a break in the middle of unhighlighted text
        let result = wrap_styled_text(&styled, 40);

        // Should wrap into multiple lines
        assert!(
            result.len() > 1,
            "Should wrap into multiple lines, got {} lines: {:?}",
            result.len(),
            result
        );

        // The key assertion: each wrapped line should start with dim
        // wrap_ansi should restore the dim state at the start of each wrapped line
        let dim_code = "\x1b[2m";

        for (i, line) in result.iter().enumerate() {
            // Every line should start with dim code
            // Line 0: starts with our initial dim
            // Line 1+: wrap_ansi should prepend dim to maintain state
            assert!(
                line.starts_with(dim_code),
                "Line {} should START with dim code, but got: {:?}",
                i + 1,
                &line[..line.len().min(30)]
            );
        }
    }

    #[test]
    fn test_format_bash_with_gutter_template_command() {
        // Test that format_bash_with_gutter produces consistent dim styling
        // for a realistic command with Jinja-style template variables.
        // This is a regression test for wrap-point discontinuity.

        let command = "cp -cR {{ repo_root }}/target/debug/.fingerprint {{ repo_root }}/target/debug/build {{ worktree }}/target/debug/";

        // Use explicit width for deterministic output (avoids env var mutation in parallel tests)
        let result = format_bash_with_gutter_at_width(command, 80);

        // Snapshot the raw output to verify ANSI codes are consistent
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_format_bash_multiline_command_consistent_styling() {
        // This test simulates the REAL user scenario: a multi-line command
        // where each line is processed separately by tree-sitter.
        //
        // The user's actual command:
        // ```
        // [ -d {{ repo_root }}/target/debug/deps ] && [ ! -e {{ worktree }}/target ] &&
        // mkdir -p {{ worktree }}/target/debug/deps &&
        // cp -c {{ repo_root }}/target/debug/deps/*.rlib ... {{ worktree
        // }}/target/debug/deps/ &&
        // ```
        //
        // Note: line 3 ends with `{{ worktree` and line 4 starts with `}}`
        // These should have IDENTICAL styling since both are unhighlighted text.

        let multiline_command = r#"[ -d {{ repo_root }}/target/debug/deps ] && [ ! -e {{ worktree }}/target ] &&
mkdir -p {{ worktree }}/target/debug/deps &&
cp -c {{ repo_root }}/target/debug/deps/*.rlib {{ repo_root }}/target/debug/deps/*.rmeta {{ worktree
}}/target/debug/deps/ &&
cp -cR {{ repo_root }}/target/debug/.fingerprint {{ repo_root }}/target/debug/build {{ worktree
}}/target/debug/"#;

        // Use explicit width for deterministic output (avoids env var mutation in parallel tests)
        let result = format_bash_with_gutter_at_width(multiline_command, 80);

        // Snapshot the output - each line should have consistent dim styling
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_unhighlighted_text_has_consistent_dim_across_lines() {
        // Verify that unhighlighted text (like template variables) has consistent
        // styling whether it appears at the end of one line or start of another.
        //
        // The template text `{{ worktree` and `}}` should have identical ANSI codes.

        let multiline = "echo {{ worktree\n}}/path";
        let result = format_bash_with_gutter(multiline);

        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2, "Should have 2 lines");

        // Extract the styling for unhighlighted text on each line
        // Line 1 ends with `{{ worktree` - find what style it's under
        // Line 2 starts with `}}` - find what style it's under

        let dim_code = "\x1b[2m";

        // Both lines should contain dim code for the unhighlighted text
        // Line 1: after "echo" (highlighted), the `{{ worktree` should be dimmed
        // Line 2: the `}}` at the start should be dimmed

        // Check that line 2 starts with dim after the gutter
        // Gutter pattern: [107m [0m  (then content)
        // The search string "\x1b[0m  " is 6 characters, so content starts at index + 6
        let line2 = lines[1];
        let content_start = line2.find("\x1b[0m  ").map(|i| i + 6);
        if let Some(start) = content_start {
            let content = &line2[start..];
            assert!(
                content.starts_with(dim_code),
                "Line 2 content should start with dim for `}}`, got: {:?}",
                &content[..content.len().min(20)]
            );
        }

        // Snapshot for visual verification
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_syntax_highlighting_produces_multiple_colors() {
        // Verify that bash syntax highlighting produces different colors for different
        // token types, not just a single dim color for everything.
        //
        // This catches regressions where all text becomes the same color.

        let command = "echo 'hello' | grep hello > output.txt && cat output.txt";
        let result = format_bash_with_gutter(command);

        // Check for presence of different ANSI color codes:
        // - Blue (34m) for commands like echo, grep, cat
        // - Green (32m) for strings like 'hello'
        // - Cyan (36m) for operators like |, >
        let has_blue = result.contains("\x1b[34m");
        let has_green = result.contains("\x1b[32m");
        let has_cyan = result.contains("\x1b[36m");

        assert!(
            has_blue,
            "Output should contain blue (34m) for commands, got: {:?}",
            result
        );
        assert!(
            has_green,
            "Output should contain green (32m) for strings, got: {:?}",
            result
        );
        assert!(
            has_cyan,
            "Output should contain cyan (36m) for operators, got: {:?}",
            result
        );

        // Also verify dim is present (base styling)
        assert!(
            result.contains("\x1b[2m"),
            "Output should contain dim (2m) for base styling"
        );

        // Snapshot for visual verification
        insta::assert_snapshot!(result);
    }

    #[test]
    fn test_no_color_discontinuity_in_template_variables() {
        // Regression test: wrap_ansi injects [39m (reset foreground color) at line ends
        // when it thinks a color is "open". This creates visual discontinuity where
        // template variables like `{{ worktree` on line N and `}}` on line N+1 have
        // different styling even though both should just be dim.
        //
        // We never emit [39m ourselves - all our resets use [0m (full reset).
        // So any [39m in the output is an artifact from wrap_ansi that we must strip.
        //
        // This is the actual post-start command from user config that exposed the bug.
        let command = r#"[ -d {{ repo_root }}/target/debug/deps ] && [ ! -e {{ worktree }}/target ] &&
mkdir -p {{ worktree }}/target/debug/deps &&
cp -c {{ repo_root }}/target/debug/deps/*.rlib {{ repo_root }}/target/debug/deps/*.rmeta {{ worktree
}}/target/debug/deps/ &&
cp -cR {{ repo_root }}/target/debug/.fingerprint {{ repo_root }}/target/debug/build {{ worktree
}}/target/debug/"#;

        let result = format_bash_with_gutter(command);

        // The critical assertion: NO [39m codes should appear anywhere in the output.
        // [39m is "reset foreground to default" - we never emit this, only [0m (full reset).
        assert!(
            !result.contains("\x1b[39m"),
            "Output should NOT contain [39m (foreground reset) - this indicates wrap_ansi discontinuity.\n\
             Found [39m in output:\n{}",
            result
                .lines()
                .filter(|line| line.contains("\x1b[39m"))
                .collect::<Vec<_>>()
                .join("\n")
        );

        // Similarly, [49m (reset background) shouldn't appear - we use [0m for all resets.
        assert!(
            !result.contains("\x1b[49m"),
            "Output should NOT contain [49m (background reset)"
        );

        // Verify we DO have the expected codes:
        // - [2m for dim (base styling)
        // - [0m for reset
        // - Various color codes (34m blue, 36m cyan) for syntax highlighting
        assert!(
            result.contains("\x1b[2m"),
            "Output should contain [2m (dim)"
        );
        assert!(
            result.contains("\x1b[0m"),
            "Output should contain [0m (reset)"
        );
    }

    #[test]
    fn test_no_bold_dim_conflict() {
        // Regression test: We must never mix bold (SGR 1) and dim (SGR 2) in the same
        // sequence because they are mutually exclusive in some terminals like Alacritty.
        //
        // The problematic pattern was [2m][1m][2m][34m] where dim, then bold, then dim
        // again would cause the final dim to not render correctly.
        //
        // The fix: token styles use dim+color only (no bold). This test ensures we
        // don't regress by checking that [2m][1m][2m] never appears.
        let command = "cp -cR path/to/source path/to/dest";
        let result = format_bash_with_gutter(command);

        // The problematic pattern is [2m] followed by [1m] followed by [2m]
        // This happens when: line starts with dim, style adds bold+dim
        assert!(
            !result.contains("\x1b[2m\x1b[1m\x1b[2m"),
            "Output should NOT contain [2m][1m][2m] - this indicates redundant dim in token styles.\n\
             Token styles should not include .dimmed() since the line already starts dim.\n\
             Found pattern in output:\n{:?}",
            result
        );
    }

    #[test]
    fn test_all_tokens_are_dimmed() {
        // Regression test: All highlighted tokens should be dimmed to match the base text.
        // We don't use bold because bold (SGR 1) and dim (SGR 2) are mutually exclusive
        // in some terminals like Alacritty.
        //
        // Token styles should emit [2m] (dim) along with their color.
        let command = "cp -cR path/to/source path/to/dest";
        let result = format_bash_with_gutter(command);

        // Verify commands are dim+blue: [0m][2m][34m] (reset, dim, blue)
        assert!(
            result.contains("\x1b[0m\x1b[2m\x1b[34m"),
            "Commands should be dim+blue [0m][2m][34m].\n\
             Output:\n{:?}",
            result
        );

        // Verify flags are dim+cyan: [0m][2m][36m] (reset, dim, cyan)
        assert!(
            result.contains("\x1b[0m\x1b[2m\x1b[36m"),
            "Flags should be dim+cyan [0m][2m][36m].\n\
             Output:\n{:?}",
            result
        );

        // Verify NO bold codes appear (we removed bold to avoid bold/dim conflict)
        assert!(
            !result.contains("\x1b[1m"),
            "Output should NOT contain [1m] (bold) - we use dim instead.\n\
             Output:\n{:?}",
            result
        );
    }

    #[test]
    fn test_fix_dim_after_color_reset() {
        // Basic case: foreground reset followed by dim
        assert_eq!(
            fix_dim_after_color_reset("\x1b[39m\x1b[2m"),
            "\x1b[0m\x1b[2m"
        );

        // With surrounding content (cyan ? then dim ^)
        assert_eq!(
            fix_dim_after_color_reset("\x1b[36m?\x1b[39m\x1b[2m^\x1b[22m"),
            "\x1b[36m?\x1b[0m\x1b[2m^\x1b[22m"
        );

        // Multiple occurrences
        assert_eq!(
            fix_dim_after_color_reset("a\x1b[39m\x1b[2mb\x1b[39m\x1b[2mc"),
            "a\x1b[0m\x1b[2mb\x1b[0m\x1b[2mc"
        );

        // No matches - idempotent
        assert_eq!(fix_dim_after_color_reset("no escapes"), "no escapes");

        // Similar but different sequence (bold, not dim) - should not match
        assert_eq!(
            fix_dim_after_color_reset("\x1b[39m\x1b[1m"),
            "\x1b[39m\x1b[1m"
        );
    }
}
