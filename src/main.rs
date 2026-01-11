use anyhow::Context;
use clap::FromArgMatches;
use color_print::cformat;
use std::path::PathBuf;
use std::process;
use worktrunk::config::{WorktrunkConfig, set_config_path};
use worktrunk::git::{Repository, exit_code, set_base_path};
use worktrunk::path::format_path_for_display;
use worktrunk::shell::extract_filename_from_path;
use worktrunk::styling::{
    error_message, format_with_gutter, hint_message, info_message, println, success_message,
    warning_message,
};

mod cli;
mod commands;
mod completion;
mod diagnostic;
mod display;
pub(crate) mod help_pager;
mod llm;
mod md_help;
mod output;
mod pager;
mod verbose_log;

pub use crate::cli::OutputFormat;

use commands::command_executor::CommandContext;
#[cfg(unix)]
use commands::handle_select;
use commands::worktree::{SwitchResult, handle_push};
use commands::{
    MergeOptions, RebaseResult, ResolutionContext, SquashResult, add_approvals, approve_hooks,
    clear_approvals, compute_worktree_path, handle_config_create, handle_config_show,
    handle_configure_shell, handle_hints_clear, handle_hints_get, handle_hook_show, handle_init,
    handle_list, handle_merge, handle_rebase, handle_remove, handle_remove_current,
    handle_show_theme, handle_squash, handle_state_clear, handle_state_clear_all, handle_state_get,
    handle_state_set, handle_state_show, handle_switch, handle_unconfigure_shell,
    resolve_worktree_arg, run_hook, step_commit, step_copy_ignored, step_for_each,
};
use output::{execute_user_command, handle_remove_output, handle_switch_output};

use cli::{
    ApprovalsCommand, CiStatusAction, Cli, Commands, ConfigCommand, ConfigShellCommand,
    DefaultBranchAction, HintsAction, HookCommand, ListSubcommand, LogsAction, MarkerAction,
    PreviousBranchAction, StateCommand, StepCommand,
};
use worktrunk::HookType;

/// Get the binary name from `argv[0]`, falling back to "wt".
///
/// Used as the default for `--cmd` in shell integration commands.
/// When invoked as `git-wt`, returns "git-wt"; when invoked as `wt`, returns "wt".
/// On Windows, strips `.exe` extension — users should use `wt` not `wt.exe` in aliases.
fn binary_name() -> String {
    std::env::args()
        .next()
        .and_then(|arg0| {
            std::path::Path::new(&arg0)
                .file_stem()
                .and_then(|name| name.to_str())
                .map(String::from)
        })
        .unwrap_or_else(|| "wt".to_string())
}

/// Check if we're running as a git subcommand (e.g., `git wt` instead of `git-wt`).
///
/// When git runs a subcommand like `git wt`, it sets `GIT_EXEC_PATH` in the environment.
/// This is NOT set when running `git-wt` directly or via a shell function.
///
/// This distinction matters for shell integration: `git wt` runs as a subprocess of git,
/// so even with shell integration configured, the `cd` directive cannot propagate to
/// the parent shell. Users must use `git-wt` directly (via shell function) for automatic cd.
fn is_git_subcommand() -> bool {
    std::env::var_os("GIT_EXEC_PATH").is_some()
}

/// Get the raw `argv[0]` value (how we were invoked).
///
/// Used in error messages to show what command was actually run.
/// Returns the full invocation path (e.g., `target/debug/wt`, `./wt`, `wt`).
pub fn invocation_path() -> String {
    std::env::args().next().unwrap_or_else(|| "wt".to_string())
}

/// Check if we were invoked via an explicit path rather than PATH lookup.
///
/// # Purpose
///
/// When shell integration is configured (e.g., `eval "$(wt config shell init)"`),
/// the shell wrapper function intercepts calls to `wt` and handles directory
/// changes. However, this only works when the shell finds `wt` via PATH lookup.
///
/// If the user runs a specific binary path (like `cargo run` or `./target/debug/wt`),
/// the shell wrapper won't intercept it, and shell integration won't work.
///
/// # Heuristic
///
/// Returns `true` if argv\[0\] contains a path separator (`/` or `\`).
///
/// - PATH lookup: shell sets argv\[0\] to just the command name (`wt`)
/// - Explicit path: argv\[0\] contains the path (`./wt`, `target/debug/wt`, `/usr/bin/wt`)
///
/// # Examples
///
/// | Invocation                  | argv\[0\]            | Returns | Reason                    |
/// |-----------------------------|----------------------|---------|---------------------------|
/// | `wt switch foo`             | `wt`                 | `false` | PATH lookup, wrapper works|
/// | `cargo run -- switch foo`   | `target/debug/wt`    | `true`  | Explicit path, no wrapper |
/// | `./target/debug/wt switch`  | `./target/debug/wt`  | `true`  | Explicit path, no wrapper |
/// | `/usr/local/bin/wt switch`  | `/usr/local/bin/wt`  | `true`  | Explicit path, no wrapper |
///
/// # Edge Cases
///
/// - **False positive**: User types full path to installed binary (`/usr/local/bin/wt`).
///   Harmless — if they're typing the full path, shell wrapper wouldn't intercept anyway.
///
/// - **Aliases**: `alias wt='...'` — shell expands alias before setting argv\[0\], so:
///   - `alias wt='wt'` → argv\[0\] = `wt` → `false` (correct)
///   - `alias wt='./target/debug/wt'` → argv\[0\] = `./target/debug/wt` → `true` (correct)
///
/// - **Symlinks**: If `~/bin/wt` is a symlink to `target/debug/wt`, argv\[0\] = `~/bin/wt`
///   (contains `/`) → `true`. This is correct — the shell wrapper wraps PATH's `wt`,
///   not the symlink.
///
/// - **`git wt` subcommand**: When invoked as `git wt`, git dispatches to `git-wt` binary
///   and sets argv\[0\] = `git-wt` (no path separator) → returns `false`. However, shell
///   integration configured for `wt` won't intercept `git wt` — they're different commands.
///   This is handled separately by `is_integration_configured()` which checks for the
///   actual binary name (`git-wt`), not `wt`.
///
/// # Why Not Other Approaches?
///
/// - **`current_exe()` + check for `/target/debug/`**: Only catches cargo builds,
///   misses other "ran specific path" scenarios.
///
/// - **Compare with `which wt`**: More accurate but requires subprocess overhead
///   and `which` behavior varies across shells.
///
/// - **Check if `current_exe()` is in PATH**: Complex PATH parsing, platform differences.
///
/// The argv\[0\] heuristic is simple, fast, and catches all cases where shell
/// integration won't work because the shell wrapper wasn't invoked.
pub fn was_invoked_with_explicit_path() -> bool {
    std::env::args()
        .next()
        .map(|arg0| arg0.contains('/') || arg0.contains('\\'))
        .unwrap_or(false)
}

