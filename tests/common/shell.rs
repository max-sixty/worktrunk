use super::{TestRepo, wt_command};
use insta_cmd::get_cargo_bin;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// Get the path to the workspace Cargo.toml.
/// Computed once and cached for the lifetime of the test process.
fn workspace_manifest() -> &'static PathBuf {
    static MANIFEST: OnceLock<PathBuf> = OnceLock::new();
    MANIFEST.get_or_init(|| {
        // Tests run from workspace root, so Cargo.toml is in current directory
        std::env::current_dir()
            .expect("Failed to get current directory")
            .join("Cargo.toml")
    })
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
/// Uses `cargo run --manifest-path <workspace>/Cargo.toml -p dev-detach` so cargo
/// can find the workspace manifest while the shell executes in the test repo directory.
fn build_shell_command(repo: &TestRepo, shell: &str, script: &str) -> Command {
    let mut cmd = Command::new("cargo");
    cmd.args(["run", "--manifest-path"])
        .arg(workspace_manifest())
        .args(["-p", "dev-detach", "--"]);
    repo.clean_cli_env(&mut cmd);

    // Prevent user shell config from leaking into tests
    cmd.env_remove("BASH_ENV");
    cmd.env_remove("ENV");
    cmd.env_remove("ZDOTDIR");
    cmd.env_remove("XONSHRC");
    cmd.env_remove("XDG_CONFIG_HOME");

    // Build argument list: cargo run --manifest-path <...> -p dev-detach -- <shell> [shell-flags...] -c <script>
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
        // If killed by signal with no output, retry once (handles transient failures)
        if output.status.code().is_none() && output.stdout.is_empty() && output.stderr.is_empty() {
            eprintln!(
                "{} script killed by signal with no output - retrying after 150ms",
                shell
            );
            std::thread::sleep(std::time::Duration::from_millis(150));

            let retry_output = build_shell_command(repo, shell, script)
                .current_dir(repo.root_path())
                .output()
                .unwrap_or_else(|e| panic!("Failed to execute {} script on retry: {}", shell, e));

            if !retry_output.status.success() {
                let exit_info = match retry_output.status.code() {
                    Some(code) => format!("exit code {}", code),
                    None => "killed by signal".to_string(),
                };
                panic!(
                    "Shell script failed on retry ({}):\nCommand: cargo run -p dev-detach -- {} [shell-flags...] -c <script>\nstdout: {}\nstderr: {}",
                    exit_info,
                    shell,
                    String::from_utf8_lossy(&retry_output.stdout),
                    String::from_utf8_lossy(&retry_output.stderr)
                );
            }

            return String::from_utf8_lossy(&retry_output.stdout).to_string()
                + &String::from_utf8_lossy(&retry_output.stderr);
        }

        let exit_info = match output.status.code() {
            Some(code) => format!("exit code {}", code),
            None => "killed by signal".to_string(),
        };
        panic!(
            "Shell script failed ({}):\nCommand: cargo run -p dev-detach -- {} [shell-flags...] -c <script>\nstdout: {}\nstderr: {}",
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
