// Cross-platform mock command helpers
//
// These helpers create mock executables that work on both Unix and Windows.
// On Unix: creates shell scripts with #!/bin/sh shebang
// On Windows: creates .cmd batch files
//
// The API abstracts platform differences so test logic remains identical.

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
///
/// # Example
/// ```ignore
/// create_mock_command(
///     &bin_dir,
///     "cargo",
///     &[
///         MockBranch {
///             arg: "test",
///             output: vec!["running 18 tests", "test result: ok"],
///             exit_code: 0,
///         },
///         MockBranch {
///             arg: "clippy",
///             output: vec!["Checking worktrunk v0.1.0"],
///             exit_code: 0,
///         },
///     ],
///     1, // default exit code for unknown subcommands
/// );
/// ```
#[cfg(unix)]
pub fn create_mock_command(bin_dir: &Path, name: &str, branches: &[MockBranch], default_exit: i32) {
    use std::os::unix::fs::PermissionsExt;

    let mut script = String::from("#!/bin/sh\ncase \"$1\" in\n");

    for branch in branches {
        script.push_str(&format!("    {})\n", branch.arg));
        for line in &branch.output {
            // Escape single quotes in output
            let escaped = line.replace('\'', "'\"'\"'");
            script.push_str(&format!("        echo '{}'\n", escaped));
        }
        script.push_str(&format!("        exit {}\n", branch.exit_code));
        script.push_str("        ;;\n");
    }

    script.push_str("    *)\n");
    script.push_str(&format!("        exit {}\n", default_exit));
    script.push_str("        ;;\n");
    script.push_str("esac\n");

    let script_path = bin_dir.join(name);
    fs::write(&script_path, script).unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(windows)]
pub fn create_mock_command(bin_dir: &Path, name: &str, branches: &[MockBranch], default_exit: i32) {
    // Use goto-based structure for reliable exit codes on Windows.
    // Single-line `if ... exit /b N` can have inconsistent behavior
    // when scripts are invoked via `cmd /c`.

    let mut script = String::from("@echo off\n");

    // Generate if-goto chain
    for (i, branch) in branches.iter().enumerate() {
        script.push_str(&format!("if \"%1\"==\"{}\" goto branch{}\n", branch.arg, i));
    }
    script.push_str("goto default\n\n");

    // Generate branch labels
    for (i, branch) in branches.iter().enumerate() {
        script.push_str(&format!(":branch{}\n", i));
        for line in &branch.output {
            // In batch files, special characters need escaping
            // Empty strings use echo. (no space) to print blank line
            let escaped = escape_batch_string(line);
            if escaped.is_empty() {
                script.push_str("echo.\n");
            } else {
                script.push_str(&format!("echo {}\n", escaped));
            }
        }
        script.push_str(&format!("exit /b {}\n\n", branch.exit_code));
    }

    script.push_str(":default\n");
    script.push_str(&format!("exit /b {}\n", default_exit));

    // Write both .cmd and .bat for maximum compatibility
    fs::write(bin_dir.join(format!("{}.cmd", name)), &script).unwrap();
    fs::write(bin_dir.join(format!("{}.bat", name)), &script).unwrap();
}

/// Create a mock command that outputs fixed content regardless of arguments.
///
/// Useful for simple mocks like `llm` that just return canned output.
#[cfg(unix)]
pub fn create_simple_mock(bin_dir: &Path, name: &str, output: &[&str], exit_code: i32) {
    use std::os::unix::fs::PermissionsExt;

    let mut script = String::from("#!/bin/sh\n");
    // Discard stdin (for mocks that receive piped input)
    script.push_str("cat > /dev/null\n");

    for line in output {
        // Escape single quotes in output
        let escaped = line.replace('\'', "'\"'\"'");
        script.push_str(&format!("echo '{}'\n", escaped));
    }
    script.push_str(&format!("exit {}\n", exit_code));

    let script_path = bin_dir.join(name);
    fs::write(&script_path, script).unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(windows)]
pub fn create_simple_mock(bin_dir: &Path, name: &str, output: &[&str], exit_code: i32) {
    let mut script = String::from("@echo off\n");

    // Note: Windows batch files don't read stdin by default, which is
    // sufficient for our mocks. The child exits without consuming stdin,
    // and the parent's write either completes or gets BrokenPipe.

    for line in output {
        let escaped = escape_batch_string(line);
        if escaped.is_empty() {
            script.push_str("echo.\n");
        } else {
            script.push_str(&format!("echo {}\n", escaped));
        }
    }
    script.push_str(&format!("exit /b {}\n", exit_code));

    fs::write(bin_dir.join(format!("{}.cmd", name)), &script).unwrap();
    fs::write(bin_dir.join(format!("{}.bat", name)), &script).unwrap();
}

#[cfg(windows)]
fn escape_batch_string(s: &str) -> String {
    // Batch file escaping rules:
    // - ^ escapes special chars: & | < > ^
    // - % needs to be doubled: %%
    // - Empty strings: use special marker (handled by caller with echo.)
    if s.is_empty() {
        return String::new();
    }

    s.replace('%', "%%")
        .replace('^', "^^")
        .replace('&', "^&")
        .replace('|', "^|")
        .replace('<', "^<")
        .replace('>', "^>")
        .replace('(', "^(")
        .replace(')', "^)")
}