/// Custom help handling for pager support and markdown rendering.
///
/// We intercept help requests to provide:
/// 1. **Pager support**: Help is shown through the detected pager (git-style precedence)
/// 2. **Markdown rendering**: `## Headers` become green, code blocks are dimmed
///
/// Uses `Error::render()` to get clap's pre-formatted help, which already
/// respects `-h` (short) vs `--help` (long) distinction.
fn maybe_handle_help_with_pager() -> bool {
    use clap::ColorChoice;
    use clap::error::ErrorKind;

    let args: Vec<String> = std::env::args().collect();

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
    cmd = cmd.color(ColorChoice::Always); // Force clap to emit ANSI codes

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
                    let help =
                        md_help::render_markdown_in_help_with_width(&clap_output, Some(width));

                    // show_help_in_pager checks if stdout or stderr is a TTY.
                    // If neither is a TTY (e.g., `wt --help &>file`), it skips the pager.
                    if let Err(e) = help_pager::show_help_in_pager(&help) {
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
    use clap::ColorChoice;
    use clap::error::ErrorKind;

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
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip escape sequence: ESC [ ... m (SGR) or ESC [ ... other
            if chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                // Consume until we hit a letter (the command character)
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            result.push(c);
        }
    }
    result
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
    use clap::ColorChoice;

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
        eprintln!("Usage: wt <command> --help-page");
        eprintln!("Commands with pages: merge, switch, remove, list");
        return;
    };

    // Navigate to the subcommand
    let sub = cmd.find_subcommand(subcommand);
    let Some(sub) = sub else {
        eprintln!("Unknown command: {subcommand}");
        return;
    };

    // Get the after_long_help content
    // Transform for web docs: console→bash, status colors, demo images
    // Subdocs are expanded separately so main Command reference comes first
    let parent_name = format!("wt {}", subcommand);
    let raw_help = sub
        .get_after_long_help()
        .map(|s| s.to_string())
        .unwrap_or_default();

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

    // Get the raw after_long_help content
    let raw_help = sub
        .get_after_long_help()
        .map(|s| s.to_string())
        .unwrap_or_default();

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
            // Add blank line before the figure; blank line after is already in source
            let replacement = format!(
                "\n<figure class=\"demo\">\n<picture>\n  <source srcset=\"/assets/docs/dark/{filename}\" media=\"(prefers-color-scheme: dark)\">\n  <img src=\"/assets/docs/light/{filename}\" alt=\"{alt_text} demo\"{dim_attrs}>\n</picture>\n</figure>"
            );
            let end = after_prefix + end_offset + SUFFIX.len();
            result.replace_range(start..end, &replacement);
        } else {
            break;
        }
    }
    result
}

/// Enhance clap errors with command-specific hints, then exit.
///
/// For `wt switch` missing the branch argument, adds hints about shortcuts.
fn enhance_and_exit_error(err: clap::Error) -> ! {
    use clap::error::ErrorKind;
    use color_print::ceprintln;

    // Enhance `wt switch` missing argument error with shortcut hints.
    // Hints go to stderr, which is safe since stdout is reserved for data output.
    // Check for both "wt switch" and "wt.exe switch" (Windows)
    let err_str = format!("{err}");
    let is_switch_missing_arg = err.kind() == ErrorKind::MissingRequiredArgument
        && (err_str.contains("wt switch") || err_str.contains("wt.exe switch"));
    if is_switch_missing_arg {
        eprint!("{}", err.render().ansi());
        eprintln!();
        ceprintln!("<green,bold>Quick switches:</>");
        ceprintln!("  <cyan,bold>wt switch ^</>    default branch's worktree");
        ceprintln!("  <cyan,bold>wt switch -</>    previous worktree");
        ceprintln!("  <cyan,bold>wt select</>      interactive picker");
        process::exit(2);
    }

    err.exit()
}

