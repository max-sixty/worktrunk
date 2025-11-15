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
use std::sync::OnceLock;
use worktrunk::git::{GitError, Repository};
use worktrunk::styling::{ERROR, ERROR_EMOJI, println};

/// Cache the built command to avoid repeated construction cost
static CMD: OnceLock<Command> = OnceLock::new();

/// Get or build the cached command
fn built_cmd() -> &'static Command {
    CMD.get_or_init(|| {
        let mut cmd = crate::cli::Cli::command();
        cmd.build();
        cmd
    })
}

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

/// True if `arg` takes one or more values (vs. boolean/COUNT flags)
fn takes_value(arg: &Arg) -> bool {
    arg.get_num_args().is_some_and(|n| n.max_values() > 0)
}

/// Build a fast lookup of options (active subcommand + globals) -> takes_value
fn build_opt_index<'a>(
    active: &'a Command,
    root: &'a Command,
) -> (
    std::collections::HashMap<String, bool>,
    std::collections::HashMap<char, bool>,
) {
    use std::collections::HashMap;

    let mut long_map: HashMap<String, bool> = HashMap::new();
    let mut short_map: HashMap<char, bool> = HashMap::new();

    // Local opts
    for opt in active.get_opts() {
        if let Some(l) = opt.get_long() {
            long_map.insert(l.to_string(), takes_value(opt));
        }
        if let Some(s) = opt.get_short() {
            short_map.insert(s, takes_value(opt));
        }
    }

    // Global opts
    for opt in root.get_opts() {
        if opt.is_global_set() {
            if let Some(l) = opt.get_long() {
                long_map.entry(l.to_string()).or_insert(takes_value(opt));
            }
            if let Some(s) = opt.get_short() {
                short_map.entry(s).or_insert(takes_value(opt));
            }
        }
    }

    (long_map, short_map)
}

/// Check if a positional argument should be completed using clap introspection
///
/// This function uses clap metadata to determine whether we're still completing the first
/// positional argument. It handles:
/// - Flags that take values (--flag value, --flag=value, -f value, -fvalue)
/// - Short flag clusters (-abc where any flag might consume a value)
/// - POSIX `--` end-of-options terminator
/// - Global flags (--verbose, --source, --internal)
///
/// Returns true if we're still on the first positional argument (should complete branches).
/// Returns false if a positional has been provided and we've moved past it.
fn should_complete_positional_arg(
    args: &[String],
    start_index: usize,
    active: &Command,
    root: &Command,
) -> bool {
    let (longs, shorts) = build_opt_index(active, root);

    let mut i = start_index;
    let mut accept_opts = true;

    'scan: while i < args.len() {
        let tok = &args[i];

        if tok.is_empty() {
            // Cursor placeholder; skip and keep scanning
            i += 1;
            continue 'scan;
        }

        if accept_opts && tok == "--" {
            accept_opts = false;
            i += 1;
            continue 'scan;
        }

        if accept_opts && tok.starts_with("--") {
            let body = &tok[2..];
            if let Some(_eq) = body.find('=') {
                // --long=value
                i += 1;
                continue 'scan;
            } else {
                // --long [value?]
                let name = body;
                if longs.get(name).copied().unwrap_or(false) {
                    // Value is next arg unless we are currently typing it
                    if i == args.len() - 1 {
                        // Completing option value, not a positional
                        return false;
                    }
                    i += 2;
                    continue 'scan;
                }
                i += 1;
                continue 'scan;
            }
        }

        if accept_opts && tok.starts_with('-') && tok.len() > 1 {
            // Short clusters: -abc (if some short takes a value, the remainder is its value)
            let mut chars = tok[1..].chars().peekable();
            while let Some(c) = chars.next() {
                let takes = shorts.get(&c).copied().unwrap_or(false);
                if takes {
                    // If anything remains in-cluster, it's the value: consume token only
                    if chars.peek().is_some() {
                        i += 1;
                        continue 'scan;
                    } else {
                        // Value is next token, unless we're currently typing it
                        if i == args.len() - 1 {
                            return false;
                        }
                        i += 2;
                        continue 'scan;
                    }
                }
            }
            // No short in cluster takes a value -> just a bunch of booleans
            i += 1;
            continue 'scan;
        }

        // If we got here, `tok` is positional (or opts are terminated)
        // Return true iff it's the last token (i.e., we are still completing it)
        return i >= args.len() - 1;
    }

    // No positional arg found yet -> still need to complete it
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
                if !has_create && should_complete_positional_arg(args, i, cur, cmd) {
                    return CompletionTarget::PositionalBranch(last.to_string());
                }
            }
            "push" | "merge" | "remove" => {
                if should_complete_positional_arg(args, i, cur, cmd) {
                    return CompletionTarget::PositionalBranch(last.to_string());
                }
            }
            _ => {}
        }
    }

    CompletionTarget::Unknown
}

pub fn handle_complete(args: Vec<String>) -> Result<(), GitError> {
    let cmd = built_cmd();

    let target = detect_completion_target(&args, cmd);

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
            let mut cur = cmd;
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

            // Determine the prefix to filter by:
            // - If we've consumed all args as subcommands (i >= args.len()), use empty prefix
            // - Otherwise, use the last arg as prefix for filtering
            let prefix = if i >= args.len() {
                ""
            } else {
                args.last().map(String::as_str).unwrap_or("")
            };

            // Check if there's a positional with possible_values
            if let Some(arg) = cur
                .get_positionals()
                .find(|a| !a.get_possible_values().is_empty())
            {
                let items = items_from_arg(arg, prefix);
                if !items.is_empty() {
                    print_items(items);
                }
            }
        }
    }

    Ok(())
}
