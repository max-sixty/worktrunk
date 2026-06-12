# Worktrunk Development Guidelines

## Quick Start

```bash
cargo run -- hook pre-merge --yes   # all tests + lints; run before committing
```

Claude Code web: run `task setup-web` first. Test commands, isolation, and coverage investigation: `tests/CLAUDE.md`.

## Project Status

Maturing mode: a growing user base, so balance clean design with compatibility.

- External-interface breaks need justification (a real improvement, not cleanup); prefer deprecation warnings over silent breaks.
- **Protected interfaces:** config file format (`wt.toml`, user config) and CLI flags/arguments. Everything else (internal APIs, output formatting, log locations) is flexible.
- No Rust library compatibility concerns (CLI tool only).
- MSRV: latest stable − 1, bumped during weekly tend maintenance (`running-tend` skill).

## Terminology

Use consistently in docs, help text, and code comments:

- **main worktree** — the original git directory (from clone/init); bare repos have none
- **linked worktree** — created via `git worktree add` (git's term)
- **primary worktree** — the "home" worktree: main worktree for normal repos, default-branch worktree for bare repos
- **default branch** — the branch (main, master, …), not "main branch"
- **target** — destination for merge/rebase/push ("merge target"). Never use "target" for worktrees; say "worktree"

## Skills

Load relevant skills before starting; reload when scope changes mid-session. Project-local skills in `.claude/skills/`:

- `writing-user-outputs` — before editing code that calls `warning_message`, `hint_message`, `error_message`, `info_message`, `eprintln`, `println`, or otherwise produces user-visible strings (CLI help, progress UI, snapshots).
- `running-tend` — operating in CI or writing tend workflows.
- `release` — cutting a release.

## Worktree Model

- Worktrees are **addressed by branch name**, not filesystem path.
- Each worktree maps to **exactly one branch**.
- **Never retarget an existing worktree** to a different branch; create/switch/remove instead. (Sole exception: `wt step promote`, experimental, exchanges branches between two worktrees.)

## Documentation

Behavior changes require doc updates. `src/cli/mod.rs` (`after_long_help` plus clap attributes) is the PRIMARY SOURCE for command pages; never hand-edit the generated mirrors under `docs/content/` or `skills/worktrunk/reference/`. Ask: "does `--help` still describe what the code does?" After any doc change run `cargo test --test integration test_docs_are_in_sync`. Sync taxonomy, help-text authoring (three render contexts, link text, config-TOML blocks): `docs/CLAUDE.md`.

## Plugin Layout

Per-tool layout and path resolution (Claude/Codex/Gemini), the Codex no-hooks re-enablement conditions, the accepted `wt-switch-create` tradeoff, and `test_plugin_layout_is_consolidated`: `plugins/worktrunk/CLAUDE.md`.

## Data Safety

Never risk data loss without explicit user consent. A failed command that preserves data beats a "successful" one that silently destroys work.

- **Prefer failure over silent loss** — if an operation might destroy untracked files, uncommitted changes, or user data, fail with an error.
- **Explicit consent for destructive ops** — force-removing data (e.g. `--force` on remove) requires the user to explicitly request it.
- **No implicit destructive side effects** — never silently delete/overwrite as a side effect of an unrelated operation; make cleanup a separate explicit action the user chooses.
- **Favor the failing variant on races** — `git reset --keep` (fails if tracked files were modified) over `--hard`; `git checkout --merge` over `--force`. If no safer variant exists, document the risk inline.
- **Time-of-check vs time-of-use** — be conservative when there's a gap between the safety check and the operation. `wt merge` verifies clean before rebasing, but files could appear before cleanup — don't force-remove during cleanup.

Full inventory: FAQ [What files does Worktrunk create?](docs/content/faq.md#what-files-does-worktrunk-create) and [What can Worktrunk delete?](docs/content/faq.md#what-can-worktrunk-delete). Review new code that changes this surface against those sections.

## Command Execution Principles

### All Commands Through `shell_exec::Cmd`

Every external command goes through `shell_exec::Cmd` for consistent debug logging (`$ git status [worktree-name]`) and `[wt-trace]` timing. Never call `cmd.output()` directly. For git, prefer `Repository::run_command()` (wraps `Cmd` with worktree context). Pipe stdin via `.stdin_bytes(...)`. The `[wt-trace]` grammar is owned by `src/trace/emit.rs` — emit new trace records through that module, not ad-hoc `log::debug!("[wt-trace] …")` strings.

```rust
Cmd::new("git").args(["status", "--porcelain"]).current_dir(&wt).context("worktree-name").run()?;
Cmd::new("gh").args(["pr", "list"]).run()?;  // no context for standalone tools
```

### Real-time Output Streaming

Stream command output line-by-line rather than buffering. Responsiveness is a priority.

### Structured Output Over Error-Message Parsing

Prefer exit codes / `--porcelain` / `--json` over parsing human-readable messages, which break on locale, version, and rewording changes. `git merge-base` exit codes encode meaning (0 found, 1 no common ancestor, 128 invalid ref) — branch on `status.code()`, not message text.

| Tool | Fragile | Structured |
|------|---------|------------|
| `git diff` | `--stat` (localized) | `--numstat`, `--shortstat` (`(+)`/`(-)` hardcoded) |
| `git status` | default | `--porcelain=v2` |
| `git merge-base` | error messages | exit codes |
| `gh` / `glab` | default | `--json` |

When no structured alternative exists, document the fragility inline.

### Network Access

worktrunk is local-first: the network is touched only when the user asked for it. **One detection helper is exempt:** the *first* `Repository::default_branch()` per repo may fall through to `git ls-remote`; the result caches in `worktrunk.default-branch` and every later call is local. No other detection helper may add a similar fallback.

Why: silent "lookup" paths that walk to the wire (alias dispatch, hook context build, recovery) stall commands the user wouldn't expect to do network work, worst on a fresh clone. The `default_branch()` bootstrap keeps a fresh clone usable while bounding the exception to one helper firing at most once per repo.

Before adding an accessor that could reach the wire (`gh`, `glab`, `git fetch`, `git ls-remote`, HTTP), confirm the command that calls it is not intended to be fast. A foreground command the user runs and waits on absorbs the latency; a command in a synchronous hot path like a shell prompt cannot, and must not reach the wire. `wt list statusline` is not a fast command despite running on every prompt: Claude Code consumes its output asynchronously.

What currently reaches the wire:

- `wt list --full`, `wt list statusline` — CI status
- generating a branch summary with a `commit.generation` command
- generating a commit message with a `commit.generation` command
- `wt switch pr:<n>`, `wt switch mr:<n>` — host API to resolve the PR/MR, then `git fetch` of its branch
- `wt config show --full` — version check against GitHub
- the first `Repository::default_branch()` per repo — `git ls-remote` (above)

### Signal Handling: Ctrl-C Cancels the Current Command

When a child process exits from a signal (SIGINT, SIGTERM), every loop in the foreground execution path MUST abort rather than continue to the next iteration. This applies to worktree loops (`wt step for-each`), hook pipelines, alias steps, concurrent groups, and any future code running multiple child processes in sequence.

Why: wt installs a `signal_hook` SIGINT/SIGTERM handler so it can forward signals to child process groups before exiting cleanly. As a side effect wt itself does not die from the user's Ctrl-C — only the current child does. Without this policy a single Ctrl-C against `wt merge` would charge through the remaining hook steps, with `FailureStrategy::Warn` silently swallowing each interrupt.

- Signal-derived child exits surface as `WorktrunkError::ChildProcessExited { signal: Some(sig), .. }`. The `signal` field is the structured channel — never sniff `code >= 128` or parse error messages.
- Detect via `err.interrupt_exit_code()` (the `worktrunk::git::ErrorExt` trait). When it returns `Some(exit_code)`, propagate as `WorktrunkError::AlreadyDisplayed { exit_code }` (`128 + sig` by convention — 130 SIGINT, 143 SIGTERM) and break the loop.
- The check happens **before** any `FailureStrategy` branch — Warn must NOT swallow signal-derived errors.
- `handle_command_error` in `src/commands/command_executor.rs` enforces this for hook and alias pipelines (foreground and concurrent groups); `for_each.rs` enforces it for the worktree loop. New code that loops over child processes calls `.interrupt_exit_code()` on per-iteration errors and breaks.

### Project Commands Run Only After Approval

**Policy:** project-defined commands (`pre-*` / `post-*` hooks, `[aliases]`, `--execute` bodies from project config) are arbitrary code shipped in a repo the user may have just cloned, so they run only after the approval system (`Approvals` plus `approve_command_batch` / `approve_or_skip` in `src/commands/command_approval.rs`) clears them. Never build a code path that runs project commands without that gate. A context that can't prompt (a TUI mid-render, a background recovery path) consults the approval state read-only and runs only the already-approved subset: `commands::picker::do_removal` builds the plan via `HookPlan::approve_readonly` (no prompt).

**Why:** the gate is the only thing between `git clone && wt switch` and a `post-switch` hook running `curl … | sh`. A "we already validated the operation, so run the hooks too" shortcut turns every command that touches project config into remote code execution.

**Implementation:** the operation-driven hooks (`pre-merge`, `post-merge`, `pre-remove`, `post-remove`, `post-switch`, `pre-start`, `post-start`) are gated *before* a state mutation and run *after* it, so a second config read could select an unapproved command. `src/commands/hook_plan.rs` closes this structurally: each gate (`wt remove` / `wt merge` / `wt step prune` / `wt switch`) selects the command set once into an immutable `ApprovedHookPlan` (`HookPlan::approve`); the executor consumes only that value via `execute_planned_hook` / `register_planned` and holds no `ProjectConfig` to re-derive from, so re-selection is a compile error, not a review check. An empty plan (`--no-hooks`, declined, or no project config) runs nothing. The adjacent hooks with no gate→exec mutation (`pre-commit`, `post-commit`, `pre-switch`, `wt hook <type>`, aliases) still resolve config at invocation via `execute_hook` / `HookAnnouncer::register`. See `src/commands/hook_plan.rs` and the `commands::hooks` module spec.

## Hook Output Logs

`.git/wt/logs/` layout — per-branch and repo-wide log paths, plus the `sanitize_for_filename` filename rule: the `HookLog` spec in `src/commands/process.rs`. The top-level file-vs-directory split that `wt config state` walks: the "Log layout invariant" in `src/commands/config/state.rs`.

## Coverage

**NEVER merge a PR with failing `codecov/patch` without explicit user approval.** It is marked "not required" in GitHub but still gates merge. On PR heads codecov posts **check runs** (codecov GitHub App), not commit statuses: poll with `gh pr checks <number>` or the check-runs API; the combined-status API (`/commits/<sha>/status`) never shows them, and the check run lands a few minutes after the `code-coverage` job finishes. On failure, investigate and fix the gap — write tests, or remove unused code (including specialized error handlers for rare cases where falling through to a general handler suffices). If you believe it's a false positive, ask the user before merging. Coverage runs include `--features shell-integration-tests` (CI `code-coverage` and local `task coverage`) — don't dismiss failures by claiming the feature is off. Investigation commands, rename false-positives, and the "N functions mismatched" warning: `tests/CLAUDE.md`.

## Benchmarks & Traces

`cargo bench --bench list <filter>` (Criterion takes a positional substring filter; there's no `--skip`). `cargo run -p wt-perf -- timeline -- <args>` traces one `wt` invocation. Real-repo benchmarks clone rust-lang/rust on first run. The `benchmarks` CI job is non-required — only `test (linux|macos|windows)` block merge; `mergeStateStatus: UNSTABLE` from a still-pending bench run is mergeable. Filter map, expected numbers, and trace queries: `benches/CLAUDE.md`.

## Code Quality

### Use Existing Dependencies

Check `Cargo.toml` before hand-rolling a utility:

| Need | Use | Not |
|------|-----|-----|
| Path normalization | `path_slash::PathExt::to_slash_lossy()` | `.to_string_lossy().replace('\\', "/")` |
| Shell escaping | `shell_escape::unix::escape()` | manual quoting |
| ANSI colors | `color_print::cformat!()` | raw escape codes |
| Template var detection | `minijinja::undeclared_variables(false)` | regex/substring on `{{ var }}` |

### Other

- **Don't suppress warnings** with `#[allow(dead_code)]` — delete the code or add `// TODO(topic): used by <upcoming work>`.
- **System docstrings** — complex systems (state machines, cached state, cross-module coordination, non-obvious invalidation) get a module-level spec docstring (purpose, key decisions, contracts, invariants); keep it current. Exemplar: `commands/list/collect/mod.rs`.
- **No test code in library code** — no `#[cfg(test)]` convenience methods on library types; tests call the real API or define their own helpers.
- **Multiline strings** — plain literals with real embedded newlines (`r#"…"#` to avoid escaping `"`); never `\` continuation (silently strips following whitespace) or `concat!()`. Place long constants at module level.

## Error Handling

`anyhow` with context. `bail!` for business-logic errors (dirty worktree, missing branch, invalid state); `.context()` for wrapping I/O and external-command failures. Never `.expect()` / `.unwrap()` in a function returning `Result` — use `?`, `bail!`, or return an error.

## Config Deprecation

All config deprecation lives in one layer: pre-deserialization TOML migration in `src/config/deprecation.rs`. `migrate_content()` rewrites deprecated patterns into canonical form before serde parses; `check_and_migrate()` reuses it, and additionally detects patterns and emits per-process-deduped warnings (the user materializes migrations via `wt config update`). **Never silently drop an old config key** — that's a silent behavior change for users; migrate it.

Every deprecation is one row in the `DEPRECATION_RULES` table; the row's `RuleMode` carries the detection function and decides when the idempotent migration applies (`Structural` rewrites on every load; `UpdateOnly` only via `wt config update`, for deprecated forms that still work at runtime; `Silent` rewrites on every load with no detection). Table order is both the warning-emission order and the migration order. Detection produces a `Vec<DeprecationKind>` (`Deprecations`): each variant carries its own display payload, so `format_deprecation_warnings()` is one match over the kinds and `is_empty` is just `Vec::is_empty()`. Adding a deprecation: (1) a detection function and an idempotent migration function; (2) a `DeprecationKind` variant plus its match arm in `format_deprecation_warnings()`; (3) a `DEPRECATION_RULES` row; (4) for a removed top-level section, add a `DeprecatedSection` to `DEPRECATED_SECTION_KEYS` (canonical key plus display form) so `warn_unknown_fields` defers to the deprecation messaging and suggests the correct config file. A silently-migrated rename (e.g. `pre-create` → `pre-start`) is a `RuleMode::Silent` row with no variant — it emits no warning by construction. Renaming a field within a section follows the same shape via a TOML-level rename function (see `migrate_negated_bool`); the struct never needs the old field since migration precedes serde.

## Adding CLI Commands

Recipe, help-text placement, and flag-description conventions: `src/commands/CLAUDE.md`.

## Accessor Function Naming

| Prefix | Returns | Side effects | Absent → | Example |
|--------|---------|--------------|----------|---------|
| (bare noun) | `Option<T>` / `T` | none (may cache) | None/default | `config()`, `switch_previous()` |
| `set_*` | `Result<()>` | writes state | errors | `set_config()` |
| `require_*` | `Result<T>` | none | errors | `require_branch()` |
| `fetch_*` | `Result<T>` | network I/O | errors | `fetch_pr_info()` |
| `load_*` | `Result<T>` | file I/O | errors | `load_project_config()` |

No `get_*` — bare nouns follow Rust stdlib convention.

## Repository Caching

`Repository` caches read-only values via `Arc<RepoCache>` (cloning shares it). What is and isn't cached, the `list_worktrees()` post-mutation invariant, and the two storage patterns: the `# Caching` section in `src/git/repository/mod.rs`.

## Releases

Use the `release` skill (version bump, changelog, crates.io publish, GitHub release).
