//! Gutter formatting for quoted content
//!
//! Provides functions for formatting commands and configuration with visual gutters.

#[cfg(feature = "syntax-highlighting")]
use super::highlighting::bash_token_style;

// Import canonical implementations from parent module
use super::{get_terminal_width, visual_width};

/// Width overhead added by format_with_gutter()
///
/// The gutter formatting adds:
/// - 1 column: colored space (gutter)
/// - 1 column: regular space for padding
///
/// Total: 2 columns
///
/// This aligns with message symbols (1 char) + space (1 char) = 2 columns,
/// so gutter content starts at the same column as message text.
///
/// When passing widths to tools like git --stat-width, subtract this overhead
/// so the final output (content + gutter) fits within the terminal width.
pub const GUTTER_OVERHEAD: usize = 2;

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
/// * `max_width` - Optional maximum width (for testing). If None, auto-detects terminal width.
///
/// The gutter appears at column 0, followed by 1 space, then the content starts at column 2.
/// This aligns with message symbols (1 column) + space (1 column) = content at column 2.
///
/// # Example
/// ```
/// use worktrunk::styling::format_with_gutter;
///
/// print!("{}", format_with_gutter("hello world", Some(80)));
/// ```
pub fn format_with_gutter(content: &str, max_width: Option<usize>) -> String {
    let gutter = super::GUTTER;

    // Use provided width or detect terminal width (respects COLUMNS env var)
    let term_width = max_width.unwrap_or_else(get_terminal_width);

    // Account for gutter (1) + space (1)
    let available_width = term_width.saturating_sub(2);

    // Build lines without trailing newline - caller is responsible for element separation
    let lines: Vec<String> = content
        .lines()
        .flat_map(|line| {
            wrap_text_at_width(line, available_width)
                .into_iter()
                .map(|wrapped_line| format!("{gutter} {gutter:#} {wrapped_line}"))
        })
        .collect();

    lines.join("\n")
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
///
/// Additionally, wrap_ansi may split between styled content and its reset code,
/// leaving [0m at the start of continuation lines. We move these to line ends.
///
/// IMPORTANT: wrap_ansi only restores foreground colors on continuation lines,
/// not text attributes like dim. We detect this and prepend dim (\x1b[2m) to
/// continuation lines that start with a color code, ensuring consistent dimming.
///
/// Leading indentation is preserved: if the input starts with spaces, continuation
/// lines will have the same indentation (wrapping happens within the remaining width).
pub fn wrap_styled_text(styled: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![styled.to_string()];
    }

    // Detect leading indentation (spaces before any content or ANSI codes)
    let leading_spaces = styled.chars().take_while(|c| *c == ' ').count();
    let indent = " ".repeat(leading_spaces);
    let content = &styled[leading_spaces..];

    // Handle whitespace-only or empty content
    if content.is_empty() {
        return vec![styled.to_string()];
    }

    // Calculate width for content (excluding indent)
    let content_width = max_width.saturating_sub(leading_spaces);
    if content_width < 10 {
        // Width too narrow for meaningful wrapping
        return vec![styled.to_string()];
    }

    // wrap_ansi returns a string with '\n' at wrap points, preserving ANSI styles
    // Preserve leading whitespace (wrap_ansi's default trims it)
    let options = wrap_ansi::WrapOptions::builder()
        .trim_whitespace(false)
        .build();
    let wrapped = wrap_ansi::wrap_ansi(content, content_width, Some(options));

    if wrapped.is_empty() {
        return vec![String::new()];
    }

    // Strip color reset codes injected by wrap_ansi - we never emit these ourselves,
    // so any occurrence is an artifact that creates visual discontinuity
    let cleaned = wrapped
        .replace("\x1b[39m", "") // reset foreground to default
        .replace("\x1b[49m", ""); // reset background to default

    // Fix reset codes that got separated from their content by wrapping.
    // When wrap happens between styled text and its [0m reset, the reset
    // ends up at the start of the next line. Strip leading resets.
    //
    // Also fix missing dim on continuation lines: wrap_ansi restores colors
    // but not text attributes like dim. If a line starts with a color code
    // (e.g., \x1b[32m) but no dim (\x1b[2m), prepend dim to maintain consistency.
    let lines: Vec<_> = cleaned.lines().collect();
    let mut result = Vec::with_capacity(lines.len());

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.strip_prefix("\x1b[0m").unwrap_or(line);

        // For continuation lines (not first), check if we need to restore dim.
        // wrap_ansi restores foreground colors (\x1b[3Xm where X is 0-7 or 8;...)
        // but drops text attributes like dim (\x1b[2m).
        //
        // We restore dim for lines that start with a color code. This is safe
        // because format_bash_with_gutter_impl always starts lines with dim,
        // so any wrapped continuation should also be dimmed.
        let with_dim = if i > 0 && trimmed.starts_with("\x1b[3") {
            format!("\x1b[2m{trimmed}")
        } else {
            trimmed.to_owned()
        };

        // Add the original indentation to all lines
        result.push(format!("{indent}{with_dim}"));
    }

    result
}

