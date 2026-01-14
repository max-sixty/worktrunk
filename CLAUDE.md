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

## Terminology

Use consistent terminology in documentation, help text, and code comments:

- **main worktree** — the primary worktree (the original git directory), not "main branch worktree"
- **default branch** — the branch (main, master, etc.), not "main branch"
- **target** — the destination for merge/rebase/push (e.g., "merge target"). Don't use "target" to mean worktrees — say "worktree" or "worktrees"

## Testing

### Running Tests

```bash
# Run all tests + lints (recommended before committing)
cargo run -- hook pre-merge --yes
```

**For faster iteration:**

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

### Claude Code Web Environment

When working in Claude Code web, install the task runner and run setup:

```bash
# Install task (go-task) - https://taskfile.dev
sh -c "$(curl --location https://taskfile.dev/install.sh)" -- -d -b ~/bin
export PATH="$HOME/bin:$PATH"

# Run setup
task setup-web
```

This installs required shells (zsh, fish) for shell integration tests and builds the project. Also installs `gh` and other dev tools—run this if any command is not found. The permission tests (`test_permission_error_prevents_save`, `test_approval_prompt_permission_error`) automatically skip when running as root, which is common in containerized environments.

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

## Documentation

**Behavior changes require documentation updates.**

When changing:
- Detection logic
- CLI flags or their defaults
- Error conditions or messages

Ask: "Does `--help` still describe what the code does?" If not, update `src/cli/mod.rs` first.

### Auto-generated docs

Documentation has three categories:

1. **Command pages** (config, hook, list, merge, remove, select, step, switch):
   ```
   src/cli/mod.rs (PRIMARY SOURCE)
       ↓ test_command_pages_and_skill_files_are_in_sync
   docs/content/{command}.md → .claude-plugin/skills/worktrunk/reference/{command}.md
   ```
   Edit `src/cli/mod.rs` (`after_long_help` attributes), never the docs directly.

2. **Non-command docs** (claude-code, faq, llm-commits, tips-patterns, worktrunk):
   ```
   docs/content/*.md (PRIMARY SOURCE)
       ↓ test_command_pages_and_skill_files_are_in_sync
   .claude-plugin/skills/worktrunk/reference/*.md
   ```
   Edit the docs file directly. Skill reference is auto-synced.

3. **Skill-only files** (shell-integration.md, troubleshooting.md):
   Edit `.claude-plugin/skills/worktrunk/reference/` directly — no docs equivalent.

After any doc changes, run tests to sync:

```bash
cargo test --test integration test_command_pages_and_skill_files_are_in_sync
```

## Data Safety

Never risk data loss without explicit user consent. A failed command that preserves data is better than a "successful" command that silently destroys work.

- **Prefer failure over silent data loss** — If an operation might destroy untracked files, uncommitted changes, or user data, fail with an error
- **Explicit consent for destructive operations** — Operations that force-remove data (like `--force` on remove) require the user to explicitly request that behavior
- **Time-of-check vs time-of-use** — Be conservative when there's a gap between checking safety and performing an operation. Example: `wt merge` verifies the worktree is clean before rebasing, but files could be added before cleanup — don't force-remove during cleanup

## Command Execution Principles

### All Commands Through `shell_exec::Cmd`

All external commands go through `shell_exec::Cmd` for consistent logging and tracing:

```rust
use crate::shell_exec::Cmd;

let output = Cmd::new("git")
    .args(["status", "--porcelain"])
    .current_dir(&worktree_path)
    .context("worktree-name")  // for git commands
    .run()?;

let output = Cmd::new("gh")
    .args(["pr", "list"])
    .run()?;  // no context for standalone tools
```

Never use `cmd.output()` directly. `Cmd` provides debug logging (`$ git status [worktree-name]`) and timing traces (`[wt-trace] cmd="..." dur=12.3ms ok=true`).

For git commands, prefer `Repository::run_command()` which wraps `Cmd` with worktree context.

For commands that need stdin piping:
```rust
let output = Cmd::new("git")
    .args(["diff-tree", "--stdin", "--numstat"])
    .stdin_bytes(hashes.join("\n"))
    .run()?;
```

