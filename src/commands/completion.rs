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

use clap::Command;
use clap_complete::{Shell as CompletionShell, generate};
use std::io;
use worktrunk::git::{GitError, Repository};
use worktrunk::shell::Shell;
use worktrunk::styling::{ERROR, ERROR_EMOJI, println};

pub fn handle_completion(shell: Shell, cli_cmd: &mut Command) {
    let completion_shell = match shell {
        Shell::Bash | Shell::Oil => CompletionShell::Bash,
        Shell::Fish => CompletionShell::Fish,
        Shell::Zsh => CompletionShell::Zsh,
        _ => unreachable!(
            "CLI parsing ensures only shells that support completion can be passed here"
        ),
    };
    generate(completion_shell, cli_cmd, "wt", &mut io::stdout());
}

#[derive(Debug, PartialEq)]
enum CompletionContext {
    SwitchBranch,
    PushTarget,
    MergeTarget,
    RemoveBranch,
    BaseFlag,
    DevRunHook,
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

fn parse_completion_context(args: &[String]) -> CompletionContext {
    // args format: ["wt", "switch", "partial"]
    // or: ["wt", "switch", "--create", "new", "--base", "partial"]
    // or: ["wt", "dev", "run-hook", "partial"]

    if args.len() < 2 {
        return CompletionContext::Unknown;
    }

    let subcommand = &args[1];

    // Check if the previous argument was a flag that expects a value
    // If so, we're completing that flag's value
    if args.len() >= 3 {
        let prev_arg = &args[args.len() - 2];
        if prev_arg == "--base" || prev_arg == "-b" {
            return CompletionContext::BaseFlag;
        }
    }

    // Handle dev subcommand
    if subcommand == "dev" && args.len() >= 3 {
        let dev_subcommand = &args[2];
        if dev_subcommand == "run-hook" {
            // Complete hook types for the positional argument
            return CompletionContext::DevRunHook;
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

    if should_complete_positional_arg(args, 2) {
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

pub fn handle_complete(args: Vec<String>) -> Result<(), GitError> {
    let context = parse_completion_context(&args);

    match context {
        CompletionContext::SwitchBranch => {
            // Complete with all branches
            let branches = get_branches_for_completion(|| Repository::current().all_branches());
            for branch in branches {
                println!("{}", branch);
            }
        }
        CompletionContext::PushTarget
        | CompletionContext::MergeTarget
        | CompletionContext::RemoveBranch
        | CompletionContext::BaseFlag => {
            // Complete with all branches
            let branches = get_branches_for_completion(|| Repository::current().all_branches());
            for branch in branches {
                println!("{}", branch);
            }
        }
        CompletionContext::DevRunHook => {
            // Complete with hook types
            println!("post-create");
            println!("post-start");
            println!("pre-commit");
            println!("pre-squash");
            println!("pre-merge");
            println!("post-merge");
        }
        CompletionContext::Unknown => {
            // No completions
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_completion_context_switch() {
        let args = vec!["wt".to_string(), "switch".to_string(), "feat".to_string()];
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
        // "wt dev run-hook <cursor>"
        let args = vec!["wt".to_string(), "dev".to_string(), "run-hook".to_string()];
        assert_eq!(
            parse_completion_context(&args),
            CompletionContext::DevRunHook
        );
    }

    #[test]
    fn test_parse_completion_context_dev_run_hook_partial() {
        // "wt dev run-hook po<cursor>"
        let args = vec![
            "wt".to_string(),
            "dev".to_string(),
            "run-hook".to_string(),
            "po".to_string(),
        ];
        assert_eq!(
            parse_completion_context(&args),
            CompletionContext::DevRunHook
        );
    }

    #[test]
    fn test_parse_completion_context_dev_only() {
        // "wt dev <cursor>" - should not complete
        let args = vec!["wt".to_string(), "dev".to_string()];
        assert_eq!(parse_completion_context(&args), CompletionContext::Unknown);
    }
}