#[cfg(feature = "syntax-highlighting")]
fn format_bash_with_gutter_impl(content: &str, width_override: Option<usize>) -> String {
    use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

    let gutter = super::GUTTER;
    let reset = anstyle::Reset;
    let dim = anstyle::Style::new().dimmed();

    // Calculate available width for content
    let term_width = width_override.unwrap_or_else(get_terminal_width);
    let available_width = term_width.saturating_sub(2);

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

    // Build lines without trailing newline - caller is responsible for element separation
    let mut output_lines: Vec<String> = Vec::new();

    // Process each line separately - this is required because tree-sitter's bash
    // grammar fails to highlight multi-line commands when `&&` appears at line ends.
    // Per-line processing gives proper highlighting for each line's content.
    for line in content.lines() {
        let mut styled_line = format!("{dim}");

        let Ok(highlights) = highlighter.highlight(&config, line.as_bytes(), None, |_| None) else {
            // Fallback: if highlighting fails, use plain dim
            styled_line.push_str(line);
            for wrapped in wrap_styled_text(&styled_line, available_width) {
                output_lines.push(format!("{gutter} {gutter:#} {wrapped}{reset}"));
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

        // Wrap and collect gutter lines
        for wrapped in wrap_styled_text(&styled_line, available_width) {
            output_lines.push(format!("{gutter} {gutter:#} {wrapped}{reset}"));
        }
    }

    output_lines.join("\n")
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
/// print!("{}", format_bash_with_gutter("npm install --frozen-lockfile"));
/// ```
#[cfg(feature = "syntax-highlighting")]
pub fn format_bash_with_gutter(content: &str) -> String {
    format_bash_with_gutter_impl(content, None)
}

/// Test-only helper to force a specific terminal width for deterministic output.
///
/// This avoids env var mutation which is unsafe in parallel tests.
#[cfg(all(test, feature = "syntax-highlighting"))]
pub(crate) fn format_bash_with_gutter_at_width(content: &str, width: usize) -> String {
    format_bash_with_gutter_impl(content, Some(width))
}

/// Format bash commands with gutter (fallback without syntax highlighting)
///
/// This version is used when the `syntax-highlighting` feature is disabled.
/// It provides the same gutter formatting without tree-sitter dependencies.
#[cfg(not(feature = "syntax-highlighting"))]
pub fn format_bash_with_gutter(content: &str) -> String {
    format_with_gutter(content, None)
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
    fn test_wrap_styled_text_preserves_leading_whitespace() {
        let result = wrap_styled_text("          Print help", 80);
        assert_eq!(result, vec!["          Print help"]);
    }

    #[test]
    fn test_wrap_styled_text_only_whitespace() {
        let result = wrap_styled_text("          ", 80);
        assert_eq!(result, vec!["          "]);
    }

    #[test]
    fn test_wrap_styled_text_preserves_indent_on_wrap() {
        // Force wrapping by using a narrow width - text should wrap and preserve indent
        let result = wrap_styled_text(
            "          This is a longer text that should wrap across multiple lines",
            40,
        );
        assert!(result.len() > 1, "Should have wrapped");
        // All lines should have the 10-space indent
        for line in &result {
            assert!(
                line.starts_with("          "),
                "Line should start with 10 spaces: {:?}",
                line
            );
        }
    }

    #[test]
    fn test_format_with_gutter_basic() {
        let result = format_with_gutter("hello", Some(80));
        // Should have gutter formatting, no trailing newline (caller adds it)
        assert!(result.contains("hello"));
        assert!(!result.ends_with('\n'));
    }

    #[test]
    fn test_format_with_gutter_multiline() {
        let result = format_with_gutter("line1\nline2", Some(80));
        // Each line should be formatted separately
        assert!(result.contains("line1"));
        assert!(result.contains("line2"));
        // Should have 1 newline (between lines, not trailing)
        assert_eq!(result.matches('\n').count(), 1);
    }

    #[test]
    fn test_gutter_overhead_constant() {
        // Verify the overhead matches documented value
        assert_eq!(GUTTER_OVERHEAD, 2);
    }

    #[test]
    fn test_format_with_gutter_empty() {
        let result = format_with_gutter("", Some(80));
        // Empty input should produce empty output
        assert!(result.is_empty());
    }

    #[test]
    fn test_format_with_gutter_wrapping() {
        // Use a very narrow width to force wrapping
        let result = format_with_gutter("word1 word2 word3 word4", Some(15));
        // Content should be wrapped to multiple lines (newlines between, not trailing)
        let line_count = result.matches('\n').count();
        assert!(
            line_count >= 1,
            "Expected at least one newline (between wrapped lines), got {}",
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
    fn test_wrap_styled_text_restores_dim_on_continuation() {
        // When wrap_ansi wraps dim+color text, it restores the color but not dim.
        // We fix this by prepending dim to continuation lines that start with a color.
        let dim = "\x1b[2m";
        let green = "\x1b[32m";
        let reset = "\x1b[0m";

        // Simulate what format_bash_with_gutter_impl produces for a string token
        let styled = format!(
            "{dim}{green}This is a very long string that definitely needs to wrap across multiple lines{reset}"
        );

        // Force wrapping at 30 chars - should produce multiple lines
        let result = wrap_styled_text(&styled, 30);
        assert!(result.len() > 1);

        // First line should have dim+green (as input)
        assert!(result[0].starts_with("\x1b[2m\x1b[32m"));

        // Continuation lines should ALSO have dim before the color (restored by our fix)
        for line in result.iter().skip(1) {
            assert!(line.starts_with("\x1b[2m\x1b[32m") || line.starts_with("\x1b[2m"));
        }
    }

    #[test]
    #[cfg(feature = "syntax-highlighting")]
    fn test_format_bash_with_gutter_at_width_basic() {
        let result = format_bash_with_gutter_at_width("echo hello", 80);
        assert!(result.contains("echo"));
        assert!(result.contains("hello"));
        // No trailing newline - caller is responsible for element separation
        assert!(!result.ends_with('\n'));
    }

    #[test]
    #[cfg(feature = "syntax-highlighting")]
    fn test_format_bash_with_gutter_at_width_multiline() {
        let result = format_bash_with_gutter_at_width("echo line1\necho line2", 80);
        assert!(result.contains("line1"));
        assert!(result.contains("line2"));
        // Two lines should have one newline (between, not trailing)
        assert_eq!(result.matches('\n').count(), 1);
    }

    #[test]
    #[cfg(feature = "syntax-highlighting")]
    fn test_format_bash_with_gutter_complex_command() {
        let result = format_bash_with_gutter_at_width("npm install && cargo build --release", 100);
        assert!(result.contains("npm"));
        assert!(result.contains("cargo"));
        assert!(result.contains("--release"));
    }
}
