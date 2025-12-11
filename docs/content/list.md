+++
title = "wt list"
weight = 11

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt list --help-page` — edit cli.rs to update -->

Show all worktrees with their status. The table includes uncommitted changes, divergence from main and remote, and optional CI status.

The table renders progressively: branch names, paths, and commit hashes appear immediately, then status, divergence, and other columns fill in as background git operations complete. With `--full`, CI status fetches from the network — the table displays instantly and CI fills in as results arrive.

## Examples

List all worktrees:

<!-- ⚠️ AUTO-GENERATED from tests/snapshots/integration__integration_tests__list__readme_example_list.snap — edit source to update -->

{% terminal() %}
<span class="prompt">$</span> <span class="cmd">wt list</span>
  <b>Branch</b>       <b>Status</b>        <b>HEAD±</b>    <b>main↕</b>  <b>Path</b>                <b>Remote⇅</b>  <b>Commit</b>    <b>Age</b>   <b>Message</b>
@ <b>feature-api</b>  <span class=c>+</span>   <span class=d>↕</span><span class=d>⇡</span>     <span class=g>+54</span>   <span class=r>-5</span>   <span class=g>↑4</span>  <span class=d><span class=r>↓1</span></span>  <b>./repo.feature-api</b>   <span class=g>⇡3</span>      <span class=d>d35485d7</span>  <span class=d>30m</span>   <span class=d>Add API tests</span>
^ main             <span class=d>^</span><span class=d>⇅</span>                        ./repo               <span class=g>⇡1</span>  <span class=d><span class=r>⇣1</span></span>  <span class=d>e18e1b4d</span>  <span class=d>4d</span>    <span class=d>Merge fix-auth:…</span>
+ fix-auth         <span class=d>↕</span><span class=d>|</span>                <span class=g>↑2</span>  <span class=d><span class=r>↓1</span></span>  ./repo.fix-auth        <span class=d>|</span>     <span class=d>2517d700</span>  <span class=d>5h</span>    <span class=d>Add secure token…</span>

⚪ <span class=d>Showing 3 worktrees, 1 with changes, 2 ahead</span>
{% end %}

<!-- END AUTO-GENERATED -->

Include CI status and line diffs:

<!-- ⚠️ AUTO-GENERATED from tests/snapshots/integration__integration_tests__list__readme_example_list_full.snap — edit source to update -->

{% terminal() %}
<span class="prompt">$</span> <span class="cmd">wt list --full</span>
  <b>Branch</b>       <b>Status</b>        <b>HEAD±</b>    <b>main↕</b>     <b>main…±</b>  <b>Path</b>                <b>Remote⇅</b>  <b>CI</b>  <b>Commit</b>    <b>Age</b>
@ <b>feature-api</b>  <span class=c>+</span>   <span class=d>↕</span><span class=d>⇡</span>     <span class=g>+54</span>   <span class=r>-5</span>   <span class=g>↑4</span>  <span class=d><span class=r>↓1</span></span>  <span class=g>+234</span>  <span class=r>-24</span>  <b>./repo.feature-api</b>   <span class=g>⇡3</span>      <span class=d><span style='color:var(--blue,#00a)'>●</span></span>   <span class=d>d35485d7</span>  <span class=d>30m</span>
^ main             <span class=d>^</span><span class=d>⇅</span>                                   ./repo               <span class=g>⇡1</span>  <span class=d><span class=r>⇣1</span></span>  <span class=g>●</span>   <span class=d>e18e1b4d</span>  <span class=d>4d</span>
+ fix-auth         <span class=d>↕</span><span class=d>|</span>                <span class=g>↑2</span>  <span class=d><span class=r>↓1</span></span>   <span class=g>+25</span>  <span class=r>-11</span>  ./repo.fix-auth        <span class=d>|</span>     <span class=g>●</span>   <span class=d>2517d700</span>  <span class=d>5h</span>

⚪ <span class=d>Showing 3 worktrees, 1 with changes, 2 ahead, 1 column hidden</span>
{% end %}

<!-- END AUTO-GENERATED -->

Include branches that don't have worktrees:

<!-- ⚠️ AUTO-GENERATED from tests/snapshots/integration__integration_tests__list__readme_example_list_branches.snap — edit source to update -->

{% terminal() %}
<span class="prompt">$</span> <span class="cmd">wt list --branches --full</span>
  <b>Branch</b>       <b>Status</b>        <b>HEAD±</b>    <b>main↕</b>     <b>main…±</b>  <b>Path</b>                <b>Remote⇅</b>  <b>CI</b>  <b>Commit</b>    <b>Age</b>
@ <b>feature-api</b>  <span class=c>+</span>   <span class=d>↕</span><span class=d>⇡</span>     <span class=g>+54</span>   <span class=r>-5</span>   <span class=g>↑4</span>  <span class=d><span class=r>↓1</span></span>  <span class=g>+234</span>  <span class=r>-24</span>  <b>./repo.feature-api</b>   <span class=g>⇡3</span>      <span class=d><span style='color:var(--blue,#00a)'>●</span></span>   <span class=d>d35485d7</span>  <span class=d>30m</span>
^ main             <span class=d>^</span><span class=d>⇅</span>                                   ./repo               <span class=g>⇡1</span>  <span class=d><span class=r>⇣1</span></span>  <span class=g>●</span>   <span class=d>e18e1b4d</span>  <span class=d>4d</span>
+ fix-auth         <span class=d>↕</span><span class=d>|</span>                <span class=g>↑2</span>  <span class=d><span class=r>↓1</span></span>   <span class=g>+25</span>  <span class=r>-11</span>  ./repo.fix-auth        <span class=d>|</span>     <span class=g>●</span>   <span class=d>2517d700</span>  <span class=d>5h</span>
  exp             <span class=d>/</span><span class=d>↕</span>                 <span class=g>↑2</span>  <span class=d><span class=r>↓1</span></span>  <span class=g>+137</span>                                        <span class=d>52cad122</span>  <span class=d>2d</span>
  wip             <span class=d>/</span><span class=d>↕</span>                 <span class=g>↑1</span>  <span class=d><span class=r>↓1</span></span>   <span class=g>+33</span>                                        <span class=d>7ca6d817</span>  <span class=d>3d</span>

⚪ <span class=d>Showing 3 worktrees, 2 branches, 1 with changes, 4 ahead, 1 column hidden</span>
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
| main↕ | Commits ahead/behind main |
| main…± | Line diffs in commits ahead of main (`--full`) |
| Path | Worktree directory |
| Remote⇅ | Commits ahead/behind tracking branch |
| CI | Pipeline status (`--full`) |
| Commit | Short hash (8 chars) |
| Age | Time since last commit |
| Message | Last commit message (truncated) |

### CI status

The CI column shows GitHub/GitLab pipeline status:

| Indicator | Meaning |
|-----------|---------|
| <span style='color:#0a0'>●</span> green | All checks passed |
| <span style='color:#00a'>●</span> blue | Checks running |
| <span style='color:#a00'>●</span> red | Checks failed |
| <span style='color:#a60'>●</span> yellow | Merge conflicts with base |
| <span style='color:#888'>●</span> gray | No checks configured |
| (blank) | No upstream or no PR/MR |

CI indicators are clickable links to the PR or pipeline page. Any CI dot appears dimmed when there are unpushed local changes (stale status). PRs/MRs are checked first, then branch workflows/pipelines for branches with an upstream. Local-only branches show blank. Results are cached for 30-60 seconds; use `wt config cache` to view or clear.

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
| | `⚑` | Path doesn't match template |
| | `⊟` | Prunable (directory missing) |
| | `⊞` | Locked worktree |
| Main | `^` | Is the main branch |
| | `✗` | Would conflict if merged to main |
| | `_` | Same commit as main |
| | `⊂` | [Content integrated](@/remove.md#branch-cleanup) |
| | `↕` | Diverged from main |
| | `↑` | Ahead of main |
| | `↓` | Behind main |
| Remote | `\|` | In sync with remote |
| | `⇅` | Diverged from remote |
| | `⇡` | Ahead of remote |
| | `⇣` | Behind remote |

Rows are dimmed when the branch [content is already in main](@/remove.md#branch-cleanup) (`_` same commit or `⊂` content integrated).

## JSON output

Query structured data with `--format=json`:

```bash
# Worktrees with merge conflicts
wt list --format=json | jq '.[] | select(.operation_state == "conflicts")'

