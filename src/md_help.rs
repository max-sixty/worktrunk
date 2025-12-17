//! Minimal markdown rendering for CLI help text.

use anstyle::{AnsiColor, Color, Style};
use unicode_width::UnicodeWidthStr;

use worktrunk::styling::wrap_styled_text;

/// Render markdown in help text to ANSI without prose wrapping
#[cfg(test)]
fn render_markdown_in_help(help: &str) -> String {
    render_markdown_in_help_with_width(help, None)
}

/// Render markdown in help text to ANSI with minimal styling (green headers only)
///
/// If `width` is provided, prose text is wrapped to that width. Tables, code blocks,
/// and headers are never wrapped (tables need full-width rows for alignment).
pub fn render_markdown_in_help_with_width(help: &str, width: Option<usize>) -> String {
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

        // Handle code fences - check for ```table specially
        if trimmed.starts_with("```") {
            if trimmed == "```table" || trimmed.starts_with("```table ") {
                // Table code fence - collect lines until closing ```
                i += 1; // Skip opening fence
                let mut table_content: Vec<String> = Vec::new();
                while i < lines.len() {
                    let tl = lines[i].trim();
                    if tl == "```" {
                        i += 1; // Skip closing fence
                        break;
                    }
                    table_content.push(lines[i].to_string());
                    i += 1;
                }
                // Convert pipe-delimited format to markdown table format
                let md_lines: Vec<String> = table_content
                    .iter()
                    .enumerate()
                    .flat_map(|(idx, line)| {
                        let md_line = format!("|{}|", line.replace(" | ", "|"));
                        if idx == 0 {
                            // Add separator row after header
                            let cols = line.split(" | ").count();
                            let sep = format!("|{}|", vec!["---"; cols].join("|"));
                            vec![md_line, sep]
                        } else {
                            vec![md_line]
                        }
                    })
                    .collect();
                let md_refs: Vec<&str> = md_lines.iter().map(|s| s.as_str()).collect();
                result.push_str(&render_table(&md_refs, width));
                continue;
            } else {
                // Regular code block
                in_code_block = !in_code_block;
                i += 1;
                continue;
            }
        }

        // Inside code blocks, render dimmed with indent
        if in_code_block {
            result.push_str(&format!("  {dimmed}{line}{dimmed:#}\n"));
            i += 1;
            continue;
        }

        // Detect markdown table rows (legacy format, still supported)
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

        // Outside code blocks, render markdown headers (never wrapped)
        if let Some(header_text) = trimmed.strip_prefix("### ") {
            let bold = Style::new().bold();
            result.push_str(&format!("{bold}{header_text}{bold:#}\n"));
        } else if let Some(header_text) = trimmed.strip_prefix("## ") {
            result.push_str(&format!("{green}{header_text}{green:#}\n"));
        } else if let Some(header_text) = trimmed.strip_prefix("# ") {
            result.push_str(&format!("{green}{header_text}{green:#}\n"));
        } else {
            // Prose text - wrap if width is specified
            let formatted = render_inline_formatting(line);
            if let Some(w) = width {
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

/// Render a markdown table with proper column alignment (for help text, adds 2-space indent)
fn render_table(lines: &[&str], max_width: Option<usize>) -> String {
    render_markdown_table_impl(lines, "  ", max_width)
}

/// Render a markdown table from markdown source string (no indent)
pub fn render_markdown_table(markdown: &str) -> String {
    let lines: Vec<&str> = markdown
        .lines()
        .filter(|l| l.trim().starts_with('|') && l.trim().ends_with('|'))
        .collect();
    render_markdown_table_impl(&lines, "", None)
}

/// Core table rendering with configurable indent and optional width constraint
///
/// If `max_width` is specified and the table exceeds it, the last column wraps
/// to fit. Continuation lines are indented to align with the column start.
fn render_markdown_table_impl(lines: &[&str], indent: &str, max_width: Option<usize>) -> String {
    // Parse table cells
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut separator_idx: Option<usize> = None;

    // Placeholder for escaped pipes (use a character sequence unlikely to appear)
    const ESCAPED_PIPE_PLACEHOLDER: &str = "\x00PIPE\x00";

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        // Remove leading/trailing pipes and split
        let inner = trimmed.trim_start_matches('|').trim_end_matches('|');
        // Replace escaped pipes before splitting, then restore after
        let inner_escaped = inner.replace("\\|", ESCAPED_PIPE_PLACEHOLDER);
        let cells: Vec<String> = inner_escaped
            .split('|')
            .map(|s| s.trim().replace(ESCAPED_PIPE_PLACEHOLDER, "|").to_string())
            .collect();

        // Check if this is the separator row (contains only dashes and colons)
        if cells
            .iter()
            .all(|c| c.chars().all(|ch| ch == '-' || ch == ':'))
        {
            separator_idx = Some(idx);
        } else {
            rows.push(cells);
        }
    }

    if rows.is_empty() {
        return String::new();
    }

    // Calculate column widths (using display width for Unicode)
    let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut col_widths: Vec<usize> = vec![0; num_cols];

    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if i < num_cols {
                // Apply inline formatting to measure rendered width
                let formatted = render_inline_formatting(cell);
                let display_width = strip_ansi(&formatted).width();
                col_widths[i] = col_widths[i].max(display_width);
            }
        }
    }

    // Calculate total table width and adjust last column if needed
    let indent_width = indent.width();
    let separators_width = (num_cols.saturating_sub(1)) * 2; // 2 spaces between columns
    let total_width: usize = indent_width + col_widths.iter().sum::<usize>() + separators_width;

    // If we have a width constraint and table exceeds it, shrink last column
    let last_col_wrap_width = if let Some(max_w) = max_width {
        if total_width > max_w && num_cols > 0 {
            let overflow = total_width - max_w;
            let last_col_natural = col_widths[num_cols - 1];
            // Minimum width for last column (don't go below 20 chars)
            let min_last_col = 20;
            let new_last_col = last_col_natural.saturating_sub(overflow).max(min_last_col);
            col_widths[num_cols - 1] = new_last_col;
            Some(new_last_col)
        } else {
            None
        }
    } else {
        None
    };

    // Calculate continuation indent (for wrapped last column)
    // This is the position where the last column starts
    let continuation_indent: usize = indent_width
        + col_widths[..num_cols.saturating_sub(1)]
            .iter()
            .sum::<usize>()
        + separators_width;

    // Render rows
    let mut result = String::new();
    let has_header = separator_idx.is_some();

    for (row_idx, row) in rows.iter().enumerate() {
        // Format all cells and potentially wrap the last one
        let mut formatted_cells: Vec<Vec<String>> = Vec::new();

        for (col_idx, cell) in row.iter().enumerate() {
            let formatted = render_inline_formatting(cell);
            let is_last_col = col_idx == num_cols - 1;

            if let (true, Some(wrap_width)) = (is_last_col, last_col_wrap_width) {
                // Wrap the last column if needed
                let wrapped = wrap_styled_text(&formatted, wrap_width);
                formatted_cells.push(wrapped);
            } else {
                formatted_cells.push(vec![formatted]);
            }
        }

        // Determine the maximum number of lines in any cell (for multi-line rows)
        let max_lines = formatted_cells.iter().map(|c| c.len()).max().unwrap_or(1);

        for line_idx in 0..max_lines {
            if line_idx == 0 {
                result.push_str(indent);
            } else {
                // Continuation line: indent to last column position
                for _ in 0..continuation_indent {
                    result.push(' ');
                }
            }

            for (col_idx, cell_lines) in formatted_cells.iter().enumerate() {
                let is_last_col = col_idx == num_cols - 1;

                if line_idx == 0 && col_idx > 0 {
                    result.push_str("  "); // Column separator
                }

                // Skip non-last columns on continuation lines
                if line_idx > 0 && !is_last_col {
                    continue;
                }

                let cell_content = cell_lines.get(line_idx).map(|s| s.as_str()).unwrap_or("");
                let display_width = strip_ansi(cell_content).width();
                let col_width = col_widths.get(col_idx).unwrap_or(&0);
                let padding = col_width.saturating_sub(display_width);

                result.push_str(cell_content);

                // Add padding (except for last column on last line of cell)
                let is_last_line_of_cell = line_idx == cell_lines.len().saturating_sub(1);
                if !is_last_col || !is_last_line_of_cell {
                    for _ in 0..padding {
                        result.push(' ');
                    }
                }
            }
            result.push('\n');
        }

        // Add visual separator after header row
        if has_header && row_idx == 0 {
            result.push_str(indent);
            for (col_idx, width) in col_widths.iter().enumerate() {
                if col_idx > 0 {
                    result.push_str("  ");
                }
                for _ in 0..*width {
                    result.push('─');
                }
            }
            result.push('\n');
        }
    }

    result
}

