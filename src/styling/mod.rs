//! Consolidated styling module for terminal output.
//!
//! This module uses the anstyle ecosystem:
//! - anstream for auto-detecting color support
//! - anstyle for composable styling
//! - Semantic style constants for domain-specific use
//!
//! ## stdout vs stderr principle
//!
//! - **stdout**: ALL worktrunk output (messages, errors, warnings, directives, data)
//! - **stderr**: ALL child process output (git, npm, user commands)
//! - **Exception**: Interactive prompts use stderr so they appear even when stdout is redirected
//!
//! Use `println!` for all worktrunk messages. Use `eprintln!` only for interactive prompts.

mod constants;
mod format;
mod highlighting;
mod line;

// Re-exports from anstream (auto-detecting output)
pub use anstream::{eprint, eprintln, print, println, stderr, stdout};

// Re-exports from anstyle (for composition)
pub use anstyle::Style as AnstyleStyle;

// Re-export our public types
pub use constants::*;
pub use format::{GUTTER_OVERHEAD, format_bash_with_gutter, format_with_gutter};
pub use highlighting::format_toml;
pub use line::{StyledLine, StyledString};

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

        let output = format_toml(toml_content, "");

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

        // Check that gutter background is present (Black background = 40)
        assert!(
            output.contains("\x1b[40m"),
            "Should contain gutter background color (Black = 40)"
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
        let result = format_with_gutter(long_text, "", Some(80));

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
                line.contains("\x1b[40m"),
                "Each line should contain gutter (Black background = 40)"
            );
        }
    }

    #[test]
    fn test_format_with_gutter_preserves_newlines() {
        let multi_line = "Line 1\nLine 2\nLine 3";
        let result = format_with_gutter(multi_line, "", None);

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
        let result = format_with_gutter(commit_msg, "", Some(80));

        insta::assert_snapshot!(result, @r"
        [40m [0m  This commit refactors the authentication system to use a more secure
        [40m [0m  token-based approach instead of the previous session-based system which had
        [40m [0m  several security vulnerabilities that were identified during the security
        [40m [0m  audit last month. The new implementation follows industry best practices and
        [40m [0m  includes proper token rotation and expiration handling.
        ");
    }

    #[test]
    fn test_bash_gutter_formatting_ends_with_reset() {
        // Test that bash gutter formatting properly resets colors at the end of each line
        // to prevent color bleeding into subsequent output (like child process output)
        let command = "pre-commit run --all-files";
        let result = format_bash_with_gutter(command, "");

        // The output should end with ANSI reset code followed by newline
        // ANSI reset is \x1b[0m (ESC[0m)
        assert!(
            result.ends_with("\x1b[0m\n"),
            "Bash gutter formatting should end with ANSI reset code followed by newline, got: {:?}",
            result.chars().rev().take(20).collect::<String>()
        );

        // Verify the reset appears at the end of EVERY line (for multi-line commands)
        let multi_line_command = "npm install && \\\n    npm run build";
        let multi_result = format_bash_with_gutter(multi_line_command, "");

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

        // Most importantly: the final output should end with reset + newline
        assert!(
            multi_result.ends_with("\x1b[0m\n"),
            "Multi-line bash gutter formatting should end with ANSI reset + newline"
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
}
