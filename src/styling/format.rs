//! Gutter formatting for quoted content
//!
//! Provides functions for formatting commands and configuration with visual gutters.

use unicode_width::UnicodeWidthStr;

#[cfg(feature = "syntax-highlighting")]
use super::highlighting::bash_token_style;

// Import canonical implementations from parent module
use super::{get_terminal_width, visual_width};

/// Width overhead added by format_with_gutter()
///
/// The gutter formatting adds:
/// - 1 column: colored space (gutter)
/// - 2 columns: regular spaces for padding
///
/// Total: 3 columns
///
/// When passing widths to tools like git --stat-width, subtract this overhead
/// so the final output (content + gutter) fits within the terminal width.
pub const GUTTER_OVERHEAD: usize = 3;

/// Wraps text at word boundaries to fit within the specified width
///
/// # Arguments
/// * `text` - The text to wrap (may contain ANSI codes)
/// * `max_width` - Maximum visual width for each line
///
/// # Returns
/// A vector of wrapped lines
///
/// # Note
/// Width calculation ignores ANSI escape codes to handle colored output correctly.
pub(super) fn wrap_text_at_width(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }

    // Use visual width (ignoring ANSI codes) for proper wrapping of colored text
    let text_width = visual_width(text);

    // If the line fits, return it as-is
    if text_width <= max_width {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0;

    for word in text.split_whitespace() {
        let word_width = visual_width(word);

        // If this is the first word in the line
        if current_line.is_empty() {
            // If a single word is longer than max_width, we have to include it anyway
            current_line = word.to_string();
            current_width = word_width;
        } else {
            // Calculate width with space before the word
            let new_width = current_width + 1 + word_width;

            if new_width <= max_width {
                // Word fits on current line
                current_line.push(' ');
                current_line.push_str(word);
                current_width = new_width;
            } else {
                // Word doesn't fit, start a new line
                lines.push(current_line);
                current_line = word.to_string();
                current_width = word_width;
            }
        }
    }

    // Add the last line if there's content
    if !current_line.is_empty() {
        lines.push(current_line);
    }

    // Handle empty input
    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

/// Formats text with a gutter (single-space with background color) on each line
///
/// This creates a subtle visual separator for quoted content like commands or configuration.
/// Text is automatically word-wrapped at terminal width to prevent overflow.
///
/// # Arguments
/// * `content` - The text to format (preserves internal structure for multi-line)
/// * `left_margin` - Should always be "" (gutter provides all visual separation)
/// * `max_width` - Optional maximum width (for testing). If None, auto-detects terminal width.
///
/// The gutter appears at column 0, followed by 2 spaces, then the content starts at column 3.
/// This aligns with emoji messages where the emoji (2 columns) + space (1 column) also starts content at column 3.
///
/// # Example
/// ```
/// use worktrunk::styling::format_with_gutter;
///
/// print!("{}", format_with_gutter("hello world", "", Some(80)));
/// ```
pub fn format_with_gutter(content: &str, left_margin: &str, max_width: Option<usize>) -> String {
    let gutter = super::GUTTER;
    let mut output = String::new();

    // Use provided width or detect terminal width (respects COLUMNS env var)
    let term_width = max_width.unwrap_or_else(get_terminal_width);

    // Account for gutter (1) + spaces (2) + left_margin
    let left_margin_width = left_margin.width();
    let available_width = term_width.saturating_sub(3 + left_margin_width);

    for line in content.lines() {
        // Wrap the line at word boundaries
        let wrapped_lines = wrap_text_at_width(line, available_width);

        for wrapped_line in wrapped_lines {
            output.push_str(&format!(
                "{left_margin}{gutter} {gutter:#}  {wrapped_line}\n"
            ));
        }
    }

    output
}

/// Wrap ANSI-styled text at word boundaries, preserving styles across line breaks
///
/// Uses `wrap-ansi` crate which handles ANSI escape sequences, Unicode width,
/// and OSC 8 hyperlinks automatically.
///
/// Note: wrap_ansi injects color reset codes ([39m for foreground, [49m for background)
/// at line ends to make each line "self-contained". We strip these because:
/// 1. We never emit [39m/[49m ourselves - all our resets use [0m (full reset)
/// 2. These injected codes create visual discontinuity when styled text wraps
pub(super) fn wrap_styled_text(styled: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![styled.to_string()];
    }

    // wrap_ansi returns a string with '\n' at wrap points, preserving ANSI styles
    let wrapped = wrap_ansi::wrap_ansi(styled, max_width, None);

    if wrapped.is_empty() {
        return vec![String::new()];
    }

    // Strip color reset codes injected by wrap_ansi - we never emit these ourselves,
    // so any occurrence is an artifact that creates visual discontinuity
    let cleaned = wrapped
        .replace("\x1b[39m", "") // reset foreground to default
        .replace("\x1b[49m", ""); // reset background to default

    cleaned.lines().map(|s| s.to_owned()).collect()
}

