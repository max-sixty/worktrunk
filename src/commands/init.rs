use clap::CommandFactory;
use clap_complete::generate;
use std::io;
use worktrunk::shell;
use worktrunk::styling::println;

use crate::cli::{Cli, CompletionShell};

pub fn handle_init(shell: shell::Shell, cmd: String) -> Result<(), String> {
    let init = shell::ShellInit::with_prefix(shell, cmd);

    // Generate shell integration code (includes dynamic completion registration)
    let integration_output = init
        .generate()
        .map_err(|e| format!("Failed to generate shell code: {}", e))?;

    println!("{}", integration_output);

    Ok(())
}

/// Generate static shell completions to stdout.
///
/// This is the handler for `wt completions <shell>`. It outputs completion
/// scripts suitable for package manager integration (e.g., Homebrew's
/// `generate_completions_from_executable`).
///
/// Unlike `wt config shell init`, this does not:
/// - Modify any files
/// - Include shell integration (cd-on-switch functionality)
/// - Register dynamic completions
pub fn handle_completions(shell: CompletionShell) -> anyhow::Result<()> {
    let mut cmd = Cli::command();
    let cmd_name = crate::binary_name();
    let mut stdout = io::stdout();

    match shell {
        CompletionShell::Bash => {
            generate(clap_complete::shells::Bash, &mut cmd, &cmd_name, &mut stdout);
        }
        CompletionShell::Fish => {
            generate(clap_complete::shells::Fish, &mut cmd, &cmd_name, &mut stdout);
        }
        CompletionShell::Zsh => {
            generate(clap_complete::shells::Zsh, &mut cmd, &cmd_name, &mut stdout);
        }
        CompletionShell::PowerShell => {
            generate(
                clap_complete::shells::PowerShell,
                &mut cmd,
                &cmd_name,
                &mut stdout,
            );
        }
    }

    Ok(())
}
