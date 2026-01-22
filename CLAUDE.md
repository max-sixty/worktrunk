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

- **main worktree** — the original git directory (from clone/init); bare repos have none
- **linked worktree** — worktree created via `git worktree add` (git's official term)
- **primary worktree** — the "home" worktree: main worktree for normal repos, default branch worktree for bare repos
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

### How Coverage Works with Integration Tests

Coverage is collected via `cargo llvm-cov` which instruments the binary. **Subprocess execution IS captured** — when tests spawn `wt` via `assert_cmd_snapshot!`, the instrumented binary writes coverage data to profile files that get merged into the report.

When investigating uncovered lines:

1. Run `task coverage` first to see actual coverage % (~92% is normal)
2. Use `cargo llvm-cov report --show-missing-lines | grep <file>` to find specific uncovered lines
3. **Check if tests already exist** for that functionality before writing new ones
4. Remaining uncovered lines are typically:
   - Error handling paths requiring mocked git failures
   - Edge cases in shell integration states (e.g., running as `git wt`)
   - Test assertion code (only executes when tests fail)

Code that only runs on test failure (assertion messages, custom panic handlers) shows as uncovered since tests pass. Keep this code minimal — useful for debugging but a rarely-traveled path.

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

## Repository Caching Strategy

Most data is stable for the duration of a command. The only things worktrunk modifies are:

- **Worktree list** — `wt switch --create`, `wt remove` create/remove worktrees
- **Working tree state** — `wt merge` commits, stages files
- **Git config** — `wt config` modifies settings

Everything else (remote URLs, project config, branch metadata) is read-only.

### Caching Implementation

`Repository` holds its cache directly via `Arc<RepoCache>`. Cloning a Repository shares the cache — all clones see the same cached values.

**Key patterns:**

- **Command entry points** create Repository via `Repository::current()` or `Repository::at(path)`
- **Parallel tasks** (e.g., `wt list`) clone the Repository, sharing the cache
- **Tests** naturally get isolation since each test creates its own Repository

**Currently cached:**

- `git_common_dir` — computed at construction, stored on struct
- `worktree_root()` — per-worktree, keyed by path
- `repo_path()` — derived from git_common_dir and is_bare
- `is_bare()` — git config, doesn't change
- `current_branch()` — per-worktree, keyed by path
- `project_identifier()` — derived from remote URL
- `primary_remote()` — git config, doesn't change
- `primary_remote_url()` — derived from primary_remote, doesn't change
- `default_branch()` — from git config or detection, doesn't change
- `integration_target()` — effective target for integration checks (local default or upstream if ahead)
- `merge_base()` — keyed by (commit1, commit2) pair
- `ahead_behind` — keyed by (base_ref, branch_name), populated by `batch_ahead_behind()`
- `project_config` — loaded from .config/wt.toml

**Not cached (intentionally):**

- `is_dirty()` — changes as we stage/commit
- `list_worktrees()` — changes as we create/remove worktrees

### Adding New Cached Methods

1. Add field to `RepoCache` struct: `field_name: OnceCell<T>`
2. Access via `self.cache.field_name`
3. Return owned values (String, PathBuf, bool)

```rust
// For repo-wide values (same for all clones)
pub fn cached_value(&self) -> anyhow::Result<String> {
    self.cache
        .field_name
        .get_or_init(|| { /* compute value */ })
        .clone()
}

// For per-worktree values (different per worktree path)
// Use DashMap for concurrent access
pub fn cached_per_worktree(&self, path: &Path) -> String {
    self.cache
        .field_name
        .entry(path.to_path_buf())
        .or_insert_with(|| { /* compute value */ })
        .clone()
}
```
