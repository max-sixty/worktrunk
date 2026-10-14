# Testing guidelines

## Running the suite

The project gate is cargo run -- hook pre-merge --yes. Use cargo nextest run --all-features for the full suite, cargo test --lib --bins for unit tests, and cargo test --test integration for integration tests. Add --features shell-integration-tests for real-shell and PTY tests. On Claude Code web, run task setup-web first.

Tests run under cargo so CARGO_BIN_EXE_wt names the just-built binary. Spawn wt through wt_bin() or the test command helpers, which pin a hardlink against concurrent Cargo replacement. Never derive a target/debug/wt path.

## One Result Per Test, Whatever Runs It

A test must behave the same under cargo test, nextest, llvm-cov, and Nix. Keep behavior-critical configuration in shared fixtures or .cargo/config.toml, not nextest-only environment settings.

## Profiling the Suite

Run task profile-tests. Compare user and system CPU time when sibling worktrees share the machine; target/nextest/default/junit.xml has per-test durations. Use test_tempdir() for fixture scratch space. A static TempDir leaks because its destructor never runs.

## Coverage Investigation

Run task coverage and inspect target/llvm-cov/html/index.html or cargo llvm-cov report --show-missing-lines. CI and local coverage both enable shell-integration-tests. For a posted codecov/patch discrepancy, query the compare API with full base and head SHAs and inspect only added diff lines. Moved, unchanged lines can count as patch misses; compare them with main before changing code. A compare result spanning files outside the PR may mean the base commit lacks a coverage report. The merge gate is in the root AGENTS.md.

In a Tend sandbox without llvm-cov, use the Codecov API with full SHAs:

```bash
codecov_compare=$(mktemp)
curl -sL 'https://api.codecov.io/api/v2/github/max-sixty/repos/worktrunk/compare/?base=<full-base-sha>&head=<full-head-sha>' > "$codecov_compare"
jq '.files[] | select(.has_diff) | {name: .name.head, patch: .totals.patch}' "$codecov_compare"
jq '.files[].lines[] | select(.is_diff and .added and .coverage.head == 1) | .number.head' "$codecov_compare"
```

The `file_report/<path>?sha=<full-sha>` endpoint shows whole-file coverage; omit a trailing slash from the path.

## Running wt Commands in Tests

Use repo.wt_command() with TestRepo and wt_command() without one. Both isolate host configuration, Git variables, directive files, and network access. Set a needed cwd explicitly; the free helper starts outside a repository. Use repo.git_command() for Git through shell_exec::Cmd.

Test child environment layers live in src/testing/mod.rs: STATIC_TEST_ENV_VARS, git_test_env, PTY_TEST_ENV_VARS, and pty_env_vars. Add a variable at the layer all relevant callers share. Test-only variables use the WORKTRUNK_TEST_ prefix so host values are scrubbed.

## Git Config Isolation

No suite Git process may read the developer's global or system config. shell_exec::HERMETIC_TEST_GIT_ENV and the test command helpers apply the same deny floor, including in-process Git through the harness latch. Never mutate the test process environment. A test that changes fixture-construction Git behavior must rebuild the cached standard fixture or bump STANDARD_FIXTURE_VERSION before trusting local results. The detailed mechanism is in src/testing/mod.rs and src/shell_exec.rs.

## Config Isolation for In-Process Unit Tests

Subprocess helpers do not isolate calls made directly into library code. For in-process approval and user-config tests, start from default state and pass a tempdir-backed path to mutation methods. Do not call global resolvers such as Approvals::load(), approvals_path(), or config_path(): bin-crate tests link the library without its test-only guards and can read the developer's real config. Do not change process-global environment, logging, or shared config files.

## Timing Tests: Polling and Absence Windows

For eventual presence, poll quickly with a generous cap and assert a diagnostic on timeout. For absence, use SLEEP_FOR_ABSENCE_CHECK after proving the event could occur; an immediate negative assertion proves only that it has not happened yet. Drive event-based behavior from its callback where possible. Do not hide a race with a generic retry. A bounded poll for one understood transient error belongs at the shared spawn boundary.

### Testing absence

An absence test must keep observing through the interval in which the unwanted event could occur. Set up and assert the precondition that would make that event possible before starting the window.

## No Retries

Reproduce intermittent failures under representative concurrent load and fix the shared cause. wt_bin() handles Cargo's binary-replacement window. Put named temporary files inside a TempDir; a shared temp namespace can fail differently on Windows.

## Testing with --execute Commands

Use --yes to avoid interactive approval prompts. Do not pipe a synthetic yes through stdin.

## Feature Flags, Not Runtime Skipping

Gate tests requiring installed shells or tools with shell-integration-tests. A runtime availability check that returns early records a passing test without exercising it.

## PTY Tests and README Examples

Use insta_cmd for ordinary command behavior. Use a PTY only for terminal contracts: prompts, shell directives, pager choice, or stdout/stderr interleaving. Keep representative shell-boundary workflows rather than a command-by-shell matrix. PTY commands use configure_pty_command so the hermetic environment and coverage variables survive env_clear().

## No Global State Mutations in Tests

Do not set process environment, logging levels, or unsynchronized statics in parallel tests. Configure a child Command or use a fixture instead.

## Snapshot Filters

For a path placeholder that may have adjacent ANSI styling, use add_path_placeholder_filter in tests/common/mod.rs. It consumes styling around the path while preserving surrounding semantic color. A bare add_filter can leave bold escape codes in the snapshot.

## Test Style

Test a belief at the cheapest boundary that proves it: direct tests for parsing and state transitions; integration for Git, filesystem, and process wiring; PTY for terminal behavior. Assert topology and absence preconditions in the fixture, not only in comments. When route choice matters, assert the recorded call as well as the result. Prefer one test with minimal contrasting inputs for one belief. Do not test a dependency's own constructor.

### Guards that scan source text

Absence guards must read every file they claim to cover and fail on traversal or read errors. Reuse tests/common/source_scan.rs, whose module doc explains visit_files and per-root coverage. An empty or partially skipped walk can make an absence assertion pass falsely.

### Snapshot env drift: cosmetic vs. a leak

An env line with a deterministic value is harmless. Redact host paths, usernames, PIDs, and timestamps with add_standard_env_redactions; add_filter affects body text, not the env block. Path arguments need their own redaction. Use nextest when checking whether thread-local snapshot settings leaked across cargo test cases. The guard in snapshot_formatting_guard.rs scans committed snapshots for host paths.

### Inline snapshots over multi-assert

For complete formatted output, prefer one inline insta snapshot over several contains assertions. Accept snapshots with cargo insta test --accept; do not hand-edit ANSI-heavy .snap files. Help snapshots use cargo insta test --accept --test integration -- test_help.

## Deterministic Time in Tests

Use TEST_EPOCH from src/testing/mod.rs for timestamped fixtures. Production code uses utils::epoch_now(), which respects WORKTRUNK_TEST_EPOCH.
