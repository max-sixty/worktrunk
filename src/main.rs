use anyhow::Context;
use clap::FromArgMatches;
use color_print::cformat;
use std::path::PathBuf;
use std::process;
use worktrunk::config::{WorktrunkConfig, set_config_path};
use worktrunk::git::{Repository, exit_code, set_base_path};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    error_message, format_with_gutter, hint_message, info_message, println, success_message,
    warning_message,
};

mod cli;
mod commands;
mod completion;
mod display;
pub(crate) mod help_pager;
mod llm;
mod md_help;
mod output;

pub use crate::cli::OutputFormat;

use commands::command_executor::CommandContext;
#[cfg(unix)]
use commands::handle_select;
use commands::worktree::{SwitchResult, handle_push};
use commands::{
    ConfigAction, RebaseResult, SquashResult, add_approvals, approve_hooks, clear_approvals,
    compute_worktree_path, handle_config_create, handle_config_show, handle_configure_shell,
    handle_hook_show, handle_init, handle_list, handle_merge, handle_rebase, handle_remove,
    handle_remove_by_path, handle_remove_current, handle_show_theme, handle_squash,
    handle_state_clear, handle_state_clear_all, handle_state_get, handle_state_set,
    handle_state_show, handle_switch, handle_unconfigure_shell, resolve_worktree_path_first,
    run_hook, step_commit, step_for_each,
};
use output::{execute_user_command, handle_remove_output, handle_switch_output};

use cli::{
    ApprovalsCommand, CiStatusAction, Cli, Commands, ConfigCommand, ConfigShellCommand,
    DefaultBranchAction, HookCommand, ListSubcommand, LogsAction, MarkerAction,
    PreviousBranchAction, StateCommand, StepCommand,
};
use worktrunk::HookType;

/// Get the binary name from `argv[0]`, falling back to "wt".
///
/// Used as the default for `--cmd` in shell integration commands.
/// When invoked as `git-wt`, returns "git-wt"; when invoked as `wt`, returns "wt".
fn binary_name() -> String {
    std::env::args()
        .next()
        .and_then(|arg0| {
            std::path::Path::new(&arg0)
                .file_stem() // Use file_stem to strip .exe on Windows
                .and_then(|name| name.to_str())
                .map(String::from)
        })
        .unwrap_or_else(|| "wt".to_string())
}

/// Custom help handling for pager support and markdown rendering.
///
/// We intercept help requests to provide:
/// 1. **Pager support**: Help is shown through `less` (like git)
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
                    let mut help = err.render().ansi().to_string();

                    // Render markdown sections to ANSI
                    help = md_help::render_markdown_in_help(&help);

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
fn get_help_reference(command_path: &[&str]) -> String {
    use clap::ColorChoice;
    use clap::error::ErrorKind;

    // Build args: ["wt", "config", "create", "--help"]
    let mut args: Vec<String> = vec!["wt".to_string()];
    args.extend(command_path.iter().map(|s| s.to_string()));
    args.push("--help".to_string());

    let mut cmd = cli::build_command();
    cmd = cmd.color(ColorChoice::Never);

    let help_block = if let Err(err) = cmd.try_get_matches_from_mut(args)
        && matches!(
            err.kind(),
            ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        ) {
        err.render()
            .to_string()
            .replace("```text\n", "```\n")
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
        if line.starts_with("Global Options:") {
            past_global_options = true;
            offset += line.len() + 1;
            continue;
        }

        if past_global_options {
            if line.is_empty() {
                saw_blank_after_options = true;
                blank_offset = Some(offset);
            } else if saw_blank_after_options && !line.starts_with(' ') {
                // Non-indented line after blank = start of after_long_help
                return blank_offset;
            } else if line.starts_with(' ') {
                // Still in indented options, reset blank tracking
                saw_blank_after_options = false;
            }
        }
        offset += line.len() + 1;
    }
    None
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

    // Get the help reference block
    let reference_block = get_help_reference(&[subcommand]);

    // Output the generated content (frontmatter is in skeleton files)
    // Uses region markers so sync can replace just this content
    // END tag mirrors the ID for unambiguous matching with nested markers
    println!(
        "<!-- ⚠️ AUTO-GENERATED from `wt {subcommand} --help-page` — edit cli.rs to update -->"
    );
    println!();
    println!("{}", main_help.trim());
    println!();

    // Main command reference immediately after its content
    println!("## Command reference");
    println!();
    println!("```");
    print!("{}", reference_block.trim());
    println!();
    println!("```");

    // Subdocs follow, each with their own command reference at the end
    if let Some(subdocs) = subdoc_content {
        let subdocs_expanded = expand_subdoc_placeholders(subdocs, sub, &parent_name);
        let subdocs_processed = colorize_ci_status_for_html(&subdocs_expanded);
        println!();
        println!("{}", subdocs_processed.trim());
    }

    println!();
    println!("<!-- END AUTO-GENERATED from `wt {subcommand} --help-page` -->");
}

