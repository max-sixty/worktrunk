//! Help system with pager support and web documentation generation.
//!
//! This module provides:
//! - Pager support for `--help` output (git-style)
//! - Markdown rendering for help text
//! - Web documentation generation via `--help-page` and `--help-md`
//!
//! # Web docs generation (`--help-page`)
//!
//! Each command page flows through several transforms before becoming web docs:
//!
//! ```text
//! cli.rs (source of truth)
//!   ├── after_long_help: markdown prose with [experimental] markers, `●` dots, plain URLs
//!   └── doc comments (/// lines): definition + subtitle for lead paragraph
//!         │
//!         ▼
//! combine_command_docs()         — assembles "definition. subtitle\n\n<after_long_help>"
//!         │
//!         ▼
//! convert_dollar_console_to_terminal() — ```console with $ → {% terminal() %} shortcode
//! console→bash replacement             — remaining ```console → ```bash
//!         │
//!         ▼
//! post_process_for_html()        — text replacements on after_long_help markdown:
//!         │                        [experimental] → badge <span>
//!         │                        `●` green → colored <span>
//!         │                        plain URLs → markdown links
//!         ▼
//! --help-page stdout             — markdown with embedded HTML spans
//!         │
//!         ▼  (readme_sync.rs test captures and writes to docs/)
//!         │
//! convert_command_reference_to_html()  — backtick-fenced --help blocks → {% terminal() %}
//! expand_command_placeholders()        — ```bash wt list``` → snapshot terminal blocks
//!         │
//!         ▼
//! docs/content/{command}.md      — final markdown consumed by Zola
//! ```
//!
//! **Manually-written pages** (faq.md, llm-commits.md) bypass this pipeline.
//! They use `<span class="badge-experimental"></span>` directly for badges.
//!
//! **Skill reference files** mirror docs/ content via `transform_docs_for_skill()`,
//! which strips Zola syntax (terminal shortcodes, badge `<span>` → `[experimental]`)
//! for plain-markdown consumption.

use std::process;

use ansi_str::AnsiStr;
use clap::ColorChoice;
use clap::error::ErrorKind;
use worktrunk::docs::convert_dollar_console_to_terminal;
use worktrunk::styling::{eprintln, println};

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
///
/// `is_step_help` is computed by the caller from the same early-parse pass that
/// extracts global options, and controls whether we splice the configured
/// aliases into the rendered output.
pub fn maybe_handle_help_with_pager(is_step_help: bool) -> bool {
    let args: Vec<String> = std::env::args().collect();

    // --help uses pager, -h prints directly (git convention)
    let use_pager = args.iter().any(|a| a == "--help");

    // Check for --help-page flag (output full doc page with frontmatter)
    if args.iter().any(|a| a == "--help-page") {
        let plain = args.iter().any(|a| a == "--plain");
        handle_help_page(&args, plain);
        process::exit(0);
    }

    // Check for --help-description flag (output meta description for docs)
    if args.iter().any(|a| a == "--help-description") {
        handle_help_description(&args);
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
            print!("{output}");
            process::exit(0);
        }
        // Fall through if not a help request
    }

    let mut cmd = cli::build_command();
    cmd = cmd.color(clap::ColorChoice::Always); // Force clap to emit ANSI codes

    match cmd.try_get_matches_from_mut(&args) {
        Ok(_) => false, // Normal args, not help
        Err(err) => {
            match err.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                    // err.render() returns a StyledStr containing ANSI codes.
                    // Use .ansi() to preserve them; .to_string() strips ANSI codes.
                    let clap_output = err.render().ansi().to_string();

                    // Splice configured aliases into `wt step --help` / `-h`
                    // so the help here matches bare `wt step`. Scoped to the
                    // step subcommand only — other help passes through.
                    let clap_output = if is_step_help {
                        crate::commands::augment_step_help(&clap_output)
                    } else {
                        clap_output
                    };

                    // Render markdown sections (tables, code blocks, prose) with proper wrapping.
                    // Since we disabled clap's wrapping above, our renderer controls all line breaks.
                    let width = worktrunk::styling::terminal_width();
                    let help = crate::md_help::render_markdown_in_help_with_width(
                        &clap_output,
                        Some(width),
                    );

                    // show_help_in_pager checks if stdout or stderr is a TTY.
                    // If neither is a TTY (e.g., `wt --help &>file`), it skips the pager.
                    // use_pager=false for -h (short help), true for --help (long help)
                    if let Err(e) = crate::help_pager::show_help_in_pager(&help, use_pager) {
                        log::debug!("Pager invocation failed: {}", e);
                        println!("{}", help);
                    }
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
                    false
                }
            }
        }
    }
}

