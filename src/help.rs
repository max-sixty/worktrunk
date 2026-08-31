//! Help system with pager support and web documentation generation.
//!
//! This module provides:
//! - Pager support for `--help` output (git-style)
//! - Markdown rendering for help text
//! - Web documentation generation via `--help-page` and `--help-md`
//!
//! # Web docs generation (`--help-page`)
//!
//! Each command page flows through a small set of transforms before becoming
//! portable Markdown:
//!
//! ```text
//! cli.rs (source of truth)
//!   ├── after_long_help: markdown prose with [experimental] markers, `#` CI samples, plain URLs
//!   └── doc comments (/// lines): definition + subtitle for lead paragraph
//!         │
//!         ▼
//! combine_command_docs()         — assembles "definition. subtitle\n\n<after_long_help>"
//!         │
//!         ▼
//! post_process_for_html()        — text replacements on after_long_help markdown:
//!         │                        [experimental] → badge <span>
//!         │                        `#` green → colored <span>
//!         │                        plain URLs → markdown links
//!         ▼
//! --help-page stdout             — Markdown with small semantic HTML spans
//!         │
//!         ▼  (readme_sync.rs test captures and writes to docs/)
//!         │
//! expand_command_placeholders()        — command placeholders → snapshot console blocks
//!         │
//!         ▼
//! docs/src/content/docs/{command}.md — final Markdown consumed by Astro
//! ```
//!
//! **Manually-written pages** (faq.md, llm-commits.md) bypass this pipeline.
//! They use `<span class="badge-experimental"></span>` directly for badges.
//!
//! **Skill reference files** mirror docs content via `transform_docs_for_skill()`,
//! which strips site-only elements such as demo figures and converts badge
//! `<span>` elements back to `[experimental]`.

use std::ffi::OsString;
use std::process;

use ansi_str::AnsiStr;
use clap::error::ErrorKind;
use worktrunk::docs::{
    BADGE_EXPERIMENTAL_HTML, DEMO_MARKER_PREFIX, MARKER_CLOSE, MARKER_OPEN_PREFIX,
    SUBDOC_MARKER_PREFIX,
};
use worktrunk::styling::{ColorChoice, eprintln, print, println};

use crate::cli;

/// Output format for a `--help-page`. Web pages keep semantic HTML spans and
/// demo GIF figures for the docs site; plain pages strip those for skill
/// reference files that need portable Markdown.
#[derive(Clone, Copy)]
enum PageMode {
    Web,
    Plain,
}

impl PageMode {
    /// Transform the body (everything before subdoc placeholders). Web expands
    /// demo GIFs into `<picture>` figures and converts CLI markers into HTML
    /// spans; plain strips demo placeholders so they don't leak as comments.
    fn process_body(self, text: &str) -> String {
        match self {
            Self::Web => post_process_for_html(&expand_demo_placeholders(text)),
            Self::Plain => strip_demo_placeholders(text),
        }
    }

    /// Transform subdoc trailing content (e.g. an `Aliases` section after the
    /// last subdoc marker). Web post-processes CLI markers; plain passes
    /// through unchanged — each subdoc's body is processed during expansion.
    fn process_subdoc_trailing(self, text: &str) -> String {
        match self {
            Self::Web => post_process_for_html(text),
            Self::Plain => text.to_string(),
        }
    }

