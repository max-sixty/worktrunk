// Custom completion implementation rather than clap's unstable-dynamic feature.
//
// While clap_complete offers CompleteEnv and ArgValueCompleter traits, we implement
// our own completion logic because:
// - unstable-dynamic is an unstable API that may change between versions
// - We need conditional completion logic (e.g., don't complete branches when --create is present)
// - We need runtime-fetched values (git branches) with context-aware filtering
// - We need precise control over positional argument state tracking with flags
//
// This approach uses stable APIs and handles edge cases that clap's completion system
// isn't designed for. See the extensive test suite in tests/integration_tests/completion.rs

use clap::{Arg, Command, CommandFactory};
use worktrunk::git::{GitError, Repository};
use worktrunk::styling::{ERROR, ERROR_EMOJI, println};

/// Completion item with optional help text for fish shell descriptions
#[derive(Debug)]
struct Item {
    name: String,
    help: Option<String>,
}

/// Represents what we're trying to complete
#[derive(Debug)]
enum CompletionTarget<'a> {
    /// Completing a value for an option flag (e.g., `--base <value>` or `--base=<value>`)
    Option(&'a Arg, String), // (clap Arg, prefix to complete)
    /// Completing a positional branch argument (switch/push/merge/remove commands)
    PositionalBranch(String), // prefix to complete
    /// No special completion needed
    Unknown,
}

/// Print completion items in fish-friendly format (name\thelp)
/// Other shells ignore the tab separator and just use the name
fn print_items(items: impl IntoIterator<Item = Item>) {
    for Item { name, help } in items {
        if let Some(help) = help {
            println!("{name}\t{help}");
        } else {
            println!("{name}");
        }
    }
}

/// Check if a positional argument should be completed
/// Returns true if we're still completing the first positional arg
/// Returns false if the positional arg has been provided and we've moved past it
fn should_complete_positional_arg(args: &[String], start_index: usize) -> bool {
    let mut i = start_index;

    while i < args.len() {
        let arg = &args[i];

        if arg == "--base" || arg == "-b" {
            // Skip flag and its value
            i += 2;
        } else if arg.starts_with("--") || (arg.starts_with('-') && arg.len() > 1) {
            // Skip other flags
            i += 1;
        } else if !arg.is_empty() {
            // Found a positional argument
            // Only continue completing if it's at the last position
            return i >= args.len() - 1;
        } else {
            // Empty string (cursor position)
            i += 1;
        }
    }

    // No positional arg found yet - should complete
    true
}

fn get_branches_for_completion<F>(get_branches_fn: F) -> Vec<String>
where
    F: FnOnce() -> Result<Vec<String>, GitError>,
{
    get_branches_fn().unwrap_or_else(|e| {
        if std::env::var("WT_DEBUG_COMPLETION").is_ok() {
            println!("{ERROR_EMOJI} {ERROR}Completion error: {e}{ERROR:#}");
        }
        Vec::new()
    })
}

/// Extract possible values from a clap Arg (handles ValueEnum and explicit PossibleValue lists)
fn items_from_arg(arg: &Arg, prefix: &str) -> Vec<Item> {
    // Read clap's declared possible values (ValueEnum or explicit PossibleValue list)
    let possible_values = arg.get_possible_values();

    if possible_values.is_empty() {
        return Vec::new();
    }

    possible_values
        .into_iter()
        .filter(|pv| !pv.is_hide_set())
        .map(|pv| {
            let name = pv.get_name().to_string();
            let help = pv.get_help().map(|s| s.to_string());
            (name, help)
        })
        // Do a cheap prefix filter; shells will filter too, but this helps bash/zsh
        .filter(|(name, _)| name.starts_with(prefix))
        .map(|(name, help)| Item { name, help })
        .collect()
}

