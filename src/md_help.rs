//! Minimal markdown rendering for CLI help text.

use anstyle::{AnsiColor, Color, Color as AnsiStyleColor, Style};
use crossterm::style::Attribute;
use termimad::{CompoundStyle, MadSkin, TableBorderChars};
use unicode_width::UnicodeWidthStr;

use worktrunk::styling::{
    DEFAULT_HELP_WIDTH, format_bash_with_gutter, format_toml, format_with_gutter, wrap_styled_text,
};

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
    // Render backtick-enclosed text as dimmed, matching render_inline_formatting().
    // This is needed for colorize_status_symbols() to find and recolor symbols
    // like `●` that appear in table cells.
    skin.inline_code = CompoundStyle::with_attr(Attribute::Dim);
    skin
}

/// Render markdown in help text to ANSI with minimal styling (green headers only)
///
/// If `width` is provided, prose text is wrapped to that width. Tables, code blocks,
/// and headers are never wrapped (tables need full-width rows for alignment).
pub(crate) fn render_markdown_in_help_with_width(help: &str, width: Option<usize>) -> String {
    let green = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));

    let mut result = String::new();
    let mut in_code_block = false;
    let mut code_block_lang = String::new();
    let mut code_block_lines: Vec<&str> = Vec::new();
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
        if let Some(after_fence) = trimmed.strip_prefix("```") {
            if !in_code_block {
                // Opening fence — extract language identifier
                code_block_lang = after_fence.trim().to_string();
                code_block_lines.clear();
                in_code_block = true;
            } else {
                // Closing fence — render collected code block with gutter
                let content = code_block_lines.join("\n");
                let formatted = match code_block_lang.as_str() {
                    "toml" => format_toml(&content),
                    "console" | "bash" | "sh" => format_bash_with_gutter(&content),
                    _ => {
                        // Dim the content before adding gutter (format_with_gutter
                        // doesn't style text; bash/toml formatters handle their own)
                        let dim = Style::new().dimmed();
                        let dimmed = code_block_lines
                            .iter()
                            .map(|l| format!("{dim}{l}{dim:#}"))
                            .collect::<Vec<_>>()
                            .join("\n");
                        format_with_gutter(&dimmed, None)
                    }
                };
                result.push_str(&formatted);
                result.push('\n');
                in_code_block = false;
            }
            i += 1;
            continue;
        }

        // Inside code blocks, collect lines for deferred rendering
        if in_code_block {
            code_block_lines.push(line);
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

/// Render a markdown table using termimad (for help text, no indent)
fn render_table(lines: &[&str], max_width: Option<usize>) -> String {
    render_table_with_termimad(lines, "", max_width)
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
    use insta::assert_snapshot;

    /// Test helper: render markdown without prose wrapping
    fn render_markdown_in_help(help: &str) -> String {
        render_markdown_in_help_with_width(help, None)
    }

    // ============================================================================
    // strip_markdown_links / unescape_table_pipes (exact transformations)
    // ============================================================================

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
    fn test_unescape_table_pipes() {
        assert_eq!(unescape_table_pipes(r"a \| b"), "a | b");
        assert_eq!(
            unescape_table_pipes(r"\| start \| end \|"),
            "| start | end |"
        );
        assert_eq!(unescape_table_pipes("no pipes here"), "no pipes here");
        assert_eq!(unescape_table_pipes("a | b"), "a | b");
    }

    // ============================================================================
    // render_inline_formatting (ANSI styling)
    // ============================================================================

    #[test]
    fn test_render_inline_formatting_styles() {
        // Inline code
        let code = render_inline_formatting("`code`");
        assert_snapshot!(code, @"[2mcode[0m");

        // Bold
        let bold = render_inline_formatting("**bold**");
        assert_snapshot!(bold, @"[1mbold[0m");

        // Nested: code inside bold
        let bold_code = render_inline_formatting("**`wt list`:**");
        assert_snapshot!(bold_code, @"[1m[2mwt list[0m:[0m");

        // Mixed formatting
        let mixed = render_inline_formatting("text `code` more **bold** end");
        assert_snapshot!(mixed, @"text [2mcode[0m more [1mbold[0m end");

        // Backticks inside link text
        let link_code = render_inline_formatting("See [`wt hook`](@/hook.md) for details");
        assert_snapshot!(link_code, @"See [2mwt hook[0m for details");

        // Unclosed backtick
        let unclosed_code = render_inline_formatting("`unclosed");
        assert_snapshot!(unclosed_code, @"[2munclosed[0m");

        // Unclosed bold
        let unclosed_bold = render_inline_formatting("**unclosed");
        assert_snapshot!(unclosed_bold, @"[1munclosed[0m");
    }

    // ============================================================================
    // render_markdown_in_help
    // ============================================================================

    #[test]
    fn test_render_markdown_in_help_headers() {
        // All header levels in one snapshot to show the visual hierarchy:
        // H1: UPPERCASE green, H2: bold green, H3: green, H4: bold
        let md = "# Title\n## Section\n### Subsection\n#### Nested";
        let result = render_markdown_in_help(md);
        assert_snapshot!(result, @"
        [32mTITLE[0m
        [1m[32mSection[0m
        [32mSubsection[0m
        [1mNested[0m
        ");
    }

    #[test]
    fn test_render_markdown_in_help_horizontal_rule() {
        let result = render_markdown_in_help("before\n\n---\n\n## Section");
        assert_snapshot!(result, @"
        before

        [2m────────────────────────────────────────[0m

        [1m[32mSection[0m
        ");
    }

    #[test]
    fn test_render_markdown_in_help_code_block() {
        let result = render_markdown_in_help("```\ncode here\n```\nafter");
        assert_snapshot!(result, @"
        [107m [0m [2mcode here[0m
        after
        ");
    }

    #[test]
    fn test_render_markdown_in_help_toml_code_block() {
        let result = render_markdown_in_help("```toml\n[section]\nkey = \"value\"\n```\nafter");
        assert_snapshot!(result, @r#"
        [107m [0m [2m[36m[section][0m
        [107m [0m [2mkey = [0m[2m[32m"value"[0m
        after
        "#);
    }

    #[test]
    fn test_render_markdown_in_help_html_comment() {
        let result = render_markdown_in_help("<!-- comment -->\nvisible");
        assert_snapshot!(result, @"visible");
    }

    #[test]
    fn test_render_markdown_in_help_plain_text() {
        let result = render_markdown_in_help("Just plain text");
        assert_snapshot!(result, @"Just plain text");
    }

    #[test]
    fn test_render_markdown_in_help_table() {
        let result = render_markdown_in_help("| A | B |\n| - | - |\n| 1 | 2 |");
        assert_snapshot!(result, @"
         A   B  
        ─── ─── 
        1   2
        ");
    }

    // ============================================================================
    // render_markdown_table
    // ============================================================================

    #[test]
    fn test_render_markdown_table_basic() {
        let result = render_markdown_table("| Col1 | Col2 |\n| ---- | ---- |\n| A | B |");
        assert_snapshot!(result, @"
        Col1 Col2 
        ──── ──── 
        A    B
        ");
    }

    #[test]
    fn test_render_markdown_table_empty() {
        let result = render_markdown_table("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_render_markdown_table_with_non_table_lines() {
        let result =
            render_markdown_table("Not a table\n| A | B |\nAlso not\n| - | - |\n| 1 | 2 |");
        assert_snapshot!(result, @"
         A   B  
        ─── ─── 
        1   2
        ");
    }

    // ============================================================================
    // colorize_status_symbols
    // ============================================================================

    #[test]
    fn test_colorize_status_symbols() {
        let dim = Style::new().dimmed();

        // Working tree symbols → cyan
        let working_tree = colorize_status_symbols(&format!("{dim}+{dim:#} staged"));
        assert_snapshot!(working_tree, @"[36m+[0m staged");

        // Conflicts → red
        let conflicts = colorize_status_symbols(&format!("{dim}✘{dim:#} conflicts"));
        assert_snapshot!(conflicts, @"[31m✘[0m conflicts");

        // Git operations → yellow
        let git_ops = colorize_status_symbols(&format!("{dim}⤴{dim:#} rebase"));
        assert_snapshot!(git_ops, @"[33m⤴[0m rebase");

        // CI status: passed → green, failed → red, running → blue
        let ci_passed = colorize_status_symbols("● passed");
        assert_snapshot!(ci_passed, @"[32m●[0m passed");

        let ci_failed = colorize_status_symbols("● failed");
        assert_snapshot!(ci_failed, @"[31m●[0m failed");

        let ci_running = colorize_status_symbols("● running");
        assert_snapshot!(ci_running, @"[34m●[0m running");
    }

    #[test]
    fn test_colorize_status_symbols_no_change() {
        let input = "plain text here";
        let result = colorize_status_symbols(input);
        assert_eq!(result, input);
    }

    // ============================================================================
    // render_table
    // ============================================================================

    #[test]
    fn test_render_table_escaped_pipe() {
        let lines = vec![
            "| Category | Symbol | Meaning |",
            "| --- | --- | --- |",
            r"| Remote | `\|` | In sync |",
        ];
        let result = render_table(&lines, None);
        assert_snapshot!(result, @"
        Category Symbol Meaning 
        ──────── ────── ─────── 
        Remote   [2m|[0m      In sync
        ");
    }

    #[test]
    fn test_render_table_column_alignment() {
        let lines = vec![
            "| Short | LongerHeader |",
            "| ----- | ------------ |",
            "| A | B |",
        ];
        let result = render_table(&lines, None);
        assert_snapshot!(result, @"
        Short LongerHeader 
        ───── ──────────── 
        A     B
        ");
    }

    #[test]
    fn test_render_table_uneven_columns() {
        let lines = vec!["| A | B | C |", "| --- | --- | --- |", "| 1 | 2 |"];
        let result = render_table(&lines, None);
        assert_snapshot!(result, @"
         A   B   C  
        ─── ─── ─── 
        1   2
        ");
    }

    #[test]
    fn test_render_table_no_separator() {
        let lines = vec!["| A | B |", "| 1 | 2 |"];
        let result = render_table(&lines, None);
        assert_snapshot!(result, @"
        A   B  
        1   2
        ");
    }

    #[test]
    fn test_render_markdown_in_help_table_wrapping() {
        let help = r#"### Other environment variables

| Variable | Purpose |
|----------|---------|
| `WORKTRUNK_BIN` | Override binary path for shell wrappers (useful for testing dev builds) |
| WORKTRUNK_CONFIG_PATH | Override user config file location |
| `WORKTRUNK_MAX_CONCURRENT_COMMANDS` | Max parallel git commands (default: 32). Lower if hitting resource limits. |
| NO_COLOR | Disable colored output (standard) |
"#;
        let result = render_markdown_in_help_with_width(help, Some(80));
        assert_snapshot!(result, @"
        [32mOther environment variables[0m

                     Variable                                Purpose                    
         ───────────────────────────────── ──────────────────────────────────────────── 
         [2mWORKTRUNK_BIN[0m                     Override binary path for shell wrappers      
                                           (useful for testing dev builds)              
         WORKTRUNK_CONFIG_PATH             Override user config file location           
         [2mWORKTRUNK_MAX_CONCURRENT_COMMANDS[0m Max parallel git commands (default: 32).     
                                           Lower if hitting resource limits.            
         NO_COLOR                          Disable colored output (standard)
        ");
    }
}
