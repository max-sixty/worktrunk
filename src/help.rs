//! Help system with pager support and web documentation generation.
//!
//! This module provides:
//! - Pager support for `--help` output (git-style)
//! - Markdown rendering for help text
//! - Web documentation generation via `--help-page` and `--help-md`

use std::process;

use ansi_str::AnsiStr;
use clap::ColorChoice;
use clap::error::ErrorKind;
use worktrunk::styling::eprintln;

use crate::cli;

/// Custom help handling for pager support and markdown rendering.
///
/// We intercept help requests to provide:
/// 1. **Pager support**: Long help (`--help`) shown through pager, short (`-h`) prints directly
/// 2. **Markdown rendering**: `## Headers` become green, code blocks are dimmed
///
/// This follows git's convention:
/// - `-h` never opens a pager (short help, muscle-memory safe)
/// - `--help` opens a pager when content doesn't fit (via less -F flag)
///
/// Uses `Error::render()` to get clap's pre-formatted help, which already
/// respects `-h` (short) vs `--help` (long) distinction.
///
/// Returns `true` if help was handled (caller should exit), `false` to continue normal parsing.
pub fn maybe_handle_help_with_pager() -> bool {
    let args: Vec<String> = std::env::args().collect();

    // --help uses pager, -h prints directly (git convention)
    let use_pager = args.iter().any(|a| a == "--help");

    // Check for --help-page flag (output full doc page with frontmatter)
    if args.iter().any(|a| a == "--help-page") {
        handle_help_page(&args);
        process::exit(0);
    }

    // Check for --help-md flag (output raw markdown without ANSI rendering)
    if args.iter().any(|a| a == "--help-md") {
        let mut cmd = cli::build_command();
        cmd = cmd.color(ColorChoice::Never); // No ANSI codes for raw markdown

        // Replace --help-md with --help for clap
        let filtered_args: Vec<String> = args
            .iter()
            .map(|a| {
                if a == "--help-md" {
                    "--help".to_string()
                } else {
                    a.clone()
                }
            })
            .collect();

        if let Err(err) = cmd.try_get_matches_from_mut(filtered_args)
            && matches!(
                err.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            )
        {
            // Transform code block languages for Zola compatibility:
            // - ```text (clap's default for usage) -> ``` (no highlighting)
            // - ```console (our examples) -> ```bash
            let output = err
                .render()
                .to_string()
                .replace("```text\n", "```\n")
                .replace("```console\n", "```bash\n");
            println!("{output}");
            process::exit(0);
        }
        // Fall through if not a help request
    }

    let mut cmd = cli::build_command();
    cmd = cmd.color(clap::ColorChoice::Always); // Force clap to emit ANSI codes

    match cmd.try_get_matches_from_mut(args) {
        Ok(_) => false, // Normal args, not help
        Err(err) => {
            match err.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                    // err.render() returns a StyledStr containing ANSI codes.
                    // Use .ansi() to preserve them; .to_string() strips ANSI codes.
                    let clap_output = err.render().ansi().to_string();

                    // Render markdown sections (tables, code blocks, prose) with proper wrapping.
                    // Since we disabled clap's wrapping above, our renderer controls all line breaks.
                    let width = worktrunk::styling::get_terminal_width();
                    let help = crate::md_help::render_markdown_in_help_with_width(
                        &clap_output,
                        Some(width),
                    );

                    // show_help_in_pager checks if stdout or stderr is a TTY.
                    // If neither is a TTY (e.g., `wt --help &>file`), it skips the pager.
                    // use_pager=false for -h (short help), true for --help (long help)
                    if let Err(e) = crate::help_pager::show_help_in_pager(&help, use_pager) {
                        log::debug!("Pager invocation failed: {}", e);
                        eprintln!("{}", help);
                    }
                    process::exit(0);
                }
                ErrorKind::DisplayVersion => {
                    // Print to stderr - stdout is reserved for data/scripts
                    // Use eprint! because clap's Error Display already includes a trailing newline
                    eprint!("{}", err);
                    process::exit(0);
                }
                _ => {
                    // Not help or version - will be re-parsed by Cli::parse()
                    false
                }
            }
        }
    }
}

