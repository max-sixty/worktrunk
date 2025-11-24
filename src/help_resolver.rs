//! Resolves which subcommand's help to display based on argv.
//!
//! Walks through command-line arguments to find the deepest matching subcommand,
//! ignoring flags and their values.

use clap::Command;

/// Walk argv and descend into subcommands; ignore flags and values
pub fn resolve_target_command(
    cmd: &mut Command,
    argv: impl IntoIterator<Item = String>,
) -> &mut Command {
    // First pass: collect subcommand path by examining argv
    let subcommand_path = extract_subcommand_path(cmd, argv);

    // Second pass: navigate to the target command
    navigate_to_command(cmd, &subcommand_path)
}

/// Extract the sequence of subcommand names from argv
fn extract_subcommand_path(
    base_cmd: &Command,
    argv: impl IntoIterator<Item = String>,
) -> Vec<String> {
    let mut it = argv.into_iter().peekable();
    let _bin = it.next(); // skip program name
    let mut path = Vec::new();
    let mut current_cmd = base_cmd;

    while let Some(tok) = it.next() {
        if tok == "-h" || tok == "--help" {
            break;
        }
        if tok.starts_with('-') {
            // skip flag + its possible value (best-effort)
            if let Some(next) = it.peek()
                && !next.starts_with('-')
                && !is_subcommand(current_cmd, next)
            {
                let _ = it.next();
            }
            continue;
        }
        // Check if this is a subcommand
        if let Some(sub) = find_subcommand(current_cmd, &tok) {
            path.push(tok);
            current_cmd = sub;
        } else {
            // Token isn't a subcommand, stop searching
            break;
        }
    }

    path
}

/// Navigate to a subcommand by following a path
fn navigate_to_command<'a>(mut cmd: &'a mut Command, path: &[String]) -> &'a mut Command {
    for name in path {
        cmd = cmd.find_subcommand_mut(name).unwrap();
    }
    cmd
}

fn is_subcommand(cmd: &Command, name: &str) -> bool {
    cmd.get_subcommands()
        .any(|c| c.get_name() == name || c.get_visible_aliases().any(|a| a == name))
}

fn find_subcommand<'a>(cmd: &'a Command, name: &str) -> Option<&'a Command> {
    cmd.get_subcommands()
        .find(|c| c.get_name() == name || c.get_visible_aliases().any(|a| a == name))
}
