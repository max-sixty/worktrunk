//! Syntax highlighting for bash and TOML
//!
//! Provides token-to-style mappings for tree-sitter bash and synoptic TOML highlighting.

use anstyle::{AnsiColor, Color, Style};
use synoptic::{TokOpt, from_extension};

// ============================================================================
// Bash Syntax Highlighting
// ============================================================================

/// Maps bash token kinds to anstyle styles
///
/// Token names come from tree-sitter-bash 0.25's highlight queries.
/// Must match the @-names in highlights.scm:
/// - "function": commands (command_name nodes)
/// - "keyword": bash keywords (if, then, for, while, do, done, etc.)
/// - "string": quoted strings
/// - "comment": hash-prefixed comments
/// - "operator": operators (&&, ||, |, $, -, etc.)
/// - "property": variables (variable_name nodes)
/// - "constant": constants/flags
/// - "number": numeric values
/// - "embedded": embedded content
#[cfg(feature = "syntax-highlighting")]
pub(super) fn bash_token_style(kind: &str) -> Option<Style> {
    // All styles include .dimmed() so highlighted tokens match the dim base text.
    // We do NOT use .bold() because bold (SGR 1) and dim (SGR 2) are mutually
    // exclusive in some terminals like Alacritty - bold would cancel dim.
    match kind {
        // Commands (npm, git, cargo, echo, cd, etc.) - dim blue
        "function" => Some(
            Style::new()
                .fg_color(Some(Color::Ansi(AnsiColor::Blue)))
                .dimmed(),
        ),

        // Keywords (if, then, for, while, do, done, etc.) - dim magenta
        "keyword" => Some(
            Style::new()
                .fg_color(Some(Color::Ansi(AnsiColor::Magenta)))
                .dimmed(),
        ),

        // Strings (quoted values) - dim green
        "string" => Some(
            Style::new()
                .fg_color(Some(Color::Ansi(AnsiColor::Green)))
                .dimmed(),
        ),

        // Operators (&&, ||, |, $, -, >, <, etc.) - dim cyan
        "operator" => Some(
            Style::new()
                .fg_color(Some(Color::Ansi(AnsiColor::Cyan)))
                .dimmed(),
        ),

        // Variables ($VAR, ${VAR}) - tree-sitter-bash 0.25 uses "property" not "variable"
        "property" => Some(
            Style::new()
                .fg_color(Some(Color::Ansi(AnsiColor::Yellow)))
                .dimmed(),
        ),

        // Numbers - dim yellow
        "number" => Some(
            Style::new()
                .fg_color(Some(Color::Ansi(AnsiColor::Yellow)))
                .dimmed(),
        ),

        // Constants/flags (--flag, -f) - dim cyan
        "constant" => Some(
            Style::new()
                .fg_color(Some(Color::Ansi(AnsiColor::Cyan)))
                .dimmed(),
        ),

        // Comments, embedded content, and everything else - no styling (will use base dim)
        _ => None,
    }
}

// ============================================================================
// TOML Syntax Highlighting
// ============================================================================