/// Get the help reference block for a command by invoking clap's help system.
///
/// Returns the usage/options/subcommands section without the after_long_help content.
/// If `width` is provided, wraps text at that width (for web docs); otherwise uses default.
/// Always preserves ANSI color codes for HTML conversion.
fn get_help_reference(command_path: &[&str], width: Option<usize>) -> String {
    let output = get_help_reference_inner(command_path, width);
    // Strip OSC 8 hyperlinks. Clap generates these from markdown links like [text](url),
    // but web docs convert ANSI to HTML via ansi_to_html which only handles SGR codes
    // (colors), not OSC sequences - hyperlinks leak through as garbage.
    strip_osc8_hyperlinks(&output)
}

/// Strip OSC 8 hyperlinks while preserving other ANSI sequences (colors).
///
/// Clap's `unstable-markdown` feature (added in 4.5.28) converts markdown links like
/// `[text](url)` in doc comments to OSC 8 terminal hyperlinks. This is great for terminal
/// output (clickable links!), but our web docs use `ansi_to_html` which only handles SGR
/// codes (colors), not OSC sequences—hyperlinks leak through as garbage text.
///
/// There's no runtime toggle in clap to disable this; the OSC 8 bytes are emitted
/// unconditionally by the markdown renderer. Environment variables like `FORCE_HYPERLINK`,
/// `NO_COLOR`, and `TERM=dumb` have no effect because clap's hyperlink generation is
/// separate from `supports-hyperlinks` and `anstyle-query`.
///
/// Uses the `osc8` crate's parser to find hyperlinks and extract just the visible text.
fn strip_osc8_hyperlinks(s: &str) -> String {
    let mut result = s.to_string();
    // Keep parsing and removing hyperlinks until none remain
    while let Ok(Some((_, range))) = osc8::Hyperlink::parse(&result) {
        // The range covers the opening escape sequence only.
        // We need to find and remove the closing sequence too.
        let after_open = range.end;
        // Find the closing sequence (empty hyperlink that ends the link)
        if let Ok(Some((_, close_range))) = osc8::Hyperlink::parse(&result[after_open..]) {
            // Extract the text between opening and closing sequences
            let text_between = result[after_open..after_open + close_range.start].to_string();
            // Replace the entire hyperlink (open + text + close) with just the text
            let full_end = after_open + close_range.end;
            result.replace_range(range.start..full_end, &text_between);
        } else {
            // Malformed hyperlink (no closing sequence) - just remove the opening
            result.replace_range(range.clone(), "");
            break;
        }
    }
    result
}

fn get_help_reference_inner(command_path: &[&str], width: Option<usize>) -> String {
    // Build args: ["wt", "config", "create", "--help"]
    let mut args: Vec<String> = vec!["wt".to_string()];
    args.extend(command_path.iter().map(|s| s.to_string()));
    args.push("--help".to_string());

    let mut cmd = cli::build_command();
    cmd = cmd.color(ColorChoice::Always);
    if let Some(w) = width {
        cmd = cmd.term_width(w);
    }

    let help_block = if let Err(err) = cmd.try_get_matches_from_mut(args)
        && matches!(
            err.kind(),
            ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        ) {
        let rendered = err.render();
        let text = rendered.ansi().to_string();
        text.replace("```text\n", "```\n")
            .replace("```console\n", "```bash\n")
    } else {
        return String::new();
    };

    // Strip after_long_help if present (it appears at the end)
    // Find it by looking for the first ## heading after Options/Arguments
    if let Some(after_help_start) = find_after_help_start(&help_block) {
        help_block[..after_help_start].trim_end().to_string()
    } else {
        help_block
    }
}