/// Detect what we're trying to complete using clap introspection
/// Handles both --arg value and --arg=value formats
fn detect_completion_target<'a>(args: &[String], cmd: &'a Command) -> CompletionTarget<'a> {
    if args.len() < 2 {
        return CompletionTarget::Unknown;
    }

    // Find the active subcommand frame by walking the command tree
    let mut i = 1; // Skip binary name
    let mut cur = cmd;
    let mut subcommand_name = None;
    while i < args.len() {
        let tok = &args[i];
        // Skip global flags
        if tok == "--source" || tok == "--internal" || tok == "-v" || tok == "--verbose" {
            i += 1;
            continue;
        }
        if let Some(sc) = cur.find_subcommand(tok) {
            subcommand_name = Some(sc.get_name());
            cur = sc;
            i += 1;
        } else {
            break;
        }
    }

    let last = args.last().map(String::as_str).unwrap_or("");
    let prev = args.iter().rev().nth(1).map(|s| s.as_str());

    // Check for --arg=value format in last argument
    if let Some(equals_pos) = last.find('=') {
        let flag_part = &last[..equals_pos];
        let value_part = &last[equals_pos + 1..];

        // Long form: --name=value
        if let Some(long) = flag_part.strip_prefix("--")
            && let Some(arg) = cur
                .get_opts()
                .find(|a| a.get_long().is_some_and(|l| l == long))
        {
            return CompletionTarget::Option(arg, value_part.to_string());
        }

        // Short form: -n=value
        if let Some(short) = flag_part
            .strip_prefix('-')
            .filter(|s| s.len() == 1)
            .and_then(|s| s.chars().next())
            && let Some(arg) = cur.get_opts().find(|a| a.get_short() == Some(short))
        {
            return CompletionTarget::Option(arg, value_part.to_string());
        }
    }

    // Check for --arg value format (space-separated)
    if let Some(p) = prev {
        // Long form: --name
        if let Some(long) = p.strip_prefix("--")
            && let Some(arg) = cur
                .get_opts()
                .find(|a| a.get_long().is_some_and(|l| l == long))
        {
            return CompletionTarget::Option(arg, last.to_string());
        }

        // Short form: -n
        if let Some(short) = p
            .strip_prefix('-')
            .filter(|s| s.len() == 1)
            .and_then(|s| s.chars().next())
            && let Some(arg) = cur.get_opts().find(|a| a.get_short() == Some(short))
        {
            return CompletionTarget::Option(arg, last.to_string());
        }
    }

    // Check if we're completing a positional branch argument
    // Special handling for switch --create: don't complete when creating new branches
    if let Some(subcmd) = subcommand_name {
        match subcmd {
            "switch" => {
                let has_create = args.iter().any(|arg| arg == "--create" || arg == "-c");
                if !has_create && should_complete_positional_arg(args, i) {
                    return CompletionTarget::PositionalBranch(last.to_string());
                }
            }
            "push" | "merge" | "remove" => {
                if should_complete_positional_arg(args, i) {
                    return CompletionTarget::PositionalBranch(last.to_string());
                }
            }
            _ => {}
        }
    }

    CompletionTarget::Unknown
}

pub fn handle_complete(args: Vec<String>) -> Result<(), GitError> {
    let mut cmd = crate::cli::Cli::command();
    cmd.build(); // Required for introspection

    let target = detect_completion_target(&args, &cmd);

    match target {
        CompletionTarget::Option(arg, prefix) => {
            // Check if this is the "base" option that needs branch completion
            if arg.get_long() == Some("base") {
                // Complete with all branches (runtime-fetched values)
                let branches = get_branches_for_completion(|| Repository::current().all_branches());
                for branch in branches {
                    println!("{}", branch);
                }
            } else {
                // Use the arg's declared possible_values (ValueEnum types)
                let items = items_from_arg(arg, &prefix);
                if !items.is_empty() {
                    print_items(items);
                }
            }
        }
        CompletionTarget::PositionalBranch(_prefix) => {
            // Complete with all branches (runtime-fetched values)
            let branches = get_branches_for_completion(|| Repository::current().all_branches());
            for branch in branches {
                println!("{}", branch);
            }
        }
        CompletionTarget::Unknown => {
            // Check for positionals with ValueEnum possible_values (e.g., init <Shell>, beta run-hook <HookType>)
            // Walk the command tree to find the active subcommand
            let mut i = 1;
            let mut cur = &cmd;
            while i < args.len() {
                let tok = &args[i];
                if tok == "--source" || tok == "--internal" || tok == "-v" || tok == "--verbose" {
                    i += 1;
                    continue;
                }
                if let Some(sc) = cur.find_subcommand(tok) {
                    cur = sc;
                    i += 1;
                } else {
                    break;
                }
            }

            let last = args.last().map(String::as_str).unwrap_or("");

            // Check if there's a positional with possible_values
            if let Some(arg) = cur
                .get_positionals()
                .find(|a| !a.get_possible_values().is_empty())
            {
                let items = items_from_arg(arg, last);
                if !items.is_empty() {
                    print_items(items);
                }
            }
        }
    }

    Ok(())
}
