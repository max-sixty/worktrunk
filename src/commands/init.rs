use anyhow::Context;
use clap_complete::env::{
    Bash as EnvBash, EnvCompleter, Powershell as EnvPowershell, Zsh as EnvZsh,
};
use std::io::{self, Write};
use worktrunk::shell;
use worktrunk::styling::println;

pub fn handle_init(shell: shell::Shell, cmd: String) -> Result<(), String> {
    let init = shell::ShellInit::with_prefix(shell, cmd);

    // Generate shell integration code (includes dynamic completion registration)
    let integration_output = init
        .generate()
        .map_err(|e| format!("Failed to generate shell code: {}", e))?;

    println!("{}", integration_output);

    Ok(())
}

/// Generate shell completions to stdout for package manager integration.
///
/// This is the handler for `wt config shell completions <shell>`. It outputs completion
/// scripts suitable for package manager integration (e.g., Homebrew's
/// `generate_completions_from_executable`).
///
/// Bash, Zsh, Fish, and PowerShell all emit a *dynamic* registration that calls
/// the binary at TAB time. This means a plain package install gets live branch and
/// worktree name completion with no `wt config shell install`.
///
/// Two shells need a transform on top of clap's raw registration:
/// - **Zsh** needs the script to work when autoloaded from `fpath` (Homebrew installs
///   it as `site-functions/_wt`). See `make_zsh_autoload_safe`.
/// - **Fish** must resolve the real binary (via `type -P`, with a `WORKTRUNK_BIN`
///   override) rather than calling the bare `wt` command. clap's raw output invokes
///   `COMPLETE=fish wt -- …`; when worktrunk's lazy-load wrapper function shadows the
///   binary, that bare call recurses into the wrapper to fish's call-stack limit
///   (#3240). So fish reuses `configure_shell::fish_completion_content`, the same
///   binary-resolving registration `wt config shell install` writes.
///
/// Bash and PowerShell need no transform: Homebrew sources bash files and PowerShell
/// registrations are sourced into the profile.
///
/// Nushell uses template-based integration (the shell wrapper and completer in one
/// file), which is already dynamic, so its output is the full `init` template.
///
/// Unlike `wt config shell init`, this does not:
/// - Modify any files
/// - Include shell integration (cd-on-switch functionality)
pub fn handle_completions(shell: shell::Shell) -> anyhow::Result<()> {
    let cmd_name = crate::binary_name();
    let mut stdout = io::stdout();

    match shell {
        shell::Shell::Bash => {
            EnvBash
                .write_registration("COMPLETE", &cmd_name, &cmd_name, &cmd_name, &mut stdout)
                .context("failed to write bash completion registration")?;
        }
        shell::Shell::Zsh => {
            let mut buf = Vec::new();
            EnvZsh
                .write_registration("COMPLETE", &cmd_name, &cmd_name, &cmd_name, &mut buf)
                .context("failed to write zsh completion registration")?;
            let script = String::from_utf8(buf)
                .context("zsh completion registration was not valid UTF-8")?;
            let script = make_zsh_autoload_safe(&script, &cmd_name);
            write!(stdout, "{}", script).context("failed to write to stdout")?;
        }
        shell::Shell::Fish => {
            // clap's fish registration calls the bare command (`COMPLETE=fish wt -- …`).
            // When worktrunk's lazy-load wrapper function shadows the binary, that bare
            // call re-enters the wrapper: fish has already exported `COMPLETE=fish`, so
            // the wrapper's `command wt config shell init fish | source` emits completions
            // instead of the init script — the real function is never defined — and the
            // wrapper's trailing `wt $argv` recurses to fish's call-stack limit (#3240).
            // Emit the same binary-resolving registration as `wt config shell install`,
            // which goes through `type -P` (with `WORKTRUNK_BIN` override) to bypass the
            // wrapper.
            let registration = super::configure_shell::fish_completion_content(&cmd_name);
            write!(stdout, "{}", registration).context("failed to write to stdout")?;
        }
        shell::Shell::Nushell => {
            // Nushell uses template-based integration (shell wrapper + completions in one)
            // Unlike other shells, it doesn't use clap_complete
            let init = shell::ShellInit::with_prefix(shell, cmd_name.clone());
            let code = init
                .generate()
                .context("failed to generate nushell integration")?;
            write!(stdout, "{}", code).context("failed to write to stdout")?;
        }
        shell::Shell::PowerShell => {
            EnvPowershell
                .write_registration("COMPLETE", &cmd_name, &cmd_name, &cmd_name, &mut stdout)
                .context("failed to write powershell completion registration")?;
        }
    }

    Ok(())
}

/// Make clap's dynamic zsh registration safe to autoload from `fpath`.
///
/// clap's registration ends with `compdef <func> <cmd>`, which assumes the script is
/// sourced or eval'd. Homebrew instead installs it as `site-functions/_<cmd>` and lets
/// compinit autoload it on the first completion, where the file body runs as the `_<cmd>`
/// completion function. In that mode a bare `compdef` is too late. zsh expects the file
/// to *perform* the completion, not register a handler.
///
/// The replacement is a dual-mode guard: when `funcstack[1]` is `_<cmd>` (autoloaded),
/// call clap's completer directly. Otherwise (sourced/eval'd) register it via `compdef`.
/// The leading `#compdef <cmd>` line clap already emits stays first, which is what marks
/// the file for compinit autoload.
///
/// The two display zstyles match `templates/zsh.zsh` so package-installed users get the
/// same single-column branch listing as `install` users. They precede the guard because
/// in autoload mode the whole file body runs as `_<cmd>` on every completion: setting the
/// styles before the completer call means `_describe` sees them on the *first* TAB, not
/// only on the second.
fn make_zsh_autoload_safe(script: &str, cmd_name: &str) -> String {
    let func = format!("_clap_dynamic_completer_{}", cmd_name.replace('-', "_"));
    let trailing = format!("compdef {func} {cmd_name}");
    let replacement = format!(
        r#"# Single-column display keeps descriptions visually associated with each branch.
zstyle ':completion:*:{cmd_name}:*' list-max 1
# Prevent grouping branches with identical descriptions (same timestamp) on one line.
zstyle ':completion:*:*:{cmd_name}:*' list-grouped false

if [ "$funcstack[1]" = "_{cmd_name}" ]; then
    {func} "$@"
else
    compdef {func} {cmd_name}
fi"#
    );
    script.replace(&trailing, &replacement)
}
