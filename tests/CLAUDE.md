# Testing Guidelines

## Running the Suite

```bash
cargo run -- hook pre-merge --yes                                  # all tests + lints
pre-commit run --all-files                                         # lints only
cargo test --lib --bins                                            # unit tests
cargo test --test integration                                      # integration (no shell tests)
cargo test --test integration --features shell-integration-tests   # + shell tests
```

A filtered `--test integration` run on a fresh `target/` panics with "mock-stub binary not found" (a target filter skips the helper-bin build). Fix: `cargo build -p mock-stub`, or use `cargo nextest run` / `cargo llvm-cov nextest`.

**Claude Code web:** `task setup-web` installs zsh/fish/nushell, `gh`, and dev tools. Install `task` first if needed: `sh -c "$(curl --location https://taskfile.dev/install.sh)" -- -d -b ~/bin` then `export PATH="$HOME/bin:$PATH"`. The permission tests (`test_permission_error_prevents_save`, `test_approval_prompt_permission_error`) skip automatically when running as root.

**Shell/PTY tests** (`shell-integration-tests` feature: approval prompts, picker, progressive rendering, shell wrappers): tests that spawn interactive shells (`zsh -ic`, `bash -ic`) make nextest's InputHandler take SIGTTOU when restoring terminal settings, suspending the run mid-test (`zsh: suspended (tty output)`; see [nextest#2878](https://github.com/nextest-rs/nextest/issues/2878)). Use `cargo test` instead of `cargo nextest run`, or set `NEXTEST_NO_INPUT_HANDLER=1`. The pre-merge hook sets it automatically.

## Coverage Investigation

`task coverage` runs the suite and writes an HTML report to `target/llvm-cov/html/index.html`. Both CI (`code-coverage` job) and local `task coverage` pass `--features shell-integration-tests`, so code behind that flag is compiled and measured.

When `codecov/patch` fails, investigate before declaring ready (the merge gate itself is in the root `CLAUDE.md` → Coverage):

```bash
task coverage
cargo llvm-cov report --show-missing-lines | grep <file>   # authoritative miss list; matches codecov line-for-line
```

For each uncovered function, either write a test (integration tests via `assert_cmd_snapshot!` do capture subprocess coverage) or document why it's intentionally untested. If codecov's compare API must be queried directly, `coverage.head` is a `LineType` enum: `0=hit`, `1=miss`, `2=partial`.

**Renames and moves:** `git mv` can trigger codecov/patch failures on pre-existing uncovered lines — codecov treats changed lines in renamed files as part of the patch. If the lines are unchanged and predate the rename it's a false positive; verify against `main` under the old path.

