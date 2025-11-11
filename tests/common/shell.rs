use super::{TestRepo, wt_command};
use insta_cmd::get_cargo_bin;
use std::process::Command;

/// Get the path to the dev-detach helper binary.
/// This binary calls setsid() before exec'ing the shell, detaching it from
/// any controlling terminal and preventing PTY-related hangs.
fn get_dev_detach_bin() -> std::path::PathBuf {
    get_cargo_bin("dev-detach")
}

/// Map shell display names to actual binaries.
pub fn get_shell_binary(shell: &str) -> &str {
    match shell {
        "nushell" => "nu",
        "powershell" => "pwsh",
        "oil" => "osh",
        _ => shell,
    }
}

/// Execute a script in the given shell with the repo's isolated environment.
pub fn execute_shell_script(repo: &TestRepo, shell: &str, script: &str) -> String {
    // Use dev-detach wrapper to fully isolate shell from controlling terminals.
    // The dev-detach binary calls setsid() before exec'ing the shell, preventing
    // PTY-related hangs in nextest environments (unbuffer, script, terminal emulators).
    let detach = get_dev_detach_bin();
    let mut cmd = Command::new(detach);
    repo.clean_cli_env(&mut cmd);

    // Prevent user shell config from leaking into tests.
    cmd.env_remove("BASH_ENV");
    cmd.env_remove("ENV");
    cmd.env_remove("ZDOTDIR");
    cmd.env_remove("XONSHRC");
    cmd.env_remove("XDG_CONFIG_HOME");

    // Build argument list: dev-detach <shell> [shell-flags...] -c <script>
    let binary = get_shell_binary(shell);
    cmd.arg(binary);

    // Add shell-specific no-config flags
    match shell {
        "bash" => {
            cmd.arg("--noprofile").arg("--norc");
        }
        "zsh" => {
            cmd.arg("--no-globalrcs").arg("-f");
        }
        "fish" => {
            cmd.arg("--no-config");
        }
        "powershell" | "pwsh" => {
            cmd.arg("-NoProfile");
        }
        "xonsh" => {
            cmd.arg("--no-rc");
        }
        "nushell" | "nu" => {
            cmd.arg("--no-config-file");
        }
        _ => {}
    }

    cmd.arg("-c").arg(script);

    // Null stdin, piped stdout/stderr for full TTY isolation.
    // Combined with setsid() from dev-detach, this guarantees no controlling terminal.
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let output = cmd
        .current_dir(repo.root_path())
        .output()
        .unwrap_or_else(|e| panic!("Failed to execute {} script: {}", shell, e));

    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        panic!(
            "Shell script failed:\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            stderr
        );
    }

    // Check for shell errors in stderr (command not found, syntax errors, etc.)
    // These indicate problems with our shell integration code
    if stderr.contains("command not found") || stderr.contains("not defined") {
        panic!(
            "Shell integration error detected:\nstderr: {}\nstdout: {}",
            stderr,
            String::from_utf8_lossy(&output.stdout)
        );
    }

    String::from_utf8(output.stdout).expect("Invalid UTF-8 in output")
}

/// Generate `wt init <shell>` output for the repo.
pub fn generate_init_code(repo: &TestRepo, shell: &str) -> String {
    let mut cmd = wt_command();
    repo.clean_cli_env(&mut cmd);

    let output = cmd
        .args(["init", shell])
        .current_dir(repo.root_path())
        .output()
        .expect("Failed to generate init code");

    let stdout = String::from_utf8(output.stdout).expect("Invalid UTF-8 in init code");
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() && stdout.trim().is_empty() {
        panic!("Failed to generate init code:\nstderr: {}", stderr);
    }

    // Check for shell errors in the generated init code when it's evaluated
    // This catches issues like missing compdef guards
    if stderr.contains("command not found") || stderr.contains("not defined") {
        panic!(
            "Init code contains errors:\nstderr: {}\nGenerated code:\n{}",
            stderr, stdout
        );
    }

    stdout
}

/// Format PATH mutation per shell.
pub fn path_export_syntax(shell: &str, bin_path: &str) -> String {
    match shell {
        "fish" => format!(r#"set -x PATH {} $PATH"#, bin_path),
        "nushell" => format!(r#"$env.PATH = ($env.PATH | prepend "{}")"#, bin_path),
        "powershell" => format!(r#"$env:PATH = "{}:$env:PATH""#, bin_path),
        "elvish" => format!(r#"set E:PATH = {}:$E:PATH"#, bin_path),
        "xonsh" => format!(r#"$PATH.insert(0, "{}")"#, bin_path),
        _ => format!(r#"export PATH="{}:$PATH""#, bin_path),
    }
}

/// Helper that returns the `wt` binary directory for PATH injection.
pub fn wt_bin_dir() -> String {
    get_cargo_bin("wt")
        .parent()
        .unwrap()
        .to_string_lossy()
        .to_string()
}