/// A branch that matches two arguments (e.g., "run pytest").
pub struct MockBranch2 {
    /// First argument to match
    pub arg1: &'static str,
    /// Second argument to match
    pub arg2: &'static str,
    /// Lines of output to print
    pub output: Vec<&'static str>,
    /// Exit code
    pub exit_code: i32,
}

/// Create a mock command that switches on two arguments.
///
/// Useful for commands like `uv run pytest` where both args matter.
#[cfg(unix)]
pub fn create_mock_command_2arg(
    bin_dir: &Path,
    name: &str,
    branches: &[MockBranch2],
    default_exit: i32,
) {
    use std::os::unix::fs::PermissionsExt;

    let mut script = String::from("#!/bin/sh\n");

    // Generate if-elif chain for two-arg matching
    let mut first = true;
    for branch in branches {
        if first {
            script.push_str(&format!(
                "if [ \"$1\" = \"{}\" ] && [ \"$2\" = \"{}\" ]; then\n",
                branch.arg1, branch.arg2
            ));
            first = false;
        } else {
            script.push_str(&format!(
                "elif [ \"$1\" = \"{}\" ] && [ \"$2\" = \"{}\" ]; then\n",
                branch.arg1, branch.arg2
            ));
        }
        for line in &branch.output {
            let escaped = line.replace('\'', "'\"'\"'");
            script.push_str(&format!("    echo '{}'\n", escaped));
        }
        script.push_str(&format!("    exit {}\n", branch.exit_code));
    }

    if !first {
        script.push_str("else\n");
        script.push_str(&format!("    exit {}\n", default_exit));
        script.push_str("fi\n");
    } else {
        script.push_str(&format!("exit {}\n", default_exit));
    }

    let script_path = bin_dir.join(name);
    fs::write(&script_path, script).unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(windows)]
pub fn create_mock_command_2arg(
    bin_dir: &Path,
    name: &str,
    branches: &[MockBranch2],
    default_exit: i32,
) {
    let mut script = String::from("@echo off\n");

    // Generate if-goto chain for two-arg matching
    for (i, branch) in branches.iter().enumerate() {
        script.push_str(&format!(
            "if \"%1\"==\"{}\" if \"%2\"==\"{}\" goto branch{}\n",
            branch.arg1, branch.arg2, i
        ));
    }
    script.push_str("goto default\n\n");

    // Generate branch labels
    for (i, branch) in branches.iter().enumerate() {
        script.push_str(&format!(":branch{}\n", i));
        for line in &branch.output {
            // Empty strings use echo. (no space) to print blank line
            let escaped = escape_batch_string(line);
            if escaped.is_empty() {
                script.push_str("echo.\n");
            } else {
                script.push_str(&format!("echo {}\n", escaped));
            }
        }
        script.push_str(&format!("exit /b {}\n\n", branch.exit_code));
    }

    script.push_str(":default\n");
    script.push_str(&format!("exit /b {}\n", default_exit));

    fs::write(bin_dir.join(format!("{}.cmd", name)), &script).unwrap();
    fs::write(bin_dir.join(format!("{}.bat", name)), &script).unwrap();
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
#[cfg(unix)]
pub fn create_mock_uv_sync(bin_dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
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
    let path = bin_dir.join("uv");
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(windows)]
pub fn create_mock_uv_sync(bin_dir: &Path) {
    let script = r#"@echo off
if "%1"=="sync" goto sync
if "%1"=="run" if "%2"=="dev" goto rundev
goto fail

:sync
echo.
echo   Resolved 24 packages in 145ms
echo   Installed 24 packages in 1.2s
exit /b 0

:rundev
echo.
echo   Starting dev server on http://localhost:3000...
exit /b 0

:fail
echo uv: unknown command '%1 %2'
exit /b 1
"#;
    fs::write(bin_dir.join("uv.cmd"), script).unwrap();
    fs::write(bin_dir.join("uv.bat"), script).unwrap();
}

/// Create mock uv that delegates to pytest/ruff commands.
///
/// On Unix, this uses exec. On Windows, it calls the command directly.
#[cfg(unix)]
pub fn create_mock_uv_pytest_ruff(bin_dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
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
    let path = bin_dir.join("uv");
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(windows)]
pub fn create_mock_uv_pytest_ruff(bin_dir: &Path) {
    // Windows can't exec, so we use call instead
    let script = r#"@echo off
if "%1"=="run" if "%2"=="pytest" goto pytest
if "%1"=="run" if "%2"=="ruff" goto ruff
goto fail

:pytest
call pytest.cmd
exit /b %ERRORLEVEL%

:ruff
call ruff.cmd check
exit /b %ERRORLEVEL%

:fail
echo uv: unknown command '%1 %2'
exit /b 1
"#;
    fs::write(bin_dir.join("uv.cmd"), script).unwrap();
    fs::write(bin_dir.join("uv.bat"), script).unwrap();
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
            assert!(bin_dir.join("simple-cmd.cmd").exists());
            assert!(bin_dir.join("simple-cmd.bat").exists());
        }
    }
}
