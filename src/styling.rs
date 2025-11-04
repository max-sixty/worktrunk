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

use anstyle::{AnsiColor, Color, Style};
use synoptic::{TokOpt, from_extension}; // Still used for TOML highlighting
use unicode_width::UnicodeWidthStr;

// ============================================================================
// Re-exports from anstream (auto-detecting output)
// ============================================================================

/// Auto-detecting println that respects NO_COLOR, CLICOLOR_FORCE, and terminal capabilities
pub use anstream::println;

/// Auto-detecting eprintln that respects NO_COLOR, CLICOLOR_FORCE, and terminal capabilities
pub use anstream::eprintln;

/// Auto-detecting print that respects NO_COLOR, CLICOLOR_FORCE, and terminal capabilities
pub use anstream::print;

/// Auto-detecting eprint that respects NO_COLOR, CLICOLOR_FORCE, and terminal capabilities
pub use anstream::eprint;

/// Auto-detecting stderr stream that respects NO_COLOR, CLICOLOR_FORCE, and terminal capabilities
pub use anstream::stderr;

/// Auto-detecting stdout stream that respects NO_COLOR, CLICOLOR_FORCE, and terminal capabilities
pub use anstream::stdout;

// ============================================================================
// Re-exports from anstyle (for composition)
// ============================================================================

/// Re-export Style for users who want to compose custom styles
pub use anstyle::Style as AnstyleStyle;

// ============================================================================
// Semantic Style Constants
// ============================================================================

/// Error style (red) - use as `{ERROR}text{ERROR:#}`
pub const ERROR: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Red)));

// ============================================================================
// Message Emojis
// ============================================================================

/// Progress emoji - use with cyan style: `println!("{PROGRESS_EMOJI} {cyan}message{cyan:#}");`
pub const PROGRESS_EMOJI: &str = "🔄";

/// Success emoji - use with GREEN style: `println!("{SUCCESS_EMOJI} {GREEN}message{GREEN:#}");`
pub const SUCCESS_EMOJI: &str = "✅";

/// Error emoji - use with ERROR style: `println!("{ERROR_EMOJI} {ERROR}message{ERROR:#}");`
pub const ERROR_EMOJI: &str = "❌";

/// Warning emoji - use with WARNING style: `println!("{WARNING_EMOJI} {WARNING}message{WARNING:#}");`
pub const WARNING_EMOJI: &str = "🟡";

/// Hint emoji - use with HINT style: `println!("{HINT_EMOJI} {HINT}message{HINT:#}");`
pub const HINT_EMOJI: &str = "💡";

/// Warning style (yellow) - use as `{WARNING}text{WARNING:#}`
pub const WARNING: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));

/// Hint style (dimmed) - use as `{HINT}text{HINT:#}`
pub const HINT: Style = Style::new().dimmed();

/// Current worktree style (magenta + bold)
pub const CURRENT: Style = Style::new()
    .bold()
    .fg_color(Some(Color::Ansi(AnsiColor::Magenta)));

/// Addition style for diffs (green)
pub const ADDITION: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));

/// Deletion style for diffs (red)
pub const DELETION: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Red)));

/// Cyan style - use as `{CYAN}text{CYAN:#}`
pub const CYAN: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));

/// Cyan bold style - use as `{CYAN_BOLD}text{CYAN_BOLD:#}`
pub const CYAN_BOLD: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Cyan)))
    .bold();

/// Green style - use as `{GREEN}text{GREEN:#}`
pub const GREEN: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));

/// Green bold style - use as `{GREEN_BOLD}text{GREEN_BOLD:#}`
pub const GREEN_BOLD: Style = Style::new()
    .fg_color(Some(Color::Ansi(AnsiColor::Green)))
    .bold();

// ============================================================================
// Styled Output Types
// ============================================================================

/// A piece of text with an optional style
#[derive(Clone, Debug)]
pub struct StyledString {
    pub text: String,
    pub style: Option<Style>,
}

impl StyledString {
    pub fn new(text: impl Into<String>, style: Option<Style>) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    pub fn raw(text: impl Into<String>) -> Self {
        Self::new(text, None)
    }

    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self::new(text, Some(style))
    }

    /// Returns the visual width (unicode-aware, no ANSI codes)
    pub fn width(&self) -> usize {
        self.text.width()
    }

    /// Renders to a string with ANSI escape codes
    pub fn render(&self) -> String {
        if let Some(style) = &self.style {
            format!("{}{}{}", style.render(), self.text, style.render_reset())
        } else {
            self.text.clone()
        }
    }
}

/// A line composed of multiple styled strings
#[derive(Clone, Debug, Default)]
pub struct StyledLine {
    pub segments: Vec<StyledString>,
}

impl StyledLine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a raw (unstyled) segment
    pub fn push_raw(&mut self, text: impl Into<String>) {
        self.segments.push(StyledString::raw(text));
    }

    /// Add a styled segment
    pub fn push_styled(&mut self, text: impl Into<String>, style: Style) {
        self.segments.push(StyledString::styled(text, style));
    }

    /// Add a segment (StyledString)
    pub fn push(&mut self, segment: StyledString) {
        self.segments.push(segment);
    }

    /// Pad with spaces to reach a specific width
    pub fn pad_to(&mut self, target_width: usize) {
        let current_width = self.width();
        if current_width < target_width {
            self.push_raw(" ".repeat(target_width - current_width));
        }
    }

    /// Returns the total visual width
    pub fn width(&self) -> usize {
        self.segments.iter().map(|s| s.width()).sum()
    }

    /// Renders the entire line with ANSI escape codes
    pub fn render(&self) -> String {
        self.segments.iter().map(|s| s.render()).collect()
    }
}

