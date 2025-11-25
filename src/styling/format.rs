//! Gutter formatting for quoted content
//!
//! Provides functions for formatting commands and configuration with visual gutters.

use ansi_str::AnsiStr;
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

/// Strip ANSI escape codes from a string
pub fn strip_ansi_codes(s: &str) -> String {
    s.ansi_strip().into_owned()
}

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
/// ```ignore
/// // All contexts use empty left margin and auto-detect width
/// print!("{}", format_with_gutter(&config, "", None));
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

/// Wrap content lines for gutter display
fn wrap_content_for_gutter(content: &str, left_margin: &str) -> Vec<String> {
    let term_width = get_terminal_width();
    let left_margin_width = left_margin.width();
    let available_width = term_width.saturating_sub(3 + left_margin_width);

    let mut wrapped_lines = Vec::new();
    for line in content.lines() {
        let wrapped = wrap_text_at_width(line, available_width);
        wrapped_lines.extend(wrapped);
    }
    wrapped_lines
}

/// Format wrapped lines with plain gutter (no syntax highlighting)
#[cfg(not(feature = "syntax-highlighting"))]
fn format_wrapped_lines_plain(wrapped_lines: &[String], left_margin: &str) -> String {
    let gutter = super::GUTTER;
    let dimmed = anstyle::Style::new().dimmed();
    let mut output = String::new();

    for line in wrapped_lines {
        output.push_str(&format!("{left_margin}{gutter} {gutter:#}  "));
        output.push_str(&format!("{dimmed}{line}{dimmed:#}"));
        output.push_str(&format!("{}", anstyle::Reset));
        output.push('\n');
    }

    output
}

/// Formats bash/shell commands with syntax highlighting and gutter
///
/// Similar to `format_with_gutter` but applies bash syntax highlighting using tree-sitter.
/// Long lines are wrapped at word boundaries to fit terminal width.
///
/// # Example
/// ```ignore
/// print!("{}", format_bash_with_gutter("npm install --frozen-lockfile"));
/// ```
#[cfg(feature = "syntax-highlighting")]
pub fn format_bash_with_gutter(content: &str, left_margin: &str) -> String {
    use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

    let gutter = super::GUTTER;
    // Use gray (BrightBlack) for visible dimming of unhighlighted content
    let base_style = super::GRAY;
    let reset = anstyle::Reset;
    let mut output = String::new();

    let wrapped_lines = wrap_content_for_gutter(content, left_margin);

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

    // Process each wrapped line
    for line in &wrapped_lines {
        // Start line with gutter and base gray style for unhighlighted content
        output.push_str(&format!("{left_margin}{gutter} {gutter:#}  {base_style}"));

        // Highlight this line
        let Ok(highlights) = highlighter.highlight(&config, line.as_bytes(), None, |_| None) else {
            // Fallback: just print plain text if highlighting fails (already gray)
            output.push_str(line);
            output.push_str(&format!("{reset}"));
            output.push('\n');
            continue;
        };

        let line_bytes = line.as_bytes();

        for event in highlights {
            match event.unwrap() {
                HighlightEvent::Source { start, end } => {
                    // Output the text for this source region (inherits current style)
                    if let Ok(text) = std::str::from_utf8(&line_bytes[start..end]) {
                        output.push_str(text);
                    }
                }
                HighlightEvent::HighlightStart(idx) => {
                    // Start of a highlighted region - apply style (includes dimmed)
                    if let Some(name) = highlight_names.get(idx.0)
                        && let Some(style) = bash_token_style(name)
                    {
                        output.push_str(&format!("{style}"));
                    }
                }
                HighlightEvent::HighlightEnd => {
                    // End of highlighted region - return to base gray style
                    output.push_str(&format!("{base_style}"));
                }
            }
        }

        // Full reset at end of line to prevent leaking into child process output
        output.push_str(&format!("{reset}"));
        output.push('\n');
    }

    output
}

/// Format bash commands with gutter (fallback without syntax highlighting)
///
/// This version is used when the `syntax-highlighting` feature is disabled.
/// It provides the same gutter formatting without tree-sitter dependencies.
#[cfg(not(feature = "syntax-highlighting"))]
pub fn format_bash_with_gutter(content: &str, left_margin: &str) -> String {
    let wrapped_lines = wrap_content_for_gutter(content, left_margin);
    format_wrapped_lines_plain(&wrapped_lines, left_margin)
}