/// Find where after_long_help starts in help output.
///
/// Clap outputs: usage, description, commands/options, Global Options, then after_long_help.
/// The after_long_help can start with a heading or plain text.
fn find_after_help_start(help: &str) -> Option<usize> {
    // After Global Options section, a blank line followed by non-indented text is after_long_help
    let mut past_global_options = false;
    let mut saw_blank_after_options = false;
    let mut blank_offset = None;
    let mut offset = 0;

    for line in help.lines() {
        // Strip ANSI codes for pattern matching
        let plain_line = strip_ansi_codes(line);

        if plain_line.starts_with("Global Options:") {
            past_global_options = true;
            offset += line.len() + 1;
            continue;
        }

        if past_global_options {
            if plain_line.is_empty() {
                saw_blank_after_options = true;
                blank_offset = Some(offset);
            } else if saw_blank_after_options && !plain_line.starts_with(' ') {
                // Non-indented line after blank = start of after_long_help
                return blank_offset;
            } else if plain_line.starts_with(' ') {
                // Still in indented options, reset blank tracking
                saw_blank_after_options = false;
            }
        }
        offset += line.len() + 1;
    }
    None
}

/// Strip ANSI escape codes from a string for pattern matching.
fn strip_ansi_codes(s: &str) -> String {
    s.ansi_strip().into_owned()
}

/// Generate a full documentation page for a command.
///
/// Output format:
/// ```markdown
/// +++
/// title = "Merging"
/// weight = 5
/// +++
///
/// [after_long_help content - the conceptual docs]
///
/// ---
///
/// ## Command reference
///
/// ```bash
/// wt merge — ...
/// Usage: ...
/// ```
/// ```
///
/// This is used to generate docs/content/merge.md etc from the source.
fn handle_help_page(args: &[String]) {
    let mut cmd = cli::build_command();
    cmd = cmd.color(ColorChoice::Never);

    // Find the subcommand name (the arg before --help-page, or after wt)
    let subcommand = args
        .iter()
        .filter(|a| *a != "--help-page" && !a.starts_with('-') && !a.ends_with("/wt"))
        .find(|a| {
            // Skip the binary name
            !a.contains("target/") && *a != "wt"
        });

    let Some(subcommand) = subcommand else {
        eprintln!(
            "Usage: wt <command> --help-page
Commands with pages: merge, switch, remove, list"
        );
        return;
    };

    // Navigate to the subcommand
    let sub = cmd.find_subcommand(subcommand);
    let Some(sub) = sub else {
        eprintln!("Unknown command: {subcommand}");
        return;
    };

    // Get combined docs: about + subtitle + after_long_help
    // Transform for web docs: console→bash, status colors, demo images
    // Subdocs are expanded separately so main Command reference comes first
    let parent_name = format!("wt {}", subcommand);
    let raw_help = combine_command_docs(sub);

    // Split content at first subdoc placeholder
    let subdoc_marker = "<!-- subdoc:";
    let (main_content, subdoc_content) = if let Some(pos) = raw_help.find(subdoc_marker) {
        (&raw_help[..pos], Some(&raw_help[pos..]))
    } else {
        (raw_help.as_str(), None)
    };

    // Process main content (before subdocs)
    let main_help = {
        let text = main_content.replace("```console\n", "```bash\n");
        let text = expand_demo_placeholders(&text);
        colorize_ci_status_for_html(&text)
    };

    // Get the help reference block (wrap at 80 chars for web docs, with colors for HTML)
    let reference_block = get_help_reference(&[subcommand], Some(80));

    // Output the generated content (frontmatter is in skeleton files)
    // Uses region markers so sync can replace just this content
    // END tag mirrors the ID for unambiguous matching with nested markers
    // Use std::println! to preserve ANSI codes in output (the styling::println strips them)
    std::println!(
        "<!-- ⚠️ AUTO-GENERATED from `wt {subcommand} --help-page` — edit cli.rs to update -->"
    );
    std::println!();
    std::println!("{}", main_help.trim());
    std::println!();

    // Main command reference immediately after its content
    std::println!("## Command reference");
    std::println!();
    std::println!("```");
    std::print!("{}", reference_block.trim());
    std::println!();
    std::println!("```");

    // Subdocs follow, each with their own command reference at the end
    if let Some(subdocs) = subdoc_content {
        let subdocs_expanded = expand_subdoc_placeholders(subdocs, sub, &parent_name);
        let subdocs_processed = colorize_ci_status_for_html(&subdocs_expanded);
        std::println!();
        std::println!("# Subcommands");
        std::println!();
        std::println!("{}", subdocs_processed.trim());
    }

    std::println!();
    std::println!("<!-- END AUTO-GENERATED from `wt {subcommand} --help-page` -->");
}

