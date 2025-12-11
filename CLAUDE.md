# Worktrunk Development Guidelines

> **Note**: This CLAUDE.md is just getting started. More guidelines will be added as patterns emerge.

## Project Status

**This project was released very recently and has very few backward compatibility concerns.**

We are in **early release** mode:
- Breaking changes are generally acceptable
- Optimize for the best solution, not compatibility with previous versions
- No Rust library compatibility concerns (this is a CLI tool only)

**CLI deprecation policy:** When renaming or removing CLI options, retain the old option with a deprecation warning if there's no code complication cost. If supporting both old and new would add complexity, just make the breaking change.

When making decisions, prioritize:
1. **Best technical solution** over backward compatibility
2. **Clean design** over maintaining old patterns
3. **Modern conventions** over legacy approaches

Acceptable breaking changes: config locations, output formats, dependencies, codebase structure. CLI flag changes can be breaking if deprecation warnings would complicate the code.

## Code Quality

Claude commonly makes the mistake of adding `#[allow(dead_code)]` when writing code that isn't immediately used. Don't suppress the warning—either delete the code or add a TODO comment explaining when it will be used.

Example of escalating instead of suppressing:

```rust
// TODO(feature-name): Used by upcoming config validation
fn parse_config() { ... }
```

## Terminology

Use consistent terminology in documentation, help text, and code comments:

- **main worktree** — when referring to the primary worktree (the original git directory)
- **default branch** — when referring to the branch (main, master, etc.)

Avoid mixing: "main/default branch worktree" is confusing. Use "main worktree" for worktrees and "default branch" for branches.

## Testing

### Running Tests

```bash
# Unit tests (fast)
cargo test --lib --bins

# Integration tests without shell tests (no external dependencies)
cargo test --test integration

# Integration tests WITH shell tests (requires bash/zsh/fish)
cargo test --test integration --features shell-integration-tests

# Run all tests via pre-merge hook (recommended before committing)
cargo run -- hook pre-merge --force
```

The pre-merge hook runs the full test suite and is the recommended way to verify changes before committing.

**Shell integration tests** require bash, zsh, and fish. On Linux, run `./dev/setup-claude-code-web.sh` to install them.

### Claude Code Web Environment

When working in Claude Code web, run the setup script first:

```bash
./dev/setup-claude-code-web.sh
```

This installs required shells (zsh, fish) and builds the project. The permission tests (`test_permission_error_prevents_save`, `test_approval_prompt_permission_error`) automatically skip when running as root, which is common in containerized environments.

### CLI Flag Descriptions

Keep the first line of flag and argument descriptions brief—aim for 3-6 words. Use parenthetical defaults sparingly, only when the default isn't obvious from context.

**Good examples:**
- `/// Skip approval prompts`
- `/// Show CI and \`main\` diffstat`
- `/// Target branch (defaults to default branch)`

**Bad examples (too verbose):**
- `/// Auto-approve project commands without saving approvals.`
- `/// Show CI status, conflict detection, and complete diff statistics`

The help text should be scannable. Users reading `wt switch --help` need to quickly understand what each flag does without parsing long sentences.

## Command Execution Principles

### Real-time Output Streaming

Command output must stream in real-time. Never buffer external command output.

```rust
// ✅ GOOD - streaming
for line in reader.lines() {
    println!("{}", line);
    stdout().flush();
}
// ❌ BAD - buffering
let lines: Vec<_> = reader.lines().collect();
```

## Background Operation Logs

### Unified Logging Location

All background operation logs are centralized in `.git/wt-logs/` (main worktree's git directory):

- **Post-start commands**: `{branch}-{source}-post-start-{command}.log` (source is `user` or `project`)
- **Background removal**: `{branch}-remove.log`

Examples (where command names are from config):
- `feature-user-post-start-npm.log`
- `feature-project-post-start-build.log`
- `bugfix-remove.log`

### Log Behavior

- **Centralized**: All logs go to main worktree's `.git/wt-logs/`, shared across all worktrees
- **Overwrites**: Same operation on same branch overwrites previous log (prevents accumulation)
- **Not tracked**: Logs are in `.git/` directory, which git doesn't track
- **Manual cleanup**: Stale logs (from deleted branches) persist but are bounded by branch count

Users can clean up old logs manually or use a git hook. No automatic cleanup is provided.

## Testing Guidelines

### Timing Tests: Long Timeouts with Fast Polling

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

### Testing with --execute Commands

Use `--force` to skip interactive prompts in tests. Don't pipe input to stdin.

## Coverage

- Install once: `cargo install cargo-llvm-cov`.
- Run: `./dev/coverage.sh` — runs tests once, then generates HTML (`target/llvm-cov/html/index.html`) and LCOV (`target/llvm-cov/lcov.info`). Set `COVERAGE_OPEN=0` to skip opening the HTML report.
- Pass extra args to the test run (for example to filter tests): `./dev/coverage.sh -- --test test_name`.

## Benchmarks

For detailed benchmark documentation, see `benches/CLAUDE.md`.

### Quick Start

```bash
# Run fast synthetic benchmarks (skip slow ones)
cargo bench --bench list -- --skip cold --skip real

# Run specific benchmark
cargo bench --bench list bench_list_by_worktree_count
cargo bench --bench completion
```

Real repo benchmarks clone rust-lang/rust (~2-5 min first run, cached thereafter). Skip during normal development with `--skip real`.

## JSON Output Format

Use `wt list --format=json` for structured data access. See `wt list --help` for complete field documentation, status variants, and query examples.

## Worktree Model

- Worktrees are **addressed by branch name**, not by filesystem path.
- Each worktree should map to **exactly one branch**.
- We **never retarget an existing worktree** to a different branch; instead create/switch/remove worktrees.
