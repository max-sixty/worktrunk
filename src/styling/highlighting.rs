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
pub fn format_toml(content: &str, left_margin: &str) -> String {
    let gutter = super::GUTTER;

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
        let result = format_toml(content, "");
        // Should contain the original content (highlighted or not)
        assert!(result.contains("section"));
        assert!(result.contains("key"));
        assert!(result.contains("value"));
        // Should have multiple lines (one per input line)
        assert!(result.lines().count() >= 2);
    }

    #[test]
    fn test_format_toml_with_margin() {
        let content = "key = true";
        let result = format_toml(content, "  ");
        // Should have margin prefix
        assert!(result.starts_with("  "));
        assert!(result.contains("key"));
        assert!(result.contains("true"));
    }

    #[test]
    fn test_format_toml_multiline() {
        let content = "[table]\nkey1 = \"value1\"\nkey2 = 42\n# comment\nkey3 = false";
        let result = format_toml(content, "");
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
        let result = format_toml(content, "");
        // Empty content should produce empty output (or just newlines)
        assert!(result.is_empty() || result.trim().is_empty() || result == "\n");
    }
}