/// Add HTML color spans for CI status dots in help page output.
///
/// Transforms plain text like "`●` green" into colored HTML spans for web rendering.
/// This is the web-docs counterpart to md_help::colorize_status_symbols() which
/// produces ANSI codes for terminal output.
///
/// Also converts plain URL references to markdown links for web docs.
fn colorize_ci_status_for_html(text: &str) -> String {
    text
        // CI status colors (in table cells)
        .replace("`●` green", "<span style='color:#0a0'>●</span> green")
        .replace("`●` blue", "<span style='color:#00a'>●</span> blue")
        .replace("`●` red", "<span style='color:#a00'>●</span> red")
        .replace("`●` yellow", "<span style='color:#a60'>●</span> yellow")
        .replace("`⚠` yellow", "<span style='color:#a60'>⚠</span> yellow")
        .replace("`●` gray", "<span style='color:#888'>●</span> gray")
        // Convert plain URL references to markdown links for web docs
        // CLI shows: "Open an issue at https://github.com/max-sixty/worktrunk."
        // Web shows: "[Open an issue](https://github.com/max-sixty/worktrunk/issues)."
        .replace(
            "Open an issue at https://github.com/max-sixty/worktrunk.",
            "[Open an issue](https://github.com/max-sixty/worktrunk/issues).",
        )
}

/// Increase markdown heading levels by one (## -> ###, ### -> ####, etc.)
///
/// This makes subdoc headings children of the subdoc's main heading.
/// Only transforms actual markdown headings, not code block content.
fn increase_heading_levels(content: &str) -> String {
    let mut result = Vec::new();
    let mut in_code_block = false;

    for line in content.lines() {
        // Track code block boundaries (``` or ````+)
        if line.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
            result.push(line.to_string());
            continue;
        }

        // Only transform headings outside code blocks
        if !in_code_block && line.starts_with('#') {
            result.push(format!("#{}", line));
        } else {
            result.push(line.to_string());
        }
    }

    let mut output = result.join("\n");
    // Preserve trailing newline if present (.lines() strips it)
    if content.ends_with('\n') {
        output.push('\n');
    }
    output
}

/// Expand subdoc placeholders for web docs.
///
/// Transforms `<!-- subdoc: subcommand -->` into an H2 section with the subcommand's help output.
/// For example, `<!-- subdoc: create -->` in `wt config` expands to:
///
/// ```markdown
/// ## wt config create
///
/// [help output for `wt config create`]
/// ```
///
/// This allows including subcommand documentation inline in the parent command's docs page.
fn expand_subdoc_placeholders(text: &str, parent_cmd: &clap::Command, parent_name: &str) -> String {
    const PREFIX: &str = "<!-- subdoc: ";
    const SUFFIX: &str = " -->";

    let mut result = text.to_string();
    while let Some(start) = result.find(PREFIX) {
        let after_prefix = start + PREFIX.len();
        if let Some(end_offset) = result[after_prefix..].find(SUFFIX) {
            let subcommand_name = result[after_prefix..after_prefix + end_offset].trim();
            let end = after_prefix + end_offset + SUFFIX.len();

            // Find the subcommand in the parent
            let replacement = if let Some(sub) = parent_cmd
                .get_subcommands()
                .find(|s| s.get_name() == subcommand_name)
            {
                format_subcommand_section(sub, parent_name, subcommand_name)
            } else {
                format!(
                    "<!-- subdoc error: subcommand '{}' not found -->",
                    subcommand_name
                )
            };

            result.replace_range(start..end, &replacement);
        } else {
            break;
        }
    }
    result
}

