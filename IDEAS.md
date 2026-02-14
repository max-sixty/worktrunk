# Arbitrary per-worktree state

Context: https://github.com/max-sixty/worktrunk/issues/947#issuecomment-3867737861

## The problem

A user needs port numbers derived from branch names for dev servers. They found
`wt step for-each` with templates works across all worktrees:

```
wt step for-each -- echo '{{ branch }},{{ branch | hash_port }}'
```

...but there's no way to evaluate a template for a single branch. And no way to
store or display arbitrary per-branch metadata.

Two orthogonal dimensions:

- **Computed values** — derived from branch name via templates. Same formula,
  different per branch. Example: `{{ branch | hash_port }}`. Defined in project
  config, shared across developers.
- **Stored values** — set manually per branch. Example: `env=staging`,
  `ticket=PROJ-123`. In git config, per-repo.

## Feature 1: Template evaluation for a single branch

The simplest option: add `--branch` to `for-each` so it runs on one branch
instead of all:

```bash
wt step for-each --branch feature/x -- echo '{{ branch | hash_port }}'
```

This is exactly what the commenter asked for — "found no way to run it on a
single branch." Minimal change, no new subcommand.

A more targeted alternative: a dedicated `eval` subcommand (under `step` or
`config`) that evaluates a template without running a shell command:

```bash
wt step eval '{{ branch | hash_port }}'                     # current branch
wt step eval '{{ branch | hash_port }}' --branch feature/x  # specific branch
wt step eval '{{ ("supabase-api-" ~ branch) | hash_port }}' # expressions
```

Both reuse the existing minijinja infrastructure (`expand_template`) with the
same filters and variables.

## Feature 2: Arbitrary per-branch state

Generalize the existing marker pattern. Today `worktrunk.state.{branch}.marker`
and `worktrunk.state.{branch}.ci-status` already exist. The infrastructure is
there — `config.rs` reads/writes these, `state.rs` handles CLI get/set/clear.
Open it up to arbitrary keys:

```bash
wt config state set env staging              # current branch
wt config state set env production --branch main
wt config state get env                      # → "staging"
wt config state list                         # all keys for current branch
wt config state list --all                   # all keys for all branches
wt config state clear env
```

Storage: `worktrunk.state.{branch}.{key}` = raw string in git config. No JSON
wrapper needed (unlike markers which store `{"marker": "...", "set_at": ...}` —
that timestamping is marker-specific).

### State in templates

Make stored state available as template variables in *all* template contexts —
not just a hypothetical `eval` command, but everywhere templates are already
used: `for-each`, `url` template, `post-start` hooks, worktree path template.

```bash
wt step eval '{{ state.env | default("dev") }}'
```

```toml
# .config/wt.toml — state in the URL template
url = "http://localhost:{{ branch | hash_port }}/{{ state.env | default('dev') }}"

# post-start hook using state
post-start = ["echo 'Environment: {{ state.env | default(\"dev\") }}'"]
```

`expand_template` would load `worktrunk.state.{branch}.*` keys via
`--get-regexp` and inject them into the template context. Since all template
expansion flows through the same function, this is one change that lights up
everywhere.

### State in JSON output

`wt list --format=json` would include:

```json
{
  "branch": "feature/auth",
  "state": {"env": "staging", "ticket": "PROJ-123"},
  ...
}
```

Natural extension — the JSON output already includes `url`, `ci`, `symbols`.

### Relationship to markers

Markers could become sugar over `wt config state set marker "🚧"`, or keep the
dedicated subcommand for ergonomics (markers have display semantics in `wt list`
that arbitrary keys don't). Either way, the underlying storage is the same
pattern.

## Feature 3: Custom columns in `wt list`

The most impactful for visual workflows, but also the most complex.

### Option A: Named columns in project config

```toml
# .config/wt.toml
[list.columns]
port = "{{ branch | hash_port }}"
supabase = "{{ ('supabase-api-' ~ branch) | hash_port }}"
env = "{{ state.env | default('') }}"
```

`wt list` renders extra columns:

```
  main           ^|  12107  14260
  feature/auth   ↑⇡  16066  16739  staging
  log-alias      ↑|  19471  18599
```

Implementation: columns evaluate in pre-skeleton phase (templates are cheap).
Add as tasks in `collect/tasks.rs`, thread through to `render.rs`.

The complication is column layout — already complex with responsive hiding,
progressive rendering, overflow handling. User-defined columns add variable
widths, a hide policy when there are too many, and width calculation before
content is known.

### Option B: Single template column

One free-form template instead of N named columns:

```toml
[list]
info = ":{{ branch | hash_port }}{% if state.env %} [{{ state.env }}]{% endif %}"
```

```
  main           ^|  :12107
  feature/auth   ↑⇡  :16066 [staging]
  log-alias      ↑|  :19471
```

Simpler to implement — one column, one template, user controls formatting. Less
structured, no per-column hiding or sorting.

### Precedent: the URL column

The existing `url` template in project config already provides one computed
column. Custom columns are "more of that." The URL implementation in
`collect/tasks.rs` shows the pattern: evaluate template per-branch, optionally
check liveness, display in table.

### Where to define columns

Project-level (`.config/wt.toml`) for shared computed values like port
mappings. User-level (`~/.config/worktrunk/config.toml`) for personal
preferences like ticket IDs. Project takes precedence on conflicts.

## What to build first

Features 1+2 (`step eval` + generalized `config state`) are low-risk,
high-value, build directly on existing infrastructure. The commenter's use case
is solved immediately by `step eval`.

Feature 3 is where the design discussion lives. The key question: named columns
(Option A) or single template (Option B)? The URL column precedent suggests
named columns, but that's more machinery.