    /// Rendering for the `[experimental]` badge.
    fn experimental_marker(self) -> &'static str {
        match self {
            Self::Web => BADGE_EXPERIMENTAL_HTML,
            Self::Plain => "[experimental]",
        }
    }

    /// Opening fence for generated command references. The web marker gives
    /// the renderer explicit ownership; plain skill pages stay portable.
    fn command_reference_fence(self) -> &'static str {
        match self {
            Self::Web => "```text wt-command-reference",
            Self::Plain => "```",
        }
    }

    /// Emit the page header — an auto-generated comment for web (sync test
    /// uses it as a region marker), or an H1 title for plain skill pages.
    fn emit_header(self, subcommand: &str) {
        match self {
            Self::Web => println!(
                "{MARKER_OPEN_PREFIX}`wt {subcommand} --help-page` — edit src/cli/mod.rs to update -->"
            ),
            Self::Plain => println!("# wt {subcommand}"),
        }
        println!();
    }

    /// Emit the closing END marker (web only).
    fn emit_footer(self) {
        if matches!(self, Self::Web) {
            println!();
            println!("{MARKER_CLOSE}");
        }
    }
}

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
/// On a help/version/doc-generation request, prints output and calls
/// `process::exit(0)`. Otherwise returns so the caller can continue normal parsing.
///
/// `alias_help_context` and `subcommand` both come from the caller's
/// early-parse pass over the real argv, the same one that extracts global
/// options. When `alias_help_context` is `Some`, the configured aliases are
/// spliced into the rendered output — at the top level for `wt --help`, or
/// under the Aliases section for `wt step --help`. `subcommand` is the command
/// the doc-generation entry points below render, taken from clap rather than
/// re-scanned here, so `wt.exe`, a renamed binary, or `wt -C <path> list
/// --print-schema` all name the same thing.
pub fn maybe_handle_help_with_pager(
    alias_help_context: Option<crate::commands::HelpContext>,
    subcommand: Option<&str>,
) {
    let args_os: Vec<OsString> = std::env::args_os().collect();
    let args: Vec<String> = args_os
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();

    // --help uses pager, -h prints directly (git convention)
    let use_pager = args.iter().any(|a| a == "--help");

    // Check for --help-page flag (output full doc page with frontmatter)
    if args.iter().any(|a| a == "--help-page") {
        let mode = if args.iter().any(|a| a == "--plain") {
            PageMode::Plain
        } else {
            PageMode::Web
        };
        handle_help_page(subcommand, mode);
        process::exit(0);
    }

    // Check for --print-schema flag (output the JSON Schema for a command's
    // --format=json payload)
    if args.iter().any(|a| a == "--print-schema") {
        handle_print_schema(subcommand);
        process::exit(0);
    }

    // Check for --help-description flag (output meta description for docs)
    if args.iter().any(|a| a == "--help-description") {
        handle_help_description(subcommand);
        process::exit(0);
    }

    // Check for --help-md flag (output raw markdown without ANSI rendering)
    if args.iter().any(|a| a == "--help-md") {
        let mut cmd = cli::build_command();
        cmd = crate::completion::inject_hook_subcommands(cmd);

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
            // Escape-free markdown on stdout is this entry point's contract,
            // tty or not, so the choice is declared rather than left to the
            // stream's tty check.
            ColorChoice::Never.write_global();
            let output = normalize_clap_help_fences(&err.render().ansi().to_string());
            print!("{output}");
            process::exit(0);
        }
        // Fall through if not a help request
    }

    let mut cmd = cli::build_command();
    cmd = crate::completion::inject_hook_subcommands(cmd);

    if let Err(err) = cmd.try_get_matches_from_mut(args_os) {
        match err.kind() {
            ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                // .ansi() renders the StyledStr's styles as escapes; its
                // Display renders the text without them.
                let clap_output = err.render().ansi().to_string();

                // Splice configured aliases into `wt --help` and
                // `wt step --help` (and their `-h` / bare-subcommand
                // equivalents) so the help surfaces both the built-ins and
                // the user's aliases. Other help pages pass through.
                let clap_output = match alias_help_context {
                    Some(ctx) => crate::commands::augment_help(&clap_output, ctx),
                    None => clap_output,
                };

                // Render markdown sections (tables, code blocks, prose) with proper wrapping.
                // Since we disabled clap's wrapping above, our renderer controls all line breaks.
                let help = crate::md_help::render_markdown_in_help_with_width(
                    &clap_output,
                    worktrunk::styling::terminal_width(),
                );

                // show_help_in_pager checks if stdout or stderr is a TTY.
                // If neither is a TTY (e.g., `wt --help &>file`), it skips the pager.
                // use_pager=false for -h (short help), true for --help (long help)
                crate::help_pager::show_help_in_pager(&help, use_pager);
                process::exit(0);
            }
            ErrorKind::DisplayVersion => {
                // Print to stdout — POSIX convention, and scripts rely on
                // `version=$(wt --version)` working without redirection (#2072).
                // Use print! because clap's Error Display already includes a trailing newline.
                print!("{}", err);
                process::exit(0);
            }
            _ => {
                // Not help or version - will be re-parsed by Cli::parse()
            }
        }
    }
}

/// The help reference block, normalized for portable Markdown output.
fn help_reference(command_path: &[&str], width: Option<usize>) -> String {
    let output = help_reference_inner(command_path, width);
    // Clap turns Markdown links into OSC 8 hyperlinks even when color is disabled.
    // Escape sequences are not valid portable Markdown, so retain their visible
    // labels and discard the terminal-only control bytes.
    worktrunk::styling::strip_osc8_hyperlinks(&output)
}