/// Combine a command's about, long_about, and after_long_help into documentation content.
///
/// The pattern is: `"definition. subtitle\n\n<after_long_help>"`
/// - `about` is the one-liner definition
/// - `subtitle` is the extra content in `long_about` beyond the `about`
/// - If `long_about` doesn't extend `about`, subtitle is empty
fn combine_command_docs(cmd: &clap::Command) -> String {
    let about = cmd.get_about().map(|s| s.to_string());
    let long_about = cmd.get_long_about().map(|s| s.to_string());
    let after_long_help = cmd
        .get_after_long_help()
        .map(|s| s.to_string())
        .unwrap_or_default();

    // Extract subtitle: the part of long_about beyond the short about
    // Doc comments produce: "Short about\n\nLong description" in long_about
    // We only want the long description part (subtitle) for web docs
    let subtitle = match (&about, &long_about) {
        (Some(short), Some(long)) if long.starts_with(short) => {
            let rest = long[short.len()..].trim_start();
            if rest.is_empty() {
                None
            } else {
                Some(rest.to_string())
            }
        }
        _ => None,
    };

    // Combine: definition + subtitle as single lead paragraph, then after_long_help
    // Definition doesn't have trailing period, subtitle does, so join with ". "
    match (&about, &subtitle) {
        (Some(def), Some(sub)) => format!("{def}. {sub}\n\n{after_long_help}"),
        (Some(def), None) => format!("{def}.\n\n{after_long_help}"),
        (None, Some(sub)) => format!("{sub}\n\n{after_long_help}"),
        (None, None) => after_long_help,
    }
}

/// Format a subcommand as an H2 section for docs.
///
/// Includes the subcommand's `after_long_help` (conceptual docs) followed by
/// the command reference (usage, options). If the subdoc has nested subdocs,
/// the command reference comes before them.
fn format_subcommand_section(
    sub: &clap::Command,
    parent_name: &str,
    subcommand_name: &str,
) -> String {
    // parent_name is "wt config", subcommand_name is "create"
    // full_command is "wt config create"
    let full_command = format!("{} {}", parent_name, subcommand_name);

    // Get combined docs: about + subtitle + after_long_help
    let raw_help = combine_command_docs(sub);

    // Split content at first subdoc placeholder so command reference comes before nested subdocs
    let subdoc_marker = "<!-- subdoc:";
    let (main_content, subdoc_content) = if let Some(pos) = raw_help.find(subdoc_marker) {
        (&raw_help[..pos], Some(&raw_help[pos..]))
    } else {
        (raw_help.as_str(), None)
    };

    // Process main content (before any nested subdocs)
    let main_help = {
        let text = main_content.replace("```console\n", "```bash\n");
        let text = increase_heading_levels(&text);
        colorize_ci_status_for_html(&text)
    };

    // Build command path from parent_name: "wt config" -> ["config", "create"]
    let command_path: Vec<&str> = parent_name
        .strip_prefix("wt ")
        .unwrap_or(parent_name)
        .split_whitespace()
        .chain(std::iter::once(subcommand_name))
        .collect();

    // Get help reference (wrap at 80 chars for web docs, with colors for HTML)
    let reference_block = get_help_reference(&command_path, Some(80));

    // Format the section: heading, main content, command reference, then nested subdocs
    let mut section = format!("## {}\n\n", full_command);

    if !main_help.is_empty() {
        section.push_str(main_help.trim());
        section.push_str("\n\n");
    }

    // Command reference comes after main content but before nested subdocs
    section.push_str("### Command reference\n\n```\n");
    section.push_str(reference_block.trim());
    section.push_str("\n```\n");

    // Expand nested subdocs after the command reference
    if let Some(subdocs) = subdoc_content {
        let subdocs_expanded = expand_subdoc_placeholders(subdocs, sub, &full_command);
        let subdocs_processed = colorize_ci_status_for_html(&subdocs_expanded);
        section.push('\n');
        section.push_str(subdocs_processed.trim());
        section.push('\n');
    }

    section
}

