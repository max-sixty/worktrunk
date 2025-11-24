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
    // All tokens are dimmed for subdued appearance in gutter content.
    // Previously unstyled tokens (embedded, unknown) now get dimmed too -
    // this ensures the entire gutter block has consistent visual weight.
    let base = match kind {
        // Commands (npm, git, cargo, echo, cd, etc.) - bold blue
        "function" => Style::new()
            .fg_color(Some(Color::Ansi(AnsiColor::Blue)))
            .bold(),

        // Keywords (if, then, for, while, do, done, etc.) - bold magenta
        "keyword" => Style::new()
            .fg_color(Some(Color::Ansi(AnsiColor::Magenta)))
            .bold(),

        // Strings (quoted values) - green
        "string" => Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green))),

        // Operators (&&, ||, |, $, -, >, <, etc.) - cyan
        "operator" => Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan))),

        // Variables ($VAR, ${VAR}) - tree-sitter-bash 0.25 uses "property" not "variable"
        "property" => Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow))),

        // Numbers - yellow
        "number" => Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow))),

        // Constants/flags (--flag, -f) - cyan
        "constant" => Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan))),

        // Comments, embedded content, and everything else - no additional styling
        _ => Style::new(),
    };
    Some(base.dimmed())
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
