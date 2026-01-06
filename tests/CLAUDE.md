# Testing Guidelines

## Running `wt` Commands in Tests

**Use the correct helper to ensure test isolation.** Tests that spawn `wt` must
be isolated from the host environment to prevent:

- **Directive leakage**: Test commands writing to the user's shell directive file
- **Config pollution**: Tests reading/writing the user's real config
- **Git interference**: Host GIT_* environment variables affecting test behavior

### With a TestRepo fixture (most tests)

Use `repo.wt_command()` which returns a pre-configured Command:

```rust
// ✅ GOOD: Simple case
let output = repo.wt_command()
    .args(["switch", "--create", "feature"])
    .output()?;

// ✅ GOOD: With additional configuration (piped stdin, etc.)
let mut cmd = repo.wt_command();
cmd.args(["switch", "--create", "feature"])
    .stdin(Stdio::piped());
```

```rust
// ❌ BAD: Missing isolation - inherits host environment
let output = Command::new(env!("CARGO_BIN_EXE_wt"))
    .args(["switch", "--create", "feature"])
    .current_dir(repo.root_path())
    .output()?;
```

### Without a TestRepo (e.g., readme_sync tests)

Use the free function `wt_command()`:

```rust
use crate::common::wt_command;

// ✅ GOOD: Isolated from host environment
let output = wt_command()
    .args(["--help"])
    .current_dir(project_root)
    .output()?;
```

### Method reference

| Method | Use when |
|--------|----------|
| `repo.wt_command()` | Running wt commands with a TestRepo |
| `wt_command()` | Running wt without a TestRepo (free function) |
| `repo.git_command()` | Running git commands |

## Timing Tests: Long Timeouts with Fast Polling

**Core principle:** Use long timeouts (5+ seconds) for reliability on slow CI, but poll frequently (10-50ms) so tests complete quickly when things work.

This achieves both goals:
- **No flaky failures** on slow machines - generous timeout accommodates worst-case
- **Fast tests** on normal machines - frequent polling means no unnecessary waiting

```rust
// ✅ GOOD: Long timeout, fast polling
let timeout = Duration::from_secs(5);
let poll_interval = Duration::from_millis(10);
let start = Instant::now();
while start.elapsed() < timeout {
    if condition_met() { break; }
    thread::sleep(poll_interval);
}

// ❌ BAD: Fixed sleep (always slow, might still fail)
thread::sleep(Duration::from_millis(500));
assert!(condition_met());

// ❌ BAD: Short timeout (flaky on slow CI)
let timeout = Duration::from_millis(100);
```

Use the helpers in `tests/common/mod.rs`:

```rust
use crate::common::{wait_for_file, wait_for_file_count};

// ✅ Poll for file existence with 5+ second timeout
wait_for_file(&log_file, Duration::from_secs(5));

// ✅ Poll for multiple files
wait_for_file_count(&log_dir, "log", 3, Duration::from_secs(5));
```

These use exponential backoff (10ms → 500ms cap) for fast initial checks that back off on slow CI.

**Exception - testing absence:** When verifying something did NOT happen, polling doesn't work. Use a fixed 500ms+ sleep:

```rust
thread::sleep(Duration::from_millis(500));
assert!(!marker_file.exists(), "Command should NOT have run");
```

## Testing with --execute Commands

Use `--yes` to skip interactive prompts in tests. Don't pipe input to stdin.

## Feature Flags, Not Runtime Skipping

**Never skip tests based on runtime availability checks.** Use Cargo feature flags instead.

```rust
// ❌ BAD: Runtime skip - test silently passes when tool unavailable
#[test]
fn test_fish_integration() {
    if !shell_available("fish") {
        eprintln!("Skipping: fish not available");
        return;
    }
    // test code...
}

// ✅ GOOD: Feature flag - test excluded from compilation
#[cfg(feature = "shell-integration-tests")]
#[test]
fn test_fish_integration() {
    // test code...
}
```

**Why:**
- Runtime skips hide missing test coverage in CI logs
- Feature flags make dependencies explicit in `Cargo.toml`
- `cargo test` output clearly shows which tests ran vs were compiled out
- CI can enable features when dependencies are installed

**Existing feature flags:**
- `shell-integration-tests` — Tests requiring bash/zsh/fish shells and PTY

## README Examples and Snapshot Testing

### Problem: Separated stdout/stderr in Standard Snapshots

README examples need to show output as users see it in their terminal - with stdout and stderr interleaved in the order they appear. However, the standard `insta_cmd` snapshot testing (used in most integration tests) separates stdout and stderr into different sections:

```yaml
----- stdout -----
🔄 Running pre-merge test:
  uv run pytest

----- stderr -----
============================= test session starts ==============================
collected 18 items
...
```

This makes snapshots **not directly copyable** into README.md because:
1. The output is split into two sections
2. We lose the temporal ordering (which output appeared first)
3. Users never see this separation - their terminal shows combined output

