# wt list

List worktrees and their status.

Shows uncommitted changes, divergence from the default branch and remote, and optional CI status and LLM summaries.

The table renders progressively: branch names, paths, and commit hashes appear immediately, then status, divergence, and other columns fill in as background git operations complete.

## Full mode

`--full` adds the two columns that reach off-machine: [CI status](#ci-status) (GitHub/GitLab pipeline pass/fail, over the network) and [LLM-generated summaries](#llm-summaries) of each branch's changes. The `main…±` line diffs are local git, so they show by default.

## Examples

List all worktrees:

```
$ wt list
  Branch       Status        HEAD±    main↕     main…±  Remote⇅  Commit   Age   Message
@ feature-api  +   ↕⇡     +54   -5   ↑4  ↓1  +234  -24   ⇡3      6814f02  30m   Add API tests
^ main             ^⇅                                    ⇡1  ⇣1  41ee083  4d    Merge fix-auth: h…
+ fix-auth         ↕|                ↑2  ↓1   +25  -11     |     b772e68  5h    Add secure token…
+ fix-typos        _|                                      |     41ee083  4d    Merge fix-auth: h…

○ Showing 4 worktrees, 1 with changes, 2 ahead, 1 column hidden
```

Include CI status and LLM summaries:

```
$ wt list --full
  Branch       Status        HEAD±    main↕     main…±  Summary                                                 Remote⇅  CI    Commit
@ feature-api  +   ↕⇡     +54   -5   ↑4  ↓1  +234  -24  Refactor API to REST architecture with middleware        ⇡3      #412  6814f02
^ main             ^⇅                                                                                            ⇡1  ⇣1  #     41ee083
+ fix-auth         ↕|                ↑2  ↓1   +25  -11  Harden auth with constant-time token validation            |     #408  b772e68
+ fix-typos        _|                                                                                              |     #410  41ee083

○ Showing 4 worktrees, 1 with changes, 2 ahead, 3 columns hidden
```

Include branches that don't have worktrees:

```
$ wt list --branches --full
  Branch       Status        HEAD±    main↕     main…±  Summary                                                 Remote⇅  CI    Commit
@ feature-api  +   ↕⇡     +54   -5   ↑4  ↓1  +234  -24  Refactor API to REST architecture with middleware        ⇡3      #412  6814f02
^ main             ^⇅                                                                                            ⇡1  ⇣1  #     41ee083
+ fix-auth         ↕|                ↑2  ↓1   +25  -11  Harden auth with constant-time token validation            |     #408  b772e68
+ fix-typos        _|                                                                                              |     #410  41ee083
/ exp             /↕                 ↑2  ↓1  +137       Explore GraphQL schema and resolvers                                   9637922
/ wip             /↕                 ↑1  ↓1   +33       Start API documentation                                                b40716d

○ Showing 4 worktrees, 2 branches, 1 with changes, 4 ahead, 3 columns hidden
```

Output as JSON for scripting:

```bash
$ wt list --format=json
```

## Columns

| Column | Shows |
|--------|-------|
| Branch | Branch name; a detached worktree has none, so it shows its short hash in dim yellow |
| Status | Compact symbols (see below) |
| HEAD± | Uncommitted changes: +added -deleted lines |
| main↕ | Commits ahead/behind default branch |
| main…± | Line diffs since the merge-base (three-dot) with the default branch |
| Summary | LLM-generated branch summary; requires `--full`, `summary = true`, and [`commit.generation`](https://worktrunk.dev/config/#commit) [experimental] |
| Remote⇅ | Commits ahead/behind tracking branch |
| CI | PR/MR number colored by pipeline status; `--full` only |
| Path | Worktree directory |
| URL | Dev server URL from project config; dimmed if port is not listening |
| *(custom)* | User-defined [custom columns](#custom-columns) from `[list.custom-columns]` user config [experimental] |
| Commit | Short hash, abbreviated per `core.abbrev` |
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
| `#` green | `"passed"` | All checks passed |
| `#` blue | `"running"` | Checks in progress |
| `#` red | `"failed"` | One or more checks failed |
| `#` yellow | `"conflicts"` | Merge conflicts with the target branch |
| `#` gray | `"no-ci"` | No PR/MR, or no checks configured |
| `⚠` yellow | `"error"` | CI status could not be fetched (rate limit, network, etc.) |
| `#` magenta | `"changes_requested"` | A reviewer requested changes |
| `#` cyan | `"pending"` | A review is required (e.g. branch protection) but not yet given |
| (blank) | `ci` absent | No upstream, or no PR/MR and no branch workflow |

The two remaining `ci.review_state` values have no indicator of their own: `"draft"` only dims the cell and `"approved"` leaves the color unchanged.

Color precedence resolves the fold: changes-requested (magenta) outranks running checks — waiting can't clear it — while an outstanding required review (cyan) only recolors an otherwise green or quiet branch. Cool colors mean waiting, warm colors mean act. An approved PR, or one with no review signal at all (no required reviewers and no reviews), keeps its plain `ci.status` color — `ci.review_state` is then `"approved"` or absent, respectively. GitLab MR data carries only `"pending"` and `"draft"` — no approved or changes-requested signal.

CI cells are clickable links to the PR or pipeline page, and appear dimmed for a draft PR/MR (`"draft"`) or when unpushed local changes make the status stale (`ci.stale`). PRs/MRs are checked first, then branch workflows/pipelines for branches with an upstream. Local-only branches show blank; remote-only branches — visible with `--remotes` — get CI status detection. Results are cached for 30-60 seconds; use `wt config state` to view or clear.

### LLM summaries [experimental]

Reuses the [`commit.generation`](https://worktrunk.dev/config/#commit) command — the same LLM that generates commit messages. Enable with `summary = true` in `[list]` config; requires `--full`. Results are cached until the branch's diff changes.

### Custom columns [experimental]

Each `[list.custom-columns]` entry in user config adds a column: the key is the header, the template renders each row's cell. Templates read two per-branch namespaces — `{{ vars.* }}`, stored with [`wt config state vars set`](https://worktrunk.dev/config/#wt-config-state-vars), and `{{ git.branch.* }}`, the branch's own git config under `branch.<name>.*` (a `jira` key you set yourself, or the git-native `description`) — useful for tracking what each of many (often agent-driven) branches is for:

```toml
[list.custom-columns.Ticket]
template = "{{ vars.ticket }}"
```

A column that renders empty for every row is dropped from the table. Templates, widths, and drop priority: [custom columns config](https://worktrunk.dev/config/#custom-columns).

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

An in-progress git operation, a worktree-location attribute, or a branch with no worktree. One symbol shows, highest priority first (`✘ > ↻ > ⊟ > ⊞ > ⚑ > /`):

| Symbol | JSON | Meaning |
|--------|------|---------|
| `✘` | `operation_state` `"conflicts"` | Merge conflicts |
| `↻` | `operation_state` `"rebase"`, `"merge"`, `"cherry_pick"`, `"revert"`, `"bisect"` | A git operation is in progress; `git status` names it |
| `⊟` | `worktree.state` `"prunable"` | Prunable (worktree directory missing) |
| `⊞` | `worktree.state` `"locked"` | Locked worktree |
| `⚑` | `worktree.state` `"duplicate_branch"` | Branch checked out in more than one worktree, so `wt` resolves it to whichever git lists first; every worktree on the branch is flagged |
| `⚑` | `worktree.state` `"branch_worktree_mismatch"` | Worktree isn't at the path its branch implies — including a detached one, which has no branch to imply a path and so is never at home |
| `/` | `kind` `"branch"` | Branch without a worktree (no `worktree` object) |

### Default branch

The single highest-priority state describing the branch's relation to the default branch; blank when none applies (a normal up-to-date branch). Each symbol is one `main_state` value:

| Symbol | main_state | Meaning |
|--------|------------|---------|
| `^` | `"is_main"` | The main worktree (the repo's home worktree) |
| `∅` | `"orphan"` | No common ancestor with the default branch |
| `_` | `"empty"` | Same commit as the default branch, working tree clean — safe to remove; row dimmed |
| `⊂` | `"integrated"` | Content [integrated](https://worktrunk.dev/remove/#branch-cleanup) into the default branch or merge target via different history; the matching check is in `integration_reason`; row dimmed |
| `✗` | `"would_conflict"` | Merging into the default branch would conflict (simulated with `git merge-tree`) and the branch isn't already integrated; with `--full`, the check includes uncommitted changes |
| `–` | `"same_commit"` | Same commit as the default branch, but with uncommitted changes |
| `↕` | `"diverged"` | Both ahead of and behind the default branch |
| `↑` | `"ahead"` | Has commits the default branch doesn't |
| `↓` | `"behind"` | Missing commits the default branch has |

Rows are dimmed when [safe to delete](https://worktrunk.dev/remove/#branch-cleanup) — `_` (`"empty"`) or `⊂` (`"integrated"`).

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
(`wt config update` adopts `= 2`); a future release flips the default to
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
                   "duplicate_branch": false,
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
| `worktree` | `{path, main, current, previous, detached, locked, prunable, branch_mismatch, duplicate_branch, operation, changes}`; absent on branch-only rows. `locked`/`prunable` are `{reason}` objects and can co-occur; `operation` is `"rebase"` or `"merge"`; `changes` holds the five working-tree flags plus `conflicted` and `diff {added, deleted}` |
| `default_branch` | Relation to the default branch: `{ahead, behind, diff, orphan, integration, merge_conflicts}`; absent on the default branch itself. `integration.reason` is one of `same_commit`, `ancestor`, `no_added_changes`, `trees_match`, `merge_adds_nothing`, `patch_id_match`; a dirty tree skips the checks, leaving `integration` null |
| `upstream` | Tracking branch: `{remote, branch, ahead, behind}`; absent when none is configured |
| `pr` | Open PR/MR: `{number, url, review, mergeable, repo}`; collected with `--full`. `review` uses the schema 1 `ci.review_state` vocabulary; `mergeable` is false when the forge reports conflicts, null otherwise |
| `checks` | CI pipeline: `{status, source, stale}`; collected with `--full`. `status` is `passed`, `running`, or `failed` — null when a conflicts report masks it |
| `dev_server` | `{url, listening}` from the project's `list.url` template |
| `summary` | LLM branch summary; needs `--full`, `[list] summary = true`, and a `[commit.generation]` command |
| `vars` | Per-branch variables from [`wt config state vars`](https://worktrunk.dev/config/#wt-config-state-vars) |
| `display` | Rendered strings: `state` (schema 1's `main_state` vocabulary), `symbols`, `statusline` (with ANSI colors and OSC 8 hyperlinks), `columns` (custom-column cells keyed by header) |

Schema 1 names map directly: `commit` → `head`, `working_tree` →
`worktree.changes`, `main` + `main_state` → `default_branch` +
`display.state`, `remote` → `upstream`, `ci` → `pr` + `checks`, `url` +
`url_active` → `dev_server`, `statusline`/`symbols`/`columns` → `display.*`,
and the per-item `repo` moves to the envelope's `repo.forge`.

```bash
# Current worktree path (for scripts)
$ wt list --format=json | jq -r '.items[] | select(.worktree.current) | .worktree.path'

# Branches with uncommitted changes
$ wt list --format=json | jq '.items[] | select(.worktree.changes.modified)'

# Integrated branches (safe to remove)
$ wt list --format=json | jq '.items[] | select(.display.state == "integrated" or .display.state == "empty") | .branch'

# Worktrees ahead of upstream (needs pushing)
$ wt list --format=json | jq '.items[] | select(.upstream.ahead > 0) | .branch'
```

A JSON Schema for the envelope is published at
[worktrunk.dev/schema/list-v2.json](https://worktrunk.dev/schema/list-v2.json).
It describes what `wt` writes, so a field the absence rule can omit is
optional there rather than required-and-null.

### Schema 1

The original bare-array format, and the default while unset:

```bash
# Current worktree path (for scripts)
$ wt list --format=json | jq -r '.[] | select(.is_current) | .path'

# Branches with uncommitted changes
$ wt list --format=json | jq '.[] | select(.working_tree.modified)'

# Worktrees with merge conflicts
$ wt list --format=json | jq '.[] | select(.operation_state == "conflicts")'

# Branches ahead of main (needs merging)
$ wt list --format=json | jq '.[] | select(.main.ahead > 0) | .branch'

# Integrated branches (safe to remove)
$ wt list --format=json | jq '.[] | select(.main_state == "integrated" or .main_state == "empty") | .branch'

# Branches without worktrees
$ wt list --format=json --branches | jq '.[] | select(.kind == "branch") | .branch'

# Worktrees ahead of remote (needs pushing)
$ wt list --format=json | jq '.[] | select(.remote.ahead > 0) | {branch, ahead: .remote.ahead}'

# Stale CI (local changes not reflected in CI)
$ wt list --format=json --full | jq '.[] | select(.ci.stale) | .branch'
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
| `statusline` | string | Pre-formatted status with colors and links |
| `symbols` | string | Raw status symbols without colors (e.g., `"!?↓"`) |
| `vars` | object | Per-branch variables from [`wt config state vars`](https://worktrunk.dev/config/#wt-config-state-vars) (absent when empty) |
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
| `state` | string | `"branch_worktree_mismatch"`, `"duplicate_branch"`, `"prunable"`, or `"locked"` (absent when normal) |
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

Missing a field that would be generally useful? Open an issue at https://github.com/max-sixty/worktrunk.

## Command reference

```
wt list - List worktrees and their status

Usage: wt list [OPTIONS]
       wt list <COMMAND>

Commands:
  statusline  Single-line status for the current worktree

Options:
      --format <FORMAT>
          Output format

          [default: table]
          [possible values: table, json]

      --branches
          Include branches without worktrees

      --remotes
          Include remote branches

      --full
          Show CI status and LLM summaries

      --progressive
          Show fast info immediately, update with slow info

          Displays local data (branches, paths, status) first, then updates with remote data (CI,
          upstream) as it arrives. Use --no-progressive to force buffered rendering. Auto-enabled
          for TTY.

  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

      --config-set <toml>
          Override config with inline TOML, e.g. --config-set list.full=true (repeatable)

  -v, --verbose...
          Verbose output (-v: info logs + hook/alias template variables on stderr; -vv: also debug
          logs and raw subprocess output written to .git/wt/logs/). Set WORKTRUNK_VERBOSE=0|1|2 to
          apply the same level everywhere — including shell completion, which no flag can reach

  -y, --yes
          Skip approval prompts
```

# Subcommands

## wt list statusline

Single-line status for the current worktree.

The line carries the same cells as the worktree's row in `wt list`. A stale CI status cache makes it reach the network for a second or two, so it fits a statusline the host renders in the background — Claude Code's, a `tmux` status bar — better than a prompt the shell blocks on. Want it fast enough for a synchronous prompt? Open an issue at https://github.com/max-sixty/worktrunk.

### Output formats

- `table` (default): `branch  status  HEAD±  main↕  main…±  Remote⇅  CI  URL`
- `json`: A one-entry array in the `wt list --format=json` schema
- `claude-code`: the `table` cells, preceded by `dir` and followed by `model  context  pace`

A cell with nothing to show is left out rather than blanked, so most lines are shorter than that; `claude-code` also drops `branch` where `dir` already ends in `.<branch>`. A line that still overruns the terminal drops whole cells, least important first, starting with the dev server URL.

The CI reference links to its PR/MR, and a dev server URL carrying a port shows as `:3000` linking to the URL in full, dim until something answers on that port. Both are underlined, which is what marks them as clickable. They are OSC 8 links, and a terminal that doesn't support those discards the escape, leaving the underlined text unclickable.

### Claude Code mode

`--format=claude-code` reads JSON context from stdin (`.workspace.current_dir` is required; the rest are optional):

- `.workspace.current_dir` — working directory
- `.model.display_name` — model name
- `.context_window.used_percentage` — context usage (0–100), rendered as `🌔 65%`, the moon waning 🌕→🌑 as context fills
- `.rate_limits.{five_hour,seven_day}.used_percentage` — rate-limit window usage (0–100)
- `.rate_limits.{five_hour,seven_day}.resets_at` — window reset time (Unix epoch seconds)

The pace segment appears only when usage is likely to hit a rate limit before its window resets, and shows the higher-risk window: `2.9×(Tue–Tue 5pm)` reads as 2.9× the pace that would exactly fill that window. Above 90% used it shows usage instead of pace — `93%(Tue–Tue 5pm)` — near the cap, how much is left matters more than how fast it's going. "Likely" is a Bayesian forecast; early-window bursts don't trigger it. Its colour deepens with severity — dim, then dim-yellow, then yellow — as the forecast lockout (how much of the window would be spent capped) grows, so a fast pace that would only tip over near the reset stays dim rather than alarming. With `-vv`, each window's inputs and projection are logged to `.git/wt/logs/trace.log`.

[Claude Code statusline setup](https://worktrunk.dev/claude-code/#statusline-claude-code-only) has the `~/.claude/settings.json` entry that feeds this mode.

### Command reference

```
wt list statusline - Single-line status for the current worktree

Usage: wt list statusline [OPTIONS]

Options:
      --format <FORMAT>
          Output format

          Possible values:
          - table
          - json
          - claude-code: Claude Code statusline mode (reads context from stdin)

          [default: table]

  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

      --config-set <toml>
          Override config with inline TOML, e.g. --config-set list.full=true (repeatable)

  -v, --verbose...
          Verbose output (-v: info logs + hook/alias template variables on stderr; -vv: also debug
          logs and raw subprocess output written to .git/wt/logs/). Set WORKTRUNK_VERBOSE=0|1|2 to
          apply the same level everywhere — including shell completion, which no flag can reach

  -y, --yes
          Skip approval prompts
```
