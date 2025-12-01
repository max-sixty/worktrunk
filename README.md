<!-- markdownlint-disable MD014 MD024 MD033 -->

# Worktrunk

<!-- User badges -->

[![Crates.io](https://img.shields.io/crates/v/worktrunk?style=for-the-badge&logo=rust)](https://crates.io/crates/worktrunk)
[![License: MIT](https://img.shields.io/badge/LICENSE-MIT-blue?style=for-the-badge)](https://opensource.org/licenses/MIT)
[![GitHub CI Status](https://img.shields.io/github/actions/workflow/status/max-sixty/worktrunk/ci.yaml?event=push&branch=main&logo=github&style=for-the-badge)](https://github.com/max-sixty/worktrunk/actions?query=branch%3Amain+workflow%3Aci)

<!-- Dev badges (uncomment when repo is public and has traction) -->
<!-- [![Downloads](https://img.shields.io/crates/d/worktrunk?style=for-the-badge&logo=rust)](https://crates.io/crates/worktrunk) -->
<!-- [![Stars](https://img.shields.io/github/stars/max-sixty/worktrunk?style=for-the-badge&logo=github)](https://github.com/max-sixty/worktrunk/stargazers) -->

Worktrunk is a CLI for Git worktree management, designed for parallel AI agent
workflows. Git worktrees give each agent an isolated branch and directory;
Worktrunk adds branch-based navigation, unified status, and lifecycle hooks. The
goal is to make spinning up a new AI "developer" for a task feel as routine as
`git switch`.

## December 2025 Project Status

I've been using Worktrunk as my daily driver, and am releasing it as Open Source
this week. It's built with love (there's no slop!). If social proof is helpful:
I also created [PRQL](https://github.com/PRQL/prql) (10k stars) and am a
maintainer of [Xarray](https://github.com/pydata/xarray) (4k stars),
[Insta](https://github.com/mitsuhiko/insta), &
[Numbagg](https://github.com/numbagg/numbagg).

I'd recommend:

- **starting with Worktrunk as a simpler & better `git worktree`**: create / navigate /
  list / clean up git worktrees with ease
- **later using the more advanced features if you find they resonate**: there's
  lots for the more ambitious, such as [LLM commit
  messages](#llm-commit-messages), or [local merging of worktrees gated on
  CI-like checks](#local-merging-with-wt-merge), or [fzf-like selector +
  preview](#interactive-worktree-picker). And QoL features, such as listing the
  CI status & the Claude Code status for all branches, or a great [Claude Code
  statusline](#statusline-integration). But they're not required to get value
  from the tool.

## Demo

![Worktrunk Demo](dev/wt-demo/out/wt-demo.gif)

## Quick Start

### 1. Install

**Homebrew (macOS):**

```console
$ brew install max-sixty/worktrunk/wt
$ wt config shell install  # allows commands to change directories
```

**Cargo:**

```console
$ cargo install worktrunk
$ wt config shell install
```

### 2. Create a worktree

<!-- ⚠️ AUTO-GENERATED from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_simple_switch.snap — edit source to update -->

```console
$ wt switch --create fix-auth
✅ Created new worktree for fix-auth from main at ../repo.fix-auth
```

<!-- END AUTO-GENERATED -->

This creates `../repo.fix-auth` on branch `fix-auth`.

### 3. Switch between worktrees

<!-- ⚠️ AUTO-GENERATED from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_switch_back.snap — edit source to update -->

```console
$ wt switch feature-api
✅ Switched to worktree for feature-api at ../repo.feature-api
```

<!-- END AUTO-GENERATED -->

### 4. List worktrees

<!-- ⚠️ AUTO-GENERATED from tests/snapshots/integration__integration_tests__list__readme_example_simple_list.snap — edit source to update -->

```console
$ wt list
  Branch       Status         HEAD±    main↕  Path                Remote⇅  Commit    Age   Message
@ feature-api  +   ↑⇡      +36  -11   ↑4      ./repo.feature-api   ⇡3      b1554967  30m   Add API tests
^ main             ^⇣                         ./repo                   ⇣1  b834638e  1d    Initial commit
+ fix-auth        _                           ./repo.fix-auth              b834638e  1d    Initial commit

⚪ Showing 3 worktrees, 1 with changes, 1 ahead
```

<!-- END AUTO-GENERATED -->

`--full` adds CI status and conflicts. `--branches` includes all branches.

### 5. Clean up

Say we merged via CI, our changes are on main, and we're finished with the worktree:

<!-- ⚠️ AUTO-GENERATED from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_remove.snap — edit source to update -->

```console
$ wt remove
🔄 Removing feature-api worktree & branch in background (already in main)
```

<!-- END AUTO-GENERATED -->

## Why git worktrees?

We have a few options for working with multiple agents:

- one working tree with many branches — agents step on each other, can't use git
  for staging & committing
- multiple clones — slow to set up, drift out of sync
- git worktrees — multiple directories backed by a single `.git` directory

So we use git worktrees! But then...

## Why Worktrunk?

Git's built-in `worktree` commands require remembering worktrees' locations, and
composing git & `cd` commands together. In contrast, Worktrunk bundles creation,
navigation, status, and cleanup into simple commands. A few examples:

<table>
<tr>
<th>Task</th>
<th>Worktrunk</th>
<th>Plain git</th>
</tr>
<tr>
<td>Switch worktrees</td>
<td><pre lang="bash">wt switch feature</pre></td>
<td><pre lang="bash">cd ../repo.feature</pre></td>
</tr>
<tr>
<td>Create + start Claude</td>
<td><pre lang="bash">wt switch -c -x claude feature</pre>
...or with an <a href="#alias">alias</a>: <code>wsc feature</code>
</td>
<td><pre lang="bash">git worktree add -b feature ../repo.feature main
cd ../repo.feature
claude</pre></td>
</tr>

<tr>
<td>Clean up</td>
<td><pre lang="bash">wt remove</pre></td>
<td><pre lang="bash">cd ../repo
git worktree remove ../repo.feature
git branch -d feature</pre></td>
</tr>
<tr>
<td>List</td>
<td><pre lang="bash">wt list</pre>
...including diffstats & status
</td>
<td><pre lang="bash">git worktree list</pre>
...just branch names & paths
</td>
</tr>
<tr>
<td>List with CI status</td>
<td><pre lang="bash">wt list --full</pre>
...including CI status & diffstat downstream of <code>main</code>. Optionally add <code>--branches</code> or <code>--remotes</code>.
</td>
<td>N/A</td>
</tr>
</table>

...and check out examples below for more advanced workflows.

## Advanced

Many Worktrunk users will just use the commands above. For more:

### LLM commit messages

Worktrunk can invoke external commands to generate commit messages.
[llm](https://llm.datasette.io/) from [**@simonw**](https://github.com/simonw) is recommended.

Add to user config (`~/.config/worktrunk/config.toml`):

```toml
[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4-5-20251001"]
```

`wt merge` generates commit messages automatically or `wt step commit` runs just the commit step.

For custom prompt templates: `wt config --help`.

### Project hooks

Automate setup and validation at worktree lifecycle events:

| Hook            | When                                | Example                      |
| --------------- | ----------------------------------- | ---------------------------- |
| **post-create** | After worktree created              | `cp -r .cache`, `ln -s`      |
| **post-start**  | After worktree created (background) | `npm install`, `cargo build` |
| **pre-commit**  | Before creating any commit          | `pre-commit run`             |
| **pre-merge**   | After squash, before push           | `cargo test`, `pytest`       |
| **post-merge**  | After successful merge              | `cargo install --path .`     |

Project commands require approval on first run; use `--force` to skip prompts
or `--no-verify` to skip hooks entirely. Configure in `.config/wt.toml`:

```toml
# Install dependencies, build setup
[post-create]
"install" = "uv sync"

# Dev servers, file watchers (runs in background)
[post-start]
"dev" = "uv run dev"

# Tests and lints before merging (blocks on failure)
[pre-merge]
"lint" = "uv run ruff check"
"test" = "uv run pytest"
```

Example output:

<!-- ⚠️ AUTO-GENERATED from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_hooks_post_create.snap — edit source to update -->

```console
$ wt switch --create feature-x
🔄 Running post-create install:
   uv sync

  Resolved 24 packages in 145ms
  Installed 24 packages in 1.2s
✅ Created new worktree for feature-x from main at ../repo.feature-x
🔄 Running post-start dev:
   uv run dev
```

<!-- END AUTO-GENERATED -->

### Local merging with `wt merge`

`wt merge` handles the full merge workflow: stage, commit, squash, rebase,
merge, cleanup. Includes [LLM commit messages](#llm-commit-messages),
[project hooks](#project-hooks), and [config](#wt-config)/[flags](#wt-merge)
for skipping steps.

<table>
<tr>
<th>Task</th>
<th>Worktrunk</th>
<th>Plain git</th>
</tr>
<tr>
<td>Merge + clean up</td>
<td><pre lang="bash">wt merge</pre></td>
<td><pre lang="bash">git add -A
git reset --soft $(git merge-base HEAD main)
git diff --staged | llm "Write a commit message based on this diff" | git commit -F -
git rebase main
# pre-merge hook
cargo test
cd ../repo && git merge --ff-only feature
git worktree remove ../repo.feature
git branch -d feature
# post-merge hook
cargo install --path .  </pre></td>
</tr>
</table>

<!-- ⚠️ AUTO-GENERATED from tests/snapshots/integration__integration_tests__merge__readme_example_complex.snap — edit source to update -->

```console
$ wt merge
🔄 Squashing 3 commits into a single commit (3 files, +33)...
🔄 Generating squash commit message...
   feat(auth): Implement JWT authentication system

   Add comprehensive JWT token handling including validation, refresh logic,
   and authentication tests. This establishes the foundation for secure
   API authentication.

   - Implement token refresh mechanism with expiry handling
   - Add JWT encoding/decoding with signature verification
   - Create test suite covering all authentication flows
✅ Squashed @ 95c3316
🔄 Running pre-merge test:
   cargo test
    Finished test [unoptimized + debuginfo] target(s) in 0.12s
     Running unittests src/lib.rs (target/debug/deps/worktrunk-abc123)

running 18 tests
test auth::tests::test_jwt_decode ... ok
test auth::tests::test_jwt_encode ... ok
test auth::tests::test_token_refresh ... ok
test auth::tests::test_token_validation ... ok

test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s
🔄 Running pre-merge lint:
   cargo clippy
    Checking worktrunk v0.1.0
    Finished dev [unoptimized + debuginfo] target(s) in 1.23s
🔄 Merging 1 commit to main @ 95c3316 (no rebase needed)
   * 95c3316 feat(auth): Implement JWT authentication system
    auth.rs      |  8 ++++++++
    auth_test.rs | 17 +++++++++++++++++
    jwt.rs       |  8 ++++++++
    3 files changed, 33 insertions(+)
✅ Merged to main (1 commit, 3 files, +33)
🔄 Removing feature-auth worktree & branch in background (already in main)
🔄 Running post-merge install:
   cargo install --path .
  Installing worktrunk v0.1.0
   Compiling worktrunk v0.1.0
    Finished release [optimized] target(s) in 2.34s
  Installing ~/.cargo/bin/wt
   Installed package `worktrunk v0.1.0` (executable `wt`)
```

<!-- END AUTO-GENERATED -->

### Claude Code Status Tracking

The Worktrunk plugin adds Claude Code session tracking to `wt list`:

<!-- ⚠️ AUTO-GENERATED from tests/snapshots/integration__integration_tests__list__with_user_status.snap — edit source to update -->

```console
$ wt list
  Branch       Status         HEAD±    main↕  Path                Remote⇅  Commit    Age   Message
@ main             ^                          ./repo                       b834638e  1d    Initial commit
+ feature-api      ↑  🤖              ↑1      ./repo.feature-api           9606cd0f  1d    Add REST API endpoints
+ review-ui      ? ↑  💬              ↑1      ./repo.review-ui             afd3b353  1d    Add dashboard component
+ wip-docs       ?_                           ./repo.wip-docs              b834638e  1d    Initial commit

⚪ Showing 4 worktrees, 2 ahead
```

<!-- END AUTO-GENERATED -->

- `🤖` — Claude is working
- `💬` — Claude is waiting for input

**Install the plugin:**

```console
claude plugin marketplace add max-sixty/worktrunk
claude plugin install worktrunk@worktrunk
```

<details>
<summary>Manual status markers</summary>

Set status markers manually for any workflow:

```console
wt config status set "🚧"           # Current branch
wt config status set "✅" --branch feature  # Specific branch
git config worktrunk.status.feature "💬"    # Direct git config
```

</details>

### Interactive Worktree Picker

`wt select` opens a fzf-like fuzzy-search worktree picker with diff preview. Unix only.

Preview tabs (toggle with `1`/`2`/`3`):

- **Tab 1**: Working tree changes (uncommitted)
- **Tab 2**: Commit history (commits not on main highlighted)
- **Tab 3**: Branch diff (changes ahead of main)

### Statusline Integration

`wt list statusline` outputs a single-line status for shell prompts, starship,
or editor integrations[^1].

[^1]:
    Currently this grabs CI status, so is too slow to use in synchronous
    contexts. If a faster version would be helpful, please add an Issue.

**Claude Code** (`--claude-code`): Reads workspace context from stdin, outputs
directory, branch status, and model.

```text
~/w/myproject.feature-auth  !🤖  ±+42 -8  ↑3  ⇡1  ●  | Opus
```

<details>
<summary>Claude Code configuration</summary>

Add to `~/.claude/settings.json`:

```json
{
  "statusLine": {
    "type": "command",
    "command": "wt list statusline --claude-code"
  }
}
```

</details>

## Tips & patterns

<a id="alias"></a>**Alias for new worktree + agent:**

```console
alias wsc='wt switch --create --execute=claude'
wsc new-feature  # Creates worktree, runs hooks, launches Claude
```

**Eliminate cold starts** — `post-create` hooks install deps and copy caches.
See [`.config/wt.toml`](.config/wt.toml) for an example using copy-on-write.

**Local CI gate** — `pre-merge` hooks run before merging. Failures abort the
merge.

**Track agent status** — Custom emoji markers show agent state in `wt list`.
Claude Code hooks can set these automatically. See [Claude Code Status
Tracking](#claude-code-status-tracking).

**Monitor CI across branches** — `wt list --full --branches` shows PR/CI status
for all branches, including those without worktrees. CI column links to PR pages
in terminals with hyperlink support.

**JSON API** — `wt list --format=json` for dashboards, statuslines, scripts.

**Task runners** — Reference Taskfile/Justfile/Makefile in hooks:

```toml
[post-create]
"setup" = "task install"

[pre-merge]
"validate" = "just test lint"
```

**Shortcuts** — `^` = default branch, `@` = current branch, `-` = previous
worktree. Example: `wt switch --create hotfix --base=@` branches from current
HEAD.

## Commands Reference

<details>
<summary><strong><code>wt switch [branch]</code></strong> - Switch to existing worktree or create a new one</summary>

<!-- ⚠️ AUTO-GENERATED from `wt switch --help-md` — edit source to update -->

```text
wt switch — Switch to a worktree
Usage: wt switch [OPTIONS] <BRANCH>

Arguments:
  <BRANCH>
          Branch or worktree name

          Shortcuts: '^' (main), '-' (previous), '@' (current)

Options:
  -c, --create
          Create a new branch

  -b, --base <BASE>
          Base branch

          Defaults to default branch.

  -x, --execute <EXECUTE>
          Command to run after switch

  -f, --force
          Skip approval prompts

      --no-verify
          Skip all project hooks

  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info

```

## Operation

### Worktree resolution

Arguments are resolved using **path-first lookup**:

1. Compute the expected path for the argument (using the configured path template)
2. If a worktree exists at that path, switch to it (regardless of what branch it's on)
3. Otherwise, treat the argument as a branch name

**Example**: If `repo.foo/` exists but is on branch `bar`:

- `wt switch foo` switches to `repo.foo/` (the `bar` branch worktree)
- `wt switch bar` also works (falls back to branch lookup)

### Switching to Existing Worktree

- If worktree exists at expected path or for branch, changes directory via shell integration
- No hooks run
- No branch creation

### Creating New Worktree (`--create`)

1. Creates new branch (defaults to current default branch as base)
2. Creates worktree in configured location (default: `../{{ main_worktree }}.{{ branch }}`)
3. Runs post-create hooks sequentially (blocking)
4. Shows success message
5. Spawns post-start hooks in background (non-blocking)
6. Changes directory to new worktree via shell integration

## Hooks

### post-create (sequential, blocking)

- Run after worktree creation, before success message
- Typically: `npm install`, `cargo build`, setup tasks
- Failures block the operation
- Skip with `--no-verify`

### post-start (parallel, background)

- Spawned after success message shown
- Typically: dev servers, file watchers, editors
- Run in background, failures logged but don't block
- Logs: `.git/wt-logs/{branch}-post-start-{name}.log`
- Skip with `--no-verify`

**Template variables:** `{{ repo }}`, `{{ branch }}`, `{{ worktree }}`, `{{ repo_root }}`

**Security:** Commands from project hooks require approval on first run.
Approvals are saved to user config. Use `--force` to bypass prompts.
See `wt config approvals --help`.

## Examples

Switch to existing worktree:

```console
wt switch feature-branch
```

Create new worktree from main:

```console
wt switch --create new-feature
```

Switch to previous worktree:

```console
wt switch -
```

Create from specific base:

```console
wt switch --create hotfix --base production
```

Create and run command:

```console
wt switch --create docs --execute "code ."
```

Skip hooks during creation:

```console
wt switch --create temp --no-verify
```

## Shortcuts

Use `@` for current HEAD, `-` for previous, `^` for main:

```console
wt switch @                              # Switch to current branch's worktree
wt switch -                              # Switch to previous worktree
wt switch --create new-feature --base=^  # Branch from main (default)
wt switch --create bugfix --base=@       # Branch from current HEAD
wt remove @                              # Remove current worktree
```

<!-- END AUTO-GENERATED -->

</details>

<details id="wt-merge">
<summary><strong><code>wt merge [target]</code></strong> - Merge, push, and cleanup</summary>

<!-- ⚠️ AUTO-GENERATED from `wt merge --help-md` — edit source to update -->

```text
wt merge — Merge worktree into target branch
Usage: wt merge [OPTIONS] [TARGET]

Arguments:
  [TARGET]
          Target branch

          Defaults to default branch.

Options:
      --no-squash
          Skip commit squashing

      --no-commit
          Skip commit, squash, and rebase

      --no-remove
          Keep worktree after merge

      --no-verify
          Skip all project hooks

  -f, --force
          Skip approval prompts

      --stage <STAGE>
          What to stage before committing [default: all]

          Possible values:
          - all:     Stage everything: untracked files + unstaged tracked changes
          - tracked: Stage tracked changes only (like git add -u)
          - none:    Stage nothing, commit only what's already in the index

  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info

```

## Operation

Commit → Squash → Rebase → Pre-merge hooks → Push → Cleanup → Post-merge hooks

### Commit

Uncommitted changes are staged and committed with LLM commit message.
Use `--stage=tracked` to stage only tracked files, or `--stage=none` to commit only what's already staged.

### Squash

Multiple commits are squashed into one (like GitHub's "Squash and merge") with LLM commit message.
Skip with `--no-squash`. Safety backup: `git reflog show refs/wt-backup/<branch>`

### Rebase

Branch is rebased onto target. Conflicts abort the merge immediately.

### Hooks

Pre-merge commands run after rebase (failures abort). Post-merge commands
run after cleanup (failures logged). Skip all with `--no-verify`.

### Push

Fast-forward push to local target branch. Non-fast-forward pushes are rejected.

### Cleanup

Worktree and branch are removed. Skip with `--no-remove`.

**Template variables:** `{{ repo }}`, `{{ branch }}`, `{{ worktree }}`, `{{ repo_root }}`, `{{ target }}`

**Security:** Commands from project hooks require approval on first run.
Approvals are saved to user config. Use `--force` to bypass prompts.
See `wt config approvals --help`.

## Examples

Basic merge to main:

```console
wt merge
```

Merge without squashing:

```console
wt merge --no-squash
```

Keep worktree after merging:

```console
wt merge --no-remove
```

Skip all hooks:

```console
wt merge --no-verify
```

<!-- END AUTO-GENERATED -->

</details>

<details>
<summary><strong><code>wt remove [worktree]</code></strong> - Remove worktree and branch</summary>

<!-- ⚠️ AUTO-GENERATED from `wt remove --help-md` — edit source to update -->

```text
wt remove — Remove worktree and branch
Usage: wt remove [OPTIONS] [WORKTREES]...

Arguments:
  [WORKTREES]...
          Worktree or branch (@ for current)

Options:
      --no-delete-branch
          Keep branch after removal

  -D, --force-delete
          Delete unmerged branches

      --no-background
          Run removal in foreground

  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info

```

## Operation

Removes worktree directory, git metadata, and branch. Requires clean working tree.

### No arguments (remove current)

- Removes current worktree and switches to main worktree
- In main worktree: switches to default branch

### By name (remove specific)

- Removes specified worktree(s) and branches
- Current worktree removed last (switches to main first)

### Worktree resolution

Arguments are resolved to worktrees using **path-first lookup**:

1. Compute the expected path for the argument (using the configured path template)
2. If a worktree exists at that path, use it (regardless of what branch it's on)
3. Otherwise, treat the argument as a branch name

**Example**: If `repo.foo/` exists but is on branch `bar`:

- `wt remove foo` removes `repo.foo/` and the `bar` branch
- `wt remove bar` also works (falls back to branch lookup)

**Conflict detection**: If path `repo.foo/` has a worktree on branch `bar`, but
branch `foo` has a different worktree at `repo.bar/`, an error is raised.

**Special arguments**:

- `@` - current worktree (by path, works in detached HEAD)
- `-` - previous worktree (from switch history)
- `^` - main worktree

### Branch deletion

By default, branches are deleted only when their content is already in the target branch:

- no changes beyond the common ancestor — `git diff --name-only target...branch` is empty:
  no files changed between the merge base of `target`/`branch` and the tip of `branch`.
- same content as target — `git rev-parse branch^{tree}` equals `git rev-parse target^{tree}`:
  both branches point at the same tracked-files snapshot (tree), even if the commits differ.

This handles workflows where PRs are squash-merged or rebased, which don't preserve
commit ancestry but do integrate the content. Use `-D` to delete unintegrated
branches, or `--no-delete-branch` to always keep branches.

### Background removal (default)

- Returns immediately for continued work
- Logs: `.git/wt-logs/{branch}-remove.log`
- Use `--no-background` for foreground (blocking)

### Cleanup

Stops any git fsmonitor daemon for the worktree before removal. This prevents orphaned processes when using builtin fsmonitor (`core.fsmonitor=true`). No effect on Watchman users.

## Examples

Remove current worktree and branch:

```console
wt remove
```

Remove specific worktree and branch:

```console
wt remove feature-branch
```

Remove worktree but keep branch:

```console
wt remove --no-delete-branch feature-branch
```

Remove multiple worktrees:

```console
wt remove old-feature another-branch
```

Remove in foreground (blocking):

```console
wt remove --no-background feature-branch
```

Switch to default in main:

```console
wt remove  # (when already in main worktree)
```

<!-- END AUTO-GENERATED -->

</details>

<details id="wt-list">
<summary><strong><code>wt list</code></strong> - Show all worktrees and branches</summary>

<!-- ⚠️ AUTO-GENERATED from `wt list --help-md` — edit source to update -->

```text
wt list — List worktrees and optionally branches
Usage: wt list [OPTIONS]
       wt list <COMMAND>

Commands:
  statusline  Single-line status for shell prompts

Options:
      --format <FORMAT>
          Output format (table, json)

          [default: table]

      --branches
          Include branches without worktrees

      --remotes
          Include remote branches

      --full
          Show CI, conflicts, diffs

      --progressive
          Show fast info immediately, update with slow info

          Displays local data (branches, paths, status) first, then updates with remote data (CI, upstream) as it arrives. Auto-enabled for TTY.

  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info

```

## Columns

- **Branch:** Branch name
- **Status:** Quick status symbols (see Status Symbols below)
- **HEAD±:** Uncommitted changes vs HEAD (+added -deleted lines, staged + unstaged)
- **main↕:** Commit count ahead↑/behind↓ relative to main (commits in HEAD vs main)
- **main…±** (`--full`): Line diffs in commits ahead of main (+added -deleted)
- **Path:** Worktree directory location
- **Remote⇅:** Commits ahead⇡/behind⇣ relative to tracking branch (e.g. `origin/branch`)
- **CI** (`--full`): CI pipeline status (tries PR/MR checks first, falls back to branch workflows)
  - `●` **passed** (green) - All checks passed
  - `●` **running** (blue) - Checks in progress
  - `●` **failed** (red) - Checks failed
  - `●` **conflicts** (yellow) - Merge conflicts with base
  - `●` **no-ci** (gray) - PR/MR or workflow found but no checks configured
  - **(blank)** - No PR/MR or workflow found, or `gh`/`glab` CLI unavailable
  - **(dimmed)** - Stale: unpushed local changes differ from PR/MR head
- **Commit:** Short commit hash (8 chars)
- **Age:** Time since last commit (relative)
- **Message:** Last commit message (truncated)

## Status Symbols

Order: `+!? ✖⚠≡_ ↻⋈ ↑↓↕ ⇡⇣⇅ ⎇⌫⊠`

- `+` Staged files (ready to commit)
- `!` Modified files (unstaged changes)
- `?` Untracked files present
- `✖` **Merge conflicts** - unresolved conflicts in working tree (fix before continuing)
- `⊘` **Would conflict** - merging into main would fail
- `≡` Working tree matches main (identical contents, regardless of commit history)
- `_` No commits (no commits ahead AND no uncommitted changes)
- `↻` Rebase in progress
- `⋈` Merge in progress
- `↑` Ahead of main branch
- `↓` Behind main branch
- `↕` Diverged (both ahead and behind main)
- `⇡` Ahead of remote tracking branch
- `⇣` Behind remote tracking branch
- `⇅` Diverged (both ahead and behind remote)
- `⎇` Branch indicator (shown for branches without worktrees)
- `⌫` Prunable worktree (directory missing, can be pruned)
- `⊠` Locked worktree (protected from auto-removal)

Rows are dimmed when there's no marginal contribution (`≡` matches main OR `_` no commits).

## JSON Output

Use `--format=json` for structured data. Each object contains two status maps
with the same fields in the same order as Status Symbols above:

**`status`** - variant names for querying:

- `working_tree`: `{untracked, modified, staged, renamed, deleted}` booleans
- `branch_state`: `""` | `"Conflicts"` | `"MergeTreeConflicts"` | `"MatchesMain"` | `"NoCommits"`
- `git_operation`: `""` | `"Rebase"` | `"Merge"`
- `main_divergence`: `""` | `"Ahead"` | `"Behind"` | `"Diverged"`
- `upstream_divergence`: `""` | `"Ahead"` | `"Behind"` | `"Diverged"`
- `user_status`: string (optional)

**`status_symbols`** - Unicode symbols for display (same fields, plus `worktree_attrs`: ⎇/⌫/⊠)

Note: `locked` and `prunable` are top-level fields on worktree objects, not in status.

**Worktree position fields** (for identifying special worktrees):

- `is_main`: boolean - is the main worktree
- `is_current`: boolean - is the current working directory (present when true)
- `is_previous`: boolean - is the previous worktree from `wt switch` (present when true)

**Query examples:**

```console
# Find worktrees with conflicts
jq '.[] | select(.status.branch_state == "Conflicts")'

# Find worktrees with untracked files
jq '.[] | select(.status.working_tree.untracked)'

# Find worktrees in rebase or merge
jq '.[] | select(.status.git_operation != "")'

# Get branches ahead of main
jq '.[] | select(.status.main_divergence == "Ahead")'

# Find locked worktrees
jq '.[] | select(.locked != null)'

# Get current worktree info (useful for statusline tools)
jq '.[] | select(.is_current == true)'
```

<!-- END AUTO-GENERATED -->

</details>

<details id="wt-config">
<summary><strong><code>wt config</code></strong> - Manage configuration</summary>

<!-- ⚠️ AUTO-GENERATED from `wt config --help-md` — edit source to update -->

```text
wt config — Manage configuration and shell integration
Usage: wt config [OPTIONS] <COMMAND>

Commands:
  shell      Shell integration setup
  create     Create user configuration file
  show       Show configuration files & locations
  cache      Manage caches (CI status, default branch)
  status     Manage branch status markers
  approvals  Manage command approvals

Options:
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info

```

## Setup Guide

1. Set up shell integration

   ```console
   wt config shell install
   ```

   Or manually add to the shell config:

   ```console
   eval "$(wt config shell init bash)"
   ```

2. (Optional) Create user config file

   ```console
   wt config create
   ```

   This creates `~/.config/worktrunk/config.toml` with examples.

3. (Optional) Enable LLM commit messages

   Install: `uv tool install -U llm`
   Configure: `llm keys set anthropic`
   Add to user config:

   ```toml
   [commit-generation]
   command = "llm"
   ```

## LLM Setup Details

For Claude:

```console
llm install llm-anthropic
llm keys set anthropic
llm models default claude-haiku-4-5-20251001
```

For OpenAI:

```console
llm keys set openai
```

Use `wt config show` to view the current configuration.
Docs: <https://llm.datasette.io/> | <https://github.com/sigoden/aichat>

## Configuration Files

**User config**:

- Location: `~/.config/worktrunk/config.toml` (or `WORKTRUNK_CONFIG_PATH`)
- Run `wt config create --help` to view documented examples

**Project config**:

- Location: `.config/wt.toml` in repository root
- Contains: post-create, post-start, pre-commit, pre-merge, post-merge hooks

<!-- END AUTO-GENERATED -->

</details>

<details>
<summary><strong><code>wt step</code></strong> - Building blocks for workflows</summary>

<!-- ⚠️ AUTO-GENERATED from `wt step --help-md` — edit source to update -->

```text
wt step — Workflow building blocks
Usage: wt step [OPTIONS] <COMMAND>

Commands:
  commit       Commit changes with LLM commit message
  squash       Squash commits with LLM commit message
  push         Push changes to local target branch
  rebase       Rebase onto target
  post-create  Run post-create hook
  post-start   Run post-start hook
  pre-commit   Run pre-commit hook
  pre-merge    Run pre-merge hook
  post-merge   Run post-merge hook

Options:
  -h, --help  Print help

Global Options:
  -C <path>            Working directory for this command
      --config <path>  User config file path
  -v, --verbose        Show commands and debug info
```

<!-- END AUTO-GENERATED -->

</details>

## FAQ

<details>
<summary><strong>What commands does Worktrunk execute?</strong></summary>

Worktrunk executes commands in three contexts:

1. **Project hooks** (project config: `.config/wt.toml`) - Automation for worktree lifecycle
2. **LLM commands** (user config: `~/.config/worktrunk/config.toml`) - Commit message generation
3. **--execute flag** - Commands provided explicitly

Commands from project hooks and LLM configuration require approval on first run. Approved commands are saved to user config under the project's configuration. If a command changes, Worktrunk requires new approval.

**Example approval prompt:**

<!-- ⚠️ AUTO-GENERATED from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_approval_prompt.snap — edit source to update -->

```text
🟡 repo needs approval to execute 3 commands:

⚪ post-create install:
   echo 'Installing dependencies...'

⚪ post-create build:
   echo 'Building project...'

⚪ post-create test:
   echo 'Running tests...'

💡 Allow and remember? [y/N]
```

<!-- END AUTO-GENERATED -->

Use `--force` to bypass prompts (useful for CI/automation).

</details>

<details>
<summary><strong>How does Worktrunk compare to alternatives?</strong></summary>

### vs. Branch Switching

Branch switching uses one directory, so only one agent can work at a time.
Worktrees give each agent its own directory.

### vs. Plain `git worktree`

Git's built-in worktree commands work but require manual lifecycle management:

```console
# Plain git worktree workflow
git worktree add -b feature-branch ../myapp-feature main
cd ../myapp-feature
# ...work, commit, push...
cd ../myapp
git merge feature-branch
git worktree remove ../myapp-feature
git branch -d feature-branch
```

Worktrunk automates the full lifecycle:

```console
wt switch --create feature-branch  # Creates worktree, runs setup hooks
# ...work...
wt merge                            # Squashes, merges, removes worktree
```

What `git worktree` doesn't provide:

- Consistent directory naming and cleanup validation
- Project-specific automation (install dependencies, start services)
- Unified status across all worktrees (commits, CI, conflicts, changes)

Worktrunk adds path management, lifecycle hooks, and `wt list --full` for viewing all worktrees—branches, uncommitted changes, commits ahead/behind, CI status, and conflicts—in a single view.

### vs. git-machete / git-town

Different scopes:

- **git-machete**: Branch stack management in a single directory
- **git-town**: Git workflow automation in a single directory
- **worktrunk**: Multi-worktree management with hooks and status aggregation

These tools can be used together—run git-machete or git-town inside individual worktrees.

### vs. Git TUIs (lazygit, gh-dash, etc.)

Git TUIs operate on a single repository. Worktrunk manages multiple worktrees,
runs automation hooks, and aggregates status across branches. TUIs work inside
each worktree directory.

</details>

<details>
<summary><strong>Installation fails with C compilation errors</strong></summary>

Errors related to tree-sitter or C compilation (C99 mode, `le16toh` undefined)
can be avoided by installing without syntax highlighting:

```console
cargo install worktrunk --no-default-features
```

This disables bash syntax highlighting in command output but keeps all core functionality. The syntax highlighting feature requires C99 compiler support and can fail on older systems or minimal Docker images.

</details>

<details>
<summary><strong>How can I contribute?</strong></summary>

- Star the repo
- Try it out and [open an issue](https://github.com/max-sixty/worktrunk/issues) with feedback
- Send to a friend
- Post about it — [X](https://twitter.com/intent/tweet?text=Worktrunk%20%E2%80%94%20CLI%20for%20git%20worktree%20management&url=https%3A%2F%2Fgithub.com%2Fmax-sixty%2Fworktrunk) · [Reddit](https://www.reddit.com/submit?url=https%3A%2F%2Fgithub.com%2Fmax-sixty%2Fworktrunk&title=Worktrunk%20%E2%80%94%20CLI%20for%20git%20worktree%20management) · [LinkedIn](https://www.linkedin.com/sharing/share-offsite/?url=https%3A%2F%2Fgithub.com%2Fmax-sixty%2Fworktrunk)

Thanks in advance!

</details>

<details>
<summary><strong>Any notes for developing this crate?</strong></summary>

### Running Tests

**Quick tests (no external dependencies):**

```bash
cargo test --lib --bins           # Unit tests (~200 tests)
cargo test --test integration     # Integration tests without shell tests (~300 tests)
```

**Full integration tests (requires bash, zsh, fish):**

```bash
cargo test --test integration --features shell-integration-tests
```

**Dependencies for shell integration tests:**

- bash, zsh, fish shells
- Quick setup: `./dev/setup-claude-code-web.sh` (installs shells on Linux)

### Releases

Use [cargo-release](https://github.com/crate-ci/cargo-release) to publish new versions:

```console
cargo install cargo-release

# Bump version, update Cargo.lock, commit, tag, and push
cargo release patch --execute   # 0.1.0 -> 0.1.1
cargo release minor --execute   # 0.1.0 -> 0.2.0
cargo release major --execute   # 0.1.0 -> 1.0.0
```

This updates Cargo.toml and Cargo.lock, creates a commit and tag, then pushes to GitHub. The tag push triggers GitHub Actions to build binaries, create the release, and publish to crates.io.

Run without `--execute` to preview changes first.

### Updating Homebrew Formula

After `cargo release` completes and the GitHub release is created, update the [homebrew-worktrunk](https://github.com/max-sixty/homebrew-worktrunk) tap:

```console
./dev/update-homebrew.sh
```

This script fetches the new tarball, computes the SHA256, updates the formula, and pushes to homebrew-worktrunk.

</details>