/// Strip ANSI escape codes for width calculation
fn strip_ansi(s: &str) -> String {
    let mut result = String::new();
    let mut in_escape = false;

    for ch in s.chars() {
        if ch == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if ch == 'm' {
                in_escape = false;
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Render inline markdown formatting (bold, inline code, links)
fn render_inline_formatting(line: &str) -> String {
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
            result.push_str(&format!("{bold}{bold_content}{bold:#}"));
        } else if ch == '[' {
            // Markdown link: [text](url) -> render just text
            // Non-links like [text] or [text are preserved literally
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
                // Render just the link text
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

    // Worktree state: PathMismatch (red), Prunable/Locked (yellow)
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

    #[test]
    fn test_render_inline_formatting_strips_links() {
        assert_eq!(render_inline_formatting("[text](url)"), "text");
        assert_eq!(
            render_inline_formatting("See [wt hook](@/hook.md) for details"),
            "See wt hook for details"
        );
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
    fn test_render_table_escaped_pipe() {
        // In markdown tables, \| represents a literal pipe character
        let lines = vec![
            "| Category | Symbol | Meaning |",
            "| --- | --- | --- |",
            "| Remote | `\\|` | In sync |",
        ];
        let result = render_table(&lines, None);
        // The \| should be rendered as | (pipe character)
        assert!(result.contains("|"), "Escaped pipe should render as |");
        assert!(
            !result.contains("\\|"),
            "Escaped sequence should not appear literally"
        );
    }

    // ============================================================================
    // strip_ansi Tests
    // ============================================================================

    #[test]
    fn test_strip_ansi_no_escapes() {
        assert_eq!(strip_ansi("plain text"), "plain text");
    }

    #[test]
    fn test_strip_ansi_with_color() {
        assert_eq!(strip_ansi("\u{1b}[32mgreen\u{1b}[0m"), "green");
    }

    #[test]
    fn test_strip_ansi_multiple_codes() {
        assert_eq!(
            strip_ansi("\u{1b}[1mbold\u{1b}[0m and \u{1b}[2mdim\u{1b}[0m"),
            "bold and dim"
        );
    }

    #[test]
    fn test_strip_ansi_nested() {
        assert_eq!(
            strip_ansi("\u{1b}[1m\u{1b}[32mtext\u{1b}[0m\u{1b}[0m"),
            "text"
        );
    }

    // ============================================================================
    // render_markdown_in_help Tests
    // ============================================================================

    #[test]
    fn test_render_markdown_in_help_h1() {
        let result = render_markdown_in_help("# Header");
        // H1 should be green
        assert!(result.contains("Header"));
        assert!(result.contains("\u{1b}[")); // Has color codes
    }

    #[test]
    fn test_render_markdown_in_help_h2() {
        let result = render_markdown_in_help("## Section");
        assert!(result.contains("Section"));
        assert!(result.contains("\u{1b}[")); // Has color codes
    }

    #[test]
    fn test_render_markdown_in_help_h3() {
        let result = render_markdown_in_help("### Subsection");
        assert!(result.contains("Subsection"));
        // H3 is bold
        assert!(result.contains("\u{1b}[1m")); // Bold
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

    #[test]
    fn test_render_markdown_in_help_table_code_fence() {
        // New ```table format - simpler than markdown tables
        let md = "```table\nA | B\n1 | 2\n```";
        let result = render_markdown_in_help(md);
        // Table should be rendered the same as markdown format
        assert!(result.contains("A"));
        assert!(result.contains("B"));
        assert!(result.contains("1"));
        assert!(result.contains("2"));
        // Should have separator line
        assert!(result.contains("─"));
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
}