#[cfg(feature = "syntax-highlighting")]
fn format_bash_with_gutter_impl(
    content: &str,
    left_margin: &str,
    width_override: Option<usize>,
) -> String {
    use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

    let gutter = super::GUTTER;
    let reset = anstyle::Reset;
    let dim = anstyle::Style::new().dimmed();
    let mut output = String::new();

    // Calculate available width for content
    let term_width = width_override.unwrap_or_else(get_terminal_width);
    let left_margin_width = left_margin.width();
    let available_width = term_width.saturating_sub(3 + left_margin_width);

    // Set up tree-sitter bash highlighting
    let highlight_names = vec![
        "function", // Commands like npm, git, cargo
        "keyword",  // Keywords like for, if, while
        "string",   // Quoted strings
        "operator", // Operators like &&, ||, |, $, -
        "comment",  // Comments
        "number",   // Numbers
        "variable", // Variables
        "constant", // Constants/flags
    ];

    let bash_language = tree_sitter_bash::LANGUAGE.into();
    let bash_highlights = tree_sitter_bash::HIGHLIGHT_QUERY;

    let mut config = match HighlightConfiguration::new(
        bash_language,
        "bash", // language name
        bash_highlights,
        "", // injections query
        "", // locals query
    ) {
        Ok(config) => config,
        Err(_) => {
            // Fallback: if tree-sitter fails, use plain gutter formatting
            HighlightConfiguration::new(
                tree_sitter_bash::LANGUAGE.into(),
                "bash", // language name
                "",     // empty query
                "",
                "",
            )
            .unwrap()
        }
    };

    config.configure(&highlight_names);

    let mut highlighter = Highlighter::new();

    // Process each line separately - this is required because tree-sitter's bash
    // grammar fails to highlight multi-line commands when `&&` appears at line ends.
    // Per-line processing gives proper highlighting for each line's content.
    for line in content.lines() {
        let mut styled_line = format!("{dim}");

        let Ok(highlights) = highlighter.highlight(&config, line.as_bytes(), None, |_| None) else {
            // Fallback: if highlighting fails, use plain dim
            styled_line.push_str(line);
            for wrapped in wrap_styled_text(&styled_line, available_width) {
                output.push_str(&format!(
                    "{left_margin}{gutter} {gutter:#}  {wrapped}{reset}\n"
                ));
            }
            continue;
        };

        let line_bytes = line.as_bytes();

        // Track the current highlight type so we can decide styling when we see the actual text
        let mut pending_highlight: Option<usize> = None;

        for event in highlights {
            match event.unwrap() {
                HighlightEvent::Source { start, end } => {
                    // Output the text for this source region
                    if let Ok(text) = std::str::from_utf8(&line_bytes[start..end]) {
                        // Apply pending highlight style, but skip command styling for template syntax
                        // (tree-sitter misinterprets `}}` at line start as a command)
                        if let Some(idx) = pending_highlight.take() {
                            let is_template_syntax =
                                text.starts_with("}}") || text.starts_with("{{");
                            let is_function = highlight_names
                                .get(idx)
                                .is_some_and(|name| *name == "function");

                            // Skip command styling for template syntax, apply normal styling otherwise
                            if !(is_function && is_template_syntax)
                                && let Some(name) = highlight_names.get(idx)
                                && let Some(style) = bash_token_style(name)
                            {
                                // Reset before applying style to clear the base dim, then apply token style.
                                // Token styles use dim+color (not bold) because bold (SGR 1) and dim (SGR 2)
                                // are mutually exclusive in some terminals like Alacritty.
                                styled_line.push_str(&format!("{reset}{style}"));
                            }
                        }

                        styled_line.push_str(text);
                    }
                }
                HighlightEvent::HighlightStart(idx) => {
                    // Remember the highlight type - we'll decide on styling when we see the text
                    pending_highlight = Some(idx.0);
                }
                HighlightEvent::HighlightEnd => {
                    // End of highlighted region - reset and restore dim for unhighlighted text
                    pending_highlight = None;
                    styled_line.push_str(&format!("{reset}{dim}"));
                }
            }
        }

        // Wrap and output with gutter
        for wrapped in wrap_styled_text(&styled_line, available_width) {
            output.push_str(&format!(
                "{left_margin}{gutter} {gutter:#}  {wrapped}{reset}\n"
            ));
        }
    }

    output
}

