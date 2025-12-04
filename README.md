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

![Worktrunk Demo](https://cdn.jsdelivr.net/gh/max-sixty/worktrunk-assets@main/demos/wt-demo.gif)

## Quick Start

### 1. Install

**Homebrew (macOS & Linux):**

```bash
$ brew install max-sixty/worktrunk/wt
$ wt config shell install  # allows commands to change directories
```

**Cargo:**

```bash
$ cargo install worktrunk
$ wt config shell install
```

### 2. Create a worktree

<!-- ⚠️ AUTO-GENERATED from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_simple_switch.snap — edit source to update -->

```bash
$ wt switch --create fix-auth
✅ Created new worktree for fix-auth from main at ../repo.fix-auth
```

<!-- END AUTO-GENERATED -->

This creates `../repo.fix-auth` on branch `fix-auth`.

### 3. Switch between worktrees

<!-- ⚠️ AUTO-GENERATED from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_switch_back.snap — edit source to update -->

```bash
$ wt switch feature-api
✅ Switched to worktree for feature-api at ../repo.feature-api
```

<!-- END AUTO-GENERATED -->

### 4. List worktrees

<!-- ⚠️ AUTO-GENERATED from tests/snapshots/integration__integration_tests__list__readme_example_list.snap — edit source to update -->

```console
$ wt list
  Branch       Status         HEAD±    main↕  Path                Remote⇅  Commit    Age   Message
@ feature-api  +   ↕⇡      +54   -5   ↑4  ↓1  ./repo.feature-api   ⇡3      28d38c20  30m   Add API tests
^ main             ^⇅                         ./repo               ⇡1  ⇣1  2e6b7a8f  4d    Merge fix-auth:…
+ fix-auth         ↕|                 ↑2  ↓1  ./repo.fix-auth        |     1d697d5b  5h    Add secure token…

⚪ Showing 3 worktrees, 1 with changes, 2 ahead
```

<!-- END AUTO-GENERATED -->

`--full` adds CI status and conflicts. `--branches` includes all branches.

### 5. Clean up

Say we merged via CI, our changes are on main, and we're finished with the worktree:

<!-- ⚠️ AUTO-GENERATED from tests/integration_tests/snapshots/integration__integration_tests__shell_wrapper__tests__readme_example_remove.snap — edit source to update -->

```bash
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

```bash
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

```bash
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

<!-- ⚠️ AUTO-GENERATED from tests/snapshots/integration__integration_tests__list__with_user_marker.snap — edit source to update -->

```bash
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

```bash
claude plugin marketplace add max-sixty/worktrunk
claude plugin install worktrunk@worktrunk
```

<details>
<summary>Manual status markers</summary>

Set status markers manually for any workflow:

```bash
wt config var set marker "🚧"                   # Current branch
wt config var set marker "✅" --branch feature  # Specific branch
git config worktrunk.marker.feature "💬"        # Direct git config
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

```
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

```bash
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

```
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

Two distinct operations:

- **Switch to existing worktree** — Changes directory, nothing else
- **Create new worktree** (`--create`) — Creates branch and worktree, runs [hooks](@/hooks.md)

```

### Examples

```bash
wt switch feature-auth           # Switch to existing worktree
wt switch -                      # Previous worktree (like cd -)
wt switch --create new-feature   # Create branch and worktree
wt switch --create hotfix --base production
```

For interactive selection, use [`wt select`](@/select.md).

### Creating worktrees

With `--create`, worktrunk:

1. Creates branch from `--base` (defaults to default branch)
2. Creates worktree at configured path
3. Runs [post-create hooks](@/hooks.md#post-create) (blocking)
4. Switches to new directory
5. Spawns [post-start hooks](@/hooks.md#post-start) (background)

```bash
wt switch --create api-refactor
wt switch --create fix --base release-2.0
wt switch --create docs --execute "code ."
wt switch --create temp --no-verify      # Skip hooks
```

### Shortcuts

| Symbol | Meaning |
|--------|---------|
| `-` | Previous worktree |
| `@` | Current branch's worktree |
| `^` | Default branch worktree |

```bash
wt switch -                      # Back to previous
wt switch ^                      # Main worktree
wt switch --create fix --base=@  # Branch from current HEAD
```

### Path-first lookup

Arguments resolve by checking the filesystem before git branches:

1. Compute expected path from argument (using configured path template)
2. If worktree exists at that path, switch to it
3. Otherwise, treat argument as branch name

**Edge case**: If `repo.foo/` exists but tracks branch `bar`:
- `wt switch foo` → switches to `repo.foo/` (the `bar` worktree)
- `wt switch bar` → also works (branch lookup finds same worktree)

### See also

- [wt select](@/select.md) — Interactive worktree selection
- [wt list](@/list.md) — View all worktrees
- [wt remove](@/remove.md) — Delete worktrees when done
- [wt merge](@/merge.md) — Integrate changes back to main

<!-- END AUTO-GENERATED -->

</details>

<details id="wt-merge">
<summary><strong><code>wt merge [target]</code></strong> - Merge, push, and cleanup</summary>

<!-- ⚠️ AUTO-GENERATED from `wt merge --help-md` — edit source to update -->

```
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

Merge the current branch into the target branch and clean up. Handles the full workflow: commit uncommitted changes, squash commits, rebase, run hooks, push to target, and remove the worktree.

When already on the target branch or in the main worktree, the worktree is preserved automatically.

```

### Examples

Basic merge to main:

```bash
wt merge
```

Keep the worktree after merging:

```bash
wt merge --no-remove
```

Preserve commit history (no squash):

```bash
wt merge --no-squash
```

Skip git operations, only run hooks and push:

```bash
wt merge --no-commit
```

### Pipeline

`wt merge` runs these steps:

1. **Commit** — Stages and commits uncommitted changes. Commit messages are LLM-generated. Use `--stage` to control what gets staged: `all` (default), `tracked`, or `none`.

2. **Squash** — Combines all commits into one (like GitHub's "Squash and merge"). Skip with `--no-squash` to preserve individual commits. A backup ref is saved to `refs/wt-backup/<branch>`.

3. **Rebase** — Rebases onto the target branch. Conflicts abort immediately.

4. **Pre-merge hooks** — Project commands run after rebase, before push. Failures abort. See [Hooks](@/hooks.md).

5. **Push** — Fast-forward push to the target branch. Non-fast-forward pushes are rejected.

6. **Cleanup** — Removes the worktree and branch. Use `--no-remove` to keep the worktree.

7. **Post-merge hooks** — Project commands run after cleanup. Failures are logged but don't abort.

Use `--no-commit` to skip steps 1-3 and only run hooks and push. Requires a clean working tree and `--no-remove`.

### See also

- [wt step](@/step.md) — Run individual merge steps (commit, squash, rebase, push)
- [wt remove](@/remove.md) — Remove worktrees without merging
- [wt switch](@/switch.md) — Navigate to other worktrees

<!-- END AUTO-GENERATED -->

</details>

<details>
<summary><strong><code>wt remove [worktree]</code></strong> - Remove worktree and branch</summary>

<!-- ⚠️ AUTO-GENERATED from `wt remove --help-md` — edit source to update -->

```
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

Removes worktrees and their branches. Without arguments, removes the current worktree and returns to the main worktree.

```

### Examples

Remove current worktree:

```bash
wt remove
```

Remove specific worktrees:

```bash
wt remove feature-branch
wt remove old-feature another-branch
```

Keep the branch:

```bash
wt remove --no-delete-branch feature-branch
```

Force-delete an unmerged branch:

```bash
wt remove -D experimental
```

### Branch cleanup

Branches delete automatically when their content is already in the target branch (typically main). This works with squash-merge and rebase workflows where commit history differs but file changes match.

Use `-D` to force-delete unmerged branches. Use `--no-delete-branch` to keep the branch.

### Background removal

Removal runs in the background by default (returns immediately). Logs are written to `.git/wt-logs/{branch}-remove.log`. Use `--no-background` to run in the foreground.

Arguments resolve by path first, then branch name—see [wt switch](@/switch.md#path-first-lookup). Shortcuts: `@` (current), `-` (previous), `^` (main worktree).

### See also

- [wt merge](@/merge.md) — Remove worktree after merging
- [wt list](@/list.md) — View all worktrees

<!-- END AUTO-GENERATED -->

</details>

<details id="wt-list">
<summary><strong><code>wt list</code></strong> - Show all worktrees and branches</summary>

<!-- ⚠️ AUTO-GENERATED from `wt list --help-md` — edit source to update -->

```
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

Show all worktrees with their status. The table includes uncommitted changes, divergence from main and remote, and optional CI status.

The table renders progressively: branch names, paths, and commit hashes appear immediately, then status, divergence, and other columns fill in as background git operations complete. CI status (with `--full`) requires network requests and may take longer.

```

### Examples

List all worktrees:

<!-- wt list -->
```bash
$ wt list
```

Include CI status and conflict detection:

<!-- wt list --full -->
```bash
$ wt list --full
```

Include branches that don't have worktrees:

<!-- wt list --branches --full -->
```bash
$ wt list --branches --full
```

Output as JSON for scripting:

```bash
$ wt list --format=json
```

### Columns

| Column | Shows |
|--------|-------|
| Branch | Branch name |
| Status | Compact symbols (see below) |
| HEAD± | Uncommitted changes: +added -deleted lines |
| main↕ | Commits ahead/behind main |
| main…± | Line diffs in commits ahead of main (`--full`) |
| Path | Worktree directory |
| Remote⇅ | Commits ahead/behind tracking branch |
| CI | Pipeline status (`--full`) |
| Commit | Short hash (8 chars) |
| Age | Time since last commit |
| Message | Last commit message (truncated) |

The CI column shows GitHub/GitLab pipeline status:

| Indicator | Meaning |
|-----------|---------|
| `●` green | All checks passed |
| `●` blue | Checks running |
| `●` red | Checks failed |
| `●` yellow | Merge conflicts with base |
| `●` gray | No checks configured |
| blank | No PR/MR found |

Any CI dot appears dimmed when there are unpushed local changes (stale status).

### Status symbols

Symbols appear in the Status column in this order:

| Category | Symbol | Meaning |
|----------|--------|---------|
| Working tree | `+` | Staged files |
| | `!` | Modified files (unstaged) |
| | `?` | Untracked files |
| | `✖` | Merge conflicts |
| | `↻` | Rebase in progress |
| | `⋈` | Merge in progress |
| Branch state | `⊘` | Would conflict if merged to main (`--full` only) |
| | `≡` | Matches main (identical contents) |
| | `_` | No commits (empty branch) |
| Divergence | `↑` | Ahead of main |
| | `↓` | Behind main |
| | `↕` | Diverged from main |
| Remote | `⇡` | Ahead of remote |
| | `⇣` | Behind remote |
| | `⇅` | Diverged from remote |
| Other | `⎇` | Branch without worktree |
| | `⌫` | Prunable (directory missing) |
| | `⊠` | Locked worktree |

Rows are dimmed when the branch has no marginal contribution (`≡` matches main or `_` no commits).

### JSON output

Query structured data with `--format=json`:

```bash
# Worktrees with conflicts
wt list --format=json | jq '.[] | select(.status.branch_state == "Conflicts")'

# Uncommitted changes
wt list --format=json | jq '.[] | select(.status.working_tree.modified)'

# Current worktree
wt list --format=json | jq '.[] | select(.is_current == true)'

# Branches ahead of main
wt list --format=json | jq '.[] | select(.status.main_divergence == "Ahead")'
```

**Status fields:**
- `working_tree`: `{untracked, modified, staged, renamed, deleted}`
- `branch_state`: `""` | `"Conflicts"` | `"MergeTreeConflicts"` | `"MatchesMain"` | `"NoCommits"`
- `git_operation`: `""` | `"Rebase"` | `"Merge"`
- `main_divergence`: `""` | `"Ahead"` | `"Behind"` | `"Diverged"`
- `upstream_divergence`: `""` | `"Ahead"` | `"Behind"` | `"Diverged"`

**Position fields:**
- `is_main` — Main worktree
- `is_current` — Current directory
- `is_previous` — Previous worktree from [wt switch](@/switch.md)

### See also

- [wt select](@/select.md) — Interactive worktree picker with live preview

<!-- END AUTO-GENERATED -->

</details>

<details id="wt-config">
<summary><strong><code>wt config</code></strong> - Manage configuration</summary>

<!-- ⚠️ AUTO-GENERATED from `wt config --help-md` — edit source to update -->

```
wt config — Manage configuration and shell integration
Usage: wt config [OPTIONS] <COMMAND>

Commands:
  shell      Shell integration setup
  create     Create user configuration file
  show       Show configuration files & locations
  cache      Manage caches (CI status, default branch)
  var        Get or set runtime variables (stored in git config)
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

Manages configuration, shell integration, and runtime settings. The command provides subcommands for setup, inspection, and cache management.

```

### Examples

Install shell integration (required for directory switching):

```bash
wt config shell install
```

Create user config file with documented examples:

```bash
wt config create
```

Show current configuration and file locations:

```bash
wt config show
```

### Shell integration

Shell integration allows Worktrunk to change the shell's working directory after `wt switch`. Without it, commands run in a subprocess and directory changes don't persist.

The `wt config shell install` command adds integration to the shell's config file. Manual installation:

```bash
# For bash: add to ~/.bashrc
eval "$(wt config shell init bash)"

# For zsh: add to ~/.zshrc
eval "$(wt config shell init zsh)"

# For fish: add to ~/.config/fish/config.fish
wt config shell init fish | source
```

### Configuration files

**User config** — `~/.config/worktrunk/config.toml` (or `$WORKTRUNK_CONFIG_PATH`):

Personal settings like LLM commit generation, path templates, and default behaviors. The `wt config create` command generates a file with documented examples.

**Project config** — `.config/wt.toml` in repository root:

Project-specific hooks: post-create, post-start, pre-commit, pre-merge, post-merge. See [Hooks](@/hooks.md) for details.

### LLM commit messages

Worktrunk can generate commit messages using an LLM. Enable in user config:

```toml
[commit-generation]
command = "llm"
```

See [LLM Commits](@/llm-commits.md) for installation, provider setup, and customization.

<!-- END AUTO-GENERATED -->

</details>

<details>
<summary><strong><code>wt step</code></strong> - Building blocks for workflows</summary>

<!-- ⚠️ AUTO-GENERATED from `wt step --help-md` — edit source to update -->

```
wt step — Run individual workflow operations
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
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose
          Show commands and debug info

Run individual workflow operations: commits, squashes, rebases, pushes, and [hooks](@/hooks.md).

```

### Examples

Commit with LLM-generated message:

```bash
wt step commit
```

Run pre-merge hooks in CI:

```bash
wt step pre-merge --force
```

Manual merge workflow with review between steps:

```bash
wt step commit
wt step squash
# Review the squashed commit
wt step rebase
wt step push
```

### Operations

**Git operations:**

- `commit` — Stage and commit with [LLM-generated message](@/llm-commits.md)
- `squash` — Squash all branch commits into one with [LLM-generated message](@/llm-commits.md)
- `rebase` — Rebase onto target branch
- `push` — Push to target branch (default: main)

**Hooks** — run project commands defined in [`.config/wt.toml`](@/hooks.md):

- `post-create` — After worktree creation
- `post-start` — After switching to a worktree
- `pre-commit` — Before committing
- `pre-merge` — Before pushing to target
- `post-merge` — After merge cleanup

### See also

- [wt merge](@/merge.md) — Runs commit → squash → rebase → hooks → push → cleanup automatically

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

```console
🟡 repo needs approval to execute 3 commands:

⚪ post-create install:
   echo 'Installing dependencies...'

⚪ post-create build:
   echo 'Building project...'

⚪ post-create test:
   echo 'Running tests...'

❓ Allow and remember? [y/N]
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

```bash
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

```bash
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

```bash
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

```bash
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

```bash
./dev/update-homebrew.sh
```

This script fetches the new tarball, computes the SHA256, updates the formula, and pushes to homebrew-worktrunk.

</details>