/// Add HTML color spans for CI status dots in help page output.
///
/// Transforms plain text like "`●` green" into colored HTML spans for web rendering.
/// This is the web-docs counterpart to md_help::colorize_status_symbols() which
/// produces ANSI codes for terminal output.
fn colorize_ci_status_for_html(text: &str) -> String {
    text
        // CI status colors (in table cells)
        .replace("`●` green", "<span style='color:#0a0'>●</span> green")
        .replace("`●` blue", "<span style='color:#00a'>●</span> blue")
        .replace("`●` red", "<span style='color:#a00'>●</span> red")
        .replace("`●` yellow", "<span style='color:#a60'>●</span> yellow")
        .replace("`●` gray", "<span style='color:#888'>●</span> gray")
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

    let reference_block = get_help_reference(&command_path);

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
            let dark_filename = filename.trim_end_matches(".gif").to_string() + "-dark.gif";
            let replacement = format!(
                "<figure class=\"demo\">\n<picture>\n  <source srcset=\"/assets/{dark_filename}\" media=\"(prefers-color-scheme: dark)\">\n  <img src=\"/assets/{filename}\" alt=\"{alt_text} demo\"{dim_attrs}>\n</picture>\n</figure>"
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
    // Safe in directive mode: hints go to stderr, only stdout is eval'd.
    // Check for both "wt switch" and "wt.exe switch" (Windows)
    let err_str = format!("{err}");
    let is_switch_missing_arg = err.kind() == ErrorKind::MissingRequiredArgument
        && (err_str.contains("wt switch") || err_str.contains("wt.exe switch"));
    if is_switch_missing_arg {
        eprint!("{}", err.render().ansi());
        ceprintln!("<green,bold>Quick switches:</>");
        ceprintln!("  <cyan,bold>wt switch ^</>    default branch's worktree");
        ceprintln!("  <cyan,bold>wt switch -</>    previous worktree");
        ceprintln!("  <cyan,bold>wt switch @</>    current branch's worktree");
        ceprintln!("  <cyan,bold>wt select</>      interactive picker");
        process::exit(2);
    }

    err.exit()
}

fn main() {
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

    // Initialize output context based on --internal flag
    let output_mode = match cli.internal {
        Some(shell) => output::OutputMode::Directive(shell),
        None => output::OutputMode::Interactive,
    };
    output::initialize(output_mode);

    // Configure logging based on --verbose flag or RUST_LOG env var
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(if cli.verbose { "debug" } else { "off" }),
    )
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
                if n <= 26 {
                    char::from(b'a' + (n - 1) as u8)
                } else if n <= 52 {
                    char::from(b'A' + (n - 27) as u8)
                } else {
                    '?'
                }
            })
            .unwrap_or('?');

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
                    ConfigShellCommand::Install { shell, force, cmd } => {
                        // Auto-write to shell config files and completions
                        let cmd = cmd.unwrap_or_else(binary_name);
                        handle_configure_shell(shell, force, cmd)
                        .map_err(|e| anyhow::anyhow!("{}", e))
                        .and_then(|scan_result| {

                            // Count shells that became (more) configured
                            // A shell counts if any of its components changed (extension or completions)
                            let shells_configured_count = scan_result
                                .configured
                                .iter()
                                .filter(|ext_result| {
                                    let ext_changed =
                                        !matches!(ext_result.action, ConfigAction::AlreadyExists);
                                    let comp_changed = scan_result
                                        .completion_results
                                        .iter()
                                        .find(|c| c.shell == ext_result.shell)
                                        .is_some_and(|c| {
                                            !matches!(c.action, ConfigAction::AlreadyExists)
                                        });
                                    ext_changed || comp_changed
                                })
                                .count();

                            // Show configured shells grouped with their completions
                            for result in &scan_result.configured {
                                let shell = result.shell;
                                let path = format_path_for_display(&result.path);
                                // For bash/zsh, completions are inline in the init script
                                let what = if matches!(shell, worktrunk::shell::Shell::Bash | worktrunk::shell::Shell::Zsh) {
                                    "shell extension & completions"
                                } else {
                                    "shell extension"
                                };
                                let message = cformat!(
                                    "{} {what} for <bold>{shell}</> @ <bold>{path}</>",
                                    result.action.description()
                                );

                                // Use appropriate output function based on action
                                // Note: WouldAdd/WouldCreate are only returned in preview mode,
                                // which is handled internally by prompt_for_confirmation()
                                match result.action {
                                    ConfigAction::Added | ConfigAction::Created => {
                                        crate::output::print(success_message(message))?;
                                    }
                                    ConfigAction::AlreadyExists => {
                                        crate::output::print(info_message(message))?;
                                    }
                                    ConfigAction::WouldAdd | ConfigAction::WouldCreate => {
                                        unreachable!("Preview actions handled by confirmation prompt")
                                    }
                                }

                                // Show completion result for this shell
                                // TODO: Inconsistent that shell extensions show gutter but completions don't.
                                // Completions are dynamic stubs (~30 lines) that call back to `wt` - not
                                // as meaningful to show, but the asymmetry is confusing.
                                if let Some(comp_result) = scan_result.completion_results.iter().find(|r| r.shell == shell) {
                                    let comp_path = format_path_for_display(&comp_result.path);
                                    let comp_message = cformat!(
                                        "{} completions for <bold>{shell}</> @ <bold>{comp_path}</>",
                                        comp_result.action.description()
                                    );
                                    match comp_result.action {
                                        ConfigAction::Added | ConfigAction::Created => {
                                            crate::output::print(success_message(comp_message))?;
                                        }
                                        ConfigAction::AlreadyExists => {
                                            crate::output::print(info_message(comp_message))?;
                                        }
                                        ConfigAction::WouldAdd | ConfigAction::WouldCreate => {
                                            unreachable!("Preview actions handled by confirmation prompt")
                                        }
                                    }
                                }
                            }

                            // Show skipped shells
                            for (shell, path) in &scan_result.skipped {
                                let path = format_path_for_display(path);
                                crate::output::print(hint_message(cformat!(
                                    "Skipped <bright-black>{shell}</>; {path} not found"
                                )))?;
                            }

                            // Exit with error if no shells configured
                            if scan_result.configured.is_empty() {
                                return Err(worktrunk::git::GitError::Other {
                                    message: "No shell config files found".into(),
                                }
                                .into());
                            }

                            // Summary
                            if shells_configured_count > 0 {
                                crate::output::blank()?;
                                let plural = if shells_configured_count == 1 { "" } else { "s" };
                                crate::output::print(success_message(format!(
                                    "Configured {shells_configured_count} shell{plural}"
                                )))?;
                            } else {
                                // No action: all shells were already configured
                                crate::output::print(success_message("All shells already configured"))?;
                            }

                            // Restart hint: only shown if the current shell's extension changed
                            // Fish completions are lazily loaded from ~/.config/fish/completions/
                            // so no restart needed. Bash/Zsh completions are inline in the init script.
                            if shells_configured_count > 0 {
                                let current_shell = std::env::var("SHELL")
                                    .ok()
                                    .and_then(|s| s.rsplit('/').next().map(String::from));

                                // Find if current shell had its extension changed
                                let current_shell_result =
                                    current_shell.as_ref().and_then(|shell_name| {
                                        scan_result
                                            .configured
                                            .iter()
                                            .filter(|r| {
                                                !matches!(r.action, ConfigAction::AlreadyExists)
                                            })
                                            .find(|r| {
                                                r.shell.to_string().eq_ignore_ascii_case(shell_name)
                                            })
                                    });

                                if let Some(result) = current_shell_result {
                                    // Fish auto-sources from conf.d, so just say "Restart shell"
                                    // Bash/Zsh can source directly for immediate activation
                                    if matches!(result.shell, worktrunk::shell::Shell::Fish) {
                                        crate::output::print(hint_message("Restart shell to activate"))?;
                                    } else {
                                        let path = format_path_for_display(&result.path);
                                        crate::output::print(hint_message(format!(
                                            "Restart shell or run: source {path}"
                                        )))?;
                                    }
                                }
                            }
                            Ok(())
                        })
                    }
                    ConfigShellCommand::Uninstall { shell, force } => {
                        let explicit_shell = shell.is_some();
                        handle_unconfigure_shell(shell, force)
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
                                    .and_then(|s| s.rsplit('/').next().map(String::from));

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
                    Some(DefaultBranchAction::Get { refresh }) => {
                        handle_state_get("default-branch", refresh, None)
                    }
                    None => handle_state_get("default-branch", false, None),
                    Some(DefaultBranchAction::Set { branch }) => {
                        handle_state_set("default-branch", branch, None)
                    }
                    Some(DefaultBranchAction::Clear) => {
                        handle_state_clear("default-branch", None, false)
                    }
                },
                StateCommand::PreviousBranch { action } => match action {
                    Some(PreviousBranchAction::Get) | None => {
                        handle_state_get("previous-branch", false, None)
                    }
                    Some(PreviousBranchAction::Set { branch }) => {
                        handle_state_set("previous-branch", branch, None)
                    }
                    Some(PreviousBranchAction::Clear) => {
                        handle_state_clear("previous-branch", None, false)
                    }
                },
                StateCommand::CiStatus { action } => match action {
                    Some(CiStatusAction::Get { refresh, branch }) => {
                        handle_state_get("ci-status", refresh, branch)
                    }
                    None => handle_state_get("ci-status", false, None),
                    Some(CiStatusAction::Clear { branch, all }) => {
                        handle_state_clear("ci-status", branch, all)
                    }
                },
                StateCommand::Marker { action } => match action {
                    Some(MarkerAction::Get { branch }) => handle_state_get("marker", false, branch),
                    None => handle_state_get("marker", false, None),
                    Some(MarkerAction::Set { value, branch }) => {
                        handle_state_set("marker", value, branch)
                    }
                    Some(MarkerAction::Clear { branch, all }) => {
                        handle_state_clear("marker", branch, all)
                    }
                },
                StateCommand::Logs { action } => match action {
                    Some(LogsAction::Get) | None => handle_state_get("logs", false, None),
                    Some(LogsAction::Clear) => handle_state_clear("logs", None, false),
                },
                StateCommand::Get { format } => handle_state_show(format),
                StateCommand::Clear => handle_state_clear_all(),
            },
        },
        Commands::Step { action } => match action {
            StepCommand::Commit {
                force,
                verify,
                stage,
                show_prompt,
            } => WorktrunkConfig::load()
                .context("Failed to load config")
                .and_then(|config| {
                    let stage_final = stage
                        .or_else(|| config.commit.and_then(|c| c.stage))
                        .unwrap_or_default();
                    step_commit(force, !verify, stage_final, show_prompt)
                }),
            StepCommand::Squash {
                target,
                force,
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
                        let ctx = env.context(force);
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

                    match handle_squash(target.as_deref(), force, !verify, stage_final)? {
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
            StepCommand::Push {
                target,
                allow_merge_commits,
            } => handle_push(target.as_deref(), allow_merge_commits, "Pushed to", None),
            StepCommand::Rebase { target } => {
                handle_rebase(target.as_deref()).and_then(|result| match result {
                    RebaseResult::Rebased => Ok(()),
                    RebaseResult::UpToDate(branch) => {
                        crate::output::print(info_message(format!(
                            "Already up-to-date with {branch}"
                        )))?;
                        Ok(())
                    }
                })
            }
            StepCommand::ForEach { args } => step_for_each(args),
        },
        Commands::Hook { action } => match action {
            HookCommand::Show {
                hook_type,
                expanded,
            } => handle_hook_show(hook_type.as_deref(), expanded),
            HookCommand::PostCreate { name, force } => {
                run_hook(HookType::PostCreate, force, name.as_deref())
            }
            HookCommand::PostStart { name, force } => {
                run_hook(HookType::PostStart, force, name.as_deref())
            }
            HookCommand::PreCommit { name, force } => {
                run_hook(HookType::PreCommit, force, name.as_deref())
            }
            HookCommand::PreMerge { name, force } => {
                run_hook(HookType::PreMerge, force, name.as_deref())
            }
            HookCommand::PostMerge { name, force } => {
                run_hook(HookType::PostMerge, force, name.as_deref())
            }
            HookCommand::PreRemove { name, force } => {
                run_hook(HookType::PreRemove, force, name.as_deref())
            }
            HookCommand::Approvals { action } => match action {
                ApprovalsCommand::Add { all } => add_approvals(all),
                ApprovalsCommand::Clear { global } => clear_approvals(global),
            },
        },
        #[cfg(unix)]
        Commands::Select => handle_select(cli.internal.is_some()),
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
            force,
            verify,
        } => WorktrunkConfig::load()
            .context("Failed to load config")
            .and_then(|config| {
                // "Approve at the Gate": collect and approve hooks upfront when creating
                // This ensures approval happens once at the command entry point
                // If user declines, skip hooks but continue with worktree creation
                let approved = if create && verify {
                    let repo = Repository::current();
                    let repo_root = repo.worktree_base()?;
                    // Compute worktree path for template expansion in approval prompt
                    let worktree_path = compute_worktree_path(&repo, &branch, &config)?;
                    let ctx = CommandContext::new(
                        &repo,
                        &config,
                        Some(&branch),
                        &worktree_path,
                        &repo_root,
                        force,
                    );
                    approve_hooks(&ctx, &[HookType::PostCreate, HookType::PostStart])?
                } else {
                    true // No hooks to approve = considered approved
                };

                // Skip hooks if --no-hooks or user declined approval
                let skip_hooks = !verify || !approved;

                // Show message if user declined approval
                if !approved {
                    crate::output::print(info_message(
                        "Commands declined, continuing worktree creation",
                    ))?;
                }

                // Execute switch operation (creates worktree, runs post-create hooks if approved)
                let (result, resolved_branch) =
                    handle_switch(&branch, create, base.as_deref(), force, skip_hooks, &config)?;

                // Show success message (temporal locality: immediately after worktree creation)
                // Pass cli.internal to indicate whether shell integration is active
                handle_switch_output(
                    &result,
                    &resolved_branch,
                    execute.is_some(),
                    cli.internal.is_some(),
                )?;

                // Now spawn post-start hooks (background processes, after success message)
                // Only run post-start commands when creating a NEW worktree, not when switching to existing
                // Hooks only run if --no-hooks wasn't passed and approval was granted (or --force used)
                if !skip_hooks && let SwitchResult::Created { path, .. } = &result {
                    let repo = Repository::current();
                    let repo_root = repo.worktree_base()?;
                    let ctx = CommandContext::new(
                        &repo,
                        &config,
                        Some(&resolved_branch),
                        path,
                        &repo_root,
                        force,
                    );
                    ctx.spawn_post_start_commands(true)?;
                }

                // Execute user command after post-start hooks have been spawned
                if let Some(cmd) = execute {
                    execute_user_command(&cmd)?;
                }

                Ok(())
            }),
        Commands::Remove {
            worktrees,
            delete_branch,
            force_delete,
            background,
            verify,
            force,
        } => WorktrunkConfig::load()
            .context("Failed to load config")
            .and_then(|config| {
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
                let repo = Repository::current();
                let verify = if verify {
                    // Create context for template expansion in approval prompt
                    let worktree_path =
                        std::env::current_dir().context("Failed to get current directory")?;
                    let repo_root = repo.worktree_base()?;
                    // Keep as Option so detached HEAD maps to None -> "HEAD" via branch_or_head()
                    let current_branch = repo.current_branch()?;
                    let ctx = CommandContext::new(
                        &repo,
                        &config,
                        current_branch.as_deref(),
                        &worktree_path,
                        &repo_root,
                        force,
                    );
                    let approved = approve_hooks(&ctx, &[HookType::PreRemove])?;
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

                if worktrees.is_empty() {
                    // No worktrees specified, remove current worktree
                    // Uses path-based removal to handle detached HEAD state
                    let result = handle_remove_current(!delete_branch, force_delete, background)?;
                    // Approval was handled at the gate
                    handle_remove_output(&result, None, background, verify)
                } else {
                    use worktrunk::git::ResolvedWorktree;
                    // When removing multiple worktrees, we need to handle the current worktree last
                    // to avoid deleting the directory we're currently in
                    let current_worktree = repo.worktree_root().ok();

                    // Partition worktrees into current, others, and branch-only using path-first
                    // resolution, which checks expected path before falling back to branch lookup.
                    // Collect errors for invalid names so we can report all of them.
                    let mut others = Vec::new();
                    let mut branch_only = Vec::new();
                    let mut current: Option<(PathBuf, Option<String>)> = None;
                    let mut errors: Vec<anyhow::Error> = Vec::new();

                    for worktree_name in &worktrees {
                        match resolve_worktree_path_first(&repo, worktree_name, &config) {
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
                                errors.push(e);
                            }
                        }
                    }

                    // If any names failed to resolve, report those errors
                    // (resolution errors are fatal - we can't proceed with unknown names)
                    if !errors.is_empty() {
                        for e in &errors {
                            output::print(error_message(e.to_string()))?;
                        }
                        anyhow::bail!(
                            "Failed to resolve {} of {} specified names",
                            errors.len(),
                            worktrees.len()
                        );
                    }

                    // Track removal errors so we can report all of them
                    let mut removal_errors: Vec<anyhow::Error> = Vec::new();

                    // Remove other worktrees first (approval was handled at the gate)
                    for (path, branch) in &others {
                        if let Some(branch_name) = branch {
                            match handle_remove(
                                branch_name,
                                !delete_branch,
                                force_delete,
                                background,
                            ) {
                                Ok(result) => {
                                    handle_remove_output(
                                        &result,
                                        Some(branch_name),
                                        background,
                                        verify,
                                    )?;
                                }
                                Err(e) => removal_errors.push(e),
                            }
                        } else {
                            // Non-current worktree is detached - remove by path (no branch to delete)
                            match handle_remove_by_path(path, None, force_delete, background) {
                                Ok(result) => {
                                    handle_remove_output(&result, None, background, verify)?;
                                }
                                Err(e) => removal_errors.push(e),
                            }
                        }
                    }

                    // Handle branch-only cases (no worktree)
                    for branch in &branch_only {
                        match handle_remove(branch, !delete_branch, force_delete, background) {
                            Ok(result) => {
                                handle_remove_output(&result, Some(branch), background, verify)?;
                            }
                            Err(e) => removal_errors.push(e),
                        }
                    }

                    // Remove current worktree last (if it was in the list)
                    if let Some((_path, branch)) = current {
                        match handle_remove_current(!delete_branch, force_delete, background) {
                            Ok(result) => {
                                handle_remove_output(
                                    &result,
                                    branch.as_deref(),
                                    background,
                                    verify,
                                )?;
                            }
                            Err(e) => removal_errors.push(e),
                        }
                    }

                    // Report all removal errors
                    if !removal_errors.is_empty() {
                        for e in &removal_errors {
                            // Errors already have their own formatting (emoji + color)
                            output::print(e.to_string())?;
                        }
                        // Only show summary for multiple targets (single target error is self-explanatory)
                        if worktrees.len() > 1 {
                            anyhow::bail!(
                                "Failed to remove {} of {} specified targets",
                                removal_errors.len(),
                                worktrees.len()
                            );
                        } else {
                            // Single target - error already printed, just exit with failure
                            // Terminate output before exiting (handles directive mode)
                            let _ = output::terminate_output();
                            process::exit(1);
                        }
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
            remove,
            no_remove,
            verify,
            no_verify,
            force,
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
                let remove_default = merge_config.and_then(|m| m.remove).unwrap_or(true);
                let verify_default = merge_config.and_then(|m| m.verify).unwrap_or(true);

                // CLI flags override config, config overrides defaults
                let squash_final = flag_pair(squash, no_squash).unwrap_or(squash_default);
                let commit_final = flag_pair(commit, no_commit).unwrap_or(commit_default);
                let remove_final = flag_pair(remove, no_remove).unwrap_or(remove_default);
                let verify_final = flag_pair(verify, no_verify).unwrap_or(verify_default);

                // Stage defaults from [commit] config section
                let stage_final = stage
                    .or_else(|| config.commit.and_then(|c| c.stage))
                    .unwrap_or_default();

                handle_merge(
                    target.as_deref(),
                    squash_final,
                    commit_final,
                    remove_final,
                    verify_final,
                    force,
                    stage_final,
                )
            }),
    };

    // Emit shell script (directive mode) or no-op (interactive mode)
    // Must be called before error handling so cd happens even on failure
    // (matches shell wrapper behavior which evals script regardless of exit code)
    let _ = output::terminate_output();

    if let Err(e) = result {
        use worktrunk::styling::ERROR_EMOJI;

        // GitError, WorktrunkError, and HookErrorWithHint produce styled output via Display
        if let Some(err) = e.downcast_ref::<worktrunk::git::GitError>() {
            let _ = output::print(err.to_string());
        } else if let Some(err) = e.downcast_ref::<worktrunk::git::WorktrunkError>() {
            let _ = output::print(err.to_string());
        } else if let Some(err) = e.downcast_ref::<worktrunk::git::HookErrorWithHint>() {
            let _ = output::print(err.to_string());
        } else {
            // Anyhow error - format with emoji, multi-line root cause gets gutter
            let msg = e.to_string();
            let root_cause = e.root_cause().to_string();
            let _ = output::print(cformat!("{ERROR_EMOJI} <red>{msg}</>"));
            if msg != root_cause && root_cause.contains('\n') {
                let _ = output::gutter(format_with_gutter(&root_cause, "", None));
            }
        }

        // Preserve exit code from child processes (especially for signals like SIGINT)
        let code = exit_code(&e).unwrap_or(1);
        process::exit(code);
    }
}
