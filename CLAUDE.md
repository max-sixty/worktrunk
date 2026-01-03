# Worktrunk Development Guidelines

> **Note**: This CLAUDE.md is just getting started. More guidelines will be added as patterns emerge.

## Project Status

**This project has a growing user base. Balance clean design with reasonable compatibility.**

We are in **maturing** mode:
- Breaking changes to external interfaces require justification (significant improvement, not just cleanup)
- Prefer deprecation warnings over silent breaks
- No Rust library compatibility concerns (this is a CLI tool only)

**External interfaces to protect:**
- **Config file format** (`wt.toml`, user config) — avoid breaking changes; provide migration guidance when necessary
- **CLI flags and arguments** — use deprecation warnings; retain old flags for at least one release cycle

**Internal changes remain flexible:**
- Codebase structure, dependencies, internal APIs
- Human-readable output formatting and messages
- Log file locations and formats

When making decisions, prioritize:
1. **Best technical solution** over backward compatibility
2. **Clean design** over maintaining old patterns
3. **Modern conventions** over legacy approaches

Use deprecation warnings to get there smoothly when external interfaces must change.

## Code Quality

Claude commonly makes the mistake of adding `#[allow(dead_code)]` when writing code that isn't immediately used. Don't suppress the warning—either delete the code or add a TODO comment explaining when it will be used.

Example of escalating instead of suppressing:

```rust
// TODO(feature-name): Used by upcoming config validation
fn parse_config() { ... }
```

## Data Safety

Never risk data loss without explicit user consent. Err on the side of failing safely.

- **Prefer failure over silent data loss** — If an operation might destroy untracked files, uncommitted changes, or user data, fail with an error rather than proceeding
- **Explicit consent for destructive operations** — Operations that force-remove data (like `--force` on remove) require the user to explicitly request that behavior
- **Time-of-check vs time-of-use** — If there's a gap between checking safety and performing an operation, be conservative. Example: `wt merge` verifies the worktree is clean before rebasing, but files could be added before cleanup — don't force-remove during cleanup

A failed command that preserves data is always better than a "successful" command that silently destroys work.

## Terminology

Use consistent terminology in documentation, help text, and code comments:

- **main worktree** — when referring to the primary worktree (the original git directory)
- **default branch** — when referring to the branch (main, master, etc.)
- **target** — the destination branch for merge/rebase/push operations (e.g., "merge target", "rebase onto target"). Do NOT use "target" to refer to worktrees being operated on — use "worktree" or "worktrees" instead.

Avoid mixing: "main/default branch worktree" is confusing. Use "main worktree" for worktrees and "default branch" for branches.

## Testing

### Running Tests

```bash
# Run all tests + lints (recommended before committing)
cargo run -- hook pre-merge --yes
```

The pre-merge hook runs lints and the full test suite.

**For faster iteration during development:**

```bash
# Lints only
pre-commit run --all-files

# Unit tests only
cargo test --lib --bins

# Integration tests (no shell tests)
cargo test --test integration

# Integration tests with shell tests (requires bash/zsh/fish)
cargo test --test integration --features shell-integration-tests
```

**Shell integration tests** require bash, zsh, and fish.

### Claude Code Web Environment

When working in Claude Code web, run the setup script first:

```bash
./dev/setup-claude-code-web.sh
```

This installs required shells (zsh, fish) for shell integration tests and builds the project. The permission tests (`test_permission_error_prevents_save`, `test_approval_prompt_permission_error`) automatically skip when running as root, which is common in containerized environments.

### Shell/PTY Integration Tests

PTY-based tests (approval prompts, TUI select, progressive rendering, shell wrappers) are behind the `shell-integration-tests` feature.

**IMPORTANT:** Tests that spawn interactive shells (`zsh -ic`, `bash -ic`) cause nextest's InputHandler to receive SIGTTOU when restoring terminal settings. This suspends the test process mid-run with `zsh: suspended (tty output)` or similar. See [nextest#2878](https://github.com/nextest-rs/nextest/issues/2878) for details.

**Solutions:**

1. Use `cargo test` instead of `cargo nextest run` (no input handler issues):
   ```bash
   cargo test --test integration --features shell-integration-tests
   ```

2. Or set `NEXTEST_NO_INPUT_HANDLER=1`:
   ```bash
   NEXTEST_NO_INPUT_HANDLER=1 cargo nextest run --features shell-integration-tests
   ```

The pre-merge hook (`wt hook pre-merge --yes`) already sets `NEXTEST_NO_INPUT_HANDLER=1` automatically.

## Command Execution Principles

### All Commands Through `shell_exec::run`

All external command execution must go through `shell_exec::run()`. This ensures consistent logging and tracing for every command:

```rust
use crate::shell_exec::run;

let mut cmd = Command::new("git");
cmd.args(["status", "--porcelain"]);
let output = run(&mut cmd, Some("worktree-name"))?;  // context for git commands
let output = run(&mut cmd, None)?;                   // None for standalone tools
```

Never use `cmd.output()` directly. The `run()` function provides:
- Debug logging: `$ git status [worktree-name]`
- Timing traces: `[wt-trace] cmd="..." dur=12.3ms ok=true`

For git commands, use `Repository::run_command()` which wraps `shell_exec::run` with worktree context.

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

## Coverage

We maintain high code coverage. The `codecov/patch` CI check enforces coverage on changed lines — respond to failures by writing tests, not by ignoring them.

### Running Coverage Locally

- Install once: `cargo install cargo-llvm-cov`.
- Run: `./dev/coverage.sh` — runs tests once, then generates HTML (`target/llvm-cov/html/index.html`) and LCOV (`target/llvm-cov/lcov.info`).
- Pass extra args to the test run (for example to filter tests): `./dev/coverage.sh -- --test test_name`.

### Investigating codecov/patch Failures

When `codecov/patch` fails, find which changed lines lack coverage:

```bash
# 1. Run coverage and show uncovered lines
./dev/coverage.sh
cargo llvm-cov report --show-missing-lines | grep <file>

# 2. Compare against your diff (use three-dot diff for PR changes)
git diff main...HEAD -- path/to/file.rs
```

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
