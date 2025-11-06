//! Gutter formatting for quoted content
//!
//! Provides functions for formatting commands and configuration with visual gutters.

use anstyle::{AnsiColor, Color, Style};
use unicode_width::UnicodeWidthStr;

#[cfg(feature = "syntax-highlighting")]
use super::highlighting::bash_token_style;

/// Default terminal width fallback if detection fails
const DEFAULT_TERMINAL_WIDTH: usize = 80;

/// Strip ANSI escape codes from a string using strip-ansi-escapes
fn strip_ansi_codes(s: &str) -> String {
    strip_ansi_escapes::strip_str(s)
}

/// Calculate visual width of a string, ignoring ANSI escape codes
fn visual_width(s: &str) -> usize {
    strip_ansi_codes(s).width()
}

/// Get terminal width, defaulting to 80 if detection fails
///
/// Checks COLUMNS environment variable first (for testing and scripts),
/// then falls back to actual terminal size detection.
fn get_terminal_width() -> usize {
    // Check COLUMNS environment variable first (for testing and scripts)
    if let Ok(cols) = std::env::var("COLUMNS")
        && let Ok(width) = cols.parse::<usize>()
    {
        return width;
    }

    // Fall back to actual terminal size
    terminal_size::terminal_size()
        .map(|(terminal_size::Width(w), _)| w as usize)
        .unwrap_or(DEFAULT_TERMINAL_WIDTH)
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
    let gutter = Style::new().bg_color(Some(Color::Ansi(AnsiColor::Black)));
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

    let gutter = Style::new().bg_color(Some(Color::Ansi(AnsiColor::Black)));
    let mut output = String::new();

    // Calculate available width for wrapping
    let term_width = get_terminal_width();
    let left_margin_width = left_margin.width();
    let available_width = term_width.saturating_sub(3 + left_margin_width);

    // Wrap lines at word boundaries
    let mut wrapped_lines = Vec::new();
    for line in content.lines() {
        let wrapped = wrap_text_at_width(line, available_width);
        wrapped_lines.extend(wrapped);
    }

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

    let bash_language = tree_sitter_bash::language();
    let bash_highlights = tree_sitter_bash::HIGHLIGHT_QUERY;

    let mut config = HighlightConfiguration::new(
        bash_language,
        bash_highlights,
        "", // injections query
        "", // locals query
    )
    .unwrap_or_else(|_| {
        // Fallback: if tree-sitter fails, use plain gutter formatting
        HighlightConfiguration::new(
            bash_language,
            "", // empty query
            "",
            "",
        )
        .unwrap()
    });

    config.configure(&highlight_names);

    let mut highlighter = Highlighter::new();

    // Process each wrapped line
    for line in &wrapped_lines {
        output.push_str(&format!("{left_margin}{gutter} {gutter:#}  "));

        // Highlight this line
        let Ok(highlights) = highlighter.highlight(&config, line.as_bytes(), None, |_| None) else {
            // Fallback: just print plain text if highlighting fails
            output.push_str(line);
            output.push('\n');
            continue;
        };

        let line_bytes = line.as_bytes();

        for event in highlights {
            match event.unwrap() {
                HighlightEvent::Source { start, end } => {
                    // Output the text for this source region
                    if let Ok(text) = std::str::from_utf8(&line_bytes[start..end]) {
                        output.push_str(text);
                    }
                }
                HighlightEvent::HighlightStart(idx) => {
                    // Start of a highlighted region - apply style
                    if let Some(name) = highlight_names.get(idx.0)
                        && let Some(style) = bash_token_style(name)
                    {
                        output.push_str(&format!("{style}"));
                    }
                }
                HighlightEvent::HighlightEnd => {
                    // End of highlighted region - reset style
                    output.push_str(&format!("{}", anstyle::Reset));
                }
            }
        }

        // Ensure all styles are reset at end of line to prevent leaking into child process output
        output.push_str(&format!("{}", anstyle::Reset));
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
    let gutter = Style::new().bg_color(Some(Color::Ansi(AnsiColor::Black)));
    let mut output = String::new();

    // Calculate available width for wrapping
    let term_width = get_terminal_width();
    let left_margin_width = left_margin.width();
    let available_width = term_width.saturating_sub(3 + left_margin_width);

    // Wrap lines at word boundaries
    let mut wrapped_lines = Vec::new();
    for line in content.lines() {
        let wrapped = wrap_text_at_width(line, available_width);
        wrapped_lines.extend(wrapped);
    }

    // Process each wrapped line with plain gutter (no syntax highlighting)
    for line in &wrapped_lines {
        output.push_str(&format!("{left_margin}{gutter} {gutter:#}  "));
        output.push_str(line);
        // Ensure all styles are reset at end of line to prevent leaking into child process output
        output.push_str(&format!("{}", anstyle::Reset));
        output.push('\n');
    }

    output
}