/// Formats TOML content with syntax highlighting using synoptic
///
/// Returns formatted output without trailing newline (consistent with format_with_gutter
/// and format_bash_with_gutter).
pub fn format_toml(content: &str) -> String {
    // synoptic has built-in TOML support, so this always succeeds
    let mut highlighter = from_extension("toml", 4).expect("synoptic supports TOML");
    let gutter = super::GUTTER;
    let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();

    // Process all lines through the highlighter
    highlighter.run(&lines);

    // Render each line with gutter and appropriate styling
    // Build lines without trailing newline - caller is responsible for element separation
    let output_lines: Vec<String> = lines
        .iter()
        .enumerate()
        .map(|(y, line)| {
            let mut line_output = format!("{gutter} {gutter:#} ");

            for token in highlighter.line(y, line) {
                let (text, style) = match token {
                    TokOpt::Some(text, kind) => (text, toml_token_style(&kind)),
                    TokOpt::None(text) => (text, None),
                };

                if let Some(s) = style {
                    line_output.push_str(&format!("{s}{text}{s:#}"));
                } else {
                    line_output.push_str(&text);
                }
            }

            line_output
        })
        .collect();

    output_lines.join("\n")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "syntax-highlighting")]
    fn test_bash_token_style_function() {
        // Commands should be blue dimmed
        let style = bash_token_style("function");
        assert!(style.is_some());
        let s = style.unwrap();
        assert_eq!(s.get_fg_color(), Some(Color::Ansi(AnsiColor::Blue)));
    }

    #[test]
    #[cfg(feature = "syntax-highlighting")]
    fn test_bash_token_style_keyword() {
        // Keywords should be magenta dimmed
        let style = bash_token_style("keyword");
        assert!(style.is_some());
        let s = style.unwrap();
        assert_eq!(s.get_fg_color(), Some(Color::Ansi(AnsiColor::Magenta)));
    }

    #[test]
    #[cfg(feature = "syntax-highlighting")]
    fn test_bash_token_style_string() {
        // Strings should be green dimmed
        let style = bash_token_style("string");
        assert!(style.is_some());
        let s = style.unwrap();
        assert_eq!(s.get_fg_color(), Some(Color::Ansi(AnsiColor::Green)));
    }

    #[test]
    #[cfg(feature = "syntax-highlighting")]
    fn test_bash_token_style_operator() {
        // Operators should be cyan dimmed
        let style = bash_token_style("operator");
        assert!(style.is_some());
        let s = style.unwrap();
        assert_eq!(s.get_fg_color(), Some(Color::Ansi(AnsiColor::Cyan)));
    }

    #[test]
    #[cfg(feature = "syntax-highlighting")]
    fn test_bash_token_style_property() {
        // Variables (property) should be yellow dimmed
        let style = bash_token_style("property");
        assert!(style.is_some());
        let s = style.unwrap();
        assert_eq!(s.get_fg_color(), Some(Color::Ansi(AnsiColor::Yellow)));
    }

    #[test]
    #[cfg(feature = "syntax-highlighting")]
    fn test_bash_token_style_number() {
        // Numbers should be yellow dimmed
        let style = bash_token_style("number");
        assert!(style.is_some());
        let s = style.unwrap();
        assert_eq!(s.get_fg_color(), Some(Color::Ansi(AnsiColor::Yellow)));
    }

    #[test]
    #[cfg(feature = "syntax-highlighting")]
    fn test_bash_token_style_constant() {
        // Constants/flags should be cyan dimmed
        let style = bash_token_style("constant");
        assert!(style.is_some());
        let s = style.unwrap();
        assert_eq!(s.get_fg_color(), Some(Color::Ansi(AnsiColor::Cyan)));
    }

    #[test]
    #[cfg(feature = "syntax-highlighting")]
    fn test_bash_token_style_unknown() {
        // Unknown tokens should return None
        assert!(bash_token_style("unknown").is_none());
        assert!(bash_token_style("comment").is_none());
        assert!(bash_token_style("embedded").is_none());
    }

    #[test]
    fn test_toml_token_style_string() {
        // Strings should be green
        let style = toml_token_style("string");
        assert!(style.is_some());
        let s = style.unwrap();
        assert_eq!(s.get_fg_color(), Some(Color::Ansi(AnsiColor::Green)));
    }

    #[test]
    fn test_toml_token_style_comment() {
        // Comments should be dimmed
        let style = toml_token_style("comment");
        assert!(style.is_some());
    }

    #[test]
    fn test_toml_token_style_table() {
        // Table headers should be cyan bold
        let style = toml_token_style("table");
        assert!(style.is_some());
        let s = style.unwrap();
        assert_eq!(s.get_fg_color(), Some(Color::Ansi(AnsiColor::Cyan)));
    }

    #[test]
    fn test_toml_token_style_boolean() {
        // Booleans should be yellow
        let style = toml_token_style("boolean");
        assert!(style.is_some());
        let s = style.unwrap();
        assert_eq!(s.get_fg_color(), Some(Color::Ansi(AnsiColor::Yellow)));
    }

    #[test]
    fn test_toml_token_style_digit() {
        // Digits should be yellow
        let style = toml_token_style("digit");
        assert!(style.is_some());
        let s = style.unwrap();
        assert_eq!(s.get_fg_color(), Some(Color::Ansi(AnsiColor::Yellow)));
    }

    #[test]
    fn test_toml_token_style_unknown() {
        // Unknown tokens should return None
        assert!(toml_token_style("unknown").is_none());
        assert!(toml_token_style("key").is_none());
        assert!(toml_token_style("operator").is_none());
    }

    #[test]
    fn test_format_toml_basic() {
        let content = "[section]\nkey = \"value\"";
        let result = format_toml(content);
        // Should contain the original content (highlighted or not)
        assert!(result.contains("section"));
        assert!(result.contains("key"));
        assert!(result.contains("value"));
        // Should have multiple lines (one per input line)
        assert!(result.lines().count() >= 2);
    }

    #[test]
    fn test_format_toml_has_styled_and_unstyled_text() {
        // This test verifies that format_toml handles both styled tokens (string, table)
        // and unstyled text (TokOpt::None for whitespace, punctuation)
        use synoptic::{TokOpt, from_extension};

        let content = "key = \"value\"";
        let mut highlighter = from_extension("toml", 4).expect("synoptic supports TOML");
        let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
        highlighter.run(&lines);

        // Collect token types
        let mut has_styled = false;
        let mut has_unstyled = false;
        for (y, line) in lines.iter().enumerate() {
            for token in highlighter.line(y, line) {
                match token {
                    TokOpt::Some(_, kind) => {
                        if toml_token_style(&kind).is_some() {
                            has_styled = true;
                        }
                    }
                    TokOpt::None(_) => {
                        has_unstyled = true;
                    }
                }
            }
        }

        // Should have styled token (the string "value")
        assert!(has_styled, "Should have at least one styled token");
        // Should have unstyled text (whitespace, "=", "key")
        assert!(
            has_unstyled,
            "Should have at least one unstyled text segment"
        );
    }

    #[test]
    fn test_format_toml_multiline() {
        let content = "[table]\nkey1 = \"value1\"\nkey2 = 42\n# comment\nkey3 = false";
        let result = format_toml(content);
        // Each line should be present
        assert!(result.contains("table"));
        assert!(result.contains("key1"));
        assert!(result.contains("key2"));
        assert!(result.contains("key3"));
        assert!(result.contains("comment"));
    }

    #[test]
    fn test_format_toml_empty() {
        let content = "";
        let result = format_toml(content);
        // Empty content should produce empty output (or just newlines)
        assert!(result.is_empty() || result.trim().is_empty() || result == "\n");
    }
}
