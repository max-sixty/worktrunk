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
    let dim = Style::new().dimmed();
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
                    // Unstyled tokens (keys, operators, whitespace) rendered dim
                    line_output.push_str(&format!("{dim}{text}{dim:#}"));
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
    // All styles include .dimmed() so highlighted tokens match the dim base text,
    // consistent with bash_token_style(). We do NOT use .bold() because bold (SGR 1)
    // and dim (SGR 2) are mutually exclusive in some terminals like Alacritty.
    match kind {
        // Strings (quoted values) - dim green
        "string" => Some(
            Style::new()
                .fg_color(Some(Color::Ansi(AnsiColor::Green)))
                .dimmed(),
        ),

        // Comments (hash-prefixed) - dim (no color, just subdued)
        "comment" => Some(Style::new().dimmed()),

        // Table headers [table] and [[array]] - dim cyan
        "table" => Some(
            Style::new()
                .fg_color(Some(Color::Ansi(AnsiColor::Cyan)))
                .dimmed(),
        ),

        // Booleans and numbers - dim yellow
        "boolean" | "digit" => Some(
            Style::new()
                .fg_color(Some(Color::Ansi(AnsiColor::Yellow)))
                .dimmed(),
        ),

        // Everything else (operators, punctuation, keys) - no styling (will use base dim)
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use super::*;

    /// Each bash token type maps to the expected color.
    #[test]
    #[cfg(feature = "syntax-highlighting")]
    fn test_bash_token_styles() {
        let cases = [
            ("function", AnsiColor::Blue),
            ("keyword", AnsiColor::Magenta),
            ("string", AnsiColor::Green),
            ("operator", AnsiColor::Cyan),
            ("constant", AnsiColor::Cyan),
            ("property", AnsiColor::Yellow),
            ("number", AnsiColor::Yellow),
        ];
        for (name, expected_color) in cases {
            let style =
                bash_token_style(name).unwrap_or_else(|| panic!("{name} should have a style"));
            assert_eq!(
                style.get_fg_color(),
                Some(Color::Ansi(expected_color)),
                "{name} should be {expected_color:?}"
            );
        }
        // Unknown/unstyled tokens return None
        assert!(bash_token_style("unknown").is_none());
        assert!(bash_token_style("comment").is_none());
        assert!(bash_token_style("embedded").is_none());
    }

    /// Each TOML token type maps to the expected color.
    #[test]
    fn test_toml_token_styles() {
        let cases = [
            ("string", AnsiColor::Green),
            ("table", AnsiColor::Cyan),
            ("boolean", AnsiColor::Yellow),
            ("digit", AnsiColor::Yellow),
        ];
        for (name, expected_color) in cases {
            let style =
                toml_token_style(name).unwrap_or_else(|| panic!("{name} should have a style"));
            assert_eq!(
                style.get_fg_color(),
                Some(Color::Ansi(expected_color)),
                "{name} should be {expected_color:?}"
            );
        }
        // Comments have a style but no specific color (dimmed)
        assert!(toml_token_style("comment").is_some());
        // Unknown tokens return None
        assert!(toml_token_style("unknown").is_none());
        assert!(toml_token_style("key").is_none());
        assert!(toml_token_style("operator").is_none());
    }

    /// format_toml produces highlighted, guttered output for various inputs.
    #[test]
    fn test_format_toml() {
        assert_snapshot!(format_toml("[section]\nkey = \"value\""), @r#"
        [107m [0m [2m[36m[section][0m
        [107m [0m [2mkey = [0m[2m[32m"value"[0m
        "#);
        assert_snapshot!(
            format_toml("[table]\nkey1 = \"value1\"\nkey2 = 42\n# comment\nkey3 = false"),
            @r#"
        [107m [0m [2m[36m[table][0m
        [107m [0m [2mkey1 = [0m[2m[32m"value1"[0m
        [107m [0m [2mkey2 = [0m[2m[33m42[0m
        [107m [0m [2m# comment[0m
        [107m [0m [2mkey3 = [0m[2m[33mfalse[0m
        "#
        );
        assert_snapshot!(format_toml(""), @"");
    }

    /// format_toml handles both styled tokens (string, table) and unstyled text.
    #[test]
    fn test_format_toml_has_styled_and_unstyled_text() {
        use synoptic::{TokOpt, from_extension};

        let content = "key = \"value\"";
        let mut highlighter = from_extension("toml", 4).expect("synoptic supports TOML");
        let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
        highlighter.run(&lines);

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

        assert!(has_styled, "Should have at least one styled token");
        assert!(
            has_unstyled,
            "Should have at least one unstyled text segment"
        );
    }
}