/// Formats bash/shell commands with syntax highlighting and gutter
///
/// Processes each line separately for highlighting (required for multi-line commands
/// with `&&` at line ends), then applies template syntax detection to avoid
/// misinterpreting `}}` as a command when it appears at line start.
///
/// # Example
/// ```
/// use worktrunk::styling::format_bash_with_gutter;
///
/// print!("{}", format_bash_with_gutter("npm install --frozen-lockfile", ""));
/// ```
#[cfg(feature = "syntax-highlighting")]
pub fn format_bash_with_gutter(content: &str, left_margin: &str) -> String {
    format_bash_with_gutter_impl(content, left_margin, None)
}

/// Test-only helper to force a specific terminal width for deterministic output.
///
/// This avoids env var mutation which is unsafe in parallel tests.
#[cfg(all(test, feature = "syntax-highlighting"))]
pub(crate) fn format_bash_with_gutter_at_width(
    content: &str,
    left_margin: &str,
    width: usize,
) -> String {
    format_bash_with_gutter_impl(content, left_margin, Some(width))
}

/// Format bash commands with gutter (fallback without syntax highlighting)
///
/// This version is used when the `syntax-highlighting` feature is disabled.
/// It provides the same gutter formatting without tree-sitter dependencies.
#[cfg(not(feature = "syntax-highlighting"))]
pub fn format_bash_with_gutter(content: &str, left_margin: &str) -> String {
    format_with_gutter(content, left_margin, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrap_text_at_width_no_wrap_needed() {
        let result = wrap_text_at_width("short text", 20);
        assert_eq!(result, vec!["short text"]);
    }

    #[test]
    fn test_wrap_text_at_width_basic_wrap() {
        let result = wrap_text_at_width("hello world foo bar", 10);
        // Words wrap at boundaries, each line fits within max_width
        assert_eq!(result, vec!["hello", "world foo", "bar"]);
    }

    #[test]
    fn test_wrap_text_at_width_zero_width() {
        let result = wrap_text_at_width("hello world", 0);
        assert_eq!(result, vec!["hello world"]);
    }

    #[test]
    fn test_wrap_text_at_width_empty_input() {
        let result = wrap_text_at_width("", 20);
        assert_eq!(result, vec![""]);
    }

    #[test]
    fn test_wrap_text_at_width_single_long_word() {
        // Single word longer than max_width should still be included
        let result = wrap_text_at_width("superlongword", 5);
        assert_eq!(result, vec!["superlongword"]);
    }

    #[test]
    fn test_wrap_styled_text_no_wrap_needed() {
        let result = wrap_styled_text("short text", 20);
        assert_eq!(result, vec!["short text"]);
    }

    #[test]
    fn test_wrap_styled_text_zero_width() {
        let result = wrap_styled_text("hello world", 0);
        assert_eq!(result, vec!["hello world"]);
    }

    #[test]
    fn test_wrap_styled_text_empty_input() {
        let result = wrap_styled_text("", 20);
        assert_eq!(result, vec![""]);
    }

    #[test]
    fn test_format_with_gutter_basic() {
        let result = format_with_gutter("hello", "", Some(80));
        // Should have gutter formatting
        assert!(result.contains("hello"));
        assert!(result.ends_with('\n'));
    }

    #[test]
    fn test_format_with_gutter_multiline() {
        let result = format_with_gutter("line1\nline2", "", Some(80));
        // Each line should be formatted separately
        assert!(result.contains("line1"));
        assert!(result.contains("line2"));
        // Should have 2 newlines (one per line)
        assert_eq!(result.matches('\n').count(), 2);
    }

    #[test]
    fn test_gutter_overhead_constant() {
        // Verify the overhead matches documented value
        assert_eq!(GUTTER_OVERHEAD, 3);
    }

    #[test]
    fn test_format_with_gutter_empty() {
        let result = format_with_gutter("", "", Some(80));
        // Empty input should produce empty output
        assert!(result.is_empty());
    }

    #[test]
    fn test_format_with_gutter_with_margin() {
        let result = format_with_gutter("content", "  ", Some(80));
        // Content with margin should still have the content
        assert!(result.contains("content"));
        // Should start with margin spaces
        assert!(result.starts_with("  "));
    }

    #[test]
    fn test_format_with_gutter_wrapping() {
        // Use a very narrow width to force wrapping
        let result = format_with_gutter("word1 word2 word3 word4", "", Some(15));
        // Content should be wrapped to multiple lines
        let line_count = result.matches('\n').count();
        assert!(
            line_count > 1,
            "Expected multiple lines, got {}",
            line_count
        );
    }

    #[test]
    fn test_wrap_text_at_width_with_multiple_spaces() {
        // wrap_text_at_width uses split_whitespace which joins with single space
        // Let's verify behavior by checking what actually happens
        let result = wrap_text_at_width("hello    world", 20);
        // split_whitespace preserves word boundaries but normalizes whitespace
        // Actually looking at the code - split_whitespace + rejoin with single space
        // yields "hello world" when joining
        assert!(result[0].contains("hello"));
        assert!(result[0].contains("world"));
    }

    #[test]
    fn test_wrap_styled_text_with_ansi() {
        // Text with ANSI codes should wrap based on visible width
        let styled = "\u{1b}[1mbold text\u{1b}[0m here";
        let result = wrap_styled_text(styled, 100);
        // Should preserve the content
        assert!(result[0].contains("bold"));
        assert!(result[0].contains("text"));
    }

    #[test]
    fn test_wrap_styled_text_strips_injected_resets() {
        // If wrap_ansi injects [39m or [49m, they should be stripped
        let styled = "some colored text";
        let result = wrap_styled_text(styled, 50);
        // Result should not contain the specific reset codes we strip
        assert!(!result[0].contains("\u{1b}[39m"));
        assert!(!result[0].contains("\u{1b}[49m"));
    }

    #[test]
    #[cfg(feature = "syntax-highlighting")]
    fn test_format_bash_with_gutter_at_width_basic() {
        let result = format_bash_with_gutter_at_width("echo hello", "", 80);
        assert!(result.contains("echo"));
        assert!(result.contains("hello"));
        assert!(result.ends_with('\n'));
    }

    #[test]
    #[cfg(feature = "syntax-highlighting")]
    fn test_format_bash_with_gutter_at_width_multiline() {
        let result = format_bash_with_gutter_at_width("echo line1\necho line2", "", 80);
        assert!(result.contains("line1"));
        assert!(result.contains("line2"));
        // Two lines should have two newlines
        assert_eq!(result.matches('\n').count(), 2);
    }

    #[test]
    #[cfg(feature = "syntax-highlighting")]
    fn test_format_bash_with_gutter_complex_command() {
        let result =
            format_bash_with_gutter_at_width("npm install && cargo build --release", "", 100);
        assert!(result.contains("npm"));
        assert!(result.contains("cargo"));
        assert!(result.contains("--release"));
    }
}