fn main() {
    // Configure Rayon's global thread pool for mixed I/O workloads.
    // The `wt list` command runs git operations (CPU + disk I/O) and network
    // requests (CI status, URL health checks) in parallel. Using 2x CPU cores
    // allows threads blocked on I/O to overlap with compute work.
    //
    // TODO: Benchmark different thread counts to find optimal value.
    // Test with `RAYON_NUM_THREADS=N wt list` on repos with many worktrees.
    let num_threads = std::thread::available_parallelism()
        .map(|n| n.get() * 2)
        .unwrap_or(8);
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(num_threads)
        .build_global();

    // Tell crossterm to always emit ANSI sequences
    crossterm::style::force_color_output(true);

    if completion::maybe_handle_env_completion() {
        return;
    }

    // Handle --help with pager before clap processes it
    if maybe_handle_help_with_pager() {
        return;
    }

    // TODO: Enhance error messages to show possible values for missing enum arguments
    // Currently `wt config shell init` doesn't show available shells, but `wt config shell init invalid` does.
    // Clap doesn't support this natively yet - see https://github.com/clap-rs/clap/issues/3320
    // When available, use built-in setting. Until then, could use try_parse() to intercept
    // MissingRequiredArgument errors and print custom messages with ValueEnum::value_variants().
    let cmd = cli::build_command();
    let matches = cmd.try_get_matches().unwrap_or_else(|e| {
        enhance_and_exit_error(e);
    });
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());

    // Initialize base path from -C flag if provided
    if let Some(path) = cli.directory {
        set_base_path(path);
    }

    // Initialize config path from --config flag if provided
    if let Some(path) = cli.config {
        set_config_path(path);
    }

    // Configure logging based on --verbose flag or RUST_LOG env var
    // When --verbose is set, also write logs to .git/wt-logs/verbose.log
    if cli.verbose >= 1 {
        verbose_log::init();
    }

    // Capture verbose level and command line before cli is partially consumed
    let verbose_level = cli.verbose;
    let command_line = std::env::args().collect::<Vec<_>>().join(" ");

    // --verbose takes precedence over RUST_LOG: use Builder::new() to ignore env var
    // Otherwise, respect RUST_LOG (defaulting to off)
    let mut builder = if cli.verbose >= 1 {
        let mut b = env_logger::Builder::new();
        b.filter_level(log::LevelFilter::Debug);
        b
    } else {
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("off"))
    };

    builder
        .format(|buf, record| {
            use std::io::Write;

            let msg = record.args().to_string();

            // Map thread ID to a single character (a-z, then A-Z)
            let thread_id = format!("{:?}", std::thread::current().id());
            let thread_num = thread_id
                .strip_prefix("ThreadId(")
                .and_then(|s| s.strip_suffix(")"))
                .and_then(|s| s.parse::<usize>().ok())
                .map(|n| {
                    if n == 0 {
                        '0'
                    } else if n <= 26 {
                        char::from(b'a' + (n - 1) as u8)
                    } else if n <= 52 {
                        char::from(b'A' + (n - 27) as u8)
                    } else {
                        '?'
                    }
                })
                .unwrap_or('?');

            // Write plain text to log file (no ANSI codes)
            verbose_log::write_line(&format!("[{thread_num}] {msg}"));

            // Commands start with $, make only the command bold (not $ or [worktree])
            if let Some(rest) = msg.strip_prefix("$ ") {
                // Split: "git command [worktree]" -> ("git command", " [worktree]")
                if let Some(bracket_pos) = rest.find(" [") {
                    let command = &rest[..bracket_pos];
                    let worktree = &rest[bracket_pos..];
                    writeln!(
                        buf,
                        "{}",
                        cformat!("<dim>[{thread_num}]</> $ <bold>{command}</>{worktree}")
                    )
                } else {
                    writeln!(
                        buf,
                        "{}",
                        cformat!("<dim>[{thread_num}]</> $ <bold>{rest}</>")
                    )
                }
            } else if msg.starts_with("  ! ") {
                // Error output - show in red
                writeln!(buf, "{}", cformat!("<dim>[{thread_num}]</> <red>{msg}</>"))
            } else {
                // Regular output with thread ID
                writeln!(buf, "{}", cformat!("<dim>[{thread_num}]</> {msg}"))
            }
        })
        .init();

    let Some(command) = cli.command else {
        // No subcommand provided - print help to stderr (stdout is eval'd by shell wrapper)
        let mut cmd = cli::build_command();
        let help = cmd.render_help().ansi().to_string();
        eprintln!("{help}");
        return;
    };

    let result = match command {
        Commands::Config { action } => match action {
            ConfigCommand::Shell { action } => {
                match action {
                    ConfigShellCommand::Init { shell, cmd } => {
                        // Generate shell code to stdout
                        let cmd = cmd.unwrap_or_else(binary_name);
                        handle_init(shell, cmd).map_err(|e| anyhow::anyhow!("{}", e))
                    }
                    ConfigShellCommand::Install { shell, yes, cmd } => {
                        // Auto-write to shell config files and completions
                        let cmd = cmd.unwrap_or_else(binary_name);
                        handle_configure_shell(shell, yes, cmd)
                            .map_err(|e| anyhow::anyhow!("{}", e))
                            .and_then(|scan_result| {
                                // Exit with error if no shells configured
                                // Show skipped shells first so user knows what was tried
                                if scan_result.configured.is_empty() {
                                    crate::output::print_skipped_shells(&scan_result.skipped)?;
                                    return Err(worktrunk::git::GitError::Other {
                                        message: "No shell config files found".into(),
                                    }
                                    .into());
                                }
                                crate::output::print_shell_install_result(&scan_result)
                            })
                    }
                    ConfigShellCommand::Uninstall { shell, yes } => {
                        let explicit_shell = shell.is_some();
                        handle_unconfigure_shell(shell, yes, &binary_name())
                            .map_err(|e| anyhow::anyhow!("{}", e))
                            .and_then(|scan_result| {
                                let shell_count = scan_result.results.len();
                                let completion_count = scan_result.completion_results.len();
                                let total_changes = shell_count + completion_count;

                                // Show shell extension results
                                for result in &scan_result.results {
                                    let shell = result.shell;
                                    let path = format_path_for_display(&result.path);
                                    // For bash/zsh, completions are inline in the init script
                                    let what = if matches!(
                                        shell,
                                        worktrunk::shell::Shell::Bash
                                            | worktrunk::shell::Shell::Zsh
                                    ) {
                                        "shell extension & completions"
                                    } else {
                                        "shell extension"
                                    };

                                    crate::output::print(success_message(cformat!(
                                        "{} {what} for <bold>{shell}</> @ <bold>{path}</>",
                                        result.action.description(),
                                    )))?;
                                }

                                // Show completion results
                                for result in &scan_result.completion_results {
                                    let shell = result.shell;
                                    let path = format_path_for_display(&result.path);

                                    crate::output::print(success_message(cformat!(
                                        "{} completions for <bold>{shell}</> @ <bold>{path}</>",
                                        result.action.description(),
                                    )))?;
                                }

                                // Show not found - warning if explicit shell, hint if auto-scan
                                for (shell, path) in &scan_result.not_found {
                                    let path = format_path_for_display(path);
                                    // Use consistent terminology matching install/uninstall messages
                                    let what = if matches!(
                                        shell,
                                        worktrunk::shell::Shell::Bash
                                            | worktrunk::shell::Shell::Zsh
                                    ) {
                                        "shell extension & completions"
                                    } else {
                                        "shell extension"
                                    };
                                    if explicit_shell {
                                        crate::output::print(warning_message(format!(
                                            "No {what} found in {path}"
                                        )))?;
                                    } else {
                                        crate::output::print(hint_message(cformat!(
                                            "No <bright-black>{shell}</> {what} in {path}"
                                        )))?;
                                    }
                                }

                                // Show completion files not found (only fish has separate completion files)
                                // Only show this if the shell extension was ALSO not found - if we removed
                                // the shell extension, no need to warn about missing completions
                                for (shell, path) in &scan_result.completion_not_found {
                                    let shell_was_removed =
                                        scan_result.results.iter().any(|r| r.shell == *shell);
                                    if shell_was_removed {
                                        continue; // Shell extension was removed, don't warn about completions
                                    }
                                    let path = format_path_for_display(path);
                                    if explicit_shell {
                                        crate::output::print(warning_message(format!(
                                            "No completions found in {path}"
                                        )))?;
                                    } else {
                                        crate::output::print(hint_message(cformat!(
                                            "No <bright-black>{shell}</> completions in {path}"
                                        )))?;
                                    }
                                }

                                // Exit with info if nothing was found
                                let all_not_found = scan_result.not_found.len()
                                    + scan_result.completion_not_found.len();
                                if total_changes == 0 {
                                    if all_not_found == 0 {
                                        crate::output::blank()?;
                                        crate::output::print(hint_message(
                                            "No shell integration found to remove",
                                        ))?;
                                    }
                                    return Ok(());
                                }

                                // Summary
                                crate::output::blank()?;
                                let plural = if shell_count == 1 { "" } else { "s" };
                                crate::output::print(success_message(format!(
                                    "Removed integration from {shell_count} shell{plural}"
                                )))?;

                                // Hint about restarting shell (only if current shell was affected)
                                let current_shell = std::env::var("SHELL")
                                    .ok()
                                    .and_then(|s| extract_filename_from_path(&s).map(String::from));

                                let current_shell_affected =
                                    current_shell.as_ref().is_some_and(|shell_name| {
                                        scan_result.results.iter().any(|r| {
                                            r.shell.to_string().eq_ignore_ascii_case(shell_name)
                                        })
                                    });

                                if current_shell_affected {
                                    crate::output::print(hint_message(
                                        "Restart shell to complete uninstall",
                                    ))?;
                                }
                                Ok(())
                            })
                    }
                    ConfigShellCommand::ShowTheme => {
                        handle_show_theme().map_err(|e| anyhow::anyhow!("{}", e))
                    }
                }
            }
            ConfigCommand::Create { project } => handle_config_create(project),
            ConfigCommand::Show { full } => handle_config_show(full),
            ConfigCommand::State { action } => match action {
                StateCommand::DefaultBranch { action } => match action {
                    Some(DefaultBranchAction::Get) | None => {
                        handle_state_get("default-branch", None)
                    }
                    Some(DefaultBranchAction::Set { branch }) => {
                        handle_state_set("default-branch", branch, None)
                    }
                    Some(DefaultBranchAction::Clear) => {
                        handle_state_clear("default-branch", None, false)
                    }
                },
                StateCommand::PreviousBranch { action } => match action {
                    Some(PreviousBranchAction::Get) | None => {
                        handle_state_get("previous-branch", None)
                    }
                    Some(PreviousBranchAction::Set { branch }) => {
                        handle_state_set("previous-branch", branch, None)
                    }
                    Some(PreviousBranchAction::Clear) => {
                        handle_state_clear("previous-branch", None, false)
                    }
                },
                StateCommand::CiStatus { action } => match action {
                    Some(CiStatusAction::Get { branch }) => handle_state_get("ci-status", branch),
                    None => handle_state_get("ci-status", None),
                    Some(CiStatusAction::Clear { branch, all }) => {
                        handle_state_clear("ci-status", branch, all)
                    }
                },
                StateCommand::Marker { action } => match action {
                    Some(MarkerAction::Get { branch }) => handle_state_get("marker", branch),
                    None => handle_state_get("marker", None),
                    Some(MarkerAction::Set { value, branch }) => {
                        handle_state_set("marker", value, branch)
                    }
                    Some(MarkerAction::Clear { branch, all }) => {
                        handle_state_clear("marker", branch, all)
                    }
                },
                StateCommand::Logs { action } => match action {
                    Some(LogsAction::Get) | None => handle_state_get("logs", None),
                    Some(LogsAction::Clear) => handle_state_clear("logs", None, false),
                },
                StateCommand::Hints { action } => match action {
                    Some(HintsAction::Get) | None => handle_hints_get(),
                    Some(HintsAction::Clear { name }) => handle_hints_clear(name),
                },
                StateCommand::Get { format } => handle_state_show(format),
                StateCommand::Clear => handle_state_clear_all(),
            },
        },
        Commands::Step { action } => match action {
            StepCommand::Commit {
                yes,
                verify,
                stage,
                show_prompt,
            } => WorktrunkConfig::load()
                .context("Failed to load config")
                .and_then(|config| {
                    let stage_final = stage
                        .or_else(|| config.commit.and_then(|c| c.stage))
                        .unwrap_or_default();
                    step_commit(yes, !verify, stage_final, show_prompt)
                }),
            StepCommand::Squash {
                target,
                yes,
                verify,
                stage,
                show_prompt,
            } => WorktrunkConfig::load()
                .context("Failed to load config")
                .and_then(|config| {
                    let stage_final = stage
                        .or_else(|| config.commit.and_then(|c| c.stage))
                        .unwrap_or_default();

                    // Handle --show-prompt early: just build and output the prompt
                    if show_prompt {
                        return commands::step_show_squash_prompt(
                            target.as_deref(),
                            &config.commit_generation,
                        );
                    }

                    // "Approve at the Gate": approve pre-commit hooks upfront (unless --no-verify)
                    // Shadow verify: if user declines approval, skip hooks but continue squash
                    let verify = if verify {
                        use commands::command_approval::approve_hooks;
                        use commands::context::CommandEnv;
                        let env = CommandEnv::for_action("squash")?;
                        let ctx = env.context(yes);
                        let approved = approve_hooks(&ctx, &[HookType::PreCommit])?;
                        if !approved {
                            crate::output::print(info_message(
                                "Commands declined, squashing without hooks",
                            ))?;
                        }
                        approved
                    } else {
                        false
                    };

                    match handle_squash(target.as_deref(), yes, !verify, stage_final)? {
                        SquashResult::Squashed | SquashResult::NoNetChanges => {}
                        SquashResult::NoCommitsAhead(branch) => {
                            crate::output::print(info_message(format!(
                                "Nothing to squash; no commits ahead of {branch}"
                            )))?;
                        }
                        SquashResult::AlreadySingleCommit => {
                            crate::output::print(info_message(
                                "Nothing to squash; already a single commit",
                            ))?;
                        }
                    }
                    Ok(())
                }),
            StepCommand::Push { target } => handle_push(target.as_deref(), "Pushed to", None),
            StepCommand::Rebase { target } => {
                handle_rebase(target.as_deref()).and_then(|result| match result {
                    RebaseResult::Rebased => Ok(()),
                    RebaseResult::UpToDate(branch) => {
                        crate::output::print(info_message(cformat!(
                            "Already up to date with <bold>{branch}</>"
                        )))?;
                        Ok(())
                    }
                })
            }
            StepCommand::CopyIgnored { from, to, dry_run } => {
                step_copy_ignored(from.as_deref(), to.as_deref(), dry_run)
            }
            StepCommand::ForEach { args } => step_for_each(args),
        },
        Commands::Hook { action } => match action {
            HookCommand::Show {
                hook_type,
                expanded,
            } => handle_hook_show(hook_type.as_deref(), expanded),
            HookCommand::PostCreate { name, yes, vars } => {
                run_hook(HookType::PostCreate, yes, None, name.as_deref(), &vars)
            }
            HookCommand::PostStart {
                name,
                yes,
                foreground,
                no_background,
                vars,
            } => {
                if no_background {
                    let _ = output::print(warning_message(
                        "--no-background is deprecated; use --foreground instead",
                    ));
                }
                run_hook(
                    HookType::PostStart,
                    yes,
                    Some(foreground || no_background),
                    name.as_deref(),
                    &vars,
                )
            }
            HookCommand::PostSwitch {
                name,
                yes,
                foreground,
                no_background,
                vars,
            } => {
                if no_background {
                    let _ = output::print(warning_message(
                        "--no-background is deprecated; use --foreground instead",
                    ));
                }
                run_hook(
                    HookType::PostSwitch,
                    yes,
                    Some(foreground || no_background),
                    name.as_deref(),
                    &vars,
                )
            }
            HookCommand::PreCommit { name, yes, vars } => {
                run_hook(HookType::PreCommit, yes, None, name.as_deref(), &vars)
            }
            HookCommand::PreMerge { name, yes, vars } => {
                run_hook(HookType::PreMerge, yes, None, name.as_deref(), &vars)
            }
            HookCommand::PostMerge { name, yes, vars } => {
                run_hook(HookType::PostMerge, yes, None, name.as_deref(), &vars)
            }
            HookCommand::PreRemove { name, yes, vars } => {
                run_hook(HookType::PreRemove, yes, None, name.as_deref(), &vars)
            }
            HookCommand::Approvals { action } => match action {
                ApprovalsCommand::Add { all } => add_approvals(all),
                ApprovalsCommand::Clear { global } => clear_approvals(global),
            },
        },
        #[cfg(unix)]
        Commands::Select => handle_select(),
        #[cfg(not(unix))]
        Commands::Select => {
            let _ = output::print(error_message("wt select is not available on Windows"));
            let _ = output::print(hint_message(cformat!(
                "To see all worktrees, run <bright-black>wt list</>; to switch directly, run <bright-black>wt switch BRANCH</>"
            )));
            std::process::exit(1);
        }
        Commands::List {
            subcommand,
            format,
            branches,
            remotes,
            full,
            progressive,
            no_progressive,
        } => match subcommand {
            Some(ListSubcommand::Statusline { claude_code }) => {
                commands::statusline::run(claude_code)
            }
            None => {
                use commands::list::progressive::RenderMode;

                // Load config and merge with CLI flags (CLI flags take precedence)
                WorktrunkConfig::load()
                    .context("Failed to load config")
                    .and_then(|config| {
                        // Get config values from global list config
                        let (show_branches_config, show_remotes_config, show_full_config) = config
                            .list
                            .as_ref()
                            .map(|l| {
                                (
                                    l.branches.unwrap_or(false),
                                    l.remotes.unwrap_or(false),
                                    l.full.unwrap_or(false),
                                )
                            })
                            .unwrap_or((false, false, false));

                        // CLI flags override config
                        let show_branches = branches || show_branches_config;
                        let show_remotes = remotes || show_remotes_config;
                        let show_full = full || show_full_config;

                        // Convert two bools to Option<bool>: Some(true), Some(false), or None
                        let progressive_opt = match (progressive, no_progressive) {
                            (true, _) => Some(true),
                            (_, true) => Some(false),
                            _ => None,
                        };
                        let render_mode = RenderMode::detect(progressive_opt);
                        handle_list(
                            format,
                            show_branches,
                            show_remotes,
                            show_full,
                            render_mode,
                            &config,
                        )
                    })
            }
        },
        Commands::Switch {
            branch,
            create,
            base,
            execute,
            execute_args,
            yes,
            clobber,
            verify,
        } => WorktrunkConfig::load()
            .context("Failed to load config")
            .and_then(|mut config| {
                // "Approve at the Gate": collect and approve hooks upfront
                // This ensures approval happens once at the command entry point
                // If user declines, skip hooks but continue with worktree operation
                let approved = if verify {
                    let repo = Repository::current().context("Failed to switch worktree")?;
                    let repo_root = repo.worktree_base().context("Failed to switch worktree")?;
                    // Compute worktree path for template expansion in approval prompt
                    let worktree_path = compute_worktree_path(&repo, &branch, &config)?;
                    let ctx = CommandContext::new(
                        &repo,
                        &config,
                        Some(&branch),
                        &worktree_path,
                        &repo_root,
                        yes,
                    );
                    // Approve different hooks based on whether we're creating or switching
                    if create {
                        approve_hooks(
                            &ctx,
                            &[
                                HookType::PostCreate,
                                HookType::PostStart,
                                HookType::PostSwitch,
                            ],
                        )?
                    } else {
                        // When switching to existing, only post-switch needs approval
                        approve_hooks(&ctx, &[HookType::PostSwitch])?
                    }
                } else {
                    true // --no-verify: skip all hooks
                };

                // Skip hooks if --no-verify or user declined approval
                let skip_hooks = !verify || !approved;

                // Show message if user declined approval
                if !approved {
                    crate::output::print(info_message(if create {
                        "Commands declined, continuing worktree creation"
                    } else {
                        "Commands declined"
                    }))?;
                }

                // Execute switch operation (creates worktree, runs post-create hooks if approved)
                let (result, branch_info) = handle_switch(
                    &branch,
                    create,
                    base.as_deref(),
                    yes,
                    clobber,
                    skip_hooks,
                    &config,
                )?;

                // Show success message (temporal locality: immediately after worktree operation)
                // Returns path to display in hooks when user's shell won't be in the worktree
                // Also shows worktree-path hint on first --create (before shell integration warning)
                let hooks_display_path =
                    handle_switch_output(&result, &branch_info, execute.as_deref())?;

                // Offer shell integration if not already installed/active
                // (only shows prompt/hint when shell integration isn't working)
                // With --execute: show hints only (don't interrupt with prompt)
                // Best-effort: don't fail switch if offer fails
                if !output::is_shell_integration_active() {
                    let skip_prompt = execute.is_some();
                    let _ =
                        output::prompt_shell_integration(&mut config, &binary_name(), skip_prompt);
                }

                // Spawn background hooks after success message
                // - post-switch: runs on ALL switches (shows "@ path" when shell won't be there)
                // - post-start: runs only when creating a NEW worktree
                if !skip_hooks {
                    let repo = Repository::current()?;
                    let repo_root = repo.worktree_base().context("Failed to switch worktree")?;
                    let ctx = CommandContext::new(
                        &repo,
                        &config,
                        Some(&branch_info.branch),
                        result.path(),
                        &repo_root,
                        yes,
                    );

                    // Build extra vars for base branch context
                    // "base" is the branch we branched from when creating a new worktree.
                    // For existing worktrees, there's no base concept.
                    let (base_branch, base_worktree_path): (Option<&str>, Option<&str>) =
                        match &result {
                            SwitchResult::Created {
                                base_branch,
                                base_worktree_path,
                                ..
                            } => (base_branch.as_deref(), base_worktree_path.as_deref()),
                            SwitchResult::Existing(_) | SwitchResult::AlreadyAt(_) => (None, None),
                        };
                    let extra_vars: Vec<(&str, &str)> = [
                        base_branch.map(|b| ("base", b)),
                        base_worktree_path.map(|p| ("base_worktree_path", p)),
                    ]
                    .into_iter()
                    .flatten()
                    .collect();

                    // Post-switch runs first (immediate "I'm here" signal)
                    ctx.spawn_post_switch_commands(&extra_vars, hooks_display_path.as_deref())?;

                    // Post-start runs only on creation (setup tasks)
                    if matches!(&result, SwitchResult::Created { .. }) {
                        ctx.spawn_post_start_commands(&extra_vars, hooks_display_path.as_deref())?;
                    }
                }

                // Execute user command after post-start hooks have been spawned
                // Note: execute_args requires execute via clap's `requires` attribute
                if let Some(cmd) = execute {
                    // Append any trailing args (after --) to the execute command
                    let full_cmd = if execute_args.is_empty() {
                        cmd
                    } else {
                        let escaped_args: Vec<_> = execute_args
                            .iter()
                            .map(|arg| shlex::try_quote(arg).unwrap_or(arg.into()).into_owned())
                            .collect();
                        format!("{} {}", cmd, escaped_args.join(" "))
                    };
                    execute_user_command(&full_cmd)?;
                }

                Ok(())
            }),
        Commands::Remove {
            branches,
            delete_branch,
            force_delete,
            foreground,
            no_background,
            verify,
            yes,
            force,
        } => WorktrunkConfig::load()
            .context("Failed to load config")
            .and_then(|config| {
                // Handle deprecated --no-background flag
                if no_background {
                    output::print(warning_message(
                        "--no-background is deprecated; use --foreground instead",
                    ))?;
                }
                let background = !(foreground || no_background);

                // Validate conflicting flags
                if !delete_branch && force_delete {
                    return Err(worktrunk::git::GitError::Other {
                        message: "Cannot use --force-delete with --no-delete-branch".into(),
                    }
                    .into());
                }

                // "Approve at the Gate": collect and approve pre-remove hooks upfront
                // This ensures approval happens once at the command entry point
                //
                // TODO(pre-remove-context): The approval context uses current worktree (cwd + current_branch),
                // but hooks execute in each target worktree. When removing another worktree, the approval
                // preview shows the wrong branch/path. Consider building approval context per target worktree.
                let repo = Repository::current().context("Failed to remove worktree")?;
                let verify = if verify {
                    // Create context for template expansion in approval prompt
                    let worktree_path =
                        std::env::current_dir().context("Failed to get current directory")?;
                    let repo_root = repo.worktree_base().context("Failed to remove worktree")?;
                    // Keep as Option so detached HEAD maps to None -> "HEAD" via branch_or_head()
                    let current_branch = repo
                        .current_worktree()
                        .branch()
                        .context("Failed to remove worktree")?;
                    let ctx = CommandContext::new(
                        &repo,
                        &config,
                        current_branch.as_deref(),
                        &worktree_path,
                        &repo_root,
                        yes,
                    );
                    let approved =
                        approve_hooks(&ctx, &[HookType::PreRemove, HookType::PostSwitch])?;
                    // If declined, skip hooks but continue with removal
                    if !approved {
                        crate::output::print(info_message(
                            "Commands declined, continuing removal",
                        ))?;
                    }
                    approved
                } else {
                    false
                };

                if branches.is_empty() {
                    // No branches specified, remove current worktree
                    // Uses path-based removal to handle detached HEAD state
                    let result =
                        handle_remove_current(!delete_branch, force_delete, force, &config)
                            .context("Failed to remove worktree")?;
                    // Approval was handled at the gate
                    // Post-switch hooks are spawned internally by handle_remove_output
                    handle_remove_output(&result, background, verify)
                } else {
                    use worktrunk::git::ResolvedWorktree;
                    // When removing multiple worktrees, we need to handle the current worktree last
                    // to avoid deleting the directory we're currently in
                    let current_worktree = repo.current_worktree().root().ok();

                    // Partition branches into current worktree, others, and branch-only.
                    // Track all errors (resolution + removal) so we can report them and continue.
                    let mut others = Vec::new();
                    let mut branch_only = Vec::new();
                    let mut current: Option<(PathBuf, Option<String>)> = None;
                    let mut all_errors: Vec<anyhow::Error> = Vec::new();

                    for branch_name in &branches {
                        match resolve_worktree_arg(
                            &repo,
                            branch_name,
                            &config,
                            ResolutionContext::Remove,
                        ) {
                            Ok(ResolvedWorktree::Worktree { path, branch }) => {
                                if Some(&path) == current_worktree.as_ref() {
                                    current = Some((path, branch));
                                } else {
                                    others.push((path, branch));
                                }
                            }
                            Ok(ResolvedWorktree::BranchOnly { branch }) => {
                                branch_only.push(branch);
                            }
                            Err(e) => {
                                // GitError variants already include emoji via error_message() in Display
                                output::print(e.to_string())?;
                                all_errors.push(e);
                            }
                        }
                    }

                    // Remove other worktrees first (approval was handled at the gate)
                    for (_path, branch) in &others {
                        // Branch is always Some for non-current worktrees - detached worktrees
                        // can only be referenced via "@" which resolves to current
                        let branch_name = branch
                            .as_ref()
                            .expect("non-current worktree should have branch");
                        match handle_remove(
                            branch_name,
                            !delete_branch,
                            force_delete,
                            force,
                            &config,
                        ) {
                            Ok(result) => {
                                handle_remove_output(&result, background, verify)?;
                            }
                            Err(e) => {
                                output::print(e.to_string())?;
                                all_errors.push(e);
                            }
                        }
                    }

                    // Handle branch-only cases (no worktree)
                    for branch in &branch_only {
                        match handle_remove(branch, !delete_branch, force_delete, force, &config) {
                            Ok(result) => {
                                handle_remove_output(&result, background, verify)?;
                            }
                            Err(e) => {
                                output::print(e.to_string())?;
                                all_errors.push(e);
                            }
                        }
                    }

                    // Remove current worktree last (if it was in the list)
                    // Post-switch hooks are spawned internally by handle_remove_output
                    if let Some((_path, _branch)) = current {
                        match handle_remove_current(!delete_branch, force_delete, force, &config) {
                            Ok(result) => {
                                handle_remove_output(&result, background, verify)?;
                            }
                            Err(e) => {
                                output::print(e.to_string())?;
                                all_errors.push(e);
                            }
                        }
                    }

                    // Exit with failure if any errors occurred (errors already printed)
                    if !all_errors.is_empty() {
                        anyhow::bail!("");
                    }

                    Ok(())
                }
            }),
        Commands::Merge {
            target,
            squash,
            no_squash,
            commit,
            no_commit,
            rebase,
            no_rebase,
            remove,
            no_remove,
            verify,
            no_verify,
            yes,
            stage,
        } => WorktrunkConfig::load()
            .context("Failed to load config")
            .and_then(|config| {
                // Convert paired flags to Option<bool>
                fn flag_pair(positive: bool, negative: bool) -> Option<bool> {
                    match (positive, negative) {
                        (true, _) => Some(true),
                        (_, true) => Some(false),
                        _ => None,
                    }
                }

                // Get config defaults (positive form: true = do it)
                let merge_config = config.merge.as_ref();
                let squash_default = merge_config.and_then(|m| m.squash).unwrap_or(true);
                let commit_default = merge_config.and_then(|m| m.commit).unwrap_or(true);
                let rebase_default = merge_config.and_then(|m| m.rebase).unwrap_or(true);
                let remove_default = merge_config.and_then(|m| m.remove).unwrap_or(true);
                let verify_default = merge_config.and_then(|m| m.verify).unwrap_or(true);

                // CLI flags override config, config overrides defaults
                let squash_final = flag_pair(squash, no_squash).unwrap_or(squash_default);
                let commit_final = flag_pair(commit, no_commit).unwrap_or(commit_default);
                let rebase_final = flag_pair(rebase, no_rebase).unwrap_or(rebase_default);
                let remove_final = flag_pair(remove, no_remove).unwrap_or(remove_default);
                let verify_final = flag_pair(verify, no_verify).unwrap_or(verify_default);

                // Stage defaults from [commit] config section
                let stage_final = stage
                    .or_else(|| config.commit.and_then(|c| c.stage))
                    .unwrap_or_default();

                handle_merge(MergeOptions {
                    target: target.as_deref(),
                    squash: squash_final,
                    commit: commit_final,
                    rebase: rebase_final,
                    remove: remove_final,
                    verify: verify_final,
                    yes,
                    stage_mode: stage_final,
                })
            }),
    };

    if let Err(e) = result {
        // GitError, WorktrunkError, and HookErrorWithHint produce styled output via Display
        if let Some(err) = e.downcast_ref::<worktrunk::git::GitError>() {
            let _ = output::print(err.to_string());
        } else if let Some(err) = e.downcast_ref::<worktrunk::git::WorktrunkError>() {
            let _ = output::print(err.to_string());
        } else if let Some(err) = e.downcast_ref::<worktrunk::git::HookErrorWithHint>() {
            let _ = output::print(err.to_string());
        } else {
            // Anyhow error formatting:
            // - With context: show context as header, root cause in gutter
            // - Simple error: inline with emoji
            // - Empty error: skip (errors already printed elsewhere)
            let msg = e.to_string();
            if !msg.is_empty() {
                // Collect the error chain (skipping the first which is in msg)
                let chain: Vec<String> = e.chain().skip(1).map(|e| e.to_string()).collect();
                if !chain.is_empty() {
                    // Has context: msg is context, chain contains intermediate + root cause
                    let _ = output::print(error_message(&msg));
                    let chain_text = chain.join("\n");
                    let _ = output::print(format_with_gutter(&chain_text, None));
                } else if msg.contains('\n') {
                    // Multiline error without context - this shouldn't happen if all
                    // errors have proper context. Fail in tests, log in production.
                    if cfg!(test) {
                        panic!("Multiline error without context: {msg}");
                    }
                    log::warn!("Multiline error without context: {msg}");
                    let _ = output::print(error_message("Command failed"));
                    let _ = output::print(format_with_gutter(&msg, None));
                } else {
                    // Single-line error without context: inline with emoji
                    let _ = output::print(error_message(&msg));
                }
            }
        }

        // Preserve exit code from child processes (especially for signals like SIGINT)
        let code = exit_code(&e).unwrap_or(1);

        // Write diagnostic if -vv was used (error case)
        write_vv_diagnostic(verbose_level, &command_line, Some(&e.to_string()));

        // Reset ANSI state before exiting
        let _ = output::terminate_output();
        process::exit(code);
    }

    // Write diagnostic if -vv was used (success case)
    write_vv_diagnostic(verbose_level, &command_line, None);

    // Reset ANSI state before returning to shell (success case)
    let _ = output::terminate_output();
}

