// Cross-platform mock command helpers
//
// These helpers create mock executables that work on both Unix and Windows.
// Mock behavior is defined via JSON config files, read by the mock-stub binary.
//
// On Unix: mock-stub is copied as the command name (e.g., `gh`)
// On Windows: mock-stub.exe is copied as `gh.exe`
//
// Both platforms read `<command>.json` for configuration.
//
// This approach:
// - Single Rust binary for all platforms
// - No bash dependency
// - Config is just JSON - easy to generate and debug

use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Path to the mock-stub binary, built by `cargo test`.
fn mock_stub_binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().expect("failed to get test executable path");
    path.pop(); // Remove test binary name
    path.pop(); // Remove deps/

    #[cfg(windows)]
    path.push("mock-stub.exe");

    #[cfg(not(windows))]
    path.push("mock-stub");

    path
}

/// Builder for mock command configuration.
///
/// Example:
/// ```ignore
/// MockConfig::new("gh")
///     .version("gh version 2.0.0 (mock)")
///     .command("auth", MockResponse::exit(0))
///     .command("pr", MockResponse::file("pr_data.json"))
///     .write(bin_dir);
/// ```
pub struct MockConfig {
    name: String,
    version: Option<String>,
    commands: HashMap<String, MockResponse>,
}

/// How to respond to a command.
pub struct MockResponse {
    file: Option<String>,
    output: Option<String>,
    stderr: Option<String>,
    exit_code: i32,
}

impl MockResponse {
    /// Respond by reading contents from a file.
    pub fn file(path: &str) -> Self {
        Self {
            file: Some(path.to_string()),
            output: None,
            stderr: None,
            exit_code: 0,
        }
    }

    /// Respond with literal output (stdout).
    pub fn output(text: &str) -> Self {
        Self {
            file: None,
            output: Some(text.to_string()),
            stderr: None,
            exit_code: 0,
        }
    }

    /// Respond with stderr output.
    pub fn stderr(text: &str) -> Self {
        Self {
            file: None,
            output: None,
            stderr: Some(text.to_string()),
            exit_code: 0,
        }
    }

    /// Just exit with a code (no output).
    pub fn exit(code: i32) -> Self {
        Self {
            file: None,
            output: None,
            stderr: None,
            exit_code: code,
        }
    }

    /// Set exit code (chainable).
    pub fn with_exit_code(mut self, code: i32) -> Self {
        self.exit_code = code;
        self
    }

    /// Add stderr output (chainable).
    pub fn with_stderr(mut self, text: &str) -> Self {
        self.stderr = Some(text.to_string());
        self
    }

    fn to_json(&self) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        if let Some(f) = &self.file {
            obj.insert("file".to_string(), json!(f));
        }
        if let Some(o) = &self.output {
            obj.insert("output".to_string(), json!(o));
        }
        if let Some(e) = &self.stderr {
            obj.insert("stderr".to_string(), json!(e));
        }
        if self.exit_code != 0
            || (self.file.is_none() && self.output.is_none() && self.stderr.is_none())
        {
            obj.insert("exit_code".to_string(), json!(self.exit_code));
        }
        serde_json::Value::Object(obj)
    }
}

impl MockConfig {
    /// Create a new mock config for the given command name.
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            version: None,
            commands: HashMap::new(),
        }
    }

    /// Set the version string returned by `--version`.
    pub fn version(mut self, v: &str) -> Self {
        self.version = Some(v.to_string());
        self
    }

    /// Add a command handler.
    pub fn command(mut self, cmd: &str, response: MockResponse) -> Self {
        self.commands.insert(cmd.to_string(), response);
        self
    }

    /// Write the config and copy the mock binary to bin_dir.
    pub fn write(self, bin_dir: &Path) {
        let mut config = serde_json::Map::new();

        if let Some(v) = &self.version {
            config.insert("version".to_string(), json!(v));
        }

        let commands: serde_json::Map<String, serde_json::Value> = self
            .commands
            .iter()
            .map(|(k, v)| (k.clone(), v.to_json()))
            .collect();
        config.insert("commands".to_string(), serde_json::Value::Object(commands));

        let json = serde_json::to_string_pretty(&serde_json::Value::Object(config)).unwrap();

        // Write config file
        let config_path = bin_dir.join(format!("{}.json", self.name));
        fs::write(&config_path, json).unwrap();

        // Copy mock binary
        copy_mock_binary(bin_dir, &self.name);
    }
}

/// Create mock binary in bin_dir with the given name.
/// Uses symlinks on Unix (instant, works across filesystems).
/// Uses hard links on Windows (symlinks require admin privileges).
pub fn copy_mock_binary(bin_dir: &Path, name: &str) {
    let stub = mock_stub_binary();

    #[cfg(unix)]
    {
        let dest = bin_dir.join(name);
        // Remove existing (config may have changed)
        let _ = fs::remove_file(&dest);
        std::os::unix::fs::symlink(&stub, &dest).expect("failed to symlink mock-stub binary");
    }

    #[cfg(windows)]
    {
        let dest = bin_dir.join(format!("{}.exe", name));
        // Remove existing (config may have changed)
        let _ = fs::remove_file(&dest);
        // Copy on Windows - hard links fail across drives (common on CI),
        // and symlinks require admin privileges
        fs::copy(&stub, &dest).expect("failed to copy mock-stub.exe");
    }
}

