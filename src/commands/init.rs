use clap::CommandFactory;
use clap_complete::generate;
use std::io::{self, Write};
use worktrunk::shell;
use worktrunk::styling::println;

use crate::cli::Cli;

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
/// This is the handler for `wt config shell completions <shell>`. It outputs completion
/// scripts suitable for package manager integration (e.g., Homebrew's
/// `generate_completions_from_executable`).
///
/// Unlike `wt config shell init`, this does not:
/// - Modify any files
/// - Include shell integration (cd-on-switch functionality)
/// - Register dynamic completions
///
/// TODO(completions): We output static completions because that's the package manager
/// convention, but dynamic completions (one-liner that calls the binary at tab-time)
/// might be better—users would get branch name completion. See the fish example in
/// `~/.config/fish/completions/wt.fish` which calls `COMPLETE=fish wt` at runtime.
/// Other tools like gh/kubectl also call their binaries at runtime.
pub fn handle_completions(shell: shell::Shell) -> anyhow::Result<()> {
    let mut cmd = Cli::command();
    let cmd_name = crate::binary_name();
    let mut stdout = io::stdout();

    match shell {
        shell::Shell::Bash => {
            generate(
                clap_complete::shells::Bash,
                &mut cmd,
                &cmd_name,
                &mut stdout,
            );
        }
        shell::Shell::Fish => {
            generate(
                clap_complete::shells::Fish,
                &mut cmd,
                &cmd_name,
                &mut stdout,
            );
        }
        shell::Shell::Zsh => {
            generate(clap_complete::shells::Zsh, &mut cmd, &cmd_name, &mut stdout);
        }
        shell::Shell::Nushell => {
            // Nushell uses template-based integration (shell wrapper + completions in one)
            // Unlike other shells, it doesn't use clap_complete
            let init = shell::ShellInit::with_prefix(shell, cmd_name.clone());
            let code = init
                .generate()
                .expect("Failed to generate nushell integration");
            write!(stdout, "{}", code).expect("Failed to write to stdout");
        }
        shell::Shell::PowerShell => {
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
