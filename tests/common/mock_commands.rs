// Cross-platform mock command helpers
//
// These helpers create mock executables that work on both Unix and Windows.
// All mock logic is written as shell scripts (#!/bin/sh).
//
// On Unix: shell scripts are directly executable via shebang
// On Windows: thin .bat/.cmd shims invoke the scripts via bash
//
// Requirements:
// - On Windows, Git Bash must be installed and `bash` must be in PATH
// - This matches production: hooks require Git Bash on Windows anyway
//
// This approach:
// - Single source of truth for mock behavior
// - Simpler than maintaining parallel shell/batch implementations

use std::fs;
use std::path::Path;

/// A branch in a mock command's logic.
///
/// Each branch matches an argument and produces output with an exit code.
pub struct MockBranch {
    /// The argument to match (e.g., "test", "clippy", "--version")
    pub arg: &'static str,
    /// Lines of output to print (each line is echoed)
    pub output: Vec<&'static str>,
    /// Exit code (0 for success)
    pub exit_code: i32,
}

/// Create a mock command that switches on the first argument.
///
/// # Arguments
/// * `bin_dir` - Directory to create the mock in (should be in PATH)
/// * `name` - Command name (e.g., "cargo", "gh")
/// * `branches` - List of argument matches and their behavior
/// * `default_exit` - Exit code when no branch matches
pub fn create_mock_command(bin_dir: &Path, name: &str, branches: &[MockBranch], default_exit: i32) {
    let mut script = String::from("#!/bin/sh\ncase \"$1\" in\n");

    for branch in branches {
        script.push_str(&format!("    {})\n", branch.arg));
        for line in &branch.output {
            let escaped = escape_shell_string(line);
            script.push_str(&format!("        echo '{}'\n", escaped));
        }
        script.push_str(&format!("        exit {}\n", branch.exit_code));
        script.push_str("        ;;\n");
    }

    script.push_str("    *)\n");
    script.push_str(&format!("        exit {}\n", default_exit));
    script.push_str("        ;;\n");
    script.push_str("esac\n");

    write_mock_script(bin_dir, name, &script);
}

/// Create a mock command that outputs fixed content regardless of arguments.
///
/// Useful for simple mocks like `llm` that just return canned output.
pub fn create_simple_mock(bin_dir: &Path, name: &str, output: &[&str], exit_code: i32) {
    let mut script = String::from("#!/bin/sh\n");
    // Discard stdin (for mocks that receive piped input)
    script.push_str("cat > /dev/null\n");

    for line in output {
        let escaped = escape_shell_string(line);
        script.push_str(&format!("echo '{}'\n", escaped));
    }
    script.push_str(&format!("exit {}\n", exit_code));

    write_mock_script(bin_dir, name, &script);
}

/// Escape single quotes in shell strings.
fn escape_shell_string(s: &str) -> String {
    s.replace('\'', "'\"'\"'")
}

/// Write a mock shell script, with platform-appropriate setup.
///
/// On Unix: writes directly as executable script
/// On Windows: writes script + .bat/.cmd shims that invoke via bash
fn write_mock_script(bin_dir: &Path, name: &str, script: &str) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script_path = bin_dir.join(name);
        fs::write(&script_path, script).unwrap();
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(windows)]
    {
        // Write the shell script with .sh extension
        let script_path = bin_dir.join(format!("{}.sh", name));
        fs::write(&script_path, script).unwrap();

        // Create .bat and .cmd shims that invoke via bash
        // %~dp0 expands to the directory containing the batch file
        // %* forwards all arguments
        let shim = format!("@bash \"%~dp0{}.sh\" %*\n", name);
        fs::write(bin_dir.join(format!("{}.cmd", name)), &shim).unwrap();
        fs::write(bin_dir.join(format!("{}.bat", name)), &shim).unwrap();
    }
}

// === High-level mock helpers for common test scenarios ===

/// Create a mock cargo command for tests.
///
/// Handles: test, clippy, install subcommands with realistic output.
pub fn create_mock_cargo(bin_dir: &Path) {
    create_mock_command(
        bin_dir,
        "cargo",
        &[
            MockBranch {
                arg: "test",
                output: vec![
                    "    Finished test [unoptimized + debuginfo] target(s) in 0.12s",
                    "     Running unittests src/lib.rs (target/debug/deps/worktrunk-abc123)",
                    "",
                    "running 18 tests",
                    "test auth::tests::test_jwt_decode ... ok",
                    "test auth::tests::test_jwt_encode ... ok",
                    "test auth::tests::test_token_refresh ... ok",
                    "test auth::tests::test_token_validation ... ok",
                    "",
                    "test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s",
                ],
                exit_code: 0,
            },
            MockBranch {
                arg: "clippy",
                output: vec![
                    "    Checking worktrunk v0.1.0",
                    "    Finished dev [unoptimized + debuginfo] target(s) in 1.23s",
                ],
                exit_code: 0,
            },
            MockBranch {
                arg: "install",
                output: vec![
                    "  Installing worktrunk v0.1.0",
                    "   Compiling worktrunk v0.1.0",
                    "    Finished release [optimized] target(s) in 2.34s",
                    "  Installing ~/.cargo/bin/wt",
                    "   Installed package `worktrunk v0.1.0` (executable `wt`)",
                ],
                exit_code: 0,
            },
        ],
        1,
    );
}

