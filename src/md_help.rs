//! Minimal markdown rendering for CLI help text.

use anstyle::{AnsiColor, Color, Style};
use termimad::{MadSkin, TableBorderChars};
use unicode_width::UnicodeWidthStr;

use worktrunk::styling::{DEFAULT_HELP_WIDTH, wrap_styled_text};

/// Table border style matching our help text format:
/// - Horizontal lines under headers with spaces between column segments
/// - No vertical borders
static HELP_TABLE_BORDERS: TableBorderChars = TableBorderChars {
    horizontal: '─',
    vertical: ' ',
    top_left_corner: ' ',
    top_right_corner: ' ',
    bottom_right_corner: ' ',
    bottom_left_corner: ' ',
    top_junction: ' ',
    right_junction: ' ',
    bottom_junction: ' ',
    left_junction: ' ',
    cross: ' ', // Space at intersections gives separate line segments
};

/// Create a termimad skin for help text tables
fn help_table_skin() -> MadSkin {
    let mut skin = MadSkin::no_style();
    skin.table_border_chars = &HELP_TABLE_BORDERS;
    skin
}

/// Render markdown in help text to ANSI with minimal styling (green headers only)
///
/// If `width` is provided, prose text is wrapped to that width. Tables, code blocks,
/// and headers are never wrapped (tables need full-width rows for alignment).
pub(crate) fn render_markdown_in_help_with_width(help: &str, width: Option<usize>) -> String {
    let green = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));
    let dimmed = Style::new().dimmed();

    let mut result = String::new();
    let mut in_code_block = false;
    let mut table_lines: Vec<&str> = Vec::new();

    let lines: Vec<&str> = help.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();

        // Skip HTML comments (expansion markers for web docs, see readme_sync.rs)
        if trimmed.starts_with("<!--") && trimmed.ends_with("-->") {
            i += 1;
            continue;
        }

        // Handle code fences
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            i += 1;
            continue;
        }

        // Inside code blocks, render dimmed with indent
        if in_code_block {
            result.push_str(&format!("  {dimmed}{line}{dimmed:#}\n"));
            i += 1;
            continue;
        }

        // Detect markdown table rows
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            // Collect all consecutive table lines
            table_lines.clear();
            while i < lines.len() {
                let tl = lines[i].trim_start();
                if tl.starts_with('|') && tl.ends_with('|') {
                    table_lines.push(lines[i]);
                    i += 1;
                } else {
                    break;
                }
            }
            // Render the table, wrapping to fit terminal width if specified
            result.push_str(&render_table(&table_lines, width));
            continue;
        }

        // Horizontal rules (---, ***, ___) render as visible divider
        // No extra newlines - markdown source already has blank lines around ---
        //
        // TODO: We use `---` dividers instead of H1 headers because H1s break web docs
        // (pages already have a title from frontmatter). This decouples visual hierarchy
        // from heading semantics. Alternatives considered:
        // - Strip H1s during doc sync (demote to H2 for web)
        // - Treat `---` + H2 combo as "major section" (render H2 as UPPERCASE when preceded by ---)
        // - Use marker comments like `<!-- major -->` before H2
        // See git history for discussion.
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            let dimmed = Style::new().dimmed();
            let rule_width = width.unwrap_or(40);
            let rule: String = "─".repeat(rule_width);
            result.push_str(&format!("{dimmed}{rule}{dimmed:#}\n"));
            i += 1;
            continue;
        }

        // Outside code blocks, render markdown headers (never wrapped)
        // Visual hierarchy: H1 > H2 > H3 > H4
        // - H1: UPPERCASE green (most prominent, rarely used)
        // - H2: Bold green (major sections like "Examples", "Columns")
        // - H3: Normal green (subsections like "CI status", "commit object")
        // - H4: Bold (nested subsections like "Commit template")
        if let Some(header_text) = trimmed.strip_prefix("#### ") {
            let bold = Style::new().bold();
            result.push_str(&format!("{bold}{header_text}{bold:#}\n"));
        } else if let Some(header_text) = trimmed.strip_prefix("### ") {
            result.push_str(&format!("{green}{header_text}{green:#}\n"));
        } else if let Some(header_text) = trimmed.strip_prefix("## ") {
            let bold_green = Style::new()
                .bold()
                .fg_color(Some(Color::Ansi(AnsiColor::Green)));
            result.push_str(&format!("{bold_green}{header_text}{bold_green:#}\n"));
        } else if let Some(header_text) = trimmed.strip_prefix("# ") {
            result.push_str(&format!("{green}{}{green:#}\n", header_text.to_uppercase()));
        } else {
            // Prose text - wrap if width is specified
            let formatted = render_inline_formatting(line);
            if let Some(w) = width {
                // wrap_styled_text preserves leading indentation on continuation lines
                for wrapped_line in wrap_styled_text(&formatted, w) {
                    result.push_str(&wrapped_line);
                    result.push('\n');
                }
            } else {
                result.push_str(&formatted);
                result.push('\n');
            }
        }
        i += 1;
    }

    // Color status symbols to match their descriptions
    colorize_status_symbols(&result)
}

