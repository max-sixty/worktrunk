//! Minimal markdown rendering for CLI help text.

use anstyle::{AnsiColor, Color, Style};

/// Render markdown in help text to ANSI with minimal styling (green headers only)
pub fn render_markdown_in_help(help: &str) -> String {
    let green = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));
    let dimmed = Style::new().dimmed();

    let mut result = String::new();
    let mut in_code_block = false;

    for line in help.lines() {
        let trimmed = line.trim_start();

        // Track code block state
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            // Skip the fence markers themselves
            continue;
        }

        // Inside code blocks, render dimmed with indent
        if in_code_block {
            result.push_str(&format!("  {dimmed}{line}{dimmed:#}\n"));
            continue;
        }

        // Outside code blocks, render markdown headers
        if let Some(header_text) = trimmed.strip_prefix("## ") {
            result.push_str(&format!("{green}{header_text}{green:#}\n"));
        } else if let Some(header_text) = trimmed.strip_prefix("# ") {
            result.push_str(&format!("{green}{header_text}{green:#}\n"));
        } else {
            // Render inline formatting (bold, inline code)
            let formatted = render_inline_formatting(line);
            result.push_str(&formatted);
            result.push('\n');
        }
    }

    // Color status symbols to match their descriptions
    colorize_status_symbols(&result)
}

/// Render inline markdown formatting (bold, inline code)
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
        } else {
            result.push(ch);
        }
    }

    result
}

/// Add colors to status symbols in help text (matching wt list output colors)
fn colorize_status_symbols(text: &str) -> String {
    use anstyle::{AnsiColor, Color as AnsiStyleColor, Style};

    // Define semantic styles matching src/commands/list/model.rs StatusSymbols::render_with_mask
    let error = Style::new().fg_color(Some(AnsiStyleColor::Ansi(AnsiColor::Red)));
    let warning = Style::new().fg_color(Some(AnsiStyleColor::Ansi(AnsiColor::Yellow)));
    let hint = Style::new().dimmed();
    let success = Style::new().fg_color(Some(AnsiStyleColor::Ansi(AnsiColor::Green)));
    let progress = Style::new().fg_color(Some(AnsiStyleColor::Ansi(AnsiColor::Blue)));
    let disabled = Style::new().fg_color(Some(AnsiStyleColor::Ansi(AnsiColor::BrightBlack)));
    let working_tree = Style::new().fg_color(Some(AnsiStyleColor::Ansi(AnsiColor::Cyan)));

    text
        // CI status circles
        .replace("● passed", &format!("{success}●{success:#} passed"))
        .replace("● running", &format!("{progress}●{progress:#} running"))
        .replace("● failed", &format!("{error}●{error:#} failed"))
        .replace("● conflicts", &format!("{warning}●{warning:#} conflicts"))
        .replace("● no-ci", &format!("{disabled}●{disabled:#} no-ci"))
        // Conflicts: ✖ is ERROR (red), ⚠ is WARNING (yellow)
        .replace(
            "✖ Merge conflicts",
            &format!("{error}✖{error:#} Merge conflicts"),
        )
        .replace(
            "⚠ Would conflict",
            &format!("{warning}⚠{warning:#} Would conflict"),
        )
        // Git operations: WARNING (yellow)
        .replace("↻ Rebase", &format!("{warning}↻{warning:#} Rebase"))
        .replace("⋈ Merge", &format!("{warning}⋈{warning:#} Merge"))
        // Worktree attributes: WARNING (yellow)
        .replace("⊠ Locked", &format!("{warning}⊠{warning:#} Locked"))
        .replace("⚠ Prunable", &format!("{warning}⚠{warning:#} Prunable"))
        // Branch state: HINT (dimmed)
        .replace(
            "≡ Working tree matches",
            &format!("{hint}≡{hint:#} Working tree matches"),
        )
        .replace("∅ No commits", &format!("{hint}∅{hint:#} No commits"))
        .replace(
            "· Branch without",
            &format!("{hint}·{hint:#} Branch without"),
        )
        // Main/upstream divergence: NO COLOR (plain text in actual output)
        // ↑, ↓, ↕, ⇡, ⇣, ⇅ remain uncolored
        // Working tree changes: CYAN
        .replace(
            "? Untracked",
            &format!("{working_tree}?{working_tree:#} Untracked"),
        )
        .replace(
            "! Modified",
            &format!("{working_tree}!{working_tree:#} Modified"),
        )
        .replace(
            "+ Staged",
            &format!("{working_tree}+{working_tree:#} Staged"),
        )
        .replace(
            "» Renamed",
            &format!("{working_tree}»{working_tree:#} Renamed"),
        )
        .replace(
            "✘ Deleted",
            &format!("{working_tree}✘{working_tree:#} Deleted"),
        )
}