// =============================================================================
// High-level mock helpers for common test scenarios
// =============================================================================

/// Create a mock cargo command for tests.
pub fn create_mock_cargo(bin_dir: &Path) {
    MockConfig::new("cargo")
        .command(
            "test",
            MockResponse::output(
                "    Finished test [unoptimized + debuginfo] target(s) in 0.12s
     Running unittests src/lib.rs (target/debug/deps/worktrunk-abc123)

running 18 tests
test auth::tests::test_jwt_decode ... ok
test auth::tests::test_jwt_encode ... ok
test auth::tests::test_token_refresh ... ok
test auth::tests::test_token_validation ... ok

test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s
",
            ),
        )
        .command(
            "clippy",
            MockResponse::output(
                "    Checking worktrunk v0.1.0
    Finished dev [unoptimized + debuginfo] target(s) in 1.23s
",
            ),
        )
        .command(
            "install",
            MockResponse::output(
                "  Installing worktrunk v0.1.0
   Compiling worktrunk v0.1.0
    Finished release [optimized] target(s) in 2.34s
  Installing ~/.cargo/bin/wt
   Installed package `worktrunk v0.1.0` (executable `wt`)
",
            ),
        )
        .write(bin_dir);
}

/// Create a mock llm command that outputs a commit message.
pub fn create_mock_llm_auth(bin_dir: &Path) {
    MockConfig::new("llm")
        .command(
            "_default",
            MockResponse::output(
                "feat(auth): Implement JWT authentication system

Add comprehensive JWT token handling including validation, refresh logic,
and authentication tests. This establishes the foundation for secure
API authentication.

- Implement token refresh mechanism with expiry handling
- Add JWT encoding/decoding with signature verification
- Create test suite covering all authentication flows",
            ),
        )
        .write(bin_dir);
}

/// Create a mock llm command for API endpoint commits.
pub fn create_mock_llm_api(bin_dir: &Path) {
    MockConfig::new("llm")
        .command(
            "_default",
            MockResponse::output(
                "feat(api): Add user authentication endpoints

Implement login and token refresh endpoints with JWT validation.
Includes comprehensive test coverage and input validation.",
            ),
        )
        .write(bin_dir);
}

/// Create a mock llm command for quickstart documentation.
/// Simple output for clean documentation examples.
pub fn create_mock_llm_quickstart(bin_dir: &Path) {
    MockConfig::new("llm")
        .command(
            "_default",
            MockResponse::output("Add authentication module"),
        )
        .write(bin_dir);
}

/// Create a mock uv command for dependency sync and dev server.
pub fn create_mock_uv_sync(bin_dir: &Path) {
    MockConfig::new("uv")
        .command(
            "sync",
            MockResponse::output(
                "
  Resolved 24 packages in 145ms
  Installed 24 packages in 1.2s
",
            ),
        )
        .command(
            "run",
            MockResponse::output(
                "
  Starting dev server on http://localhost:3000...
",
            ),
        )
        .write(bin_dir);
}

/// Create mock uv that delegates to pytest/ruff commands.
///
/// Note: This mock doesn't actually delegate - it provides fixed output.
/// For tests needing real delegation, set up both commands separately.
pub fn create_mock_uv_pytest_ruff(bin_dir: &Path) {
    MockConfig::new("uv")
        .command(
            "run",
            MockResponse::output(
                "
============================= test session starts ==============================
collected 3 items

tests/test_auth.py::test_login_success PASSED                            [ 33%]
tests/test_auth.py::test_login_invalid_password PASSED                   [ 66%]
tests/test_auth.py::test_token_validation PASSED                         [100%]

============================== 3 passed in 0.8s ===============================
",
            ),
        )
        .write(bin_dir);
}

/// Create a mock pytest command with test output.
pub fn create_mock_pytest(bin_dir: &Path) {
    MockConfig::new("pytest")
        .command(
            "_default",
            MockResponse::output(
                "
============================= test session starts ==============================
collected 3 items

tests/test_auth.py::test_login_success PASSED                            [ 33%]
tests/test_auth.py::test_login_invalid_password PASSED                   [ 66%]
tests/test_auth.py::test_token_validation PASSED                         [100%]

============================== 3 passed in 0.8s ===============================
",
            ),
        )
        .write(bin_dir);
}

/// Create a mock ruff command.
pub fn create_mock_ruff(bin_dir: &Path) {
    MockConfig::new("ruff")
        .command("check", MockResponse::output("\nAll checks passed!\n\n"))
        .write(bin_dir);
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_mock_config_write() {
        let temp = TempDir::new().unwrap();
        let bin_dir = temp.path();

        MockConfig::new("test-cmd")
            .version("test-cmd version 1.0")
            .command("foo", MockResponse::output("hello"))
            .command("bar", MockResponse::exit(42))
            .write(bin_dir);

        // Check config file exists and is valid JSON
        let config_path = bin_dir.join("test-cmd.json");
        assert!(config_path.exists());
        let content = fs::read_to_string(&config_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["version"], "test-cmd version 1.0");

        // Check binary exists
        #[cfg(unix)]
        assert!(bin_dir.join("test-cmd").exists());

        #[cfg(windows)]
        assert!(bin_dir.join("test-cmd.exe").exists());
    }
}