# Uncommitted changes
wt list --format=json | jq '.[] | select(.working_tree.modified)'

# Current worktree
wt list --format=json | jq '.[] | select(.is_current)'

# Branches ahead of main
wt list --format=json | jq '.[] | select(.main.ahead > 0)'

# Integrated branches (ready to clean up)
wt list --format=json | jq '.[] | select(.main_state == "integrated" or .main_state == "same_commit")'
```

**Fields:**

| Field | Description |
|-------|-------------|
| `branch` | Branch name (null for detached HEAD) |
| `path` | Worktree path (absent for branches without worktrees) |
| `kind` | `"worktree"` or `"branch"` |
| `commit` | `{sha, short_sha, message, timestamp}` |
| `working_tree` | `{staged, modified, untracked, renamed, deleted, diff, diff_vs_main}` |
| `main_state` | `"is_main"` `"would_conflict"` `"same_commit"` `"integrated"` `"diverged"` `"ahead"` `"behind"` |
| `integration_reason` | `"ancestor"` `"trees_match"` `"no_added_changes"` `"merge_adds_nothing"` (when `main_state == "integrated"`) |
| `operation_state` | `"conflicts"` `"rebase"` `"merge"` (absent when no operation in progress) |
| `main` | `{ahead, behind, diff}` (absent when `is_main`) |
| `remote` | `{name, branch, ahead, behind}` (absent when no tracking branch) |
| `worktree` | `{state, reason, detached, bare}` |
| `is_main` | Main worktree |
| `is_current` | Current worktree |
| `is_previous` | Previous worktree from [wt switch](@/switch.md) |
| `pr` | `{ci, source, stale, url}` — CI status from PR or branch (absent when no CI) |
| `statusline` | Pre-formatted status with ANSI colors |
| `symbols` | Raw status symbols without colors (e.g., `"!?↓"`) |

## See also

- [wt select](@/select.md) — Interactive worktree picker with live preview

---

## Command reference

```
wt list - List worktrees and optionally branches

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
          Show CI and main diffstat

      --progressive
          Show fast info immediately, update with slow info

          Displays local data (branches, paths, status) first, then updates with
          remote data (CI, upstream) as it arrives. Auto-enabled for TTY.

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

<!-- END AUTO-GENERATED from `wt list --help-page` -->