### Solution: Use PTY-based Testing for README Examples

For tests that generate README examples, use the PTY-based execution pattern from `tests/integration_tests/shell_wrapper.rs`:

**Key function:** `exec_in_pty()` (line 235)
- Uses `portable_pty` to execute commands in a pseudo-terminal
- Returns combined stdout+stderr as a single `String`
- Captures output exactly as users see it in their terminal
- Includes ANSI color codes and proper temporal ordering

**Pattern to use:**

```rust
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

fn exec_in_pty(
    shell: &str,
    script: &str,
    working_dir: &Path,
    env_vars: &[(String, String)],
) -> (String, i32) {
    // Execute command in PTY
    // Returns (combined_output, exit_code)
}

// In your test:
let (combined_output, exit_code) = exec_in_pty("bash", "wt merge", &repo_path, &env_vars);
assert_snapshot!("readme_example_name", combined_output);
```

**Benefits:**
- Output is directly copyable to README.md
- Shows actual user experience (interleaved stdout/stderr)
- Preserves temporal ordering of output
- No manual merging of stdout/stderr needed

**Example:** See `tests/integration_tests/shell_wrapper.rs`:
- Line 65-66: `ShellOutput` struct with `combined: String`
- Line 235-323: `exec_in_pty()` implementation using `portable_pty`
- Line 397+: Usage pattern in tests

### When to Use Each Approach

**Use `insta_cmd` (standard snapshots):**
- Unit and integration tests focused on correctness
- Tests that need to verify stdout/stderr separately
- Tests checking exit codes and specific error messages
- Most tests in the codebase

**Use `exec_in_pty` (PTY-based snapshots):**
- Tests generating output for README.md examples
- Tests verifying shell integration (`wt` function, directives)
- Tests needing to verify complete user experience
- Any test where temporal ordering of stdout/stderr matters

### Current Status

**README examples using PTY-based approach:**
- Shell wrapper tests (all of `tests/integration_tests/shell_wrapper.rs`)

**README examples using standard snapshots (working, but require manual editing):**
- `test_readme_example_simple()` - Quick start merge example
- `test_readme_example_complex()` - LLM commit example
- `test_readme_example_hooks_post_create()` - Post-create hooks
- `test_readme_example_hooks_pre_merge()` - Pre-merge hooks

**Current workflow:** These tests work correctly and generate accurate snapshots. However, the snapshots separate stdout and stderr into different sections, which means they cannot be directly copied into README.md. Instead, the README examples are manually edited versions that merge stdout/stderr in the correct temporal order and remove ANSI codes.

**Future improvement:** Migrate README example tests to use PTY execution so snapshots are directly copyable into README.md without manual editing. This is an enhancement for developer convenience, not a bug fix.

### Migration Checklist

When converting a README example test from `insta_cmd` to PTY-based:

1. ✅ Import `portable_pty` dependencies
2. ✅ Extract or create `exec_in_pty()` helper (can reuse from shell_wrapper.rs)
3. ✅ Replace `make_snapshot_cmd()` + `assert_cmd_snapshot!()` with `exec_in_pty()` + `assert_snapshot!()`
4. ✅ Ensure environment variables include `CLICOLOR_FORCE=1` for ANSI codes
5. ✅ Update snapshot file format (file snapshot, not inline)
6. ✅ Verify output matches expected README format
7. ✅ Update README.md to reference new snapshot location

### Implementation Note

The PTY approach is specifically for **user-facing output documentation**. It's not a replacement for standard integration tests - both approaches serve different purposes and should coexist in the test suite.

## Coverage in PTY Tests

PTY tests use `cmd.env_clear()` for isolation. To enable coverage, pass through LLVM env vars:

```rust
// Standard setup (most PTY tests)
crate::common::configure_pty_command(&mut cmd);

// Custom env setup (shell tests needing USER, SHELL, ZDOTDIR)
cmd.env_clear();
cmd.env("HOME", ...);
// ... custom env ...
crate::common::pass_coverage_env_to_pty_cmd(&mut cmd);
```

## Deterministic Time in Tests

Tests use `TEST_EPOCH` (2025-01-01) for reproducible timestamps. The constant is defined in `tests/common/mod.rs` and automatically set as `SOURCE_DATE_EPOCH` in the test environment.

**For test data with timestamps** (cache entries, etc.), use the constant:

```rust
use crate::common::TEST_EPOCH;

repo.git_command(&[
    "config", "worktrunk.state.feature.ci-status",
    &format!(r#"{{"checked_at":{TEST_EPOCH},"head":"abc123"}}"#),
]);
```

**For production code** that needs timestamps, use `worktrunk::utils::get_now()` which respects `SOURCE_DATE_EPOCH`. Using `SystemTime::now()` directly causes flaky tests.
