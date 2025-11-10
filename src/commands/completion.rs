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

#[derive(Debug, PartialEq)]
enum CompletionContext {
    SwitchBranch,
    PushTarget,
    MergeTarget,
    RemoveBranch,
    BaseFlag,
    Unknown,
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

/// Find the subcommand position by skipping global flags
///
/// Note: `--source` is handled by the shell wrapper (templates/*.sh) and stripped before
/// reaching the main Rust binary, but the completion function passes COMP_WORDS directly
/// to `wt complete`, so completion sees the raw command line with `--source` still present.
fn find_subcommand_index(args: &[String]) -> Option<usize> {
    let mut i = 1; // Start after "wt"
    while i < args.len() {
        let arg = &args[i];
        // Skip global flags (--source is shell-only, others are defined in cli.rs)
        if arg == "--source" || arg == "--internal" || arg == "-v" || arg == "--verbose" {
            i += 1;
        } else if !arg.starts_with('-') {
            // Found the subcommand
            return Some(i);
        } else {
            // Unknown flag, stop searching (fail-safe behavior)
            return None;
        }
    }
    None
}

fn parse_completion_context(args: &[String]) -> CompletionContext {
    // args format: ["wt", "switch", "partial"]
    // or: ["wt", "--source", "switch", "partial"]
    // or: ["wt", "switch", "--create", "new", "--base", "partial"]
    // or: ["wt", "beta", "run-hook", "partial"]

    if args.len() < 2 {
        return CompletionContext::Unknown;
    }

    let subcommand_index = match find_subcommand_index(args) {
        Some(idx) => idx,
        None => return CompletionContext::Unknown,
    };

    let subcommand = &args[subcommand_index];

    // Check if the previous argument was a flag that expects a value
    // If so, we're completing that flag's value
    if args.len() >= 3 {
        let prev_arg = &args[args.len() - 2];
        if prev_arg == "--base" || prev_arg == "-b" {
            return CompletionContext::BaseFlag;
        }
    }

    // Special handling for switch --create: don't complete new branch names
    if subcommand == "switch" {
        let has_create = args.iter().any(|arg| arg == "--create" || arg == "-c");
        if has_create {
            return CompletionContext::Unknown;
        }
    }

    // For commands with positional branch arguments, check if we should complete
    let context = match subcommand.as_str() {
        "switch" => CompletionContext::SwitchBranch,
        "push" => CompletionContext::PushTarget,
        "merge" => CompletionContext::MergeTarget,
        "remove" => CompletionContext::RemoveBranch,
        _ => return CompletionContext::Unknown,
    };

    if should_complete_positional_arg(args, subcommand_index + 1) {
        context
    } else {
        CompletionContext::Unknown
    }
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

/// Generic fallback that reflects on clap's Command tree to find possible values
/// This automatically handles all ValueEnum types without manual registration
fn clap_fallback(args: &[String]) -> Vec<Item> {
    // args look like: ["wt", "<subcmds...>", "<partial or prev>", [partial]?]
    let mut cmd = crate::cli::Cli::command();
    cmd.build(); // Required for introspection (completions/help)

    // Find the active subcommand frame by walking the command tree
    let mut i = 1; // Skip binary name
    let mut cur: &Command = &cmd;
    while i < args.len() {
        let tok = &args[i];
        if let Some(sc) = cur.find_subcommand(tok) {
            cur = sc;
            i += 1;
        } else {
            break;
        }
    }

    // Use the last two args to determine what we're completing
    // If we consumed all args as subcommands, we're completing a positional (empty prefix)
    // Otherwise, the last arg is the partial completion text
    let last = if i >= args.len() {
        ""
    } else {
        args.last().map(String::as_str).unwrap_or("")
    };
    let prev = args.iter().rev().nth(1).map(|s| s.as_str());

    // 1) If we are completing a value for an option (prev was a flag), use that option's possible values
    if let Some(p) = prev {
        // Long form: --name
        if let Some(long) = p.strip_prefix("--")
            && let Some(arg) = cur
                .get_opts()
                .find(|a| a.get_long().is_some_and(|l| l == long))
        {
            return items_from_arg(arg, last);
        }
        // Short form: -n
        if let Some(short) = p
            .strip_prefix('-')
            .filter(|s| s.len() == 1)
            .and_then(|s| s.chars().next())
            && let Some(arg) = cur.get_opts().find(|a| a.get_short() == Some(short))
        {
            return items_from_arg(arg, last);
        }
    }

    // 2) Otherwise, we're likely on a positional; try the "next" positional that has fixed values
    // Heuristic: first positional with possible_values
    if let Some(arg) = cur
        .get_positionals()
        .find(|a| !a.get_possible_values().is_empty())
    {
        return items_from_arg(arg, last);
    }

    // 3) Nothing to contribute
    Vec::new()
}

pub fn handle_complete(args: Vec<String>) -> Result<(), GitError> {
    let context = parse_completion_context(&args);

    match context {
        CompletionContext::SwitchBranch
        | CompletionContext::PushTarget
        | CompletionContext::MergeTarget
        | CompletionContext::RemoveBranch
        | CompletionContext::BaseFlag => {
            // Complete with all branches (runtime-fetched values)
            let branches = get_branches_for_completion(|| Repository::current().all_branches());
            for branch in branches {
                println!("{}", branch);
            }
        }
        CompletionContext::Unknown => {
            // Use clap's reflection to find possible values for ValueEnum types
            // This automatically handles init <Shell>, config shell --shell <Shell>,
            // and any future ValueEnum arguments without manual registration
            let items = clap_fallback(&args);
            if !items.is_empty() {
                print_items(items);
            }
            // else: intentionally print nothing
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_subcommand_index() {
        let args = vec!["wt".to_string(), "switch".to_string()];
        assert_eq!(find_subcommand_index(&args), Some(1));
    }

    #[test]
    fn test_find_subcommand_index_with_source() {
        let args = vec![
            "wt".to_string(),
            "--source".to_string(),
            "switch".to_string(),
        ];
        assert_eq!(find_subcommand_index(&args), Some(2));
    }

    #[test]
    fn test_find_subcommand_index_with_verbose() {
        let args = vec!["wt".to_string(), "-v".to_string(), "switch".to_string()];
        assert_eq!(find_subcommand_index(&args), Some(2));
    }

    #[test]
    fn test_find_subcommand_index_with_multiple_flags() {
        let args = vec![
            "wt".to_string(),
            "--source".to_string(),
            "-v".to_string(),
            "switch".to_string(),
        ];
        assert_eq!(find_subcommand_index(&args), Some(3));
    }

    #[test]
    fn test_find_subcommand_index_no_subcommand() {
        let args = vec!["wt".to_string()];
        assert_eq!(find_subcommand_index(&args), None);
    }

    #[test]
    fn test_parse_completion_context_switch() {
        let args = vec!["wt".to_string(), "switch".to_string(), "feat".to_string()];
        assert_eq!(
            parse_completion_context(&args),
            CompletionContext::SwitchBranch
        );
    }

    #[test]
    fn test_parse_completion_context_switch_with_source() {
        let args = vec![
            "wt".to_string(),
            "--source".to_string(),
            "switch".to_string(),
            "feat".to_string(),
        ];
        assert_eq!(
            parse_completion_context(&args),
            CompletionContext::SwitchBranch
        );
    }

    #[test]
    fn test_parse_completion_context_push() {
        let args = vec!["wt".to_string(), "push".to_string(), "ma".to_string()];
        assert_eq!(
            parse_completion_context(&args),
            CompletionContext::PushTarget
        );
    }

    #[test]
    fn test_parse_completion_context_merge() {
        let args = vec!["wt".to_string(), "merge".to_string(), "de".to_string()];
        assert_eq!(
            parse_completion_context(&args),
            CompletionContext::MergeTarget
        );
    }

    #[test]
    fn test_parse_completion_context_remove() {
        let args = vec!["wt".to_string(), "remove".to_string(), "feat".to_string()];
        assert_eq!(
            parse_completion_context(&args),
            CompletionContext::RemoveBranch
        );
    }

    #[test]
    fn test_parse_completion_context_base_flag() {
        let args = vec![
            "wt".to_string(),
            "switch".to_string(),
            "--create".to_string(),
            "new".to_string(),
            "--base".to_string(),
            "dev".to_string(),
        ];
        assert_eq!(parse_completion_context(&args), CompletionContext::BaseFlag);
    }

    #[test]
    fn test_parse_completion_context_unknown() {
        let args = vec!["wt".to_string()];
        assert_eq!(parse_completion_context(&args), CompletionContext::Unknown);
    }

    #[test]
    fn test_parse_completion_context_base_flag_short() {
        let args = vec![
            "wt".to_string(),
            "switch".to_string(),
            "--create".to_string(),
            "new".to_string(),
            "-b".to_string(),
            "dev".to_string(),
        ];
        assert_eq!(parse_completion_context(&args), CompletionContext::BaseFlag);
    }

    #[test]
    fn test_parse_completion_context_base_at_end() {
        // --base at the end with empty string (what shell sends when completing)
        let args = vec![
            "wt".to_string(),
            "switch".to_string(),
            "--create".to_string(),
            "new".to_string(),
            "--base".to_string(),
            "".to_string(), // Shell sends empty string for cursor position
        ];
        // Should detect BaseFlag context
        assert_eq!(parse_completion_context(&args), CompletionContext::BaseFlag);
    }

    #[test]
    fn test_parse_completion_context_multiple_base_flags() {
        // Multiple --base flags (last one wins)
        let args = vec![
            "wt".to_string(),
            "switch".to_string(),
            "--create".to_string(),
            "new".to_string(),
            "--base".to_string(),
            "main".to_string(),
            "--base".to_string(),
            "develop".to_string(),
        ];
        assert_eq!(parse_completion_context(&args), CompletionContext::BaseFlag);
    }

    #[test]
    fn test_parse_completion_context_empty_args() {
        let args = vec![];
        assert_eq!(parse_completion_context(&args), CompletionContext::Unknown);
    }

    #[test]
    fn test_parse_completion_context_switch_only() {
        // Just "wt switch" with no other args
        let args = vec!["wt".to_string(), "switch".to_string()];
        assert_eq!(
            parse_completion_context(&args),
            CompletionContext::SwitchBranch
        );
    }

    #[test]
    fn test_parse_completion_context_dev_run_hook() {
        // "wt beta run-hook <cursor>"
        // Now handled by clap fallback via Unknown context
        let args = vec!["wt".to_string(), "beta".to_string(), "run-hook".to_string()];
        assert_eq!(parse_completion_context(&args), CompletionContext::Unknown);
    }

    #[test]
    fn test_parse_completion_context_dev_run_hook_partial() {
        // "wt beta run-hook po<cursor>"
        // Now handled by clap fallback via Unknown context
        let args = vec![
            "wt".to_string(),
            "beta".to_string(),
            "run-hook".to_string(),
            "po".to_string(),
        ];
        assert_eq!(parse_completion_context(&args), CompletionContext::Unknown);
    }

    #[test]
    fn test_parse_completion_context_dev_only() {
        // "wt beta <cursor>" - should not complete
        let args = vec!["wt".to_string(), "beta".to_string()];
        assert_eq!(parse_completion_context(&args), CompletionContext::Unknown);
    }

    #[test]
    fn test_parse_completion_context_base_flag_with_source() {
        let args = vec![
            "wt".to_string(),
            "--source".to_string(),
            "switch".to_string(),
            "--create".to_string(),
            "new".to_string(),
            "--base".to_string(),
            "dev".to_string(),
        ];
        assert_eq!(parse_completion_context(&args), CompletionContext::BaseFlag);
    }

    #[test]
    fn test_parse_completion_context_beta_run_hook_with_source() {
        // Now handled by clap fallback via Unknown context
        let args = vec![
            "wt".to_string(),
            "--source".to_string(),
            "beta".to_string(),
            "run-hook".to_string(),
            "po".to_string(),
        ];
        assert_eq!(parse_completion_context(&args), CompletionContext::Unknown);
    }

    #[test]
    fn test_parse_completion_context_merge_with_verbose_and_source() {
        let args = vec![
            "wt".to_string(),
            "-v".to_string(),
            "--source".to_string(),
            "merge".to_string(),
            "de".to_string(),
        ];
        assert_eq!(
            parse_completion_context(&args),
            CompletionContext::MergeTarget
        );
    }

    #[test]
    fn test_find_subcommand_index_unknown_flag() {
        // Unknown flags cause completion to bail out (fail-safe behavior)
        let args = vec!["wt".to_string(), "--typo".to_string(), "switch".to_string()];
        assert_eq!(find_subcommand_index(&args), None);
    }

    #[test]
    fn test_find_subcommand_index_empty_after_flag() {
        // Empty string after flag (cursor immediately after --source with no subcommand yet)
        // Empty string doesn't start with '-', so it's treated as the subcommand position
        let args = vec!["wt".to_string(), "--source".to_string(), "".to_string()];
        assert_eq!(find_subcommand_index(&args), Some(2));
    }

    #[test]
    fn test_parse_completion_context_init() {
        // "wt init <cursor>"
        // Now handled by clap fallback via Unknown context
        let args = vec!["wt".to_string(), "init".to_string()];
        assert_eq!(parse_completion_context(&args), CompletionContext::Unknown);
    }

    #[test]
    fn test_parse_completion_context_init_partial() {
        // "wt init fi<cursor>"
        // Now handled by clap fallback via Unknown context
        let args = vec!["wt".to_string(), "init".to_string(), "fi".to_string()];
        assert_eq!(parse_completion_context(&args), CompletionContext::Unknown);
    }

    #[test]
    fn test_parse_completion_context_init_with_source() {
        // "wt --source init fi<cursor>"
        // Now handled by clap fallback via Unknown context
        let args = vec![
            "wt".to_string(),
            "--source".to_string(),
            "init".to_string(),
            "fi".to_string(),
        ];
        assert_eq!(parse_completion_context(&args), CompletionContext::Unknown);
    }

    #[test]
    fn test_parse_completion_context_init_with_verbose() {
        // "wt -v init ba<cursor>"
        // Now handled by clap fallback via Unknown context
        let args = vec![
            "wt".to_string(),
            "-v".to_string(),
            "init".to_string(),
            "ba".to_string(),
        ];
        assert_eq!(parse_completion_context(&args), CompletionContext::Unknown);
    }
}