/// Create a mock llm command that outputs a commit message.
///
/// The commit message is suitable for JWT authentication feature commits.
pub fn create_mock_llm_auth(bin_dir: &Path) {
    create_simple_mock(
        bin_dir,
        "llm",
        &[
            "feat(auth): Implement JWT authentication system",
            "",
            "Add comprehensive JWT token handling including validation, refresh logic,",
            "and authentication tests. This establishes the foundation for secure",
            "API authentication.",
            "",
            "- Implement token refresh mechanism with expiry handling",
            "- Add JWT encoding/decoding with signature verification",
            "- Create test suite covering all authentication flows",
        ],
        0,
    );
}

/// Create a mock llm command for API endpoint commits.
pub fn create_mock_llm_api(bin_dir: &Path) {
    create_simple_mock(
        bin_dir,
        "llm",
        &[
            "feat(api): Add user authentication endpoints",
            "",
            "Implement login and token refresh endpoints with JWT validation.",
            "Includes comprehensive test coverage and input validation.",
        ],
        0,
    );
}

/// Create a mock uv command for dependency sync and dev server.
///
/// Handles: `uv sync` (1 arg) and `uv run dev` (2 args).
pub fn create_mock_uv_sync(bin_dir: &Path) {
    let script = r#"#!/bin/sh
if [ "$1" = "sync" ]; then
    echo ""
    echo "  Resolved 24 packages in 145ms"
    echo "  Installed 24 packages in 1.2s"
    exit 0
elif [ "$1" = "run" ] && [ "$2" = "dev" ]; then
    echo ""
    echo "  Starting dev server on http://localhost:3000..."
    exit 0
else
    echo "uv: unknown command '$1 $2'"
    exit 1
fi
"#;
    write_mock_script(bin_dir, "uv", script);
}

/// Create mock uv that delegates to pytest/ruff commands.
pub fn create_mock_uv_pytest_ruff(bin_dir: &Path) {
    let script = r#"#!/bin/sh
if [ "$1" = "run" ] && [ "$2" = "pytest" ]; then
    exec pytest
elif [ "$1" = "run" ] && [ "$2" = "ruff" ]; then
    shift 2
    exec ruff "$@"
else
    echo "uv: unknown command '$1 $2'"
    exit 1
fi
"#;
    write_mock_script(bin_dir, "uv", script);
}

/// Create a mock pytest command with test output.
pub fn create_mock_pytest(bin_dir: &Path) {
    create_simple_mock(
        bin_dir,
        "pytest",
        &[
            "",
            "============================= test session starts ==============================",
            "collected 3 items",
            "",
            "tests/test_auth.py::test_login_success PASSED                            [ 33%]",
            "tests/test_auth.py::test_login_invalid_password PASSED                   [ 66%]",
            "tests/test_auth.py::test_token_validation PASSED                         [100%]",
            "",
            "============================== 3 passed in 0.8s ===============================",
            "",
        ],
        0,
    );
}

/// Create a mock ruff command.
pub fn create_mock_ruff(bin_dir: &Path) {
    create_mock_command(
        bin_dir,
        "ruff",
        &[MockBranch {
            arg: "check",
            output: vec!["", "All checks passed!", ""],
            exit_code: 0,
        }],
        1,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_create_mock_command() {
        let temp = TempDir::new().unwrap();
        let bin_dir = temp.path();

        create_mock_command(
            bin_dir,
            "test-cmd",
            &[
                MockBranch {
                    arg: "hello",
                    output: vec!["Hello, World!"],
                    exit_code: 0,
                },
                MockBranch {
                    arg: "fail",
                    output: vec!["Error occurred"],
                    exit_code: 1,
                },
            ],
            2,
        );

        #[cfg(unix)]
        assert!(bin_dir.join("test-cmd").exists());

        #[cfg(windows)]
        {
            assert!(bin_dir.join("test-cmd.sh").exists());
            assert!(bin_dir.join("test-cmd.cmd").exists());
            assert!(bin_dir.join("test-cmd.bat").exists());
        }
    }

    #[test]
    fn test_create_simple_mock() {
        let temp = TempDir::new().unwrap();
        let bin_dir = temp.path();

        create_simple_mock(bin_dir, "simple-cmd", &["Line 1", "Line 2", "Line 3"], 0);

        #[cfg(unix)]
        assert!(bin_dir.join("simple-cmd").exists());

        #[cfg(windows)]
        {
            assert!(bin_dir.join("simple-cmd.sh").exists());
            assert!(bin_dir.join("simple-cmd.cmd").exists());
            assert!(bin_dir.join("simple-cmd.bat").exists());
        }
    }
}