/// Get the help reference block with configurable color output.
///
/// `ColorChoice::Always` produces ANSI codes for HTML conversion (web docs).
/// `ColorChoice::Never` produces plain text (skill reference files).
fn help_reference_with_color(
    command_path: &[&str],
    width: Option<usize>,
    color: ColorChoice,
) -> String {
    let output = help_reference_inner(command_path, width, color);
    if matches!(color, ColorChoice::Always) {
        // Strip OSC 8 hyperlinks. Clap generates these from markdown links like [text](url),
        // but web docs convert ANSI to HTML via ansi_to_html which only handles SGR codes
        // (colors), not OSC sequences - hyperlinks leak through as garbage.
        worktrunk::styling::strip_osc8_hyperlinks(&output)
    } else {
        output
    }
}

fn help_reference_inner(command_path: &[&str], width: Option<usize>, color: ColorChoice) -> String {
    // Build args: ["wt", "config", "create", "--help"]
    let mut args: Vec<String> = vec!["wt".to_string()];
    args.extend(command_path.iter().map(|s| s.to_string()));
    args.push("--help".to_string());

    let mut cmd = cli::build_command();
    cmd = cmd.color(color);
    if let Some(w) = width {
        cmd = cmd.term_width(w);
    }

    let help_block = if let Err(err) = cmd.try_get_matches_from_mut(args)
        && matches!(
            err.kind(),
            ErrorKind::DisplayHelp | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        ) {
        let rendered = err.render();
        // .ansi() preserves ANSI codes; .to_string() strips them.
        // Use .ansi() only when colors are enabled (for web HTML conversion).
        let text = if matches!(color, ColorChoice::Always) {
            rendered.ansi().to_string()
        } else {
            rendered.to_string()
        };
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
fn handle_help_description(args: &[String]) {
    let mut cmd = cli::build_command();
    cmd = cmd.color(ColorChoice::Never);

    let subcommand = args
        .iter()
        .filter(|a| *a != "--help-description" && !a.starts_with('-') && !a.ends_with("/wt"))
        .find(|a| !a.contains("target/") && *a != "wt");

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
fn handle_help_page(args: &[String], plain: bool) {
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
    // Transform for web docs: $→terminal shortcode, console→bash, status colors, demo images
    // Subdocs are expanded separately so main Command reference comes first
    let parent_name = format!("wt {}", subcommand);
    let raw_help = combine_command_docs(sub);
    // Web mode: convert $ code blocks to Zola terminal shortcodes
    // Plain mode: skip shortcode conversion (skills consume plain markdown)
    let raw_help = if plain {
        raw_help
    } else {
        convert_dollar_console_to_terminal(&raw_help)
    };
    let raw_help = raw_help.replace("```console\n", "```bash\n");

    // Split content at first subdoc placeholder
    let subdoc_marker = "<!-- subdoc:";
    let (main_content, subdoc_content) = if let Some(pos) = raw_help.find(subdoc_marker) {
        (&raw_help[..pos], Some(&raw_help[pos..]))
    } else {
        (raw_help.as_str(), None)
    };

    // Process main content (before subdocs)
    // Web mode: expand demo GIFs and convert CLI markers to HTML
    // Plain mode: skip HTML transforms (skills consume plain markdown)
    let main_help = if plain {
        strip_demo_placeholders(main_content)
    } else {
        let text = expand_demo_placeholders(main_content);
        post_process_for_html(&text)
    };

    // Get the help reference block (wrap at 100 chars, with colors for HTML in web mode)
    let reference_block = help_reference_with_color(
        &[subcommand],
        Some(100),
        if plain {
            ColorChoice::Never
        } else {
            ColorChoice::Always
        },
    );

    // Use std::println! to preserve ANSI codes in output (the styling::println strips them)
    if plain {
        // Plain mode: H1 title, no AUTO-GENERATED markers
        std::println!("# wt {subcommand}");
        std::println!();
    } else {
        // Web mode: region markers for sync replacement (frontmatter is in skeleton files)
        std::println!(
            "<!-- ⚠️ AUTO-GENERATED from `wt {subcommand} --help-page` — edit cli.rs to update -->"
        );
        std::println!();
    }
    std::println!("{}", main_help.trim());
    std::println!();

    // Main command reference immediately after its content
    std::println!("## Command reference");
    std::println!();
    std::println!("```");
    std::print!("{}", reference_block.trim());
    std::println!();
    std::println!("```");

    // Subdocs follow, each with their own command reference at the end.
    if let Some(subdocs) = subdoc_content {
        let subdocs = if plain {
            subdocs.to_string()
        } else {
            // Apply post-processing to non-marker text (e.g., the Aliases section after
            // the last subdoc marker). Must happen before expansion — after expansion,
            // post_process_for_html has already run on each subcommand section internally
            // (in format_subcommand_section), so re-running it would double-convert.
            post_process_for_html(subdocs)
        };
        let subdocs_expanded = expand_subdoc_placeholders(&subdocs, sub, &parent_name, plain);
        std::println!();
        std::println!("# Subcommands");
        std::println!();
        std::println!("{}", subdocs_expanded.trim());
    }

    if !plain {
        std::println!();
        std::println!("<!-- END AUTO-GENERATED from `wt {subcommand} --help-page` -->");
    }
}

/// Post-process CLI help content for web docs rendering.
///
/// Applies text replacements to `after_long_help` content before it becomes markdown
/// in the docs site. Each replacement converts a CLI-friendly marker into styled HTML:
///
/// | CLI source | Web docs |
/// |------------|----------|
/// | `` `●` green `` | `<span style='color:#0a0'>●</span> green` |
/// | `[experimental]` | `<span class="badge-experimental"></span>` (text via CSS) |
/// | plain URL | markdown link |
/// | approval prompt code block | `{% terminal() %}` with colored symbols and gutter |
///
/// Only runs on `after_long_help` markdown — not on terminal reference blocks (those go
/// through ANSI-to-HTML via `convert_command_reference_to_html` in readme_sync.rs).
///
/// The terminal counterpart is `md_help::colorize_status_symbols()`.
fn post_process_for_html(text: &str) -> String {
    // First pass: move [experimental] from heading lines to a separate line after
    // the heading. This keeps the badge outside Zola's heading anchor link.
    // Terminal help keeps [experimental] on the heading line (different render path).
    let text = move_experimental_from_headings(text);

    text
        // CI status colors (in table cells)
        .replace("`●` green", "<span style='color:#0a0'>●</span> green")
        .replace("`●` blue", "<span style='color:#00a'>●</span> blue")
        .replace("`●` red", "<span style='color:#a00'>●</span> red")
        .replace("`●` yellow", "<span style='color:#a60'>●</span> yellow")
        .replace("`⚠` yellow", "<span style='color:#a60'>⚠</span> yellow")
        .replace("`●` gray", "<span style='color:#888'>●</span> gray")
        // Experimental badges — empty span, text added via CSS ::after.
        // Empty so the span doesn't affect Zola's heading slug generation.
        .replace(
            "[experimental]",
            "<span class=\"badge-experimental\"></span>",
        )
        // Convert plain URL references to markdown links for web docs
        // CLI shows: "Open an issue at https://github.com/max-sixty/worktrunk."
        // Web shows: "[Open an issue](https://github.com/max-sixty/worktrunk/issues)."
        .replace(
            "Open an issue at https://github.com/max-sixty/worktrunk.",
            "[Open an issue](https://github.com/max-sixty/worktrunk/issues).",
        )
        // Approval prompt: plain code block → terminal shortcode with colored symbols
        // and gutter. CLI shows a plain ``` block; web shows styled terminal output
        // matching the actual CLI appearance (yellow ▲, dim ○, cyan ❯, gutter bar).
        .replace(
            "```\n\
             ▲ repo needs approval to execute 3 commands:\n\
             \n\
             ○ pre-start install:\n\
             \x20\x20\x20npm ci\n\
             ○ pre-start build:\n\
             \x20\x20\x20cargo build --release\n\
             ○ pre-start env:\n\
             \x20\x20\x20echo 'PORT={{ branch | hash_port }}' > .env.local\n\
             \n\
             ❯ Allow and remember? [y/N]\n\
             ```",
            "{% terminal() %}\n\
             <span class=\"y\">▲ <b>repo</b> needs approval to execute <b>3</b> commands:</span>\n\
             \n\
             <span class=\"d\">○</span> pre-start <b>install</b>:\n\
             <span style='background:var(--bright-white,#fff)'> </span> <span class=\"d\"><span class=\"b\">npm</span> ci</span>\n\
             <span class=\"d\">○</span> pre-start <b>build</b>:\n\
             <span style='background:var(--bright-white,#fff)'> </span> <span class=\"d\"><span class=\"b\">cargo</span> build <span class=\"c\">--release</span></span>\n\
             <span class=\"d\">○</span> pre-start <b>env</b>:\n\
             <span style='background:var(--bright-white,#fff)'> </span> <span class=\"d\"><span class=\"b\">echo</span> <span class=\"g\">'PORT={{ branch | hash_port }}'</span> <span class=\"c\">></span> .env.local</span>\n\
             \n\
             <span class=\"c\">❯</span> Allow and remember? <b>[y/N]</b>\n\
             {% end %}",
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
/// This keeps the badge outside Zola's `<a class="zola-anchor">` wrapper so it's
/// not part of the heading link. The `[experimental]` is then replaced with the
/// badge `<span>` by the caller's `.replace()` chain.
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
    plain: bool,
) -> String {
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
                format_subcommand_section(sub, parent_name, subcommand_name, plain)
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
    plain: bool,
) -> String {
    // parent_name is "wt config", subcommand_name is "create"
    // full_command is "wt config create"
    let full_command = format!("{} {}", parent_name, subcommand_name);

    // Get combined docs: about + subtitle + after_long_help
    let raw_help = combine_command_docs(sub);
    let raw_help = if plain {
        raw_help
    } else {
        convert_dollar_console_to_terminal(&raw_help)
    };
    let raw_help = raw_help.replace("```console\n", "```bash\n");

    // Extract [experimental] marker from content start → badge after heading.
    // Web mode: placed after heading as HTML badge so Zola's anchor link doesn't wrap it.
    // Plain mode: kept as [experimental] text.
    let (has_experimental, raw_help) = if let Some(rest) = raw_help.strip_prefix("[experimental] ")
    {
        (true, rest.to_string())
    } else {
        (false, raw_help)
    };

    // Split content at first subdoc placeholder so command reference comes before nested subdocs
    let subdoc_marker = "<!-- subdoc:";
    let (main_content, subdoc_content) = if let Some(pos) = raw_help.find(subdoc_marker) {
        (&raw_help[..pos], Some(&raw_help[pos..]))
    } else {
        (raw_help.as_str(), None)
    };

    // Process main content (before any nested subdocs)
    let main_help = if plain {
        let text = increase_heading_levels(main_content);
        strip_demo_placeholders(&text)
    } else {
        let text = increase_heading_levels(main_content);
        post_process_for_html(&text)
    };

    // Build command path from parent_name: "wt config" -> ["config", "create"]
    let command_path: Vec<&str> = parent_name
        .strip_prefix("wt ")
        .unwrap_or(parent_name)
        .split_whitespace()
        .chain(std::iter::once(subcommand_name))
        .collect();

    // Get help reference with colors for web HTML conversion, plain text for skills
    let reference_block = help_reference_with_color(
        &command_path,
        Some(100),
        if plain {
            ColorChoice::Never
        } else {
            ColorChoice::Always
        },
    );

    // Format the section: heading, badge (outside heading), main content, command reference
    let mut section = format!("## {full_command}\n\n");
    if has_experimental {
        if plain {
            section.push_str("[experimental]\n\n");
        } else {
            section.push_str("<span class=\"badge-experimental\"></span>\n\n");
        }
    }

    if !main_help.is_empty() {
        section.push_str(main_help.trim());
        section.push_str("\n\n");
    }

    // Command reference comes after main content but before nested subdocs
    section.push_str("### Command reference\n\n```\n");
    section.push_str(reference_block.trim());
    section.push_str("\n```\n");

    // Expand nested subdocs after the command reference.
    if let Some(subdocs) = subdoc_content {
        let subdocs = if plain {
            subdocs.to_string()
        } else {
            post_process_for_html(subdocs)
        };
        let subdocs_expanded = expand_subdoc_placeholders(&subdocs, sub, &full_command, plain);
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
    const PREFIX: &str = "<!-- demo: ";
    const SUFFIX: &str = " -->";

    let mut result = text.to_string();
    while let Some(start) = result.find(PREFIX) {
        let after_prefix = start + PREFIX.len();
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
