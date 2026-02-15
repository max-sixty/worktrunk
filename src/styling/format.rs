//! Gutter formatting for quoted content
//!
//! Provides functions for formatting commands and configuration with visual gutters.

#[cfg(feature = "syntax-highlighting")]
use super::highlighting::bash_token_style;
#[cfg(feature = "syntax-highlighting")]
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

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
    // Normalize line endings: CRLF to LF, and trim trailing newlines.
    // Trailing newlines would create spurious blank gutter lines because
    // style restoration after newlines produces `\n[DIM]` which becomes
    // its own line when split.
    let content = content.replace("\r\n", "\n");
    let content = content.trim_end_matches('\n');

    // Replace Jinja template delimiters with identifier placeholders before parsing.
    // Tree-sitter can't parse `{{` and `}}` (especially when split across lines),
    // so we swap them out and restore after highlighting. Placeholders must NOT
    // contain quote characters — they break quote boundaries when adjacent to
    // existing quotes (e.g., `"{{ var }}"` → `""WTO" var "WTC""` with double quotes,
    // or `'text {{ var }}'` → `'text 'WTO'...'WTC''` with single quotes).
    //
    // TPL_CLOSE gets a trailing space so tree-sitter doesn't merge it with adjacent
    // path characters (e.g., `}}/path` → `WTC/path` would be one "function" token,
    // giving the path the wrong color). The space is stripped during restoration.
    const TPL_OPEN: &str = "WTO";
    const TPL_CLOSE: &str = "WTC";
    let normalized = content
        .replace("{{", TPL_OPEN)
        .replace("}}", &format!("{TPL_CLOSE} "));
    let content = normalized.as_str();

    let gutter = super::GUTTER;
    let reset = anstyle::Reset;
    let dim = anstyle::Style::new().dimmed();
    let string_style = bash_token_style("string").unwrap_or(dim);

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

    let mut config = HighlightConfiguration::new(
        bash_language,
        "bash",
        bash_highlights,
        "", // injections query
        "", // locals query
    )
    .expect("tree-sitter-bash HIGHLIGHT_QUERY should be valid");
    config.configure(&highlight_names);

    let mut highlighter = Highlighter::new();
    let highlights = highlighter
        .highlight(&config, content.as_bytes(), None, |_| None)
        .expect("highlighting valid UTF-8 should not fail");

    let content_bytes = content.as_bytes();

    // Phase 1: Build styled content with ANSI codes, restoring style after newlines.
    // Template placeholders are restored here (not in a separate phase) because we
    // need the active style context to correctly style `{{ }}` as green (string).
    // A post-hoc replace can't know whether the delimiter was inside a string
    // (inherits green) or bare (needs green injected).
    let mut styled = format!("{dim}");
    let mut pending_highlight: Option<usize> = None;
    let mut active_style: Option<anstyle::Style> = None;
    let mut ate_tpl_boundary = false;
    let close_with_space = format!("{TPL_CLOSE} ");

    for event in highlights {
        match event.unwrap() {
            HighlightEvent::Source { start, end } => {
                if let Ok(text) = std::str::from_utf8(&content_bytes[start..end]) {
                    // Strip boundary space inserted after TPL_CLOSE for token separation.
                    // The space may land in a separate Source event from TPL_CLOSE itself.
                    let text = if ate_tpl_boundary {
                        ate_tpl_boundary = false;
                        text.strip_prefix(' ').unwrap_or(text)
                    } else {
                        text
                    };

                    // Apply pending highlight style. Skip for pure placeholder tokens —
                    // tree-sitter sees e.g. `WTC` as a "function" but that's meaningless;
                    // placeholder restoration handles the styling.
                    let is_placeholder = text == TPL_CLOSE || text == TPL_OPEN;
                    if let Some(idx) = pending_highlight.take()
                        && let Some(name) = highlight_names.get(idx)
                        && let Some(style) = bash_token_style(name)
                        && !is_placeholder
                    {
                        styled.push_str(&format!("{reset}{style}"));
                        active_style = Some(style);
                    }

                    // Restore template placeholders with string styling for `{{ }}`.
                    // When already inside a string (green context), just replace the
                    // text — no ANSI injection needed since `{{ }}` inherits the style.
                    // Otherwise, inject string styling and restore the active context.
                    //
                    // Replace "WTC " before "WTC" to consume the boundary space when
                    // both land in the same Source event (e.g., inside a string token).
                    let has_placeholder = text.contains(TPL_OPEN) || text.contains(TPL_CLOSE);
                    let ends_with_close = text.ends_with(TPL_CLOSE);
                    let text = if !has_placeholder {
                        text.to_string()
                    } else if active_style == Some(string_style) {
                        text.replace(TPL_OPEN, "{{")
                            .replace(&close_with_space, "}}")
                            .replace(TPL_CLOSE, "}}")
                    } else {
                        let restore = format!("{reset}{}", active_style.unwrap_or(dim));
                        let close_repl = format!("{reset}{string_style}}}}}{restore}");
                        text.replace(TPL_OPEN, &format!("{reset}{string_style}{{{{{restore}"))
                            .replace(&close_with_space, &close_repl)
                            .replace(TPL_CLOSE, &close_repl)
                    };

                    if ends_with_close {
                        ate_tpl_boundary = true;
                    }

                    // Insert style restore after each newline so lines are self-contained
                    let style_restore = match active_style {
                        Some(style) => format!("{dim}{reset}{style}"),
                        None => format!("{dim}"),
                    };
                    styled.push_str(&text.replace('\n', &format!("\n{style_restore}")));
                }
            }
            HighlightEvent::HighlightStart(idx) => {
                pending_highlight = Some(idx.0);
            }
            HighlightEvent::HighlightEnd => {
                pending_highlight = None;
                active_style = None;
                styled.push_str(&format!("{reset}{dim}"));
            }
        }
    }

    // Phase 3: Split into lines, wrap each, add gutters
    styled
        .lines()
        .flat_map(|line| wrap_styled_text(line, available_width))
        .map(|wrapped| format!("{gutter} {gutter:#} {wrapped}{reset}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Formats bash/shell commands with syntax highlighting and gutter
///
/// Uses unified highlighting (entire command at once) to preserve context across
/// line breaks. Multi-line strings are correctly highlighted.
///
/// Template syntax (`{{ }}`) is detected to avoid misinterpreting Jinja variables
/// as shell commands.
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
        assert!(result.len() > 1);
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

    /// Regression test: tree-sitter 0.26 properly highlights multi-line commands.
    ///
    /// With tree-sitter-bash 0.25.1+, unified highlighting (processing the entire
    /// command at once) correctly identifies `&&` as operators even when they appear
    /// at line ends. This enables switching from line-by-line to unified highlighting.
    #[test]
    #[cfg(feature = "syntax-highlighting")]
    fn test_unified_multiline_highlighting() {
        use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

        let cmd = "echo 'line1' &&\necho 'line2' &&\necho 'line3'";

        let highlight_names = vec![
            "function", "keyword", "string", "operator", "comment", "number", "variable",
            "constant",
        ];

        let bash_language = tree_sitter_bash::LANGUAGE.into();
        let bash_highlights = tree_sitter_bash::HIGHLIGHT_QUERY;

        let mut config =
            HighlightConfiguration::new(bash_language, "bash", bash_highlights, "", "").unwrap();
        config.configure(&highlight_names);

        let mut highlighter = Highlighter::new();

        // Collect highlight events as tagged text for verification
        let mut output = String::new();
        let highlights = highlighter
            .highlight(&config, cmd.as_bytes(), None, |_| None)
            .unwrap();
        for event in highlights {
            match event.unwrap() {
                HighlightEvent::Source { start, end } => {
                    output.push_str(&cmd[start..end]);
                }
                HighlightEvent::HighlightStart(idx) => {
                    output.push_str(&format!("[{}:", highlight_names[idx.0]));
                }
                HighlightEvent::HighlightEnd => {
                    output.push(']');
                }
            }
        }

        // Unified highlighting should identify && as operators
        assert!(
            output.contains("[operator:&&]"),
            "Should identify && as operator in multi-line command"
        );

        // Should identify echo as function on each line
        assert_eq!(
            output.matches("[function:echo]").count(),
            3,
            "Should identify all three echo commands"
        );
    }

    /// Regression test: template variables inside quotes are restored correctly.
    ///
    /// When `{{ }}` appears inside double quotes (e.g., `"{{ target }}"`), the
    /// placeholder must not contain quote characters — otherwise tree-sitter
    /// reinterprets quote boundaries and ANSI codes break the contiguous
    /// placeholder, making the restore `.replace()` fail silently.
    #[test]
    #[cfg(feature = "syntax-highlighting")]
    fn test_template_vars_inside_quotes_restored() {
        use ansi_str::AnsiStr;

        let cmd = r#"if [ "{{ target }}" = "main" ]; then git pull && git push; fi"#;
        let result = format_bash_with_gutter_at_width(cmd, 120);

        let plain = result.ansi_strip();

        // The output must contain {{ and }}, not the placeholders
        assert!(plain.contains("{{"), "{plain}");
        assert!(plain.contains("}}"), "{plain}");
        assert!(!plain.contains("WTO"), "{plain}");
        assert!(!plain.contains("WTC"), "{plain}");
    }

    /// Regression test: template syntax ({{ }}) doesn't break highlighting.
    ///
    /// Tree-sitter parses around template variables, still identifying commands.
    #[test]
    #[cfg(feature = "syntax-highlighting")]
    fn test_highlighting_with_template_syntax() {
        use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

        let cmd = "echo {{ branch }} && mkdir {{ path }}";

        let highlight_names = vec!["function", "keyword", "string", "operator", "constant"];

        let bash_language = tree_sitter_bash::LANGUAGE.into();
        let bash_highlights = tree_sitter_bash::HIGHLIGHT_QUERY;

        let mut config =
            HighlightConfiguration::new(bash_language, "bash", bash_highlights, "", "").unwrap();
        config.configure(&highlight_names);

        let mut highlighter = Highlighter::new();

        let mut output = String::new();
        let highlights = highlighter
            .highlight(&config, cmd.as_bytes(), None, |_| None)
            .unwrap();
        for event in highlights {
            match event.unwrap() {
                HighlightEvent::Source { start, end } => {
                    output.push_str(&cmd[start..end]);
                }
                HighlightEvent::HighlightStart(idx) => {
                    output.push_str(&format!("[{}:", highlight_names[idx.0]));
                }
                HighlightEvent::HighlightEnd => {
                    output.push(']');
                }
            }
        }

        // Commands should still be identified despite template syntax
        assert!(
            output.contains("[function:echo]"),
            "Should identify echo despite template syntax"
        );
        assert!(
            output.contains("[function:mkdir]"),
            "Should identify mkdir despite template syntax"
        );
        assert!(
            output.contains("[operator:&&]"),
            "Should identify && operator"
        );
    }
}
