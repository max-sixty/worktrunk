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
- **MSRV policy: latest stable − 1** — bumped during weekly tend maintenance (see `running-tend` skill)

**External interfaces to protect:**
- **Config file format** (`wt.toml`, user config) — avoid breaking changes; provide migration guidance when necessary
- **CLI flags and arguments** — use deprecation warnings

Everything else (internal APIs, output formatting, log locations) is flexible. Prefer the best technical solution; use deprecation warnings when external interfaces must change.

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

See `tests/CLAUDE.md` for test infrastructure, assertion style, and test granularity guidelines.

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

PTY-based tests (approval prompts, TUI picker, progressive rendering, shell wrappers) are behind the `shell-integration-tests` feature.

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
- Config fields or sections — also update `dev/*.example.toml` (these are embedded in CLI help via `include_str!`, so stale examples propagate to docs and snapshots)

Ask: "Does `--help` still describe what the code does?" If not, update `src/cli/mod.rs` first.

### Auto-generated docs

Documentation has three categories:

1. **Command pages** (config, hook, list, merge, remove, step, switch):
   ```
   dev/*.example.toml (included via include_str!)
       ↓
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

### Help text authoring

Help text renders in three contexts — check all three when editing:

1. **Terminal** (`wt step X --help`): `about` and `subtitle` appear at the top, `after_long_help` appears below the Options block — separated by distance.
2. **Web docs** (`docs/content/`): `combine_command_docs()` concatenates `about` + optional `subtitle` + `after_long_help` — they appear as consecutive paragraphs.
3. **Skill reference** (`skills/worktrunk/reference/`): mirrors web docs.

Because web docs concatenate everything, the `after_long_help` opener must not restate the `about`/`subtitle`. Start with new information — examples, context, or details not already in the definition. See `docs/CLAUDE.md` → "Command documentation structure" for detailed content principles and good/bad opener patterns.

After any doc changes, run tests to sync:

```bash
cargo test --test integration test_command_pages_and_skill_files_are_in_sync
```

After editing `after_long_help` text, also update the help snapshots:

```bash
cargo insta test --accept -- --test integration "test_help"
```

### Config doc TOML blocks

Config docs (`USER_CONFIG_START`/`PROJECT_CONFIG_START` sections in `src/cli/mod.rs`) generate `dev/*.example.toml` files where every line is commented out with `#`. TOML comments inside code blocks become double-commented (`# # comment`). Use plain text descriptions ending with colons before each code block instead — inline end-of-line comments (e.g., `key = "value"  # explanation`) are fine.

## Data Safety

Never risk data loss without explicit user consent. A failed command that preserves data is better than a "successful" command that silently destroys work.

- **Prefer failure over silent data loss** — If an operation might destroy untracked files, uncommitted changes, or user data, fail with an error
- **Explicit consent for destructive operations** — Operations that force-remove data (like `--force` on remove) require the user to explicitly request that behavior
- **No implicit destructive side effects** — A command must not silently delete, remove, or overwrite files/directories as a side effect of an unrelated operation. If cleanup is needed, make it a separate explicit action the user chooses to take
- **Favor retaining data and failing on race conditions** — When there's a gap between checking safety and performing an operation, choose the variant that fails rather than silently discards work. Example: use `git reset --keep` (fails if tracked files were modified) over `git reset --hard` (silently overwrites). Similarly, prefer `git checkout --merge` over `git checkout --force`. If a safer variant doesn't exist, document the risk inline
- **Time-of-check vs time-of-use** — Be conservative when there's a gap between checking safety and performing an operation. Example: `wt merge` verifies the worktree is clean before rebasing, but files could be added before cleanup — don't force-remove during cleanup

For the full inventory of what Worktrunk creates and deletes, see the FAQ: [What files does Worktrunk create?](docs/content/faq.md#what-files-does-worktrunk-create) and [What can Worktrunk delete?](docs/content/faq.md#what-can-worktrunk-delete). New code that changes this surface area should be reviewed against these sections.

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

Responsiveness is a priority — stream command output line-by-line rather than buffering.

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
| `git diff` | `--stat` (localized) | `--numstat`, `--shortstat` (markers `(+)`/`(-)` are hardcoded) |
| `git status` | default | `--porcelain=v2` |
| `git merge-base` | error messages | exit codes |
| `gh` / `glab` | default | `--json` |

When no structured alternative exists, document the fragility inline.

## Background Operation Logs

All background logs are centralized in `.git/wt/logs/` (main worktree's git directory). Same operation on same branch overwrites the previous log.

- **Post-start commands**: `{branch}-{source}-post-start-{command}.log` (source: `user` or `project`)
- **Background removal**: `{branch}-remove.log`

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

**Renames and moves:** File renames (`git mv`) can trigger codecov/patch failures on pre-existing uncovered lines — codecov treats changed lines in renamed files as part of the patch. If the uncovered lines are unchanged and existed before the rename, this is a false positive. Verify by checking coverage on `main` for the same lines under the old path.

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
- We **never retarget an existing worktree** to a different branch; instead create/switch/remove worktrees. (The sole exception is `wt step promote`, which exchanges branches between two worktrees as an experimental escape hatch.)

## Code Quality

### Use Existing Dependencies

Never hand-roll utilities that already exist as crate dependencies. Check `Cargo.toml` before implementing:

| Need | Use | Not |
|------|-----|-----|
| Path normalization | `path_slash::PathExt::to_slash_lossy()` | `.to_string_lossy().replace('\\', "/")` |
| Shell escaping | `shell_escape::unix::escape()` | Manual quoting |
| ANSI colors | `color_print::cformat!()` | Raw escape codes |
| Template variable detection | `minijinja::undeclared_variables(false)` | Regex or substring matching for `{{ var }}` |

### Don't Suppress Warnings

Don't suppress warnings with `#[allow(dead_code)]` — either delete the code or add a TODO explaining when it will be used:

```rust
// TODO(config-validation): Used by upcoming config validation
fn validate_config() { ... }
```

### System Docstrings

Complex systems (multi-step workflows, state machines, coordination logic) should have a module-level docstring that serves as a spec — purpose, key decisions, behavioral contracts, and invariants. Keep the docstring current as the module evolves.

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
- **Never `.expect()` or `.unwrap()` in functions returning `Result`** — use `?`, `bail!`, or return an error

## Config Deprecation

All config deprecation is handled by a single layer: pre-deserialization TOML migration in `src/config/deprecation.rs`.

### How it works

`migrate_content()` is the structural TOML migration entry point before serde parses config, rewriting deprecated patterns into their canonical form. Load paths that only need migration call it directly; `check_and_migrate()` reuses the same migration path and returns the migrated TOML so callers don't reparse the file just to load it.

Separately, `check_and_migrate()` detects deprecated patterns, emits warnings, and generates a `.new` migration file (which additionally renames deprecated template variables and removes `approved-commands`). Warnings are deduplicated per-process via `WARNED_DEPRECATED_PATHS`.

### Adding a new deprecation

1. Add a detection function and extend the `Deprecations` struct + `detect_deprecations()`
2. Add a migration function (idempotent: no-op if pattern is absent or already migrated)
3. Call the migration function from `migrate_content()`
4. Add a warning message in `format_deprecation_warnings()`
5. Update `Deprecations::is_empty()` to include the new field
6. If it's a top-level section being removed from the struct, add a `DeprecatedSection` entry to `DEPRECATED_SECTION_KEYS` with the canonical top-level key and display form. This tells `warn_unknown_fields` to skip the key when it appears in the correct config type (the deprecation system provides better messaging) and to suggest the correct config when it appears in the wrong file.

### Renaming a field within a section

Recipe for renaming `old-name` → `new-name` inside an existing section (e.g., `[merge]`):

1. **Add a TOML-level migration function** (see `migrate_negated_bool_in_section` for the pattern). The function should rename the field in the TOML document, applying any transformation (e.g., boolean inversion).

2. **Call it from `migrate_content()`** so all load paths get the fix.

3. **Add detection** in `detect_deprecations()` and a warning in `format_deprecation_warnings()`.

The struct does not need the old field — migration happens before serde sees the TOML. Do not silently drop old config keys — this causes silent behavior changes for users.

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

Don't use `get_*` — bare nouns follow Rust stdlib convention.

## Repository Caching

Most data is stable for the duration of a command. `Repository` caches read-only values (remote URLs, config, branch metadata) via `Arc<RepoCache>` — cloning a Repository shares the cache.

**Not cached (changes during command execution):**
- `is_dirty()` — changes as we stage/commit
- `list_worktrees()` — changes as we create/remove worktrees

When adding new cached methods, see `RepoCache` in `src/git/repository/mod.rs` for patterns (repo-wide via `OnceCell`, per-worktree via `DashMap`).

## Releases

Use the `release` skill for cutting releases. It handles version bumping, changelog generation, crates.io publishing, and GitHub releases.
