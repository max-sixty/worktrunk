+++
title = "wt list"
weight = 11

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt list --help-page` — edit cli.rs to update -->

List worktrees and their status

Shows uncommitted changes, divergence from the default branch and remote, and optional CI status.

<figure class="demo">
<picture>
  <source srcset="/assets/docs/dark/wt-list.gif" media="(prefers-color-scheme: dark)">
  <img src="/assets/docs/light/wt-list.gif" alt="wt list demo" width="1600" height="900">
</picture>
</figure>

The table renders progressively: branch names, paths, and commit hashes appear immediately, then status, divergence, and other columns fill in as background git operations complete. With `--full`, CI status fetches from the network — the table displays instantly and CI fills in as results arrive.

## Examples

List all worktrees:

<!-- ⚠️ AUTO-GENERATED from tests/snapshots/integration__integration_tests__list__readme_example_list.snap — edit source to update -->

{% terminal() %}
<span class="prompt">$</span> <span class="cmd">wt list</span>
  <b>Branch</b>       <b>Status</b>        <b>HEAD±</b>    <b>main↕</b>  <b>Remote⇅</b>  <b>Commit</b>    <b>Age</b>   <b>Message</b>
@ feature-api  <span class=c>+</span>   <span class=d>↕</span><span class=d>⇡</span>     <span class=g>+54</span>   <span class=r>-5</span>   <span class=g>↑4</span>  <span class=d><span class=r>↓1</span></span>   <span class=g>⇡3</span>      <span class=d>6814f02a</span>  <span class=d>30m</span>   <span class=d>Add API tests</span>
^ main             <span class=d>^</span><span class=d>⇅</span>                         <span class=g>⇡1</span>  <span class=d><span class=r>⇣1</span></span>  <span class=d>41ee0834</span>  <span class=d>4d</span>    <span class=d>Merge fix-auth: hardened to…</span>
+ fix-auth         <span class=d>↕</span><span class=d>|</span>                <span class=g>↑2</span>  <span class=d><span class=r>↓1</span></span>     <span class=d>|</span>     <span class=d>b772e68b</span>  <span class=d>5h</span>    <span class=d>Add secure token storage</span>

<span class=d>○</span> <span class=d>Showing 3 worktrees, 1 with changes, 2 ahead, 1 column hidden</span>
{% end %}

<!-- END AUTO-GENERATED -->

Include CI status and line diffs:

<!-- ⚠️ AUTO-GENERATED from tests/snapshots/integration__integration_tests__list__readme_example_list_full.snap — edit source to update -->

{% terminal() %}
<span class="prompt">$</span> <span class="cmd">wt list --full</span>
  <b>Branch</b>       <b>Status</b>        <b>HEAD±</b>    <b>main↕</b>     <b>main…±</b>  <b>Remote⇅</b>  <b>CI</b>  <b>Commit</b>    <b>Age</b>   <b>Message</b>
@ feature-api  <span class=c>+</span>   <span class=d>↕</span><span class=d>⇡</span>     <span class=g>+54</span>   <span class=r>-5</span>   <span class=g>↑4</span>  <span class=d><span class=r>↓1</span></span>  <span class=g>+234</span>  <span class=r>-24</span>   <span class=g>⇡3</span>      <span class=d><span style='color:var(--blue,#00a)'>●</span></span>   <span class=d>6814f02a</span>  <span class=d>30m</span>   <span class=d>Add API tests</span>
^ main             <span class=d>^</span><span class=d>⇅</span>                                    <span class=g>⇡1</span>  <span class=d><span class=r>⇣1</span></span>  <span class=g>●</span>   <span class=d>41ee0834</span>  <span class=d>4d</span>    <span class=d>Merge fix-au…</span>
+ fix-auth         <span class=d>↕</span><span class=d>|</span>                <span class=g>↑2</span>  <span class=d><span class=r>↓1</span></span>   <span class=g>+25</span>  <span class=r>-11</span>     <span class=d>|</span>     <span class=g>●</span>   <span class=d>b772e68b</span>  <span class=d>5h</span>    <span class=d>Add secure t…</span>

<span class=d>○</span> <span class=d>Showing 3 worktrees, 1 with changes, 2 ahead, 1 column hidden</span>
{% end %}

<!-- END AUTO-GENERATED -->

Include branches that don't have worktrees:

<!-- ⚠️ AUTO-GENERATED from tests/snapshots/integration__integration_tests__list__readme_example_list_branches.snap — edit source to update -->

{% terminal() %}
<span class="prompt">$</span> <span class="cmd">wt list --branches --full</span>
  <b>Branch</b>       <b>Status</b>        <b>HEAD±</b>    <b>main↕</b>     <b>main…±</b>  <b>Remote⇅</b>  <b>CI</b>  <b>Commit</b>    <b>Age</b>   <b>Message</b>
@ feature-api  <span class=c>+</span>   <span class=d>↕</span><span class=d>⇡</span>     <span class=g>+54</span>   <span class=r>-5</span>   <span class=g>↑4</span>  <span class=d><span class=r>↓1</span></span>  <span class=g>+234</span>  <span class=r>-24</span>   <span class=g>⇡3</span>      <span class=d><span style='color:var(--blue,#00a)'>●</span></span>   <span class=d>6814f02a</span>  <span class=d>30m</span>   <span class=d>Add API tests</span>
^ main             <span class=d>^</span><span class=d>⇅</span>                                    <span class=g>⇡1</span>  <span class=d><span class=r>⇣1</span></span>  <span class=g>●</span>   <span class=d>41ee0834</span>  <span class=d>4d</span>    <span class=d>Merge fix-au…</span>
+ fix-auth         <span class=d>↕</span><span class=d>|</span>                <span class=g>↑2</span>  <span class=d><span class=r>↓1</span></span>   <span class=g>+25</span>  <span class=r>-11</span>     <span class=d>|</span>     <span class=g>●</span>   <span class=d>b772e68b</span>  <span class=d>5h</span>    <span class=d>Add secure t…</span>
  exp             <span class=d>/</span><span class=d>↕</span>                 <span class=g>↑2</span>  <span class=d><span class=r>↓1</span></span>  <span class=g>+137</span>                    <span class=d>96379229</span>  <span class=d>2d</span>    <span class=d>Add GraphQL…</span>
  wip             <span class=d>/</span><span class=d>↕</span>                 <span class=g>↑1</span>  <span class=d><span class=r>↓1</span></span>   <span class=g>+33</span>                    <span class=d>b40716dc</span>  <span class=d>3d</span>    <span class=d>Start API do…</span>

<span class=d>○</span> <span class=d>Showing 3 worktrees, 2 branches, 1 with changes, 4 ahead, 1 column hidden</span>
{% end %}

<!-- END AUTO-GENERATED -->

Output as JSON for scripting:

```bash
$ wt list --format=json
```

## Columns

| Column | Shows |
|--------|-------|
| Branch | Branch name |
| Status | Compact symbols (see below) |
| HEAD± | Uncommitted changes: +added -deleted lines |
| main↕ | Commits ahead/behind default branch |
| main…± | Line diffs since the merge-base with the default branch (`--full`) |
| Path | Worktree directory |
| Remote⇅ | Commits ahead/behind tracking branch |
| URL | Dev server URL from project config (dimmed if port not listening) |
| CI | Pipeline status (`--full`) |
| Commit | Short hash (8 chars) |
| Age | Time since last commit |
| Message | Last commit message (truncated) |

Note: `main↕` and `main…±` refer to the default branch (header label stays `main` for compactness). `main…±` uses a merge-base (three-dot) diff.

### CI status

The CI column shows GitHub/GitLab pipeline status:

| Indicator | Meaning |
|-----------|---------|
| <span style='color:#0a0'>●</span> green | All checks passed |
| <span style='color:#00a'>●</span> blue | Checks running |
| <span style='color:#a00'>●</span> red | Checks failed |
| <span style='color:#a60'>●</span> yellow | Merge conflicts with base |
| <span style='color:#888'>●</span> gray | No checks configured |
| <span style='color:#a60'>⚠</span> yellow | Fetch error (rate limit, network) |
| (blank) | No upstream or no PR/MR |

CI indicators are clickable links to the PR or pipeline page. Any CI dot appears dimmed when there are unpushed local changes (stale status). PRs/MRs are checked first, then branch workflows/pipelines for branches with an upstream. Local-only branches show blank. Results are cached for 30-60 seconds; use `wt config state` to view or clear.

## Status symbols

The Status column has multiple subcolumns. Within each, only the first matching symbol is shown (listed in priority order):

| Subcolumn | Symbol | Meaning |
|-----------|--------|---------|
| Working tree (1) | `+` | Staged files |
| Working tree (2) | `!` | Modified files (unstaged) |
| Working tree (3) | `?` | Untracked files |
| Worktree | `✘` | Merge conflicts |
| | `⤴` | Rebase in progress |
| | `⤵` | Merge in progress |
| | `/` | Branch without worktree |
| | `⚑` | Branch-worktree mismatch (branch name doesn't match worktree path) |
| | `⊟` | Prunable (directory missing) |
| | `⊞` | Locked worktree |
| Default branch | `^` | Is the default branch |
| | `∅` | Orphan branch (no common ancestor with the default branch) |
| | `✗` | Would conflict if merged to the default branch (with `--full`, includes uncommitted changes) |
| | `_` | Same commit as the default branch, clean |
| | `–` | Same commit as the default branch, uncommitted changes |
| | `⊂` | Content [integrated](@/remove.md#branch-cleanup) into the default branch or target |
| | `↕` | Diverged from the default branch |
| | `↑` | Ahead of the default branch |
| | `↓` | Behind the default branch |
| Remote | `\|` | In sync with remote |
| | `⇅` | Diverged from remote |
| | `⇡` | Ahead of remote |
| | `⇣` | Behind remote |

Rows are dimmed when [safe to delete](@/remove.md#branch-cleanup) (`_` same commit with clean working tree or `⊂` content integrated).

---

## JSON output

Query structured data with `--format=json`:

```bash
# Current worktree path (for scripts)
wt list --format=json | jq -r '.[] | select(.is_current) | .path'

