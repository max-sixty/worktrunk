// Cross-platform mock command helpers
//
// These helpers create mock executables that work on both Unix and Windows.
// All mock logic is written as shell scripts (#!/bin/sh).
//
// On Unix: shell scripts are directly executable via shebang
// On Windows: mock-stub.exe delegates to bash + shell script
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

/// Path to the mock-stub.exe binary, built by `cargo test`.
#[cfg(windows)]
fn mock_stub_exe() -> std::path::PathBuf {
    // The mock-stub binary is built by cargo test (via its dummy integration test)
    // and lives in the same target directory as the test binary.
    let mut path = std::env::current_exe().expect("failed to get test executable path");
    path.pop(); // Remove test binary name
    path.pop(); // Remove deps/
    path.push("mock-stub.exe");
    path
}

/// Write a mock shell script, with platform-appropriate setup.
///
/// On Unix: writes directly as executable script
/// On Windows: writes script + copies mock-stub.exe as name.exe
pub fn write_mock_script(bin_dir: &Path, name: &str, script: &str) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script_path = bin_dir.join(name);
        fs::write(&script_path, script).unwrap();
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(windows)]
    {
        // Write the shell script (no extension) - mock-stub.exe will invoke this via bash
        let script_path = bin_dir.join(name);
        fs::write(&script_path, script).unwrap();

        // Copy mock-stub.exe as name.exe (for Rust's Command::new which uses CreateProcessW)
        let exe_path = bin_dir.join(format!("{}.exe", name));
        fs::copy(mock_stub_exe(), &exe_path)
            .expect("failed to copy mock-stub.exe - did `cargo test` build it?");

        // Create .bat/.cmd shims for CMD/PowerShell invocations.
        // Not strictly required for our tests (Command::new finds .exe, Git Bash finds scripts),
        // but provides a fallback if anyone debugs tests from cmd.exe.
        let shim = format!("@bash \"%~dp0{}\" %*\n", name);
        fs::write(bin_dir.join(format!("{}.cmd", name)), &shim).unwrap();
        fs::write(bin_dir.join(format!("{}.bat", name)), &shim).unwrap();
    }
}

/// Create a mock command that outputs fixed lines and exits.
///
/// Each line is echoed. Stdin is discarded (for mocks that receive piped input).
pub fn create_simple_mock(bin_dir: &Path, name: &str, output: &str, exit_code: i32) {
    let echoes: String = output
        .lines()
        .map(|line| format!("echo '{}'\n", escape_shell_string(line)))
        .collect();

    let script = format!(
        r#"#!/bin/sh
cat > /dev/null
{echoes}exit {exit_code}
"#
    );

    write_mock_script(bin_dir, name, &script);
}

/// Escape single quotes in shell strings.
fn escape_shell_string(s: &str) -> String {
    s.replace('\'', "'\"'\"'")
}

// === High-level mock helpers for common test scenarios ===

/// Create a mock cargo command for tests.
///
/// Handles: test, clippy, install subcommands with realistic output.
pub fn create_mock_cargo(bin_dir: &Path) {
    write_mock_script(
        bin_dir,
        "cargo",
        r#"#!/bin/sh
case "$1" in
    test)
        echo '    Finished test [unoptimized + debuginfo] target(s) in 0.12s'
        echo '     Running unittests src/lib.rs (target/debug/deps/worktrunk-abc123)'
        echo ''
        echo 'running 18 tests'
        echo 'test auth::tests::test_jwt_decode ... ok'
        echo 'test auth::tests::test_jwt_encode ... ok'
        echo 'test auth::tests::test_token_refresh ... ok'
        echo 'test auth::tests::test_token_validation ... ok'
        echo ''
        echo 'test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s'
        ;;
    clippy)
        echo '    Checking worktrunk v0.1.0'
        echo '    Finished dev [unoptimized + debuginfo] target(s) in 1.23s'
        ;;
    install)
        echo '  Installing worktrunk v0.1.0'
        echo '   Compiling worktrunk v0.1.0'
        echo '    Finished release [optimized] target(s) in 2.34s'
        echo '  Installing ~/.cargo/bin/wt'
        echo '   Installed package `worktrunk v0.1.0` (executable `wt`)'
        ;;
    *)
        exit 1
        ;;
esac
"#,
    );
}