### Real-time Output Streaming

Stream command output in real-time — never buffer:

```rust
// ✅ GOOD - streaming
for line in reader.lines() {
    println!("{}", line);
    stdout().flush();
}
// ❌ BAD - buffering
let lines: Vec<_> = reader.lines().collect();
```

### Structured Output Over Error Message Parsing

Prefer structured output (exit codes, `--porcelain`, `--json`) over parsing human-readable messages. Error messages break on locale changes, version updates, and minor rewording.

```rust
// GOOD - exit codes encode meaning
// git merge-base: 0 = found, 1 = no common ancestor, 128 = invalid ref
if output.status.success() {
    Some(parse_sha(&output.stdout))
} else if output.status.code() == Some(1) {
    None
} else {
    bail!("git merge-base failed: {}", stderr)
}

// BAD - parsing error messages (breaks on wording changes)
if msg.contains("no merge base") { return Ok(true); }
```

**Structured alternatives:**

| Tool | Fragile | Structured |
|------|---------|------------|
| `git diff` | `--shortstat` (localized) | `--numstat` |
| `git status` | default | `--porcelain=v2` |
| `git merge-base` | error messages | exit codes |
| `gh` / `glab` | default | `--json` |

When no structured alternative exists, document the fragility inline.

## Background Operation Logs

All background logs are centralized in `.git/wt-logs/` (main worktree's git directory):

- **Post-start commands**: `{branch}-{source}-post-start-{command}.log` (source: `user` or `project`)
- **Background removal**: `{branch}-remove.log`

Examples: `feature-user-post-start-npm.log`, `feature-project-post-start-build.log`, `bugfix-remove.log`

### Log Behavior

- **Centralized**: All logs go to main worktree's `.git/wt-logs/`, shared across all worktrees
- **Overwrites**: Same operation on same branch overwrites previous log (prevents accumulation)
- **Not tracked**: Logs are in `.git/` directory, which git doesn't track
- **Manual cleanup**: Stale logs from deleted branches persist but are bounded by branch count

## Coverage

The `codecov/patch` CI check enforces coverage on changed lines — respond to failures by writing tests, not by ignoring them. If code is unused, remove it. This includes specialized error handlers for rare cases when falling through to a more general handler is sufficient.

### Running Coverage Locally

```bash
task coverage   # includes --features shell-integration-tests
# Report: target/llvm-cov/html/index.html
```

Install once: `cargo install cargo-llvm-cov`

### Investigating codecov/patch Failures

When CI shows a codecov/patch failure, investigate before declaring "ready to merge" — even if the check is marked "not required":

1. Identify uncovered lines in your changes:
   ```bash
   task coverage                                          # run tests, generate coverage
   cargo llvm-cov report --show-missing-lines | grep <file>   # query the report
   git diff main...HEAD -- path/to/file.rs
   ```

2. For each uncovered function/method you added, either:
   - Write a test that exercises it, or
   - Document why it's intentionally untested (e.g., error paths requiring external system mocks)

## Benchmarks

See `benches/CLAUDE.md` for details.

```bash
# Fast synthetic benchmarks (skip slow ones)
cargo bench --bench list -- --skip cold --skip real

# Specific benchmark
cargo bench --bench list bench_list_by_worktree_count
```

Real repo benchmarks clone rust-lang/rust (~2-5 min first run, cached thereafter). Skip with `--skip real`.

## JSON Output Format

Use `wt list --format=json` for structured data access. See `wt list --help` for complete field documentation, status variants, and query examples.

## Worktree Model

- Worktrees are **addressed by branch name**, not by filesystem path.
- Each worktree should map to **exactly one branch**.
- We **never retarget an existing worktree** to a different branch; instead create/switch/remove worktrees.

## Code Quality

Don't suppress warnings with `#[allow(dead_code)]` — either delete the code or add a TODO explaining when it will be used:

```rust
// TODO(config-validation): Used by upcoming config validation
fn validate_config() { ... }
```

### No Test Code in Library Code

Never use `#[cfg(test)]` to add test-only convenience methods to library code. Tests should call the real API directly. If tests need helpers, define them in the test module.