/// Normalize fences in clap-rendered help output for downstream Markdown
/// consumers. `--help-md` and `help_reference_inner` both run this.
fn normalize_clap_help_fences(text: &str) -> String {
    text
        // clap's default for usage blocks — strip the language for plain markdown
        .replace("```text\n", "```\n")
        // our authored examples — relabel for syntax highlighting in docs/skills
        .replace("```console\n", "```bash\n")
}

fn help_reference_inner(command_path: &[&str], width: Option<usize>) -> String {
    // Build args: ["wt", "config", "create", "--help"]
    let mut args: Vec<String> = vec!["wt".to_string()];
    args.extend(command_path.iter().map(|s| s.to_string()));
    args.push("--help".to_string());

    let mut cmd = cli::build_command();
    cmd = crate::completion::inject_hook_subcommands(cmd);
    if let Some(w) = width {
        cmd = cmd.term_width(w);
    }

    let help_block = if let Err(err) = cmd.try_get_matches_from_mut(args)
        && matches!(
            err.kind(),
            ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        ) {
        normalize_clap_help_fences(&err.render().ansi().to_string())
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
        let plain_line = line.ansi_strip();

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

/// Extract the `about` (definition) and subtitle from a command's metadata.
///
/// The subtitle is the part of `long_about` beyond the short `about`.
/// For `/// Short\n///\n/// Long description`, about = "Short", subtitle = "Long description".
fn extract_about_and_subtitle(cmd: &clap::Command) -> (Option<String>, Option<String>) {
    let about = cmd.get_about().map(|s| s.to_string());
    let long_about = cmd.get_long_about().map(|s| s.to_string());
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
    (about, subtitle)
}

/// Output the meta description for a command's docs page.
///
/// Combines the command's `about` (definition) and `long_about` subtitle into
/// a single description suitable for `<meta name="description">`. This is used
/// by the docs sync test to auto-populate the `description` field in frontmatter.
fn handle_help_description(subcommand: Option<&str>) {
    let mut cmd = cli::build_command();
    cmd = crate::completion::inject_hook_subcommands(cmd);

    let Some(subcommand) = subcommand else {
        eprintln!("Usage: wt <command> --help-description");
        return;
    };

    let Some(sub) = cmd.find_subcommand(subcommand) else {
        eprintln!("Unknown command: {subcommand}");
        return;
    };

    let (about, subtitle) = extract_about_and_subtitle(sub);

    let description = match (&about, &subtitle) {
        (Some(def), Some(sub)) => format!("{def}. {sub}"),
        (Some(def), None) => format!("{def}."),
        _ => String::new(),
    };

    print!("{description}");
}

/// Print the JSON Schema for a command's `--format=json` payload.
///
/// A developer entry point in the same family as `--help-page`: it exists so
/// the docs sync (`test_docs_are_in_sync`) can regenerate a committed schema
/// file by running the binary, which is the only way a `tests/` integration
/// test can reach a type in the bin-only `crate::commands` tree.
///
/// One command has a schema today, because one payload is versioned. The
/// subcommand is the axis a second would arrive on.
fn handle_print_schema(subcommand: Option<&str>) {
    let Some(subcommand) = subcommand else {
        eprintln!(
            "Usage: wt <command> --print-schema
Commands with schemas: list"
        );
        process::exit(2);
    };

    if subcommand != "list" {
        eprintln!("No JSON schema for '{subcommand}'; commands with schemas: list");
        process::exit(2);
    }

    let schema = crate::commands::list::json_v2::schema_document();
    crate::output::print_json(&schema).expect("schema serializes");
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
/// ```text wt-command-reference
/// wt merge — ...
/// Usage: ...
/// ```
/// ```
///
/// This is used to generate docs/src/content/docs/merge.md etc from the source.
fn handle_help_page(subcommand: Option<&str>, mode: PageMode) {
    // Help pages are portable Markdown, so terminal color escapes never enter
    // the generated source. The website adds syntax and prompt styling when
    // it renders fenced blocks.
    ColorChoice::Never.write_global();

    let mut cmd = cli::build_command();
    cmd = crate::completion::inject_hook_subcommands(cmd);

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

    let parent_name = format!("wt {}", subcommand);
    let raw_help = combine_command_docs(sub);

    // Subdocs are expanded separately so the main command reference comes first.
    let (main_content, subdoc_content) = match raw_help.find(SUBDOC_MARKER_PREFIX) {
        Some(pos) => (&raw_help[..pos], Some(&raw_help[pos..])),
        None => (raw_help.as_str(), None),
    };

    let main_help = mode.process_body(main_content);
    let reference_block = help_reference(&[subcommand], Some(100));

    mode.emit_header(subcommand);
    println!("{}", main_help.trim());
    println!();

    // Main command reference immediately after its content
    println!("## Command reference");
    println!();
    println!("{}", mode.command_reference_fence());
    println!("{}", reference_block.trim());
    println!("```");

    // Subdocs follow, each with their own command reference at the end.
    if let Some(subdocs) = subdoc_content {
        // Process trailing text (e.g., an Aliases section after the last subdoc
        // marker) before expansion — each subdoc's own body is post-processed
        // inside format_subcommand_section, so re-running would double-convert.
        let subdocs = mode.process_subdoc_trailing(subdocs);
        let subdocs_expanded = expand_subdoc_placeholders(&subdocs, sub, &parent_name, mode);
        println!();
        println!("# Subcommands");
        println!();
        println!("{}", subdocs_expanded.trim());
    }

    mode.emit_footer();
}

/// Post-process CLI help content for web docs rendering.
///
/// Applies text replacements to `after_long_help` content before it becomes markdown
/// in the docs site. Each replacement converts a CLI-friendly marker into styled HTML:
///
/// | CLI source | Web docs |
/// |------------|----------|
/// | `` `#` green `` | `<span style='color:#0a0'>#</span> green` |
/// | `[experimental]` | `<span class="badge-experimental"></span>` (text via CSS) |
/// | plain URL | markdown link |
/// | approval prompt code block | unchanged portable Markdown |
///
/// Only runs on `after_long_help` Markdown, not on terminal reference blocks.
///
/// The terminal counterpart is `md_help::colorize_status_symbols()`.
fn post_process_for_html(text: &str) -> String {
    // First pass: move [experimental] from heading lines to a separate line after
    // the heading. This keeps the badge out of the generated anchor slug.
    // Terminal help keeps [experimental] on the heading line (different render path).
    let text = move_experimental_from_headings(text);

    text
        // CI status colors (in table cells)
        .replace("`#` green", "<span style='color:#0a0'>#</span> green")
        .replace("`#` blue", "<span style='color:#00a'>#</span> blue")
        .replace("`#` red", "<span style='color:#a00'>#</span> red")
        .replace("`#` magenta", "<span style='color:#a0a'>#</span> magenta")
        .replace("`#` cyan", "<span style='color:#0aa'>#</span> cyan")
        .replace("`#` yellow", "<span style='color:#a60'>#</span> yellow")
        .replace("`⚠` yellow", "<span style='color:#a60'>⚠</span> yellow")
        .replace("`#` gray", "<span style='color:#888'>#</span> gray")
        .replace("[experimental]", BADGE_EXPERIMENTAL_HTML)
        // Convert plain URL references to markdown links for web docs
        // CLI shows: "Open an issue at https://github.com/max-sixty/worktrunk."
        // Web shows: "[Open an issue](https://github.com/max-sixty/worktrunk/issues)."
        .replace(
            "Open an issue at https://github.com/max-sixty/worktrunk.",
            "[Open an issue](https://github.com/max-sixty/worktrunk/issues).",
        )
}

/// Move `[experimental]` from heading lines to a separate line after the heading.
///
/// Transforms `## Foo [experimental]` into:
/// ```text
/// ## Foo
///
/// [experimental]
/// ```
///
/// This keeps the badge out of the heading text and therefore out of its slug.
/// The `[experimental]` is then replaced with the badge `<span>` by the
/// caller's `.replace()` chain.
fn move_experimental_from_headings(text: &str) -> String {
    if !text.contains(" [experimental]") {
        return text.to_string();
    }

    let mut result = String::with_capacity(text.len());
    let mut in_code_block = false;

    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
        }

        if !in_code_block
            && line.starts_with('#')
            && let Some(heading) = line.strip_suffix(" [experimental]")
        {
            result.push_str(heading);
            result.push_str("\n\n[experimental]");
        } else {
            result.push_str(line);
        }
        result.push('\n');
    }
    // .lines() strips the trailing newline; restore original behavior
    if !text.ends_with('\n') {
        result.pop();
    }
    result
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
fn expand_subdoc_placeholders(
    text: &str,
    parent_cmd: &clap::Command,
    parent_name: &str,
    mode: PageMode,
) -> String {
    const SUFFIX: &str = " -->";

    let mut result = text.to_string();
    while let Some(start) = result.find(SUBDOC_MARKER_PREFIX) {
        let after_prefix = start + SUBDOC_MARKER_PREFIX.len();
        if let Some(end_offset) = result[after_prefix..].find(SUFFIX) {
            let subcommand_name = result[after_prefix..after_prefix + end_offset].trim();
            let end = after_prefix + end_offset + SUFFIX.len();

            // Find the subcommand in the parent
            let replacement = if let Some(sub) = parent_cmd
                .get_subcommands()
                .find(|s| s.get_name() == subcommand_name)
            {
                format_subcommand_section(sub, parent_name, subcommand_name, mode)
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
    let (about, subtitle) = extract_about_and_subtitle(cmd);
    let after_long_help = cmd
        .get_after_long_help()
        .map(|s| s.to_string())
        .unwrap_or_default();

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
    mode: PageMode,
) -> String {
    // parent_name is "wt config", subcommand_name is "create"
    // full_command is "wt config create"
    let full_command = format!("{} {}", parent_name, subcommand_name);

    let raw_help = combine_command_docs(sub);

    // Extract [experimental] marker from content start → badge after heading.
    // Web mode: placed after the heading so it does not affect the anchor slug.
    // Plain mode: kept as [experimental] text.
    let (has_experimental, raw_help) = match raw_help.strip_prefix("[experimental] ") {
        Some(rest) => (true, rest.to_string()),
        None => (false, raw_help),
    };

    // Split content at first subdoc placeholder so command reference comes before nested subdocs
    let (main_content, subdoc_content) = match raw_help.find(SUBDOC_MARKER_PREFIX) {
        Some(pos) => (&raw_help[..pos], Some(&raw_help[pos..])),
        None => (raw_help.as_str(), None),
    };

    let main_help = mode.process_body(&increase_heading_levels(main_content));

    // Build command path from parent_name: "wt config" -> ["config", "create"]
    let command_path: Vec<&str> = parent_name
        .strip_prefix("wt ")
        .unwrap_or(parent_name)
        .split_whitespace()
        .chain(std::iter::once(subcommand_name))
        .collect();

    let reference_block = help_reference(&command_path, Some(100));

    // Format the section: heading, badge (outside heading), main content, command reference
    let mut section = format!("## {full_command}\n\n");
    if has_experimental {
        section.push_str(mode.experimental_marker());
        section.push_str("\n\n");
    }

    if !main_help.is_empty() {
        section.push_str(main_help.trim());
        section.push_str("\n\n");
    }

    // Command reference comes after main content but before nested subdocs
    section.push_str("### Command reference\n\n");
    section.push_str(mode.command_reference_fence());
    section.push('\n');
    section.push_str(reference_block.trim());
    section.push_str("\n```\n");

    // Expand nested subdocs after the command reference.
    if let Some(subdocs) = subdoc_content {
        let subdocs = mode.process_subdoc_trailing(subdocs);
        let subdocs_expanded = expand_subdoc_placeholders(&subdocs, sub, &full_command, mode);
        section.push('\n');
        section.push_str(subdocs_expanded.trim());
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
    const SUFFIX: &str = " -->";

    let mut result = text.to_string();
    while let Some(start) = result.find(DEMO_MARKER_PREFIX) {
        let after_prefix = start + DEMO_MARKER_PREFIX.len();
        if let Some(end_offset) = result[after_prefix..].find(SUFFIX) {
            let content = &result[after_prefix..after_prefix + end_offset];
            // Parse "filename.gif" or "filename.gif 1600x900"
            let mut parts = content.split_whitespace();
            let filename = parts.next().unwrap_or("");
            let dimensions = parts.next(); // Optional "WIDTHxHEIGHT"

            // Extract command name from filename (e.g., "wt-switch-picker.gif" -> "wt switch picker")
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

/// Strip demo GIF placeholders from content (for plain/skill output).
///
/// Removes `<!-- demo: filename.gif -->` lines entirely. These are invisible in
/// terminal --help but would leak as HTML comments in skill reference files.
fn strip_demo_placeholders(text: &str) -> String {
    const SUFFIX: &str = " -->";

    let mut result = text.to_string();
    while let Some(start) = result.find(DEMO_MARKER_PREFIX) {
        let after_prefix = start + DEMO_MARKER_PREFIX.len();
        if let Some(end_offset) = result[after_prefix..].find(SUFFIX) {
            let end = after_prefix + end_offset + SUFFIX.len();
            // Also strip trailing newline if present
            let end = if result[end..].starts_with('\n') {
                end + 1
            } else {
                end
            };
            result.replace_range(start..end, "");
        } else {
            break;
        }
    }
    result
}