# Branches with uncommitted changes
wt list --format=json | jq '.[] | select(.working_tree.modified)'

# Worktrees with merge conflicts
wt list --format=json | jq '.[] | select(.operation_state == "conflicts")'

# Branches ahead of main (needs merging)
wt list --format=json | jq '.[] | select(.main.ahead > 0) | .branch'

# Integrated branches (safe to remove)
wt list --format=json | jq '.[] | select(.main_state == "integrated" or .main_state == "empty") | .branch'

# Branches without worktrees
wt list --format=json --branches | jq '.[] | select(.kind == "branch") | .branch'

# Worktrees ahead of remote (needs pushing)
wt list --format=json | jq '.[] | select(.remote.ahead > 0) | {branch, ahead: .remote.ahead}'

# Stale CI (local changes not reflected in CI)
wt list --format=json --full | jq '.[] | select(.ci.stale) | .branch'
```

**Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `branch` | string/null | Branch name (null for detached HEAD) |
| `path` | string | Worktree path (absent for branches without worktrees) |
| `kind` | string | `"worktree"` or `"branch"` |
| `commit` | object | Commit info (see below) |
| `working_tree` | object | Working tree state (see below) |
| `main_state` | string | Relation to the default branch (see below) |
| `integration_reason` | string | Why branch is integrated (see below) |
| `operation_state` | string | `"conflicts"`, `"rebase"`, or `"merge"` (absent when clean) |
| `main` | object | Relationship to the default branch (see below, absent when is_main) |
| `remote` | object | Tracking branch info (see below, absent when no tracking) |
| `worktree` | object | Worktree metadata (see below) |
| `is_main` | boolean | Is the main worktree |
| `is_current` | boolean | Is the current worktree |
| `is_previous` | boolean | Previous worktree from wt switch |
| `ci` | object | CI status (see below, absent when no CI) |
| `url` | string | Dev server URL from project config (absent when not configured) |
| `url_active` | boolean | Whether the URL's port is listening (absent when not configured) |
| `statusline` | string | Pre-formatted status with ANSI colors |
| `symbols` | string | Raw status symbols without colors (e.g., `"!?↓"`) |

### Commit object

| Field | Type | Description |
|-------|------|-------------|
| `sha` | string | Full commit SHA (40 chars) |
| `short_sha` | string | Short commit SHA (7 chars) |
| `message` | string | Commit message (first line) |
| `timestamp` | number | Unix timestamp |

### working_tree object

| Field | Type | Description |
|-------|------|-------------|
| `staged` | boolean | Has staged files |
| `modified` | boolean | Has modified files (unstaged) |
| `untracked` | boolean | Has untracked files |
| `renamed` | boolean | Has renamed files |
| `deleted` | boolean | Has deleted files |
| `diff` | object | Lines changed vs HEAD: `{added, deleted}` |

### main object

| Field | Type | Description |
|-------|------|-------------|
| `ahead` | number | Commits ahead of the default branch |
| `behind` | number | Commits behind the default branch |
| `diff` | object | Lines changed vs the default branch: `{added, deleted}` |

### remote object

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Remote name (e.g., `"origin"`) |
| `branch` | string | Remote branch name |
| `ahead` | number | Commits ahead of remote |
| `behind` | number | Commits behind remote |

### worktree object

| Field | Type | Description |
|-------|------|-------------|
| `state` | string | `"no_worktree"`, `"branch_worktree_mismatch"`, `"prunable"`, `"locked"` (absent when normal) |
| `reason` | string | Reason for locked/prunable state |
| `detached` | boolean | HEAD is detached |

### ci object

| Field | Type | Description |
|-------|------|-------------|
| `status` | string | CI status (see below) |
| `source` | string | `"pr"` (PR/MR) or `"branch"` (branch workflow) |
| `stale` | boolean | Local HEAD differs from remote (unpushed changes) |
| `url` | string | URL to the PR/MR page |

### main_state values

These values describe relation to the default branch.

`"is_main"` `"orphan"` `"would_conflict"` `"empty"` `"same_commit"` `"integrated"` `"diverged"` `"ahead"` `"behind"`

### integration_reason values

When `main_state == "integrated"`: `"ancestor"` `"trees_match"` `"no_added_changes"` `"merge_adds_nothing"`

### ci.status values

`"passed"` `"running"` `"failed"` `"conflicts"` `"no-ci"` `"error"`

Missing a field that would be generally useful? [Open an issue](https://github.com/max-sixty/worktrunk/issues).

## See also

- [`wt select`](@/select.md) — Interactive worktree picker with live preview

## Command reference

{% terminal() %}
wt list - List worktrees and their status

Usage: <b><span class=c>wt list</span></b> <span class=c>[OPTIONS]</span>
       <b><span class=c>wt list</span></b> <span class=c>&lt;COMMAND&gt;</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>statusline</span></b>  Single-line status for shell prompts

<b><span class=g>Options:</span></b>
      <b><span class=c>--format</span></b><span class=c> &lt;FORMAT&gt;</span>
          Output format (table, json)

          [default: table]

      <b><span class=c>--branches</span></b>
          Include branches without worktrees

      <b><span class=c>--remotes</span></b>
          Include remote branches

      <b><span class=c>--full</span></b>
          Include CI status and diff analysis (slower)

      <b><span class=c>--progressive</span></b>
          Show fast info immediately, update with slow info

          Displays local data (branches, paths, status) first, then updates with
          remote data (CI, upstream) as it arrives. Auto-enabled for TTY.

  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Show debug info (-v), or also write diagnostic report (-vv)
{% end %}

<!-- END AUTO-GENERATED from `wt list --help-page` -->
