use super::{TestRepo, wt_command};
use insta_cmd::get_cargo_bin;
use std::{collections::HashSet, process::Command, sync::LazyLock};

/// Set of shells available on this system (cached at first access)
static AVAILABLE_SHELLS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut shells = HashSet::new();

    // Check bash availability
    if Command::new("bash")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        shells.insert("bash");
    }

    // Check zsh availability (typically not on Windows)
    if Command::new("zsh")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        shells.insert("zsh");
    }

    // Check fish availability (typically not on Windows)
    if Command::new("fish")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        shells.insert("fish");
    }

    // Check PowerShell Core availability (pwsh, cross-platform)
    if Command::new("pwsh")
        .arg("-Version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        shells.insert("pwsh");
        shells.insert("powershell"); // Alias
    }

    shells
});

/// Check if a shell is available on the current system.
pub fn shell_available(shell: &str) -> bool {
    AVAILABLE_SHELLS.contains(shell)
}

/// Path to dev-detach binary (workspace member built automatically by cargo).
static DEV_DETACH_BIN: LazyLock<std::path::PathBuf> = LazyLock::new(|| get_cargo_bin("dev-detach"));

/// Convert signal number to human-readable name
#[cfg(unix)]
fn signal_name(sig: i32) -> &'static str {
    match sig {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        6 => "SIGABRT",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        15 => "SIGTERM",
        _ => "UNKNOWN",
    }
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

/// Build a command to execute a shell script via dev-detach.
fn build_shell_command(repo: &TestRepo, shell: &str, script: &str) -> Command {
    let mut cmd = Command::new(&*DEV_DETACH_BIN);
    repo.clean_cli_env(&mut cmd);

    // Prevent user shell config from leaking into tests
    cmd.env_remove("BASH_ENV");
    cmd.env_remove("ENV");
    cmd.env_remove("ZDOTDIR");
    cmd.env_remove("XONSHRC");
    cmd.env_remove("XDG_CONFIG_HOME");

    // Build argument list: <dev-detach-binary> <shell> [shell-flags...] -c <script>
    cmd.arg(get_shell_binary(shell));

    // Add shell-specific no-config flags
    match shell {
        "bash" => cmd.arg("--noprofile").arg("--norc"),
        "zsh" => cmd.arg("--no-globalrcs").arg("-f"),
        "fish" => cmd.arg("--no-config"),
        "powershell" | "pwsh" => cmd.arg("-NoProfile"),
        "xonsh" => cmd.arg("--no-rc"),
        "nushell" | "nu" => cmd.arg("--no-config-file"),
        _ => &mut cmd,
    };

    cmd.arg("-c").arg(script);
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd
}

/// Execute a script in the given shell with the repo's isolated environment.
pub fn execute_shell_script(repo: &TestRepo, shell: &str, script: &str) -> String {
    let mut cmd = build_shell_command(repo, shell, script);

    let output = cmd
        .current_dir(repo.root_path())
        .output()
        .unwrap_or_else(|e| panic!("Failed to execute {} script: {}", shell, e));

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Check for dev-detach-specific errors (setsid failures, execvp failures, etc.)
    if stderr.contains("dev-detach:") {
        panic!(
            "dev-detach binary error:\nstderr: {}\nstdout: {}",
            stderr,
            String::from_utf8_lossy(&output.stdout)
        );
    }

    if !output.status.success() {
        let exit_info = match output.status.code() {
            Some(code) => format!("exit code {}", code),
            None => {
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    match output.status.signal() {
                        Some(sig) => format!("killed by signal {} ({})", sig, signal_name(sig)),
                        None => "killed by signal (unknown)".to_string(),
                    }
                }
                #[cfg(not(unix))]
                {
                    "killed by signal".to_string()
                }
            }
        };
        panic!(
            "Shell script failed ({}):\nCommand: dev-detach {} [shell-flags...] -c <script>\nstdout: {}\nstderr: {}",
            exit_info,
            shell,
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

    String::from_utf8(output.stdout).unwrap()
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