/// Render a markdown table using termimad (for help text, adds 2-space indent)
fn render_table(lines: &[&str], max_width: Option<usize>) -> String {
    render_table_with_termimad(lines, "  ", max_width)
}

/// Render a markdown table from markdown source string (no indent)
pub(crate) fn render_markdown_table(markdown: &str) -> String {
    let lines: Vec<&str> = markdown
        .lines()
        .filter(|l| l.trim().starts_with('|') && l.trim().ends_with('|'))
        .collect();
    render_table_with_termimad(&lines, "", None)
}

/// Render a markdown table using termimad
///
/// Termimad handles column width calculation, cell wrapping, and alignment.
fn render_table_with_termimad(lines: &[&str], indent: &str, max_width: Option<usize>) -> String {
    if lines.is_empty() {
        return String::new();
    }

    // Preprocess lines to strip markdown links and unescape pipes
    // (termimad doesn't handle either)
    let processed: Vec<String> = lines
        .iter()
        .map(|line| unescape_table_pipes(&strip_markdown_links(line)))
        .collect();
    let markdown = processed.join("\n");

    // Determine width for termimad (subtract indent)
    let width = max_width
        .map(|w| w.saturating_sub(indent.width()))
        .unwrap_or(DEFAULT_HELP_WIDTH);

    let skin = help_table_skin();
    let rendered = skin.text(&markdown, Some(width)).to_string();

    // Add indent to each line
    if indent.is_empty() {
        rendered
    } else {
        rendered
            .lines()
            .map(|line| format!("{indent}{line}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }
}

/// Unescape pipe characters in markdown table cells: `\|` -> `|`
///
/// In markdown tables, `|` is the column delimiter. To include a literal pipe
/// character inside a cell, you escape it as `\|`. Termimad doesn't handle this
/// escape sequence, so we preprocess it.
fn unescape_table_pipes(line: &str) -> String {
    line.replace(r"\|", "|")
}

/// Strip markdown links, keeping only the link text: `[text](url)` -> `text`
///
/// Limitation: Links in clap help text may be broken across lines by clap's wrapping
/// before this function runs. The simple fix (setting `cmd.term_width(0)` to disable
/// clap's wrapping) doesn't work because clap provides proper indentation for option
/// description continuation lines — our `wrap_styled_text` would lose this alignment.
///
/// To support arbitrary markdown links in `--help`, we'd need to split help output at
/// `find_after_help_start()`, keep clap's wrapped Options section, get raw after_long_help
/// via `cmd.get_after_long_help()`, process it ourselves, then combine. This requires
/// restructuring since `cmd` is consumed by `try_get_matches_from_mut`.
///
/// Current workaround: Use plain URLs in cli.rs (terminals auto-link `https://...`),
/// transform to markdown links for web docs in `colorize_ci_status_for_html()`.
fn strip_markdown_links(line: &str) -> String {
    let mut result = String::new();
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '[' {
            // Potential markdown link
            let mut link_text = String::new();
            let mut found_close = false;
            let mut bracket_depth = 0;

            for c in chars.by_ref() {
                if c == '[' {
                    bracket_depth += 1;
                    link_text.push(c);
                } else if c == ']' {
                    if bracket_depth == 0 {
                        found_close = true;
                        break;
                    }
                    bracket_depth -= 1;
                    link_text.push(c);
                } else {
                    link_text.push(c);
                }
            }

            if found_close && chars.peek() == Some(&'(') {
                chars.next(); // consume '('
                // Skip URL until closing ')'
                for c in chars.by_ref() {
                    if c == ')' {
                        break;
                    }
                }
                // Output just the link text
                result.push_str(&link_text);
            } else {
                // Not a valid link, output literally
                result.push('[');
                result.push_str(&link_text);
                if found_close {
                    result.push(']');
                }
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Render inline markdown formatting (bold, inline code, links)
fn render_inline_formatting(line: &str) -> String {
    // First strip links, preserving link text (which may contain bold/code)
    let line = strip_markdown_links(line);

    let bold = Style::new().bold();
    let code = Style::new().dimmed();

    let mut result = String::new();
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '`' {
            // Inline code
            let mut code_content = String::new();
            for c in chars.by_ref() {
                if c == '`' {
                    break;
                }
                code_content.push(c);
            }
            result.push_str(&format!("{code}{code_content}{code:#}"));
        } else if ch == '*' && chars.peek() == Some(&'*') {
            // Bold
            chars.next(); // consume second *
            let mut bold_content = String::new();
            while let Some(c) = chars.next() {
                if c == '*' && chars.peek() == Some(&'*') {
                    chars.next(); // consume closing **
                    break;
                }
                bold_content.push(c);
            }
            // Recursively process inline formatting within bold content
            let processed_content = render_inline_formatting(&bold_content);
            result.push_str(&format!("{bold}{processed_content}{bold:#}"));
        } else {
            result.push(ch);
        }
    }

    result
}

/// Add colors to status symbols in help text (matching wt list output colors)
fn colorize_status_symbols(text: &str) -> String {
    use anstyle::{AnsiColor, Color as AnsiStyleColor, Style};

    // Define semantic styles matching src/commands/list/model.rs StatusSymbols::styled_symbols
    let error = Style::new().fg_color(Some(AnsiStyleColor::Ansi(AnsiColor::Red)));
    let warning = Style::new().fg_color(Some(AnsiStyleColor::Ansi(AnsiColor::Yellow)));
    let success = Style::new().fg_color(Some(AnsiStyleColor::Ansi(AnsiColor::Green)));
    let progress = Style::new().fg_color(Some(AnsiStyleColor::Ansi(AnsiColor::Blue)));
    let disabled = Style::new().fg_color(Some(AnsiStyleColor::Ansi(AnsiColor::BrightBlack)));
    let working_tree = Style::new().fg_color(Some(AnsiStyleColor::Ansi(AnsiColor::Cyan)));

    // Pattern for dimmed text (from inline `code` rendering)
    // render_inline_formatting wraps backticked text in dimmed style
    let dim = Style::new().dimmed();

    // Helper to create dimmed symbol pattern and its colored replacement
    let replace_dim = |text: String, sym: &str, style: Style| -> String {
        let dimmed = format!("{dim}{sym}{dim:#}");
        let colored = format!("{style}{sym}{style:#}");
        text.replace(&dimmed, &colored)
    };

    let mut result = text.to_string();

    // Working tree symbols: CYAN
    result = replace_dim(result, "+", working_tree);
    result = replace_dim(result, "!", working_tree);
    result = replace_dim(result, "?", working_tree);

    // Conflicts: ERROR (red)
    result = replace_dim(result, "✘", error);

    // Git operations, MergeTreeConflicts: WARNING (yellow)
    result = replace_dim(result, "⤴", warning);
    result = replace_dim(result, "⤵", warning);
    result = replace_dim(result, "✗", warning);

    // Worktree state: BranchWorktreeMismatch (red), Prunable/Locked (yellow)
    result = replace_dim(result, "⚑", error);
    result = replace_dim(result, "⊟", warning);
    result = replace_dim(result, "⊞", warning);

    // CI status circles: replace dimmed ● followed by color name
    let dimmed_bullet = format!("{dim}●{dim:#}");
    result = result
        .replace(
            &format!("{dimmed_bullet} green"),
            &format!("{success}●{success:#} green"),
        )
        .replace(
            &format!("{dimmed_bullet} blue"),
            &format!("{progress}●{progress:#} blue"),
        )
        .replace(
            &format!("{dimmed_bullet} red"),
            &format!("{error}●{error:#} red"),
        )
        .replace(
            &format!("{dimmed_bullet} yellow"),
            &format!("{warning}●{warning:#} yellow"),
        )
        .replace(
            &format!("{dimmed_bullet} gray"),
            &format!("{disabled}●{disabled:#} gray"),
        )
        // CI error indicator: ⚠ symbol (also rendered dimmed initially)
        .replace(
            &format!("{dim}⚠{dim:#} yellow"),
            &format!("{warning}⚠{warning:#} yellow"),
        );

    // Legacy CI status circles (for statusline format)
    result = result
        .replace("● passed", &format!("{success}●{success:#} passed"))
        .replace("● running", &format!("{progress}●{progress:#} running"))
        .replace("● failed", &format!("{error}●{error:#} failed"))
        .replace("● conflicts", &format!("{warning}●{warning:#} conflicts"))
        .replace("● no-ci", &format!("{disabled}●{disabled:#} no-ci"));

    // Symbols that should remain dimmed are already dimmed from backtick rendering:
    // - Main state: _ (same commit), ⊂ (content integrated), ^, ↑, ↓, ↕
    // - Upstream divergence: |, ⇡, ⇣, ⇅
    // - Worktree state: / (branch without worktree)

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper: render markdown without prose wrapping
    fn render_markdown_in_help(help: &str) -> String {
        render_markdown_in_help_with_width(help, None)
    }

    #[test]
    fn test_render_inline_formatting_strips_links() {
        assert_eq!(render_inline_formatting("[text](url)"), "text");
        assert_eq!(
            render_inline_formatting("See [wt hook](@/hook.md) for details"),
            "See wt hook for details"
        );
    }

    #[test]
    fn test_render_inline_formatting_backticks_in_link() {
        // Backticks inside link text should be preserved and rendered as code
        let result = render_inline_formatting("See [`wt hook`](@/hook.md) for details");
        // Should contain dimmed "wt hook" (code style)
        assert!(result.contains("\u{1b}[2mwt hook\u{1b}[0m"));
        assert!(result.contains("See "));
        assert!(result.contains(" for details"));
    }

    #[test]
    fn test_render_inline_formatting_nested_brackets() {
        assert_eq!(
            render_inline_formatting("[text [with brackets]](url)"),
            "text [with brackets]"
        );
    }

    #[test]
    fn test_render_inline_formatting_multiple_links() {
        assert_eq!(render_inline_formatting("[a](b) and [c](d)"), "a and c");
    }

    #[test]
    fn test_render_inline_formatting_malformed_links() {
        // Missing URL - preserved literally
        assert_eq!(render_inline_formatting("[text]"), "[text]");
        // Unclosed bracket - preserved literally
        assert_eq!(render_inline_formatting("[text"), "[text");
        // Not followed by ( - preserved literally
        assert_eq!(render_inline_formatting("[text] more"), "[text] more");
    }

    #[test]
    fn test_render_inline_formatting_preserves_bold_and_code() {
        assert_eq!(
            render_inline_formatting("**bold** and `code`"),
            "\u{1b}[1mbold\u{1b}[0m and \u{1b}[2mcode\u{1b}[0m"
        );
    }

    #[test]
    fn test_unescape_table_pipes() {
        // Basic conversion
        assert_eq!(unescape_table_pipes(r"a \| b"), "a | b");
        // Multiple escapes
        assert_eq!(
            unescape_table_pipes(r"\| start \| end \|"),
            "| start | end |"
        );
        // No escapes - unchanged
        assert_eq!(unescape_table_pipes("no pipes here"), "no pipes here");
        // Regular pipe - unchanged
        assert_eq!(unescape_table_pipes("a | b"), "a | b");
    }

    #[test]
    fn test_render_table_escaped_pipe() {
        // In markdown tables, \| represents a literal pipe character
        // We preprocess to convert \| to | before sending to termimad
        let lines = vec![
            "| Category | Symbol | Meaning |",
            "| --- | --- | --- |",
            r"| Remote | `\|` | In sync |",
        ];
        let result = render_table(&lines, None);
        // Table should render with the content
        assert!(
            result.contains("Remote"),
            "Table should contain cell content"
        );
        assert!(
            result.contains("In sync"),
            "Table should contain cell content"
        );
        // The escaped pipe should be converted to a literal pipe
        assert!(result.contains('|'));
        assert!(!result.contains(r"\|"));
    }

    // ============================================================================
    // render_markdown_in_help Tests
    // ============================================================================

    #[test]
    fn test_render_markdown_in_help_h1() {
        let result = render_markdown_in_help("# Header");
        // H1 should be UPPERCASE green
        assert!(result.contains("HEADER")); // Uppercase
        assert!(result.contains("\u{1b}[32m")); // Green
    }

    #[test]
    fn test_render_markdown_in_help_h2() {
        let result = render_markdown_in_help("## Section");
        // H2 should be bold green (anstyle emits separate codes)
        assert!(result.contains("Section"));
        assert!(result.contains("\u{1b}[1m")); // Bold
        assert!(result.contains("\u{1b}[32m")); // Green
    }

    #[test]
    fn test_render_markdown_in_help_h3() {
        let result = render_markdown_in_help("### Subsection");
        // H3 should be green (no bold)
        assert!(result.contains("Subsection"));
        assert!(result.contains("\u{1b}[32m")); // Green
        assert!(!result.contains("\u{1b}[1m")); // Not bold
    }

    #[test]
    fn test_render_markdown_in_help_h4() {
        let result = render_markdown_in_help("#### Nested");
        // H4 should be bold (no color)
        assert!(result.contains("Nested"));
        assert!(result.contains("\u{1b}[1m")); // Bold
        assert!(!result.contains("\u{1b}[32m")); // Not green
    }

    #[test]
    fn test_render_markdown_in_help_horizontal_rule() {
        let result = render_markdown_in_help("before\n\n---\n\n## Section");
        // Horizontal rule becomes visible divider line
        assert!(!result.contains("---"));
        assert!(result.contains("────────────────────────────────────────"));
        assert!(result.contains("before"));
        assert!(result.contains("Section"));
    }

    #[test]
    fn test_render_markdown_in_help_code_block() {
        let md = "```\ncode here\n```\nafter";
        let result = render_markdown_in_help(md);
        // Code is dimmed with indent
        assert!(result.contains("code here"));
        assert!(result.contains("after"));
    }

    #[test]
    fn test_render_markdown_in_help_html_comment() {
        let md = "<!-- comment -->\nvisible";
        let result = render_markdown_in_help(md);
        // Comments should be stripped
        assert!(!result.contains("comment"));
        assert!(result.contains("visible"));
    }

    #[test]
    fn test_render_markdown_in_help_plain_text() {
        let result = render_markdown_in_help("Just plain text");
        assert!(result.contains("Just plain text"));
    }

    #[test]
    fn test_render_markdown_in_help_table() {
        let md = "| A | B |\n| - | - |\n| 1 | 2 |";
        let result = render_markdown_in_help(md);
        // Table should be rendered
        assert!(result.contains("A"));
        assert!(result.contains("B"));
        assert!(result.contains("1"));
        assert!(result.contains("2"));
    }

    // ============================================================================
    // render_markdown_table Tests
    // ============================================================================

    #[test]
    fn test_render_markdown_table_basic() {
        let md = "| Col1 | Col2 |\n| ---- | ---- |\n| A | B |";
        let result = render_markdown_table(md);
        assert!(result.contains("Col1"));
        assert!(result.contains("Col2"));
        assert!(result.contains("A"));
        assert!(result.contains("B"));
    }

    #[test]
    fn test_render_markdown_table_empty() {
        let result = render_markdown_table("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_render_markdown_table_with_non_table_lines() {
        let md = "Not a table\n| A | B |\nAlso not\n| - | - |\n| 1 | 2 |";
        let result = render_markdown_table(md);
        // Should only include table rows
        assert!(result.contains("A"));
        assert!(result.contains("B"));
        assert!(!result.contains("Not a table"));
        assert!(!result.contains("Also not"));
    }

    // ============================================================================
    // colorize_status_symbols Tests
    // ============================================================================

    #[test]
    fn test_colorize_status_symbols_working_tree() {
        // These symbols should become cyan
        let dim = Style::new().dimmed();
        let input = format!("{}+{dim:#} staged", dim);
        let result = colorize_status_symbols(&input);
        // Should have cyan color code (36)
        assert!(result.contains("\u{1b}[36m+"));
    }

    #[test]
    fn test_colorize_status_symbols_conflicts() {
        // ✘ should become red
        let dim = Style::new().dimmed();
        let input = format!("{}✘{dim:#} conflicts", dim);
        let result = colorize_status_symbols(&input);
        // Should have red color code (31)
        assert!(result.contains("\u{1b}[31m✘"));
    }

    #[test]
    fn test_colorize_status_symbols_git_ops() {
        // ⤴ and ⤵ should become yellow
        let dim = Style::new().dimmed();
        let input = format!("{}⤴{dim:#} rebase", dim);
        let result = colorize_status_symbols(&input);
        // Should have yellow color code (33)
        assert!(result.contains("\u{1b}[33m⤴"));
    }

    #[test]
    fn test_colorize_status_symbols_ci_green() {
        let result = colorize_status_symbols("● passed");
        // Should have green color (32)
        assert!(result.contains("\u{1b}[32m●"));
    }

    #[test]
    fn test_colorize_status_symbols_ci_red() {
        let result = colorize_status_symbols("● failed");
        // Should have red color (31)
        assert!(result.contains("\u{1b}[31m●"));
    }

    #[test]
    fn test_colorize_status_symbols_ci_running() {
        let result = colorize_status_symbols("● running");
        // Should have blue color (34)
        assert!(result.contains("\u{1b}[34m●"));
    }

    #[test]
    fn test_colorize_status_symbols_no_change() {
        // Text without symbols should pass through unchanged
        let input = "plain text here";
        let result = colorize_status_symbols(input);
        assert_eq!(result, input);
    }

    // ============================================================================
    // render_inline_formatting Tests
    // ============================================================================

    #[test]
    fn test_render_inline_formatting_inline_code() {
        let result = render_inline_formatting("`code`");
        // Should have dim escape codes
        assert!(result.contains("code"));
        assert!(result.contains("\u{1b}[2m")); // Dimmed
    }

    #[test]
    fn test_render_inline_formatting_bold() {
        let result = render_inline_formatting("**bold**");
        assert!(result.contains("bold"));
        assert!(result.contains("\u{1b}[1m")); // Bold
    }

    #[test]
    fn test_render_inline_formatting_bold_with_code() {
        // Nested formatting: **`wt list`:** should render code inside bold
        let result = render_inline_formatting("**`wt list`:**");
        assert!(result.contains("wt list"));
        assert!(result.contains("\u{1b}[1m")); // Bold
        assert!(result.contains("\u{1b}[2m")); // Dimmed (for code)
        assert!(!result.contains('`')); // Backticks should be consumed
    }

    #[test]
    fn test_render_inline_formatting_mixed() {
        let result = render_inline_formatting("text `code` more **bold** end");
        assert!(result.contains("text"));
        assert!(result.contains("code"));
        assert!(result.contains("more"));
        assert!(result.contains("bold"));
        assert!(result.contains("end"));
    }

    #[test]
    fn test_render_inline_formatting_unclosed_code() {
        // Unclosed backtick - should consume until end
        let result = render_inline_formatting("`unclosed");
        assert!(result.contains("unclosed"));
    }

    #[test]
    fn test_render_inline_formatting_unclosed_bold() {
        // Unclosed bold - should consume until end
        let result = render_inline_formatting("**unclosed");
        assert!(result.contains("unclosed"));
    }

    // ============================================================================
    // render_markdown_table_impl Tests (via render_table)
    // ============================================================================

    #[test]
    fn test_render_table_column_alignment() {
        let lines = vec![
            "| Short | LongerHeader |",
            "| ----- | ------------ |",
            "| A | B |",
        ];
        let result = render_table(&lines, None);
        // Should have proper column alignment
        assert!(result.contains("Short"));
        assert!(result.contains("LongerHeader"));
        // Should have separator line with ─
        assert!(result.contains('─'));
    }

    #[test]
    fn test_render_table_uneven_columns() {
        let lines = vec!["| A | B | C |", "| --- | --- | --- |", "| 1 | 2 |"];
        let result = render_table(&lines, None);
        // Should handle rows with different column counts
        assert!(result.contains("A"));
        assert!(result.contains("1"));
    }

    #[test]
    fn test_render_table_no_separator() {
        // Table without separator row
        let lines = vec!["| A | B |", "| 1 | 2 |"];
        let result = render_table(&lines, None);
        // Should still render, just without separator line
        assert!(result.contains("A"));
        assert!(result.contains("1"));
        // Should NOT have separator line
        assert!(!result.contains('─'));
    }

    #[test]
    fn test_render_markdown_in_help_table_wrapping() {
        // Test the full render_markdown_in_help_with_width function
        // which is what actually runs on the help text
        let help = r#"### Other environment variables

| Variable | Purpose |
|----------|---------|
| `WORKTRUNK_BIN` | Override binary path for shell wrappers (useful for testing dev builds) |
| WORKTRUNK_CONFIG_PATH | Override user config file location |
| `WORKTRUNK_MAX_CONCURRENT_COMMANDS` | Max parallel git commands (default: 32). Lower if hitting resource limits. |
| NO_COLOR | Disable colored output (standard) |
"#;
        let rendered = render_markdown_in_help_with_width(help, Some(80));

        // Check for pipe characters
        for line in rendered.lines() {
            assert!(
                !line.trim_start().starts_with("| "),
                "Line should not start with '| ': {:?}",
                line
            );
            assert!(
                !line.trim_end().ends_with(" |"),
                "Line should not end with ' |': {:?}",
                line
            );
        }
    }
}
