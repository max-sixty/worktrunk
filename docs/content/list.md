+++
title = "wt list"
description = "List worktrees and their status."
weight = 11

[extra]
group = "Commands"
+++

<!-- ⚠️ AUTO-GENERATED from `wt list --help-page` — edit src/cli/mod.rs to update -->

List worktrees and their status.

Shows uncommitted changes, divergence from the default branch and remote, and optional CI status and LLM summaries.

<figure class="demo">
<picture>
  <source srcset="/assets/docs/dark/wt-list.gif" media="(prefers-color-scheme: dark)">
  <img src="/assets/docs/light/wt-list.gif" alt="wt list demo" width="1600" height="900">
</picture>
</figure>

The table renders progressively: branch names, paths, and commit hashes appear immediately, then status, divergence, and other columns fill in as background git operations complete.

## Full mode

`--full` adds the two columns that reach off-machine: [CI status](#ci-status) (GitHub/GitLab pipeline pass/fail, over the network) and [LLM-generated summaries](#llm-summaries) of each branch's changes. The `main…±` line diffs are local git, so they show by default.

## Examples

List all worktrees:

{% terminal(cmd="wt list") %}
&#32;&#32;<b>Branch</b>       <b>Status</b>        <b>HEAD±</b>    <b>main↕</b>     <b>main…±</b>  <b>Remote⇅</b>  <b>Commit</b>    <b>Age</b>   <b>Message</b>
@ feature-api  <span class=c>+</span>   <span class=d>↕</span><span class=d>⇡</span>     <span class=g>+54</span>   <span class=r>-5</span>   <span class=g>↑4</span>  <span class=d><span class=r>↓1</span></span>  <span class=g>+234</span>  <span class=r>-24</span>   <span class=g>⇡3</span>      <span class=d>6814f02a</span>  <span class=d>30m</span>   <span class=d>Add API tests</span>
^ main             <span class=d>^</span><span class=d>⇅</span>                                    <span class=g>⇡1</span>  <span class=d><span class=r>⇣1</span></span>  <span class=d>41ee0834</span>  <span class=d>4d</span>    <span class=d>Merge fix-auth:…</span>
+ fix-auth         <span class=d>↕</span><span class=d>|</span>                <span class=g>↑2</span>  <span class=d><span class=r>↓1</span></span>   <span class=g>+25</span>  <span class=r>-11</span>     <span class=d>|</span>     <span class=d>b772e68b</span>  <span class=d>5h</span>    <span class=d>Add secure token…</span>
+ <span class=d>fix-typos</span>        <span class=d>_</span><span class=d>|</span>                                      <span class=d>|</span>     <span class=d>41ee0834</span>  <span class=d>4d</span>    <span class=d>Merge fix-auth:…</span>

<span class=d>○</span> <span class=d>Showing 4 worktrees, 1 with changes, 2 ahead, 1 column hidden</span>
{% end %}

Include CI status and LLM summaries:

{% terminal(cmd="wt list --full") %}
&#32;&#32;<b>Branch</b>       <b>Status</b>        <b>HEAD±</b>    <b>main↕</b>     <b>main…±</b>  <b>Summary</b>                                                <b>Remote⇅</b>  <b>CI</b>    <b>Commit</b>
@ feature-api  <span class=c>+</span>   <span class=d>↕</span><span class=d>⇡</span>     <span class=g>+54</span>   <span class=r>-5</span>   <span class=g>↑4</span>  <span class=d><span class=r>↓1</span></span>  <span class=g>+234</span>  <span class=r>-24</span>  Refactor API to REST architecture with middleware       <span class=g>⇡3</span>      <span class=d><span style='color:var(--blue,#00a)'>#412</span></span>  <span class=d>6814f02a</span>
^ main             <span class=d>^</span><span class=d>⇅</span>                                                                                           <span class=g>⇡1</span>  <span class=d><span class=r>⇣1</span></span>  <span class=g>#</span>     <span class=d>41ee0834</span>
+ fix-auth         <span class=d>↕</span><span class=d>|</span>                <span class=g>↑2</span>  <span class=d><span class=r>↓1</span></span>   <span class=g>+25</span>  <span class=r>-11</span>  Harden auth with constant-time token validation           <span class=d>|</span>     <span class=g>#408</span>  <span class=d>b772e68b</span>
+ <span class=d>fix-typos</span>        <span class=d>_</span><span class=d>|</span>                                                                                             <span class=d>|</span>     <span class=g>#410</span>  <span class=d>41ee0834</span>

<span class=d>○</span> <span class=d>Showing 4 worktrees, 1 with changes, 2 ahead, 3 columns hidden</span>
{% end %}

Include branches that don't have worktrees:

{% terminal(cmd="wt list --branches --full") %}
&#32;&#32;<b>Branch</b>       <b>Status</b>        <b>HEAD±</b>    <b>main↕</b>     <b>main…±</b>  <b>Summary</b>                                                <b>Remote⇅</b>  <b>CI</b>    <b>Commit</b>
@ feature-api  <span class=c>+</span>   <span class=d>↕</span><span class=d>⇡</span>     <span class=g>+54</span>   <span class=r>-5</span>   <span class=g>↑4</span>  <span class=d><span class=r>↓1</span></span>  <span class=g>+234</span>  <span class=r>-24</span>  Refactor API to REST architecture with middleware       <span class=g>⇡3</span>      <span class=d><span style='color:var(--blue,#00a)'>#412</span></span>  <span class=d>6814f02a</span>
^ main             <span class=d>^</span><span class=d>⇅</span>                                                                                           <span class=g>⇡1</span>  <span class=d><span class=r>⇣1</span></span>  <span class=g>#</span>     <span class=d>41ee0834</span>
+ fix-auth         <span class=d>↕</span><span class=d>|</span>                <span class=g>↑2</span>  <span class=d><span class=r>↓1</span></span>   <span class=g>+25</span>  <span class=r>-11</span>  Harden auth with constant-time token validation           <span class=d>|</span>     <span class=g>#408</span>  <span class=d>b772e68b</span>
+ <span class=d>fix-typos</span>        <span class=d>_</span><span class=d>|</span>                                                                                             <span class=d>|</span>     <span class=g>#410</span>  <span class=d>41ee0834</span>
<span class=d>/ </span>exp             <span class=d>/</span><span class=d>↕</span>                 <span class=g>↑2</span>  <span class=d><span class=r>↓1</span></span>  <span class=g>+137</span>       Explore GraphQL schema and resolvers                                  <span class=d>96379229</span>
<span class=d>/ </span>wip             <span class=d>/</span><span class=d>↕</span>                 <span class=g>↑1</span>  <span class=d><span class=r>↓1</span></span>   <span class=g>+33</span>       Start API documentation                                               <span class=d>b40716dc</span>

<span class=d>○</span> <span class=d>Showing 4 worktrees, 2 branches, 1 with changes, 4 ahead, 3 columns hidden</span>
{% end %}

Output as JSON for scripting:

{{ terminal(cmd="wt list --format=json") }}

## Columns

| Column | Shows |
|--------|-------|
| Branch | Branch name |
| Status | Compact symbols (see below) |
| HEAD± | Uncommitted changes: +added -deleted lines |
| main↕ | Commits ahead/behind default branch |
| main…± | Line diffs since the merge-base (three-dot) with the default branch |
| Summary | LLM-generated branch summary; requires `--full`, `summary = true`, and [`commit.generation`](@/config.md#commit) <span class="badge-experimental"></span> |
| Remote⇅ | Commits ahead/behind tracking branch |
| CI | PR/MR number colored by pipeline status; `--full` only |
| Path | Worktree directory |
| URL | Dev server URL from project config; dimmed if port is not listening |
| *(custom)* | User-defined [custom columns](#custom-columns) from `[list.custom-columns]` user config <span class="badge-experimental"></span> |
| Commit | Short hash (8 chars) |
| Age | Time since last commit |
| Message | Last commit message (truncated) |

The `main` header label is used regardless of the default branch's actual name.

`main↕` and `main…±` measure against the default branch's upstream tip when the local copy lags it — so in a fork whose local `main` trails `origin/main`, a branch reads as ahead of the real mainline, not of a stale local checkout. The `↑`/`↓`/`↕` Status symbols derive from these counts, so they track the upstream tip too.

### Gutter

The leftmost column marks each row by physical presence, from most present to least:

| Symbol | Meaning |
|--------|---------|
| `@` | Current worktree |
| `^` | Primary worktree (the repo's home worktree) |
| `+` | Other worktree |
| `/` | Local branch without a worktree (`--branches`) |
| `\|` | Remote branch, not present locally until fetched (`--remotes`) |

### CI status

The CI column shows the branch's open PR/MR — `#3035` on GitHub, Gitea, and Azure DevOps, `!3035` on GitLab — colored by pipeline status, or a bare `#` when no number is available (e.g. branch workflows without a PR/MR). One color folds two JSON fields: green/blue/red/yellow/gray are `ci.status`; magenta/cyan are `ci.review_state`. The `Value` column is the matching JSON string from `--format=json`:

| Indicator | Value | Meaning |
|-----------|-------|---------|
| <span style='color:#0a0'>#</span> green | `"passed"` | All checks passed |
| <span style='color:#00a'>#</span> blue | `"running"` | Checks in progress |
| <span style='color:#a00'>#</span> red | `"failed"` | One or more checks failed |
| <span style='color:#a60'>#</span> yellow | `"conflicts"` | Merge conflicts with the target branch |
| <span style='color:#888'>#</span> gray | `"no-ci"` | No PR/MR, or no checks configured |
| <span style='color:#a60'>⚠</span> yellow | `"error"` | CI status could not be fetched (rate limit, network, etc.) |
| <span style='color:#a0a'>#</span> magenta | `"changes_requested"` | A reviewer requested changes |
| <span style='color:#0aa'>#</span> cyan | `"pending"` | A review is required (e.g. branch protection) but not yet given |
| (blank) | `ci` absent | No upstream, or no PR/MR and no branch workflow |

The two remaining `ci.review_state` values have no indicator of their own: `"draft"` only dims the cell and `"approved"` leaves the color unchanged.

Color precedence resolves the fold: changes-requested (magenta) outranks running checks — waiting can't clear it — while an outstanding required review (cyan) only recolors an otherwise green or quiet branch. Cool colors mean waiting, warm colors mean act. An approved PR, or one with no review signal at all (no required reviewers and no reviews), keeps its plain `ci.status` color — `ci.review_state` is then `"approved"` or absent, respectively. GitLab MR data carries only `"pending"` and `"draft"` — no approved or changes-requested signal.

CI cells are clickable links to the PR or pipeline page, and appear dimmed for a draft PR/MR (`"draft"`) or when unpushed local changes make the status stale (`ci.stale`). PRs/MRs are checked first, then branch workflows/pipelines for branches with an upstream. Local-only branches show blank; remote-only branches — visible with `--remotes` — get CI status detection. Results are cached for 30-60 seconds; use `wt config state` to view or clear.

### LLM summaries

<span class="badge-experimental"></span>

Reuses the [`commit.generation`](@/config.md#commit) command — the same LLM that generates commit messages. Enable with `summary = true` in `[list]` config; requires `--full`. Results are cached until the branch's diff changes.

### Custom columns

<span class="badge-experimental"></span>

Each `[list.custom-columns]` entry in user config adds a column: the key is the header, the template renders each row's cell. Templates read two per-branch namespaces — `{{ vars.* }}`, stored with [`wt config state vars set`](@/config.md#wt-config-state-vars), and `{{ git.branch.* }}`, the branch's own git config under `branch.<name>.*` (a `jira` key you set yourself, or the git-native `description`) — useful for tracking what each of many (often agent-driven) branches is for:

```toml
[list.custom-columns.Ticket]
template = "{{ vars.ticket }}"
```

A column that renders empty for every row is dropped from the table. Templates, widths, and drop priority: [custom columns config](@/config.md#custom-columns).

## Status symbols

The Status column packs several subcolumns, left to right, each mapping to a field in `--format=json`. Working-tree flags are independent and co-occur — any combination shows at once. The other subcolumns are mutually exclusive: each shows a single symbol, the highest-priority state in top-to-bottom table order, and is blank when nothing applies.

### Working tree

Independent flags from `git status`; several can show at once (e.g. `+!?`). Each maps to a boolean in the `working_tree` object:

| Symbol | working_tree | Meaning |
|--------|--------------|---------|
| `+` | `staged` | Staged files |
| `!` | `modified` | Modified files (unstaged) |
| `?` | `untracked` | Untracked files |

`working_tree` also reports `renamed` and `deleted`, which have no dedicated symbol in the column.

### Worktree

An in-progress git operation, a worktree-location attribute, or a branch with no worktree. One symbol shows, highest priority first (`✘ > ⤴ > ⤵ > ⊟ > ⊞ > ⚑ > /`):

| Symbol | JSON | Meaning |
|--------|------|---------|
| `✘` | `operation_state` `"conflicts"` | Merge conflicts |
| `⤴` | `operation_state` `"rebase"` | Rebase in progress |
| `⤵` | `operation_state` `"merge"` | Merge in progress |
| `⊟` | `worktree.state` `"prunable"` | Prunable (worktree directory missing) |
| `⊞` | `worktree.state` `"locked"` | Locked worktree |
| `⚑` | `worktree.state` `"branch_worktree_mismatch"` | Branch name doesn't match the worktree path |
| `/` | `kind` `"branch"` | Branch without a worktree (no `worktree` object) |

### Default branch

The single highest-priority state describing the branch's relation to the default branch; blank when none applies (a normal up-to-date branch). Each symbol is one `main_state` value:

| Symbol | main_state | Meaning |
|--------|------------|---------|
| `^` | `"is_main"` | The main worktree (the repo's home worktree) |
| `∅` | `"orphan"` | No common ancestor with the default branch |
| `_` | `"empty"` | Same commit as the default branch, working tree clean — safe to remove; row dimmed |
| `⊂` | `"integrated"` | Content [integrated](@/remove.md#branch-cleanup) into the default branch or merge target via different history; the matching check is in `integration_reason`; row dimmed |
| `✗` | `"would_conflict"` | Merging into the default branch would conflict (simulated with `git merge-tree`) and the branch isn't already integrated; with `--full`, the check includes uncommitted changes |
| `–` | `"same_commit"` | Same commit as the default branch, but with uncommitted changes |
| `↕` | `"diverged"` | Both ahead of and behind the default branch |
| `↑` | `"ahead"` | Has commits the default branch doesn't |
| `↓` | `"behind"` | Missing commits the default branch has |

Rows are dimmed when [safe to delete](@/remove.md#branch-cleanup) — `_` (`"empty"`) or `⊂` (`"integrated"`).

### Remote

Relation to the tracking branch, derived from the `remote.ahead` / `remote.behind` counts; blank when there is no upstream:

| Symbol | remote | Meaning |
|--------|--------|---------|
| `\|` | `ahead` 0, `behind` 0 | In sync with remote |
| `⇡` | `ahead` > 0 | Ahead of remote |
| `⇣` | `behind` > 0 | Behind remote |
| `⇅` | `ahead` > 0, `behind` > 0 | Diverged from remote |

### Placeholder symbols

These appear across all columns while the table is loading:

| Symbol | Meaning |
|--------|---------|
| `·` | Data is loading, or collection timed out / branch too stale |

---

## JSON output

`--format=json` emits structured data in one of two schemas while the format
migrates: `[list] json-schema = 2` selects the envelope format below, `= 1`
the original bare-array format. Unset emits schema 1 with a warning
(`wt config update` pins `= 1`); a future release flips the default to
schema 2 and later removes schema 1.

### Schema 2

One envelope object. Items carry independent facts; rendered strings
(including the collapsed Status value) live under `display`:

```json
{
  "schema": 2,
  "repo": {
    "default_branch": "main",
    "forge": {"url": "https://github.com/org/repo", "provider": "github",
              "host": "github.com", "owner": "org", "name": "repo", "remote": "origin"}
  },
  "collected": {"ci": false, "summary": false},
  "items": [
    {
      "branch": "feature",
      "head": {"sha": "05a4a45d…", "short_sha": "05a4a45", "subject": "Add login page",
               "committed_at": "2025-01-01T08:00:00Z"},
      "worktree": {"path": "/home/user/repo.feature", "main": false, "current": true,
                   "previous": false, "detached": false, "branch_mismatch": false,
                   "changes": {"staged": false, "modified": true, "untracked": false,
                               "renamed": false, "deleted": false, "conflicted": false,
                               "diff": {"added": 10, "deleted": 2}}},
      "default_branch": {"ahead": 3, "behind": 1, "diff": {"added": 50, "deleted": 20},
                         "orphan": false, "integration": null, "merge_conflicts": false},
      "upstream": {"remote": "origin", "branch": "feature", "ahead": 0, "behind": 2},
      "display": {"state": "diverged", "symbols": "!↕", "statusline": "feature …"}
    }
  ]
}
```

How "no value" reads:

- **Absent** — nothing to report: not applicable (`worktree` on a branch-only
  row), not requested this run (the envelope's `collected` records what was),
  or determined-empty (no PR, no lock, not integrated).
- **`null`** — requested but not determined: a task timed out, the branch was
  too stale for the expensive checks, or a forge fetch failed. This is the
  JSON form of the table's `·` placeholder.

jq treats absent and `null` identically in path expressions, so filters need
no null checks; `has()` distinguishes the two when it matters.

Item fields:

| Field | Description |
|-------|-------------|
| `branch` | Branch name; null for a detached-HEAD worktree. Remote rows carry the bare name with the remote in `remote` |
| `remote` | Remote name, present only on remote-only branch rows |
| `head` | `{sha, short_sha, subject, committed_at}`; null for unborn branches. `committed_at` is RFC 3339 UTC |
| `worktree` | `{path, main, current, previous, detached, locked, prunable, branch_mismatch, operation, changes}`; absent on branch-only rows. `locked`/`prunable` are `{reason}` objects and can co-occur; `operation` is `"rebase"` or `"merge"`; `changes` holds the five working-tree flags plus `conflicted` and `diff {added, deleted}` |
| `default_branch` | Relation to the default branch: `{ahead, behind, diff, orphan, integration, merge_conflicts}`; absent on the default branch itself. `integration.reason` is one of `same_commit`, `ancestor`, `no_added_changes`, `trees_match`, `merge_adds_nothing`, `patch_id_match`; a dirty tree skips the checks, leaving `integration` null |
| `upstream` | Tracking branch: `{remote, branch, ahead, behind}`; absent when none is configured |
| `pr` | Open PR/MR: `{number, url, review, mergeable, repo}`; collected with `--full` or a listed `ci` column. `review` uses the schema 1 `ci.review_state` vocabulary; `mergeable` is false when the forge reports conflicts, null otherwise |
| `checks` | CI pipeline: `{status, source, stale}`; `status` is `passed`, `running`, or `failed` — null when a conflicts report masks it |
| `dev_server` | `{url, listening}` from the project's `list.url` template |
| `summary` | LLM branch summary (requires `[list] summary = true`) |
| `vars` | Per-branch variables from [`wt config state vars`](@/config.md#wt-config-state-vars) |
| `display` | Rendered strings: `state` (schema 1's `main_state` vocabulary), `symbols`, `statusline` (with ANSI colors), `columns` (custom-column cells keyed by header) |

Schema 1 names map directly: `commit` → `head`, `working_tree` →
`worktree.changes`, `main` + `main_state` → `default_branch` +
`display.state`, `remote` → `upstream`, `ci` → `pr` + `checks`, `url` +
`url_active` → `dev_server`, `statusline`/`symbols`/`columns` → `display.*`,
and the per-item `repo` moves to the envelope's `repo.forge`.

{{ terminal(cmd="# Current worktree path (for scripts)|||wt list --format=json | jq -r '.items[] | select(.worktree.current) | .worktree.path'||||||# Branches with uncommitted changes|||wt list --format=json | jq '.items[] | select(.worktree.changes.modified)'||||||# Integrated branches (safe to remove)|||wt list --format=json | jq '.items[] | select(.display.state == __WT_QUOT__integrated__WT_QUOT__ or .display.state == __WT_QUOT__empty__WT_QUOT__) | .branch'||||||# Worktrees ahead of upstream (needs pushing)|||wt list --format=json | jq '.items[] | select(.upstream.ahead > 0) | .branch'") }}

### Schema 1

The original bare-array format, and the default while unset:

{{ terminal(cmd="# Current worktree path (for scripts)|||wt list --format=json | jq -r '.[] | select(.is_current) | .path'||||||# Branches with uncommitted changes|||wt list --format=json | jq '.[] | select(.working_tree.modified)'||||||# Worktrees with merge conflicts|||wt list --format=json | jq '.[] | select(.operation_state == __WT_QUOT__conflicts__WT_QUOT__)'||||||# Branches ahead of main (needs merging)|||wt list --format=json | jq '.[] | select(.main.ahead > 0) | .branch'||||||# Integrated branches (safe to remove)|||wt list --format=json | jq '.[] | select(.main_state == __WT_QUOT__integrated__WT_QUOT__ or .main_state == __WT_QUOT__empty__WT_QUOT__) | .branch'||||||# Branches without worktrees|||wt list --format=json --branches | jq '.[] | select(.kind == __WT_QUOT__branch__WT_QUOT__) | .branch'||||||# Worktrees ahead of remote (needs pushing)|||wt list --format=json | jq '.[] | select(.remote.ahead > 0) | {branch, ahead: .remote.ahead}'||||||# Stale CI (local changes not reflected in CI)|||wt list --format=json --full | jq '.[] | select(.ci.stale) | .branch'") }}

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
| `operation_state` | string | `"conflicts"`, `"rebase"`, or `"merge"` (see [Worktree](#worktree)); absent when clean |
| `main` | object | Relationship to the default branch (see below); absent when is_main |
| `remote` | object | Tracking branch info (see below); absent when no tracking |
| `worktree` | object | Worktree metadata (see below) |
| `is_main` | boolean | Is the main worktree |
| `is_current` | boolean | Is the current worktree |
| `is_previous` | boolean | Previous worktree from wt switch |
| `ci` | object | CI status (see below); `--full` only, then absent when no PR/MR or branch workflow |
| `repo_url` | string | Repository web URL derived from the primary remote; absent when the remote URL cannot be parsed |
| `repo` | object | Structured repository metadata (see below); includes `remote` |
| `url` | string | Dev server URL from project config; absent when not configured |
| `url_active` | boolean | Whether the URL's port is listening; absent when not configured |
| `summary` | string | LLM-generated branch summary; `--full` only, then absent when not configured or no summary |
| `statusline` | string | Pre-formatted status with ANSI colors |
| `symbols` | string | Raw status symbols without colors (e.g., `"!?↓"`) |
| `vars` | object | Per-branch variables from [`wt config state vars`](@/config.md#wt-config-state-vars) (absent when empty) |
| `columns` | object | Rendered [custom column](#custom-columns) values keyed by header; empty cells omitted (absent when none configured) |

### Commit object

| Field | Type | Description |
|-------|------|-------------|
| `sha` | string | Full commit SHA (40 chars) |
| `short_sha` | string | Short commit SHA, abbreviated per `core.abbrev` (auto-extends for ambiguous prefixes) |
| `message` | string | Commit message (first line) |
| `timestamp` | number | Unix timestamp |

### working_tree object

The five change flags map to the [Working tree](#working-tree) symbols (`renamed` and `deleted` have none of their own):

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

`ahead` / `behind` drive the [Remote](#remote) divergence symbol:

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Remote name (e.g., `"origin"`) |
| `branch` | string | Remote branch name |
| `ahead` | number | Commits ahead of remote |
| `behind` | number | Commits behind remote |

### worktree object

Present only for worktree-kind items. `state` is the worktree-location attribute — see [Worktree](#worktree) for its symbols:

| Field | Type | Description |
|-------|------|-------------|
| `state` | string | `"branch_worktree_mismatch"`, `"prunable"`, or `"locked"` (absent when normal) |
| `reason` | string | Reason for locked/prunable state |
| `detached` | boolean | HEAD is detached |

### ci object

| Field | Type | Description |
|-------|------|-------------|
| `status` | string | CI status (see below) |
| `source` | string | `"pr"` (PR/MR) or `"branch"` (branch workflow) |
| `number` | integer | PR/MR number; absent for branch workflows |
| `stale` | boolean | Local HEAD differs from remote (unpushed changes) |
| `url` | string | URL to the PR/MR page |
| `repo_url` | string | Web URL of the repo the PR/MR targets (the upstream for fork PRs); absent when `url` is absent or unrecognized |
| `repo` | object | Structured metadata for the repository the PR/MR targets; never includes `remote` |
| `review_state` | string | Review state (see below); absent when the forge reports no review signal |

### repo object

Top-level `repo` describes the local checkout's repository as derived from the primary remote. `ci.repo` describes the repository targeted by the PR/MR URL in `ci.url` (for fork PRs, this is the upstream target). Existing `repo_url` and `ci.repo_url` fields remain available and carry the same URL as `repo.url` / `ci.repo.url`.

| Field | Type | Description |
|-------|------|-------------|
| `url` | string | Repository web URL |
| `provider` | string | `"github"`, `"gitlab"`, `"gitea"`, `"azure-devops"`, or `"unknown"` |
| `host` | string | Repository web host |
| `owner` | string | Owner, organization, or namespace path |
| `name` | string | Repository name |
| `project` | string | Azure DevOps project name; absent for other providers |
| `remote` | string | Local remote name used for top-level repo metadata; absent from `ci.repo` |

### main_state values

The single highest-priority state describing the branch's relation to the default branch; absent when none applies (a normal up-to-date branch). Each value is one Default-branch symbol — see [Default branch](#default-branch) for the symbol and the full meaning of each value (`"is_main"`, `"orphan"`, `"empty"`, `"integrated"`, `"would_conflict"`, `"same_commit"`, `"diverged"`, `"ahead"`, `"behind"`).

### integration_reason values

Set only when `main_state == "integrated"` (the `⊂` symbol), recording which check matched. Checks run cheapest-first and the first match wins. JSON-only — every reason renders as the same `⊂`:

| Value | Meaning |
|-------|---------|
| `"ancestor"` | Branch HEAD is an ancestor of the default branch, which has moved past it |
| `"no-added-changes"` | The three-dot diff (`main...branch`) is empty — no file changes beyond the merge-base |
| `"trees-match"` | Different history, but the branch's tree is identical to the default branch's |
| `"merge-adds-nothing"` | The branch has changes, but merging them leaves the default branch's tree unchanged (e.g. a squash merge where the target advanced on other files) |
| `"patch-id-match"` | The branch's squashed diff matches a single commit on the default branch (e.g. a GitHub/GitLab squash merge) |

### ci.status and ci.review_state values

The [CI status](#ci-status) section above is the single source for both fields: the table maps each colored value, and the notes below it cover `"draft"` and `"approved"`. `ci.status` is one of `"passed"`, `"running"`, `"failed"`, `"conflicts"`, `"no-ci"`, `"error"`; `ci.review_state` is one of `"changes_requested"`, `"pending"`, `"draft"`, `"approved"`, absent when the forge reports no review signal. The vocabulary matches Claude Code's statusline `pr.review_state` field.

Missing a field that would be generally useful? [Open an issue](https://github.com/max-sixty/worktrunk/issues).

## See also

- [`wt switch`](@/switch.md) — Switch worktrees or open interactive picker

## Command reference

{% terminal() %}
wt list - List worktrees and their status

Usage: <b><span class=c>wt list</span></b> <span class=c>[OPTIONS]</span>
       <b><span class=c>wt list</span></b> <span class=c>&lt;COMMAND&gt;</span>

<b><span class=g>Commands:</span></b>
  <b><span class=c>statusline</span></b>  Single-line status for shell prompts

<b><span class=g>Options:</span></b>
      <b><span class=c>--format</span></b><span class=c> &lt;FORMAT&gt;</span>
          Output format

          [default: table]
          [possible values: table, json]

      <b><span class=c>--branches</span></b>
          Include branches without worktrees

      <b><span class=c>--remotes</span></b>
          Include remote branches

      <b><span class=c>--full</span></b>
          Show CI status and LLM summaries

      <b><span class=c>--progressive</span></b>
          Show fast info immediately, update with slow info

          Displays local data (branches, paths, status) first, then updates with remote data (CI,
          upstream) as it arrives. Use --no-progressive to force buffered rendering. Auto-enabled
          for TTY.

  <b><span class=c>-h</span></b>, <b><span class=c>--help</span></b>
          Print help (see a summary with &#39;-h&#39;)

<b><span class=g>Global Options:</span></b>
  <b><span class=c>-C</span></b><span class=c> &lt;path&gt;</span>
          Working directory for this command

      <b><span class=c>--config</span></b><span class=c> &lt;path&gt;</span>
          User config file path

      <b><span class=c>--config-set</span></b><span class=c> &lt;toml&gt;</span>
          Override config with inline TOML, e.g. --config-set list.full=true (repeatable)

  <b><span class=c>-v</span></b>, <b><span class=c>--verbose</span></b><span class=c>...</span>
          Verbose output (-v: info logs + hook/alias template variables on stderr; -vv: also debug
          logs and raw subprocess output written to .git/wt/logs/). Set WORKTRUNK_VERBOSE=0|1|2 to
          apply the same level everywhere — including shell completion, which no flag can reach

  <b><span class=c>-y</span></b>, <b><span class=c>--yes</span></b>
          Skip approval prompts
{% end %}

<!-- END AUTO-GENERATED -->