// ============================================================================
// Gutter Formatting
// ============================================================================

/// Default terminal width fallback if detection fails
const DEFAULT_TERMINAL_WIDTH: usize = 80;

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
/// * `text` - The text to wrap
/// * `max_width` - Maximum width for each line
///
/// # Returns
/// A vector of wrapped lines
fn wrap_text_at_width(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }

    let text_width = text.width();

    // If the line fits, return it as-is
    if text_width <= max_width {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut current_line = String::new();
    let mut current_width = 0;

    for word in text.split_whitespace() {
        let word_width = word.width();

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

// ============================================================================
// Bash Syntax Highlighting
// ============================================================================

/// Maps bash token kinds to anstyle styles
///
/// Token names come from tree-sitter-bash's highlight queries.
/// Common tokens include:
/// - "function": commands like npm, git, cargo, echo, cd
/// - "keyword": bash keywords (if, then, for, while, do, done, etc.)
/// - "string": quoted strings
/// - "comment": hash-prefixed comments
/// - "operator": operators like &&, ||, |, $, -, etc.
/// - "constant": flags (arguments starting with -)
#[cfg(feature = "syntax-highlighting")]
fn bash_token_style(kind: &str) -> Option<Style> {
    match kind {
        // Commands (npm, git, cargo, echo, cd, etc.) - bold blue
        "function" => Some(
            Style::new()
                .fg_color(Some(Color::Ansi(AnsiColor::Blue)))
                .bold(),
        ),

        // Keywords (if, then, for, while, do, done, etc.) - bold magenta
        "keyword" => Some(
            Style::new()
                .fg_color(Some(Color::Ansi(AnsiColor::Magenta)))
                .bold(),
        ),

        // Strings (quoted values) - green
        "string" => Some(Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)))),

        // Comments (hash-prefixed) - dimmed
        "comment" => Some(Style::new().dimmed()),

        // Operators (&&, ||, |, $, -, >, <, etc.) - cyan
        "operator" => Some(Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)))),

        // Variables ($VAR, ${VAR}) - yellow
        "variable" => Some(Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)))),

        // Numbers - yellow
        "digit" | "number" => Some(Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)))),

        // Constants/flags (--flag, -f) - cyan
        "constant" => Some(Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)))),

        // Everything else (plain arguments, etc.)
        _ => None,
    }
}

// ============================================================================
// TOML Syntax Highlighting
// ============================================================================

/// Formats TOML content with syntax highlighting using synoptic
pub fn format_toml(content: &str, left_margin: &str) -> String {
    // Gutter style: subtle background for visual separation
    let gutter = Style::new().bg_color(Some(Color::Ansi(AnsiColor::Black)));

    // Get TOML highlighter from synoptic's built-in rules (tab_width = 4)
    let mut highlighter = match from_extension("toml", 4) {
        Some(h) => h,
        None => {
            // Fallback: return dimmed content if TOML highlighter not available
            let dim = Style::new().dimmed();
            let mut output = String::new();
            for line in content.lines() {
                output.push_str(&format!(
                    "{left_margin}{gutter} {gutter:#}  {dim}{line}{dim:#}\n"
                ));
            }
            return output;
        }
    };

    let mut output = String::new();
    let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();

    // Process all lines through the highlighter
    highlighter.run(&lines);

    // Render each line with appropriate styling
    for (y, line) in lines.iter().enumerate() {
        // Add left margin, gutter, and spacing
        output.push_str(&format!("{left_margin}{gutter} {gutter:#}  "));

        // Render each token with appropriate styling
        for token in highlighter.line(y, line) {
            match token {
                TokOpt::Some(text, kind) => {
                    let style = toml_token_style(&kind);
                    if let Some(s) = style {
                        output.push_str(&format!("{s}{text}{s:#}"));
                    } else {
                        output.push_str(&text);
                    }
                }
                TokOpt::None(text) => {
                    output.push_str(&text);
                }
            }
        }

        output.push('\n');
    }

    output
}

/// Maps TOML token kinds to anstyle styles
///
/// Token names come from synoptic's TOML highlighter:
/// - "string": quoted strings
/// - "comment": hash-prefixed comments
/// - "boolean": true/false values
/// - "table": table headers [...]
/// - "digit": numeric values
fn toml_token_style(kind: &str) -> Option<Style> {
    match kind {
        // Strings (quoted values)
        "string" => Some(Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)))),

        // Comments (hash-prefixed)
        "comment" => Some(Style::new().dimmed()),

        // Table headers [table] and [[array]]
        "table" => Some(
            Style::new()
                .fg_color(Some(Color::Ansi(AnsiColor::Cyan)))
                .bold(),
        ),

        // Booleans and numbers
        "boolean" | "digit" => Some(Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)))),

        // Everything else (operators, punctuation, keys)
        _ => None,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toml_formatting() {
        let toml_content = r#"worktree-path = "../{repo}.{branch}"

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
}
