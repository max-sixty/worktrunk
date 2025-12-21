use super::{TestRepo, wt_command};
use insta_cmd::get_cargo_bin;
use std::{
    collections::HashSet,
    process::{Command, Stdio},
    sync::LazyLock,
};

/// Check if a shell binary is available (non-interactive)
fn check_shell_available(binary: &str, arg: &str) -> bool {
    Command::new(binary)
        .arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Set of shells available on this system (cached at first access)
static AVAILABLE_SHELLS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut shells = HashSet::new();

    if check_shell_available("bash", "--version") {
        shells.insert("bash");
    }

    if check_shell_available("zsh", "--version") {
        shells.insert("zsh");
    }

    if check_shell_available("fish", "--version") {
        shells.insert("fish");
    }

    if check_shell_available("pwsh", "-Version") {
        shells.insert("pwsh");
        shells.insert("powershell");
    }

    shells
});

/// Check if a shell is available on the current system.
pub fn shell_available(shell: &str) -> bool {
    AVAILABLE_SHELLS.contains(shell)
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
///
/// Uses a PTY so that stdout appears as a terminal to the shell. This simulates
/// real terminal behavior for shell wrapper tests (combined stdout/stderr, ANSI codes).
#[cfg(unix)]
pub fn execute_shell_script(repo: &TestRepo, shell: &str, script: &str) -> String {
    use portable_pty::CommandBuilder;
    use std::io::Read;

    let pair = super::open_pty();

    let mut cmd = CommandBuilder::new(get_shell_binary(shell));

    // Clear inherited environment for test isolation
    cmd.env_clear();

    // Set minimal required environment for shells to function
    cmd.env("HOME", repo.home_path().to_string_lossy().to_string());
    cmd.env(
        "PATH",
        std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".to_string()),
    );
    cmd.env("USER", "testuser");
    cmd.env("SHELL", get_shell_binary(shell));

    // Add repo's test environment (git config, worktrunk config, etc.)
    for (key, value) in repo.test_env_vars() {
        cmd.env(key, value);
    }

    // Add shell-specific no-config flags
    match shell {
        "bash" => {
            cmd.arg("--noprofile");
            cmd.arg("--norc");
        }
        "zsh" => {
            cmd.arg("--no-globalrcs");
            cmd.arg("-f");
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
    };

    // PTY combines stdout/stderr at the terminal device level, so we don't need
    // explicit redirection. Redirecting would break the shell wrapper protocol:
    // wt_exec() captures stdout for directives, and stderr must stay separate.
    cmd.arg("-c");
    cmd.arg(script);
    cmd.cwd(repo.root_path());

    // Pass through LLVM coverage env vars for subprocess coverage collection
    super::pass_coverage_env_to_pty_cmd(&mut cmd);

    let mut child = pair.slave.spawn_command(cmd).unwrap();
    drop(pair.slave); // Close slave in parent

    // Read everything the "terminal" would display
    let mut reader = pair.master.try_clone_reader().unwrap();
    let mut buf = String::new();
    reader.read_to_string(&mut buf).unwrap(); // Blocks until child exits & PTY closes

    let status = child.wait().unwrap();

    if !status.success() {
        let exit_info = match status.exit_code() {
            0 => "unknown error".to_string(),
            code => format!("exit code {}", code),
        };
        panic!(
            "Shell script failed ({}):\nshell: {}\noutput: {}",
            exit_info, shell, buf
        );
    }

    // Check for shell errors in output (command not found, syntax errors, etc.)
    // These indicate problems with our shell integration code
    if buf.contains("command not found") || buf.contains("not defined") {
        panic!(
            "Shell integration error detected:\nshell: {}\noutput: {}",
            shell, buf
        );
    }

    buf
}

/// Generate `wt config shell init <shell>` output for the repo.
pub fn generate_init_code(repo: &TestRepo, shell: &str) -> String {
    let mut cmd = wt_command();
    repo.clean_cli_env(&mut cmd);

    let output = cmd
        .args(["config", "shell", "init", shell])
        .current_dir(repo.root_path())
        .output()
        .unwrap();

    let stdout = String::from_utf8(output.stdout).unwrap();
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