/// Create a mock llm command that outputs a commit message.
///
/// The commit message is suitable for JWT authentication feature commits.
pub fn create_mock_llm_auth(bin_dir: &Path) {
    create_simple_mock(
        bin_dir,
        "llm",
        r#"feat(auth): Implement JWT authentication system

Add comprehensive JWT token handling including validation, refresh logic,
and authentication tests. This establishes the foundation for secure
API authentication.

- Implement token refresh mechanism with expiry handling
- Add JWT encoding/decoding with signature verification
- Create test suite covering all authentication flows"#,
        0,
    );
}

/// Create a mock llm command for API endpoint commits.
pub fn create_mock_llm_api(bin_dir: &Path) {
    create_simple_mock(
        bin_dir,
        "llm",
        r#"feat(api): Add user authentication endpoints

Implement login and token refresh endpoints with JWT validation.
Includes comprehensive test coverage and input validation."#,
        0,
    );
}

/// Create a mock uv command for dependency sync and dev server.
///
/// Handles: `uv sync` (1 arg) and `uv run dev` (2 args).
pub fn create_mock_uv_sync(bin_dir: &Path) {
    write_mock_script(
        bin_dir,
        "uv",
        r#"#!/bin/sh
if [ "$1" = "sync" ]; then
    echo ''
    echo '  Resolved 24 packages in 145ms'
    echo '  Installed 24 packages in 1.2s'
elif [ "$1" = "run" ] && [ "$2" = "dev" ]; then
    echo ''
    echo '  Starting dev server on http://localhost:3000...'
else
    echo "uv: unknown command '$1 $2'"
    exit 1
fi
"#,
    );
}

/// Create mock uv that delegates to pytest/ruff commands.
pub fn create_mock_uv_pytest_ruff(bin_dir: &Path) {
    write_mock_script(
        bin_dir,
        "uv",
        r#"#!/bin/sh
if [ "$1" = "run" ] && [ "$2" = "pytest" ]; then
    exec pytest
elif [ "$1" = "run" ] && [ "$2" = "ruff" ]; then
    shift 2
    exec ruff "$@"
else
    echo "uv: unknown command '$1 $2'"
    exit 1
fi
"#,
    );
}

/// Create a mock pytest command with test output.
pub fn create_mock_pytest(bin_dir: &Path) {
    create_simple_mock(
        bin_dir,
        "pytest",
        r#"
============================= test session starts ==============================
collected 3 items

tests/test_auth.py::test_login_success PASSED                            [ 33%]
tests/test_auth.py::test_login_invalid_password PASSED                   [ 66%]
tests/test_auth.py::test_token_validation PASSED                         [100%]

============================== 3 passed in 0.8s ===============================
"#,
        0,
    );
}

/// Create a mock ruff command.
pub fn create_mock_ruff(bin_dir: &Path) {
    write_mock_script(
        bin_dir,
        "ruff",
        r#"#!/bin/sh
case "$1" in
    check)
        echo ''
        echo 'All checks passed!'
        echo ''
        ;;
    *)
        exit 1
        ;;
esac
"#,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_write_mock_script() {
        let temp = TempDir::new().unwrap();
        let bin_dir = temp.path();

        write_mock_script(bin_dir, "test-cmd", "#!/bin/sh\necho 'hello'\n");

        #[cfg(unix)]
        assert!(bin_dir.join("test-cmd").exists());

        #[cfg(windows)]
        {
            assert!(bin_dir.join("test-cmd").exists());
            assert!(bin_dir.join("test-cmd.exe").exists());
            assert!(bin_dir.join("test-cmd.cmd").exists());
            assert!(bin_dir.join("test-cmd.bat").exists());
        }
    }

    #[test]
    fn test_create_simple_mock() {
        let temp = TempDir::new().unwrap();
        let bin_dir = temp.path();

        create_simple_mock(bin_dir, "simple-cmd", "Line 1\nLine 2\nLine 3", 0);

        #[cfg(unix)]
        assert!(bin_dir.join("simple-cmd").exists());

        #[cfg(windows)]
        {
            assert!(bin_dir.join("simple-cmd").exists());
            assert!(bin_dir.join("simple-cmd.exe").exists());
            assert!(bin_dir.join("simple-cmd.cmd").exists());
            assert!(bin_dir.join("simple-cmd.bat").exists());
        }
    }
}