**"N functions have mismatched data" warning:** `cargo llvm-cov` merges profiles from multiple compilation targets with minor codegen differences (typically 5–20 functions). Expected, harmless, no suppression flag exists ([LLVM #97574](https://github.com/llvm/llvm-project/issues/97574)).

PTY tests need extra setup to be measured at all — they `env_clear()` the subprocess, so LLVM env vars must be passed through explicitly. See "Coverage in PTY Tests" below.

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

`wt_command()`'s default `current_dir` is a process-scoped empty tempdir
(outside any git repo, no project config) — so a bare `wt_command()` won't
pick up the test process's inherited CWD. Tests that need a specific CWD
(e.g., the worktrunk repo root for `readme_sync` help-text capture) must
call `.current_dir(...)` explicitly.

### Method reference

| Method | Returns | Use when |
|--------|---------|----------|
| `repo.wt_command()` | `Command` | Running wt commands with a TestRepo |
| `wt_command()` | `Command` | Running wt without a TestRepo (free function) |
| `repo.git_command()` | `Cmd` | Running git commands (use `.run()` not `.output()`) |

## Config Isolation for In-Process Unit Tests

`repo.wt_command()` / `wt_command()` isolate *subprocess* tests (above). An
in-process unit test that calls library functions directly gets no such
isolation: it runs in the test process, which inherits the real environment.

The `Approvals` and `UserConfig` mutation methods take an explicit `&Path`, so
a unit test passes a tempdir-backed path and the write stays isolated. The
global resolvers do not isolate: `Approvals::load()`, `approvals_path()`,
`config_path()`, and `system_config_path()` all fall back to the real
`~/.config/worktrunk/`.

<example>
<bad reason="Approvals::load() reads the real ~/.config/worktrunk/approvals.toml">

Bad:

```rust
let mut approvals = Approvals::load().unwrap();
approvals.approve_command(project, command, &approvals_path).unwrap();
```

</bad>
<good reason="Default state plus a tempdir path touches no real config">

Good:

```rust
let temp_dir = tempfile::tempdir().unwrap();
let approvals_path = temp_dir.path().join("approvals.toml");
let mut approvals = Approvals::default();
approvals.approve_command(project, command, &approvals_path).unwrap();
```

</good>
</example>

`approvals_path()` panics when `WORKTRUNK_APPROVALS_PATH` is unset, but
`#[cfg(test)]` makes that guard fire only for `worktrunk` lib-crate tests. A
bin-crate test (anything under `src/commands/`) links the lib in non-test
mode, so the guard is compiled out and a global read hits the real config
silently: it passes wherever `$HOME` is writable and fails only in a sandbox
that forbids it. `config_path()` and `system_config_path()` have no guard at
all.

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
use crate::common::{wait_for_file, wait_for_file_count, wait_for_file_content};

// ✅ Poll for file existence (60-second default timeout)
wait_for_file(&log_file);

// ✅ Poll for multiple files
wait_for_file_count(&log_dir, "log", 3);

// ✅ Poll for file with non-empty content
wait_for_file_content(&marker_file);
```

These use exponential backoff (10ms → 500ms cap) for fast initial checks that back off on slow CI. The 60-second default timeout is generous enough to avoid flakiness under CI load.

### Event-driven code: drive the scenario from the callback

When the system under test exposes a callback, channel, or event hook, drive the scenario **causally** through that hook instead of racing wall-clock timers. The callback gives you a happens-before edge into the loop — use it to inject inputs and terminate the run, so the test's timing depends on the event ordering, not on CPU scheduling.

```rust
// ✅ GOOD: causally driven — first Stall event injects a result; a Stall
// observed after the result drops tx to end the drain via Disconnected.
// Runs at threshold speed on any hardware; the 5s deadline is only a
// safety net.
let mut sender = Some(tx);
let mut saw_result = false;
let outcome = drain_results_with_timings(
    rx, /* ... */,
    Instant::now() + Duration::from_secs(5),
    StallTimings { threshold: ms(20), tick: ms(10) },
    |event| match event {
        DrainEvent::Stall { .. } if !saw_result => {
            sender.as_ref().unwrap().send(result).unwrap();
        }
        DrainEvent::Stall { .. } => { sender.take(); } // end drain
        DrainEvent::Result { .. } => { saw_result = true; }
        _ => {}
    },
);

// ❌ BAD: producer sleeps to land a result "partway through" a window
// whose size is itself a wall-clock deadline. Every extension of the
// deadline just makes the race wider, not correct.
std::thread::spawn(move || {
    std::thread::sleep(Duration::from_millis(80));
    tx.send(result).unwrap();
    std::thread::sleep(Duration::from_millis(2000));
    drop(tx);
});
let outcome = drain_results_with_timings(
    rx, /* ... */,
    Instant::now() + Duration::from_millis(1000),
    /* ... */,
);
```

**Rule of thumb:** if your producer thread needs `thread::sleep` to line up with a deadline in the code under test, you're racing the scheduler. Reach for the callback, a channel, or a condvar instead. Fixed deadlines belong only in the safety-net role — "stop if something has truly hung" — not in the assertion path.

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

**Key functions** in `tests/common/pty.rs`:
- `build_pty_command()` — builds a `CommandBuilder` with standard PTY isolation
- `exec_cmd_in_pty()` — executes in a PTY, writing all input immediately (non-interactive)
- `exec_cmd_in_pty_prompted()` — executes in a PTY, waiting for prompts before sending input

These use `portable_pty` to execute commands in a pseudo-terminal, returning
combined stdout+stderr as a single `String` with ANSI color codes and proper
temporal ordering.

**Pattern to use:**

```rust
use crate::common::pty::{build_pty_command, exec_cmd_in_pty};

let cmd = build_pty_command("wt", &["merge"], &repo_path, &env_vars, None);
let (combined_output, exit_code) = exec_cmd_in_pty(cmd, "");
assert_snapshot!("readme_example_name", combined_output);
```

**Benefits:**
- Output is directly copyable to README.md
- Shows actual user experience (interleaved stdout/stderr)
- Preserves temporal ordering of output
- No manual merging of stdout/stderr needed

**Example:** See `tests/integration_tests/shell_wrapper.rs`:
- `ShellOutput` struct with `combined: String`
- `exec_in_pty_interactive()` — shell-wrapper-specific PTY helper

### When to Use Each Approach

**Use `insta_cmd` (standard snapshots):**
- Unit and integration tests focused on correctness
- Tests that need to verify stdout/stderr separately
- Tests checking exit codes and specific error messages
- Most tests in the codebase

**Use PTY-based execution (PTY-based snapshots):**
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
- `test_readme_example_hooks_pre_create()` - Pre-create hooks
- `test_readme_example_hooks_pre_merge()` - Pre-merge hooks

**Current workflow:** These tests work correctly and generate accurate snapshots. However, the snapshots separate stdout and stderr into different sections, which means they cannot be directly copied into README.md. Instead, the README examples are manually edited versions that merge stdout/stderr in the correct temporal order and remove ANSI codes.

**Future improvement:** Migrate README example tests to use PTY execution so snapshots are directly copyable into README.md without manual editing. This is an enhancement for developer convenience, not a bug fix.

### Migration Checklist

When converting a README example test from `insta_cmd` to PTY-based:

1. ✅ Import `portable_pty` dependencies
2. ✅ Use `build_pty_command()` + `exec_cmd_in_pty()` from `tests/common/pty.rs`
3. ✅ Replace `make_snapshot_cmd()` + `assert_cmd_snapshot!()` with PTY execution + `assert_snapshot!()`
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

## No Global State Mutations in Tests

**Never mutate process-global state in tests.** Rust's test runner executes tests in parallel within the same process, so global mutations leak across tests and cause non-deterministic behavior.

Forbidden patterns:
- `log::set_max_level()` — affects all concurrent and subsequent tests
- `std::env::set_var()` — process-wide, races with other tests
- Setting global `static` variables without synchronization

If coverage tools flag uncovered `log::debug!()` format args, accept the gap — it's not meaningful coverage and not worth global side effects.

```rust
// ❌ BAD: Global mutation leaks across parallel tests
#[test]
fn test_something() {
    log::set_max_level(log::LevelFilter::Debug);
    // ...
}

// ❌ BAD: Environment variable race condition
#[test]
fn test_config_loading() {
    std::env::set_var("MY_CONFIG", "test_value");
    // ...
}
```

For environment-dependent tests, use `Command::new()` with `.env()` to set variables in a subprocess, or use the test isolation helpers (`repo.wt_command()`, `wt_command()`).

## Snapshot Filters

### Bold codes around redacted paths

Source code may wrap a path in `<bold>` for terminal styling (e.g., `cformat!("{label} @ <bold>{path}</> failed")`). Setup-side path filters in `tests/common/mod.rs` substitute the path to a placeholder like `[TEST_CONFIG]` or `[PROJECT_ID]`, and a follow-up filter strips ANSI codes immediately wrapping those placeholders so the snapshot reads as a clean `[PLACEHOLDER]`.

The strip filter only fires on placeholders established **before** it. It runs at the end of `setup_snapshot_settings*`, so any path-redaction filter the test adds *after* setup escapes it.

If a test introduces its own placeholder for a path (e.g., `_REPO_/system-config.toml` → `[TEST_SYSTEM_CONFIG_FILE]`), use `add_path_placeholder_filter` so the filter consumes any styling wrappers around the path:

```rust
// ✅ GOOD: helper wraps the pattern with optional ANSI consumption
common::add_path_placeholder_filter(
    &mut settings,
    r"_REPO_/system-config\.toml",
    "[TEST_SYSTEM_CONFIG_FILE]",
);

// ❌ BAD: bare add_filter substitutes only the path, so a `<bold>{path}</>`
// source leaves `\x1b[1m[TEST_SYSTEM_CONFIG_FILE]\x1b[22m` in the snapshot.
settings.add_filter(r"_REPO_/system-config\.toml", "[TEST_SYSTEM_CONFIG_FILE]");
```

The helper wraps the pattern in `(?:\x1b\[\d+m)*` brackets, which eat only the bold open/close immediately adjacent to the path — surrounding color spans (yellow warning, etc.) are preserved.

Setup-side path-redaction placeholders in the strip list (`add_placeholder_ansi_strip_filter` in `tests/common/mod.rs`): `[TEST_CONFIG]`, `[TEST_CONFIG_NEW]`, `[TEST_APPROVALS]`, `[TEST_GIT_CONFIG]`, `[PROJECT_ID]`, `[TEMP_HOME]`, `[TEMP]`. Placeholders that hold a real value (`[VERSION]`, `[HASH]`, `[BUILD_MODE]`, `[BINARY_PATH]`) keep their bold codes so the snapshot still asserts the user-visible styling. The strip pass is invoked at the end of every `setup_*_snapshot_settings` helper, so the contract holds uniformly across `setup_snapshot_settings*`, `setup_home_snapshot_settings`, and `setup_temp_snapshot_settings`.

## Test Style

### Snapshot env drift: cosmetic vs. a leak

`insta_cmd` snapshots record the test's environment variables in an `env:`
block. New or reordered env lines split into two cases — check the *value*
before dismissing:

- **Cosmetic (accept silently):** value is identical on every machine — a
  deterministic literal (`"0"`, `C`) or an already-redacted placeholder
  (`[TEST_HOME]`).
- **A leak (must fix):** value is host/platform/run-specific — a temp path
  (`/var/folders/…`, `/tmp/…`), `$HOME`/`$USER`, a PID, a timestamp. It will
  diff spuriously when the snapshot is regenerated elsewhere. Redact it with
  `add_redaction(".env.VAR_NAME", "[VAR_NAME]")` in
  `add_standard_env_redactions` (bound by the `repo` rstest fixture). Note
  `add_filter` does **not** work on the `env:` block — it only substitutes on
  captured snapshot content; use a redaction.

Empty-valued entries never appear: a dynamic `.env` redaction in
`add_standard_env_redactions` (`drop_empty_env_entries`) drops them. insta-cmd
records each `Command::env_remove` as `KEY: ""` (it serializes `get_envs()`,
which includes removals), so a removal is indistinguishable from a deliberate
set-to-empty — and `isolate_subprocess_env` scrubs whichever `GIT_*` /
`WORKTRUNK_*` keys exist in the *parent* environment, so which markers would
appear depends on the host (CI has `GIT_EDITOR`; a contributor's box might
have `GIT_PAGER`, neither, or both). Dropping the whole class means
regenerating on any machine produces the same block — you don't have to match
CI's `GIT_*` environment.

The predicate keys on the empty *value*, so a test that affirmatively sets a
var to `""` as its subject (`test_list_config_env_override_validation_failure`
sets `WORKTRUNK_WORKTREE_PATH=""` to trigger the validation warning) loses
that header line too — harmless, since the test body still asserts the
behavior and insta never compares the `env:` block.

The `args:` block has the same property: a repo path passed as a CLI argument
(`wt -C <root>`) is covered by the `.args[]` redaction in
`add_repo_and_worktree_path_filters`, which rewrites it to `_REPO_…` like the
body filters; any other run-specific argument needs its own redaction.

Path leaks fail `test_no_host_specific_paths_in_snapshots`
(`snapshot_formatting_guard.rs`), which scans every committed `.snap` for
host-specific path markers (other run-specific values — PIDs, timestamps —
still need review-time vigilance). The test exists because insta never *compares* the
`info:` block — a missing redaction passes on the machine that generated the
snapshot and only churns when regenerated elsewhere.

A runner caveat: the `repo` fixture leaks its settings binding (rstest has no
teardown), so under libtest's reused threads a test that binds no settings of
its own — or clones them via `Settings::clone_current()` without
`add_standard_env_redactions` — can still appear redacted. nextest (process
per test) is authoritative — regenerate snapshots with
`cargo insta test --test-runner nextest` when in doubt.

### Inline snapshots over multi-assert

When a test checks formatted output, use `insta::assert_snapshot!` with an
inline snapshot instead of multiple `assert!(x.contains(...))` calls. Snapshots
capture the complete output, so a single snapshot replaces many contains checks
and catches regressions that spot-checks miss.

```rust
use insta::assert_snapshot;

// ✅ GOOD: One snapshot captures all formatting
assert_snapshot!(format_message("hello"), @"  │ hello");

// ❌ BAD: Spot-checks that miss structural regressions
assert!(result.contains("│"));
assert!(result.contains("hello"));
assert!(!result.contains("error"));
```

Import `assert_snapshot` directly (`use insta::assert_snapshot;`) rather than
using the qualified `insta::assert_snapshot!` form.

For first-time snapshot creation, leave the inline value empty (`@""`), then
run `cargo insta test --accept` to fill it.

To update existing file-based snapshots (e.g., after editing CLI help text),
use `cargo insta test --accept`:

```bash
cargo insta test --accept -- --test integration "test_help"
```

Do not manually edit `.snap` files — they contain ANSI escape sequences that
are difficult to reproduce by hand.

### One test per belief

Group related inputs into a single test when they verify the same belief about
the code. A test named `test_wrap_text_at_width` that exercises short text, long
text, single words, and edge cases is better than five separate test functions
testing each input individually.

```rust
// ✅ GOOD: One test for the belief "wrapping respects word boundaries"
#[test]
fn test_wrap_text_at_width() {
    assert_eq!(wrap_text_at_width("short text", 20), vec!["short text"]);
    assert_eq!(wrap_text_at_width("hello world foo bar", 10), vec!["hello", "world foo", "bar"]);
    assert_eq!(wrap_text_at_width("superlongword", 5), vec!["superlongword"]);
    assert_eq!(wrap_text_at_width("", 20), vec![""]);
}
```

Table-driven tests work well for functions that map inputs to expected outputs:

```rust
#[test]
fn test_bash_token_styles() {
    let cases = [
        ("function", AnsiColor::Blue),
        ("keyword", AnsiColor::Magenta),
        ("string", AnsiColor::Green),
    ];
    for (name, expected) in cases {
        let style = bash_token_style(name).expect(name);
        assert_eq!(style.get_fg_color(), Some(Color::Ansi(expected)), "{name}");
    }
}
```

### Don't test constructors or dependencies

Tests that verify `Style::new().bold()` produces a bold style, or that
`StyledString::raw("x")` stores `"x"`, are testing the dependency — not our
code. Delete these. Test the behavior that uses these types instead.

## Deterministic Time in Tests

Tests use `TEST_EPOCH` (2025-01-01) for reproducible timestamps. The constant is defined in `src/testing/mod.rs`, re-exported via `tests/common/mod.rs`, and automatically set as `WORKTRUNK_TEST_EPOCH` in the test environment.

**For test data with timestamps** (cache entries, etc.), use the constant:

```rust
use crate::common::TEST_EPOCH;

repo.run_git(&[
    "config", "worktrunk.state.feature.ci-status",
    &format!(r#"{{"checked_at":{TEST_EPOCH},"head":"abc123"}}"#),
]);
```

**For production code** that needs timestamps, use `worktrunk::utils::epoch_now()` which respects `WORKTRUNK_TEST_EPOCH`. Using `SystemTime::now()` directly causes flaky tests.
