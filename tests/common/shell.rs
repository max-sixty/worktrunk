use super::{TestRepo, wt_command};
use fs2::FileExt;
use insta_cmd::get_cargo_bin;
use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::Command;

/// Get path to dev-detach binary, building it once if needed.
///
/// Uses file locking to ensure only one concurrent build across test processes.
/// This prevents cargo lock contention that was causing SIGKILL failures when
/// multiple test processes invoked `cargo run -p dev-detach` simultaneously.
///
/// The lock prevents concurrent builds but does not protect against the binary
/// being deleted after the lock is released. In practice, this is not an issue
/// in the test environment.
fn get_dev_detach_bin() -> PathBuf {
    let manifest_dir = std::env::current_dir().unwrap();
    let bin_path = manifest_dir.join("target/debug/dev-detach");

    // Lock file ensures only one process builds at a time
    let lock_path = manifest_dir.join("target/.dev-detach.lock");
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .unwrap_or_else(|e| panic!("Failed to create lock file at {:?}: {}", lock_path, e));

    // Acquire exclusive lock (blocks if another process is building)
    lock_file.lock_exclusive().unwrap_or_else(|e| {
        panic!(
            "Failed to acquire exclusive lock on {:?}: {}. \
             This may indicate a deadlock or filesystem permission issue.",
            lock_path, e
        )
    });

    // While holding lock: build if binary doesn't exist
    if !bin_path.exists() {
        let status = Command::new("cargo")
            .args(["build", "-p", "dev-detach", "--quiet"])
            .status()
            .unwrap();

        if !status.success() {
            panic!("Failed to build dev-detach binary");
        }
    }

    // Release lock before returning
    drop(lock_file);

    bin_path
}

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
/// Uses pre-built binary to avoid cargo lock contention during concurrent test execution.
fn build_shell_command(repo: &TestRepo, shell: &str, script: &str) -> Command {
    // Use pre-built dev-detach binary (no cargo invocation)
    let mut cmd = Command::new(get_dev_detach_bin());
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
