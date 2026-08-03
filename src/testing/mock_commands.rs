// Cross-platform mock command helpers
//
// These helpers create mock executables that work on both Unix and Windows.
// Mock behavior is defined via JSON config files, played back by the `wt`
// binary itself: `main()` dispatches to `testing::mock_stub` when
// `WORKTRUNK_TEST_MOCK_CONFIG_DIR` is set, argv[0] isn't wt's own name, and
// the config dir holds a `<argv0>.json`.
//
// On Unix: `wt` is symlinked as the command name (e.g., `gh`)
// On Windows: `wt.exe` is hard-linked (or copied) as `gh.exe`
//
// Both platforms read `<command>.json` for configuration.
//
// This approach:
// - The mocks are the binary under test, so they can't be missing or stale
// - No bash dependency
// - Config is just JSON - easy to generate and debug

use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

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
    wait_for_file: Option<String>,
}

impl MockResponse {
    /// Respond by reading contents from a file.
    pub fn file(path: &str) -> Self {
        Self {
            file: Some(path.to_string()),
            output: None,
            stderr: None,
            exit_code: 0,
            wait_for_file: None,
        }
    }

    /// Respond with literal output (stdout).
    pub fn output(text: &str) -> Self {
        Self {
            file: None,
            output: Some(text.to_string()),
            stderr: None,
            exit_code: 0,
            wait_for_file: None,
        }
    }

    /// Respond with stderr output.
    pub fn stderr(text: &str) -> Self {
        Self {
            file: None,
            output: None,
            stderr: Some(text.to_string()),
            exit_code: 0,
            wait_for_file: None,
        }
    }

    /// Just exit with a code (no output).
    pub fn exit(code: i32) -> Self {
        Self {
            file: None,
            output: None,
            stderr: None,
            exit_code: code,
            wait_for_file: None,
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

    /// Wait until `path` exists under the mock config directory before
    /// responding. This gives a test a causal release gate for an in-flight
    /// command: first assert the caller's loading state, then create the file
    /// and observe the response without sending another input.
    pub fn wait_for_file(mut self, path: &str) -> Self {
        self.wait_for_file = Some(path.to_string());
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
        if let Some(path) = &self.wait_for_file {
            obj.insert("wait_for_file".to_string(), json!(path));
        }
        serde_json::Value::Object(obj)
    }
}

impl MockConfig {
    /// Create a new mock config for the given command name.
    pub fn new(name: &str) -> Self {
        // Playback dispatches on argv[0] (see `testing::mock_stub`), and wt's
        // own names are reserved for wt itself — a mock under either name
        // would silently run the real binary. Case-insensitive to match the
        // dispatch's own guard: the config lookup goes through a filesystem
        // that is case-insensitive on macOS and Windows, so a `WT.json` *is*
        // `wt.json` there.
        assert!(
            !name.eq_ignore_ascii_case("wt") && !name.eq_ignore_ascii_case("git-wt"),
            "a mock command cannot be named {name:?}: the mocks are the wt \
             binary dispatching on argv[0]"
        );
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

/// Every invocation of mock command `name`, in call order, one entry per
/// spawn with the arguments space-joined.
///
/// Lets a test assert on the *number of spawns*, not just the response —
/// the property under test wherever a command is batched (one `lsof` for N
/// PIDs) rather than called in a loop. Returns empty when the command was
/// never invoked, so a "was not called" assertion reads naturally.
///
/// `log_dir` is the directory passed to the command as
/// `WORKTRUNK_TEST_MOCK_CALL_LOG_DIR`. It must be OUTSIDE the repo under test:
/// mocks spawned by hooks would otherwise dirty the working tree mid-run and
/// change the behavior being measured.
pub fn mock_calls(log_dir: &Path, name: &str) -> Vec<String> {
    fs::read_to_string(log_dir.join(format!("{}.calls", name)))
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Create mock binary in bin_dir with the given name: a link to the `wt`
/// binary, whose argv\[0\] dispatch plays back the mock config
/// (`testing::mock_stub`).
///
/// Private on purpose: `MockConfig::write` is the only caller, and it writes
/// `<name>.json` before linking. A link therefore can't exist without its
/// config, which is what lets the dispatch treat a foreign argv\[0\] with no
/// config as wt itself rather than a broken mock.
///
/// Uses symlinks on Unix (instant, works across filesystems).
/// Uses hard links on Windows (symlinks require admin privileges), falling
/// back to a copy when the link fails — hard links can't cross drives, and
/// the debug `wt.exe` is large enough that a copy per mock is the fallback
/// rather than the default. CI keeps the fallback rare by pinning TEMP to
/// the workspace volume (ci.yaml's Windows TEMP step); on a machine whose
/// temp dir is on another drive, mock setup degrades to a full copy per
/// mock but still works.
fn copy_mock_binary(bin_dir: &Path, name: &str) {
    let stub = super::wt_bin();

    #[cfg(unix)]
    {
        let dest = bin_dir.join(name);
        // Remove existing (config may have changed)
        let _ = fs::remove_file(&dest);
        std::os::unix::fs::symlink(&stub, &dest).expect("failed to symlink wt as mock binary");
    }

    #[cfg(windows)]
    {
        let dest = bin_dir.join(format!("{}.exe", name));
        // Remove existing (config may have changed)
        let _ = fs::remove_file(&dest);
        if fs::hard_link(&stub, &dest).is_err() {
            fs::copy(&stub, &dest).expect("failed to copy wt.exe as mock binary");
        }
    }
}

// =============================================================================
// High-level mock helpers for common test scenarios
// =============================================================================

/// The stderr `tea api --include` writes ahead of the body: the status line,
/// then the response headers, then a blank line.
///
/// Every mock `tea api` response that stands for an HTTP response carries this
/// — both Gitea backends read the status from here, and a body arriving with no
/// status line is a failed request rather than a resource. `status` is the
/// status line's tail (`200 OK`, `404 Not Found`).
///
/// A mock standing for `tea` itself failing (non-zero exit, no response) has no
/// status line to write, and must not have one.
pub fn tea_api_include_stderr(status: &str) -> String {
    format!("HTTP/1.1 {status}\r\nContent-Type: application/json;charset=utf-8\r\n\r\n")
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "a mock command cannot be named")]
    fn mock_config_rejects_wt_as_name() {
        MockConfig::new("wt");
    }

    /// The filesystem the dispatch probes is case-insensitive on macOS and
    /// Windows — `WT.json` there *is* `wt.json` — so the name guard must be
    /// case-insensitive too.
    #[test]
    #[should_panic(expected = "a mock command cannot be named")]
    fn mock_config_rejects_wt_case_insensitively() {
        MockConfig::new("WT");
    }
}
