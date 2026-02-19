# Worktrunk Development Guidelines

## Quick Start

```bash
cargo run -- hook pre-merge --yes   # run all tests + lints (do this before committing)
```

For Claude Code web environments, run `task setup-web` first. See [Testing](#testing) for more commands.

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

- **main worktree** — the original git directory (from clone/init); bare repos have none
- **linked worktree** — worktree created via `git worktree add` (git's official term)
- **primary worktree** — the "home" worktree: main worktree for normal repos, default branch worktree for bare repos
- **default branch** — the branch (main, master, etc.), not "main branch"
- **target** — the destination for merge/rebase/push (e.g., "merge target"). Don't use "target" to mean worktrees — say "worktree" or "worktrees"

## Skills

Check `.claude/skills/` for available skills and load those relevant to your task.

Key skills:

- **`writing-user-outputs`** — Required when modifying user-facing messages, hints, warnings, errors, or any terminal output formatting. Documents ANSI color nesting rules, message patterns, and output system architecture.

## Testing

### Running Tests

```bash
# All tests + lints (recommended before committing)
cargo run -- hook pre-merge --yes

# Tests with coverage report → target/llvm-cov/html/index.html
task coverage
```

**For faster iteration:**

```bash
pre-commit run --all-files              # lints only
cargo test --lib --bins                 # unit tests only
cargo test --test integration           # integration tests (no shell tests)
cargo test --test integration --features shell-integration-tests  # with shell tests
```

### Claude Code Web Environment

Run `task setup-web` to install required shells (zsh, fish, nushell), `gh`, and other dev tools. Install `task` first if needed:

```bash
sh -c "$(curl --location https://taskfile.dev/install.sh)" -- -d -b ~/bin
export PATH="$HOME/bin:$PATH"
task setup-web
```

The permission tests (`test_permission_error_prevents_save`, `test_approval_prompt_permission_error`) skip automatically when running as root.

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
   docs/content/{command}.md → skills/worktrunk/reference/{command}.md
   ```
   Edit `src/cli/mod.rs` (`after_long_help` attributes), never the docs directly.

2. **Non-command docs** (claude-code, faq, llm-commits, tips-patterns, worktrunk):
   ```
   docs/content/*.md (PRIMARY SOURCE)
       ↓ test_command_pages_and_skill_files_are_in_sync
   skills/worktrunk/reference/*.md
   ```
   Edit the docs file directly. Skill reference is auto-synced.

3. **Skill-only files** (shell-integration.md, troubleshooting.md):
   Edit `skills/worktrunk/reference/` directly — no docs equivalent.

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

Never use `cmd.output()` directly. `Cmd` provides debug logging (`$ git status [worktree-name]`) and timing traces (`[wt-trace] cmd="..." dur_us=12300 ok=true`).

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

**NEVER merge a PR with failing `codecov/patch` without explicit user approval.** The check is marked "not required" in GitHub but it requires user approval to merge. When codecov fails:

1. Investigate and fix the coverage gap (see below)
2. If you believe the failure is a false positive, ask the user before merging

The `codecov/patch` CI check enforces coverage on changed lines — respond to failures by writing tests, not by ignoring them. If code is unused, remove it. This includes specialized error handlers for rare cases when falling through to a more general handler is sufficient.

### Investigating codecov/patch Failures

When CI shows a codecov/patch failure, investigate before declaring "ready to merge":

```bash
task coverage                                              # run tests, generate coverage
cargo llvm-cov report --show-missing-lines | grep <file>   # find uncovered lines
```

For each uncovered function/method, either write a test or document why it's intentionally untested. Integration tests (via `assert_cmd_snapshot!`) do capture subprocess coverage.

## Benchmarks

Benchmarks measure `wt list` performance across worktree counts and repository sizes.

```bash
cargo bench --bench list -- --skip cold --skip real   # fast synthetic benchmarks
cargo bench --bench list bench_list_by_worktree_count # specific benchmark
```

Real repo benchmarks clone rust-lang/rust (~2-5 min first run, cached thereafter). Skip with `--skip real`. See `benches/CLAUDE.md` for methodology and adding new benchmarks.

## JSON Output Format

Use `wt list --format=json` for structured data access. See `wt list --help` for complete field documentation, status variants, and query examples.

## Worktree Model

- Worktrees are **addressed by branch name**, not by filesystem path.
- Each worktree should map to **exactly one branch**.
- We **never retarget an existing worktree** to a different branch; instead create/switch/remove worktrees.

## Code Quality

### Use Existing Dependencies

Never hand-roll utilities that already exist as crate dependencies. Check `Cargo.toml` before implementing:

| Need | Use | Not |
|------|-----|-----|
| Path normalization | `path_slash::PathExt::to_slash_lossy()` | `.to_string_lossy().replace('\\', "/")` |
| Shell escaping | `shell_escape::unix::escape()` | Manual quoting |
| ANSI colors | `color_print::cformat!()` | Raw escape codes |

### Don't Suppress Warnings

Don't suppress warnings with `#[allow(dead_code)]` — either delete the code or add a TODO explaining when it will be used:

```rust
// TODO(config-validation): Used by upcoming config validation
fn validate_config() { ... }
```

### No Test Code in Library Code

Never use `#[cfg(test)]` to add test-only convenience methods to library code. Tests should call the real API directly. If tests need helpers, define them in the test module.

## Error Handling

Use `anyhow` for error propagation with context:

```rust
use anyhow::{bail, Context, Result};

// Prefer .context() for adding helpful error messages
let data = std::fs::read_to_string(path)
    .context("Failed to read config file")?;

// Use bail! for early returns with formatted errors
if worktree.is_dirty() {
    bail!("worktree has uncommitted changes");
}
```

**Patterns:**

- **Use `bail!`** for business logic errors (dirty worktree, missing branch, invalid state)
- **Use `.context()`** for wrapping I/O and external command failures
- **Never `.expect()` or `.unwrap()` in functions returning `Result`** — use `?`, `bail!`, or return an error. Panics in fallible code bypass error handling.
- **Don't `logger.error` before raising** — include context in the error message itself
- **Let errors propagate** — don't catch and re-raise without adding information

## Adding CLI Commands

CLI commands live in `src/cli/` with implementations in `src/commands/`.

1. **Add subcommand** to `Cli` enum in `src/cli/mod.rs`
2. **Create command module** in `src/commands/` (e.g., `src/commands/mycommand.rs`)
3. **Add `after_long_help`** attribute for extended help that syncs to docs
4. **Run doc sync** after adding help text:
   ```bash
   cargo test --test integration test_command_pages_and_skill_files_are_in_sync
   ```

Help text in `after_long_help` is the source of truth for `docs/content/{command}.md`.

## Accessor Function Naming Conventions

Function prefixes signal return behavior and side effects.

| Prefix | Returns | Side Effects | Error Handling | Example |
|--------|---------|--------------|----------------|---------|
| (bare noun) | `Option<T>` or `T` | None (may cache) | Returns None/default if absent | `config()`, `switch_previous()` |
| `set_*` | `Result<()>` | Writes state | Errors on failure | `set_switch_previous()`, `set_config()` |
| `require_*` | `Result<T>` | None | Errors if absent | `require_branch()`, `require_target_ref()` |
| `fetch_*` | `Result<T>` | Network I/O | Errors on failure | `fetch_pr_info()`, `fetch_mr_info()` |
| `load_*` | `Result<T>` | File I/O | Errors on failure | `load_project_config()`, `load_template()` |

**When to use each:**

- **Bare nouns** — Value may not exist and that's fine (Rust stdlib convention)
- **`set_*`** — Write state to storage
- **`require_*`** — Value must exist for operation to proceed
- **`fetch_*`** — Retrieve from external service (network)
- **`load_*`** — Read from filesystem

**Anti-patterns:**

- Don't use bare nouns if the function makes network calls (use `fetch_*`)
- Don't use bare nouns if absence is an error (use `require_*`)
- Don't use `load_*` for computed values (use bare nouns)
- Don't use `get_*` prefix — use bare nouns instead (Rust convention)

## Repository Caching

Most data is stable for the duration of a command. `Repository` caches read-only values (remote URLs, config, branch metadata) via `Arc<RepoCache>` — cloning a Repository shares the cache.

**Not cached (changes during command execution):**
- `is_dirty()` — changes as we stage/commit
- `list_worktrees()` — changes as we create/remove worktrees

When adding new cached methods, see `RepoCache` in `src/git/repository/mod.rs` for patterns (repo-wide via `OnceCell`, per-worktree via `DashMap`).

## Releases

Use the `release` skill for cutting releases. It handles version bumping, changelog generation, crates.io publishing, and GitHub releases.