/// Expand demo GIF placeholders for web docs.
///
/// Transforms `<!-- demo: filename.gif -->` into an HTML figure with the `demo` class.
/// The HTML comment is invisible in terminal --help output, but expands to a styled figure
/// for web docs generated via --help-page.
///
/// The placeholder should be on its own line without surrounding blank lines in the source.
/// This function adds blank lines around the figure for proper markdown paragraph separation.
///
/// Supports optional dimensions: `<!-- demo: filename.gif 1600x900 -->`
fn expand_demo_placeholders(text: &str) -> String {
    const PREFIX: &str = "<!-- demo: ";
    const SUFFIX: &str = " -->";

    let mut result = text.to_string();
    while let Some(start) = result.find(PREFIX) {
        let after_prefix = start + PREFIX.len();
        if let Some(end_offset) = result[after_prefix..].find(SUFFIX) {
            let content = &result[after_prefix..after_prefix + end_offset];
            // Parse "filename.gif" or "filename.gif 1600x900"
            let mut parts = content.split_whitespace();
            let filename = parts.next().unwrap_or("");
            let dimensions = parts.next(); // Optional "WIDTHxHEIGHT"

            // Extract command name from filename (e.g., "wt-select.gif" -> "wt select")
            let alt_text = filename.trim_end_matches(".gif").replace('-', " ");

            // Build dimension attributes if provided
            let dim_attrs = dimensions
                .and_then(|d| d.split_once('x'))
                .map(|(w, h)| format!(" width=\"{w}\" height=\"{h}\""))
                .unwrap_or_default();

            // Use figure.demo class for proper mobile styling (no shrink, horizontal scroll)
            // Generate <picture> element for light/dark theme switching
            // Assets are organized as: /assets/docs/{light,dark}/filename.gif
            // Add trailing newline for markdown paragraph separation after the figure
            let replacement = format!(
                "<figure class=\"demo\">\n<picture>\n  <source srcset=\"/assets/docs/dark/{filename}\" media=\"(prefers-color-scheme: dark)\">\n  <img src=\"/assets/docs/light/{filename}\" alt=\"{alt_text} demo\"{dim_attrs}>\n</picture>\n</figure>\n"
            );
            let end = after_prefix + end_offset + SUFFIX.len();
            result.replace_range(start..end, &replacement);
        } else {
            break;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_osc8_hyperlinks_removes_hyperlink() {
        // OSC 8 format: ESC ] 8 ; params ; URL ST TEXT ESC ] 8 ; ; ST
        // ST can be ESC \ or BEL (\x07)
        let url = "https://example.com";
        let text = "link text";
        let input = format!(
            "before {}{}{}after",
            osc8::Hyperlink::new(url),
            text,
            osc8::Hyperlink::END
        );

        let result = strip_osc8_hyperlinks(&input);
        assert_eq!(result, "before link textafter");
    }

    #[test]
    fn test_strip_osc8_hyperlinks_preserves_sgr_codes() {
        // SGR codes (colors) like ESC [ 0 m should be preserved
        let url = "https://example.com";
        let text = "link";
        let input = format!(
            "\u{1b}[1m{}{}{}bold\u{1b}[0m",
            osc8::Hyperlink::new(url),
            text,
            osc8::Hyperlink::END
        );

        let result = strip_osc8_hyperlinks(&input);
        assert_eq!(result, "\u{1b}[1mlinkbold\u{1b}[0m");
    }

    #[test]
    fn test_strip_osc8_hyperlinks_handles_no_hyperlinks() {
        let input = "plain text with no hyperlinks";
        let result = strip_osc8_hyperlinks(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_strip_osc8_hyperlinks_handles_multiple() {
        let input = format!(
            "{}first{} and {}second{}",
            osc8::Hyperlink::new("url1"),
            osc8::Hyperlink::END,
            osc8::Hyperlink::new("url2"),
            osc8::Hyperlink::END
        );

        let result = strip_osc8_hyperlinks(&input);
        assert_eq!(result, "first and second");
    }
}