/// Write diagnostic file when -vv is used.
///
/// Called at the end of command execution. If verbose level is >= 2, writes
/// a diagnostic report to `.git/wt-logs/diagnostic.md` for issue filing.
///
/// Silently returns if:
/// - verbose < 2
/// - Not in a git repository
///
/// Warns if diagnostic file write fails.
fn write_vv_diagnostic(verbose: u8, command_line: &str, error_msg: Option<&str>) {
    if verbose < 2 {
        return;
    }

    // Use Repository::current() which honors the -C flag
    let Ok(repo) = worktrunk::git::Repository::current() else {
        return;
    };

    // Check if we're actually in a git repo
    if repo.current_worktree().git_dir().is_err() {
        return;
    }

    // Build context based on success/error
    let context = match error_msg {
        Some(msg) => format!("Command failed: {msg}"),
        None => "Command completed successfully".to_string(),
    };

    // Collect and write diagnostic
    let report = diagnostic::DiagnosticReport::collect(&repo, command_line, context);
    match report.write_diagnostic_file(&repo) {
        Some(path) => {
            let path_display = format_path_for_display(&path);
            let _ = output::print(info_message(format!("Diagnostic saved: {path_display}")));

            // Only show gh command if gh is installed
            if is_gh_installed() {
                // Escape single quotes for shell: 'it'\''s' -> it's
                let path_str = path.to_string_lossy().replace('\'', "'\\''");
                let _ = output::print(hint_message(cformat!(
                    "If this is a bug, draft an issue: <bright-black>gh issue create --web -R max-sixty/worktrunk -t 'Bug report' --body-file '{path_str}'</>"
                )));
            }
        }
        None => {
            let _ = output::print(warning_message("Failed to write diagnostic file"));
        }
    }
}

/// Check if the GitHub CLI (gh) is installed.
fn is_gh_installed() -> bool {
    use std::process::{Command, Stdio};
    use worktrunk::shell_exec::run;

    let mut cmd = Command::new("gh");
    cmd.args(["--version"]);
    cmd.stdin(Stdio::null());

    run(&mut cmd, None)
        .map(|o| o.status.success())
        .unwrap_or(false)
}
