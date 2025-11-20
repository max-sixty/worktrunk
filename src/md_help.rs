//! Markdown rendering for CLI help text using termimad.

use termimad::{MadSkin, crossterm::style::Color};

/// Render markdown in help text to ANSI with minimal styling (green headers only)
pub fn render_markdown_in_help(help: &str) -> String {
    let mut skin = MadSkin::no_style();
    skin.headers[0].set_fg(Color::Green);
    skin.headers[1].set_fg(Color::Green);

    let width = terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(80);

    let rendered = format!("{}", skin.text(help, Some(width)));

    // Color the CI status circles to match their descriptions
    colorize_status_circles(&rendered)
}

/// Add colors to status symbols in help text (matching wt list output colors)
fn colorize_status_circles(text: &str) -> String {
    use anstyle::{AnsiColor, Color as AnsiStyleColor, Style};

    // Define colors matching src/commands/list/model.rs StatusSymbols::render_with_mask
    let error = Style::new().fg_color(Some(AnsiStyleColor::Ansi(AnsiColor::Red))); // ERROR
    let warning = Style::new().fg_color(Some(AnsiStyleColor::Ansi(AnsiColor::Yellow))); // WARNING
    let cyan = Style::new().fg_color(Some(AnsiStyleColor::Ansi(AnsiColor::Cyan))); // CYAN
    let dim = Style::new().dimmed(); // HINT
    let green = Style::new().fg_color(Some(AnsiStyleColor::Ansi(AnsiColor::Green)));
    let blue = Style::new().fg_color(Some(AnsiStyleColor::Ansi(AnsiColor::Blue)));
    let gray = Style::new().fg_color(Some(AnsiStyleColor::Ansi(AnsiColor::BrightBlack)));

    text
        // CI status circles
        .replace("● passed", &format!("{green}●{green:#} passed"))
        .replace("● running", &format!("{blue}●{blue:#} running"))
        .replace("● failed", &format!("{error}●{error:#} failed"))
        .replace("● conflicts", &format!("{warning}●{warning:#} conflicts"))
        .replace("● no-ci", &format!("{gray}●{gray:#} no-ci"))
        // Conflicts: = is ERROR (red), ≠ is WARNING (yellow)
        .replace(
            "= Merge conflicts",
            &format!("{error}={error:#} Merge conflicts"),
        )
        .replace(
            "≠ Potential conflicts",
            &format!("{warning}≠{warning:#} Potential conflicts"),
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
            &format!("{dim}≡{dim:#} Working tree matches"),
        )
        .replace("∅ No commits", &format!("{dim}∅{dim:#} No commits"))
        .replace("· Branch without", &format!("{dim}·{dim:#} Branch without"))
        // Main/upstream divergence: NO COLOR (plain text in actual output)
        // ↑, ↓, ↕, ⇡, ⇣, ⇅ remain uncolored
        // Working tree changes: CYAN
        .replace("? Untracked", &format!("{cyan}?{cyan:#} Untracked"))
        .replace("! Modified", &format!("{cyan}!{cyan:#} Modified"))
        .replace("+ Staged", &format!("{cyan}+{cyan:#} Staged"))
        .replace("» Renamed", &format!("{cyan}»{cyan:#} Renamed"))
        .replace("✘ Deleted", &format!("{cyan}✘{cyan:#} Deleted"))
}
