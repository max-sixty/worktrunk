# Design: `wt list --format=json` from scratch

Status: accepted; the release-N phase (schema 2 behind `[list] json-schema`,
nag when unset) is implemented on the `list-json-design` branch. This reviews
the current JSON shape
(`src/commands/list/json_output.rs`, documented in `docs/content/list.md`) and
answers: how would the structure look designed today, knowing everything the
format has accumulated? It ends with a migration recommendation, since the
answer is not "keep it".

## The goals in tension

The format serves several consumers with different needs:

1. **jq one-liners** — the documented primary use. Nine examples in `list.md`
   filter on paths like `.working_tree.modified` and `.main.ahead`. Short
   paths and flat booleans win here.
2. **Statusline integrations** — `wt list statusline` emits the same
   `JsonItem` (as a one-element array) for Claude Code, tmux, and starship.
   These want pre-rendered strings (`statusline`, `symbols`) next to the data.
3. **Agents and scripts making decisions** — cleanup scripts selecting
   `main_state == "integrated"` branches to remove, CI dashboards, the
   Claude Code plugin. These need to *trust* the semantics: "no value" must
   distinguish "checked, nothing there" from "didn't finish checking".
4. **Stability** — a maturing user base scripts against this. The format is
   effectively a protected external interface, yet carries no version marker.
5. **Progressive collection** — data arrives in gates with timeouts and
   staleness cutoffs (the table renders `·` for "loading, or collection timed
   out / branch too stale"). JSON is the one renderer that silently erases
   this partiality.
6. **Cross-forge neutrality** — GitHub/GitLab/Gitea/Azure DevOps normalize
   into one vocabulary.
7. **Doc/schema fidelity** — every type derives `schemars::JsonSchema`, but
   the schema is exported nowhere; the docs field table is hand-maintained
   and the code comment at `json_output.rs:55` has already drifted from the
   wire values (it lists snake_case `trees_match` where the wire says
   kebab-case `trees-match`, and omits `patch-id-match`).

## What the current shape gets right

Worth preserving in any redesign:

- **Concept-grouped subobjects** (`working_tree`, `main`, `remote`, `commit`)
  with self-describing names — the jq examples read naturally.
- **Deliberate unknown-handling for `working_tree`**: the converter reads the
  loaded field, not the display symbols, precisely so "unknown" doesn't
  collapse into "clean" (`json_output.rs:319`). The principle is right; it
  just doesn't extend to the other fields.
- **Structured `repo` / `ci.repo` objects** — parse-don't-string-heuristic,
  applied well.
- **One `JsonItem` shared by `wt list` and `wt list statusline`** — the two
  surfaces cannot drift.
- **Compact output**: `skip_serializing_if` keeps rows small.
- **The docs' jq cookbook** — the examples are the best spec of what users do.

## Findings, ranked

### 1. Absence conflates four meanings

`skip_serializing_if` makes these indistinguishable in the output:

- **Not applicable** — `working_tree` on a branch-only row.
- **Not requested** — `ci` and `summary` without `--full`.
- **Determined-empty** — `main_state` absent means "normal up-to-date branch"
  (so documented in `list.md`).
- **Not determined** — the same `main_state` is absent when gate 3 never
  resolved (timeout, staleness cutoff): the converter maps both the
  unresolved gate (`Option::None`) and the resolved-to-nothing variant
  (`MainState::None`) to the same absent field.

The table renderer is honest about the fourth case (`·`); JSON is not. For
consumer #3 this is the difference between "this branch is not integrated"
and "we never checked" — a cleanup script driving `wt remove` on
`main_state == "integrated"` silently skips candidates on a slow run, and a
script treating absent `main_state` as "normal branch, safe to rebase" acts
on branches whose state was never computed. `working_tree` got this right;
the rest of the format didn't.

### 2. The display model is exported as the data model

`main_state` is the table's symbol column: one priority-collapsed value per
row. That collapse folds several orthogonal facts into one enum:

- `"ahead"` / `"behind"` / `"diverged"` duplicate `main.ahead` / `main.behind`
  (both appear in the docs' own jq examples, as alternative spellings of the
  same question).
- `"empty"` vs `"same_commit"` differ only by working-tree cleanliness —
  which `working_tree` already reports. A branch relation enum that changes
  value when you `touch` a file is a display artifact.
- `"would_conflict"` is masked by priority: an integrated branch that would
  vacuously conflict reports `"integrated"`, so a consumer asking "would this
  merge cleanly?" cannot ask it — the answer is only present when nothing
  outranks it.
- `operation_state` has the same shape: `"conflicts"` (a working-tree file
  fact) outranks `"rebase"` / `"merge"` (an in-progress operation), though a
  rebase *with* conflicts is both at once.

The collapse is right for a one-glyph column and useful for humans; it is the
wrong primary representation for data. Consumers need the facts; the verdict
belongs in a display block.

### 3. Two "main"s — a collision with the project's own glossary

`is_main` means "is the *main worktree*" (glossary term). `main` and
`main_state` mean "relation to the *default branch*" — the exact naming the
terminology section bans ("default branch — not 'main branch'"). Two
unrelated meanings of "main" in one document, adjacent to each other.

### 4. `ci` is really "PR/MR + checks", interleaved

One object mixes four concerns: PR identity (`number`, `url`, `repo`), review
state (`review_state`), pipeline state (`status`), and sync (`stale`). The
`status` vocabulary shows the strain — `"passed"` / `"running"` / `"failed"`
are pipeline outcomes, but `"conflicts"` is forge mergeability, `"no-ci"` is
determined-empty, and `"error"` is "fetch failed" (finding 1 wearing a
costume). The `TODO(json-pr-title-body)` in `from_pr_status` is the same
strain from the other side: `PrStatus` now carries `title`/`body`/`author`,
and they have no natural home under a key called `ci`.

### 5. No envelope

The root is a bare array, so there is nowhere to put:

- a **schema version** — for a scripted-against interface with none;
- **repo-scope data** — `repo`/`repo_url` are repeated identically on every
  row; the *name of the default branch* (which `main` measures against) is
  not exposed at all;
- **collection metadata** — what was requested (`--full`?) and whether
  collection completed, the root-level half of finding 1.

### 6. Vocabulary inconsistencies

Three case conventions coexist: snake_case (`same_commit`,
`changes_requested`), kebab-case (`trees-match`, `no-ci`, `azure-devops`),
and the stale code comment documenting values that were never on the wire.
`git.rs`'s `IntegrationReason` serde says kebab; `MainState`'s strum says
snake; nothing enforces agreement.

### 7. Legacy duplicates and sentinels

- `repo_url` ≡ `repo.url`; `ci.repo_url` ≡ `ci.repo.url` (kept for compat,
  per the docs).
- `worktree.state: "no_worktree"` ≡ `kind: "branch"`.
- Unborn branches serialize `sha: ""`, `timestamp: 0` — sentinels where JSON
  has `null`.
- `remote.branch` is fabricated from the local branch name ("in most cases
  these match" — `upstream_to_json`), a documented guess presented as data.
- `kind` collapses local and remote branches; remote-ness is recoverable only
  by string-splitting `"origin/feature"` — the string-heuristic the project
  bans elsewhere.

### 8. Presentation strings mixed into the data document

`statusline` carries ANSI escapes inside JSON; `symbols` is a second
rendering of the same facts; `columns` holds rendered template strings. All
legitimate outputs (statusline tools genuinely want the ANSI form) — but
scattered at top level they blur what is data and what is paint.

## The from-scratch shape

One envelope, orthogonal facts, explicit unknowns, presentation quarantined
under `display`. Field names use the project glossary; every enum value is
snake_case.

```jsonc
{
  "schema": 2,                          // the unversioned current format is retroactively 1
  "repo": {
    "default_branch": "main",           // previously not exposed at all
    "forge": {                          // today's per-item `repo`, hoisted; was duplicated per row
      "url": "https://github.com/max-sixty/worktrunk",
      "provider": "github",
      "host": "github.com",
      "owner": "max-sixty",
      "name": "worktrunk",
      "remote": "origin"
    }
  },
  "collected": { "ci": false, "summary": false },     // what this run requested (--full, a listed ci column)
  "items": [
    {
      "branch": "feature-login",        // null only for a detached-HEAD worktree
      "remote": null,                   // "origin" for remote-only branch rows; replaces kind + name-splitting
      "head": {                         // null for unborn branches; replaces sha:"" / timestamp:0
        "sha": "6550d7ebcb1e2783d8757e3f41f8de9ba488b81d",
        "short_sha": "6550d7ebc",
        "subject": "feat(picker): flash alt-x removal-failure reasons",
        "committed_at": "2026-06-30T18:03:14Z"
      },
      "worktree": {                     // absent on branch-only rows; replaces kind + state:"no_worktree"
        "path": "/Users/max/workspace/worktrunk.feature-login",
        "main": false,                  // glossary: the main worktree
        "current": true,
        "previous": false,
        "detached": false,
        "locked": { "reason": "manual lock" },    // absent when unlocked; locked and prunable
        "prunable": { "reason": "gitdir missing" }, // can coexist — no single `state` collapse
        "branch_mismatch": false,
        "operation": "rebase",          // "rebase" | "merge"; absent when none
        "changes": {                    // null = not yet determined (extends today's working_tree care)
          "staged": false, "modified": true, "untracked": false,
          "renamed": false, "deleted": false,
          "conflicted": false,          // was operation_state:"conflicts" — a file fact, not an operation
          "diff": { "added": 10, "deleted": 2 }
        }
      },
      "default_branch": {               // absent when this IS the default branch; replaces main + main_state
        "ahead": 3,                     // null = not determined
        "behind": 1,
        "diff": { "added": 50, "deleted": 20 },
        "orphan": false,                // no merge-base with the default branch
        "integration": { "reason": "trees_match" },  // committed-content fact (independent of the
                                        //   working tree); absent = determined not-integrated,
                                        //   null = not determined (probe pending, timed out, or
                                        //   resolved from a skip-seeded conservative default)
        "merge_conflicts": false        // was main_state:"would_conflict" — no longer masked by priority
      },
      "upstream": {                     // git's term; was `remote` (which now names remote-only rows)
        "remote": "origin",
        "branch": "feature-login",      // the actual tracking ref, not the local-name guess
        "ahead": 0,
        "behind": 2
      },
      "pr": {                           // one half of today's `ci`
        "number": 3351,
        "url": "https://github.com/max-sixty/worktrunk/pull/3351",
        "review": "approved",           // was ci.review_state
        "mergeable": true,              // was ci.status:"conflicts" — forge mergeStateStatus, a PR
                                        //   fact against its target (vs default_branch.merge_conflicts,
                                        //   the local merge-tree simulation)
        "repo": { "...": "target repo; the upstream for fork PRs" }
      },
      "checks": {                       // the other half of today's `ci`
        "status": "passed",             // "passed" | "running" | "failed" — pipeline outcomes only;
                                        //   "no-ci" → `checks` absent; "error" → `checks` null;
                                        //   "conflicts" → pr.mergeable
        "source": "pr",                 // "pr" | "branch"
        "stale": false                  // local HEAD not what CI ran against
      },
      "dev_server": {                   // was top-level url / url_active
        "url": "http://127.0.0.1:10545",
        "listening": true
      },
      "summary": "Adds the login page…",
      "vars": { "review": "approved" }, // user data, stays top-level
      "display": {                      // presentation, quarantined
        "state": "diverged",            // today's main_state collapse, kept for humans and quick scripts
        "symbols": "!↕",
        "statusline": "feature-login [2m…[22m",  // ANSI, for tmux/starship
        "columns": { "Header": "rendered value" }
      }
    }
  ]
}
```

### The absence rule

One rule, stated in the docs and held everywhere:

- **Absent** — nothing to report: not applicable to this row, not requested
  this run (the envelope's `collected` says what was requested), or
  determined-empty (no PR exists, no lock, not integrated).
- **`null`** — requested and applicable, but not determined: gate timed out,
  branch past the staleness cutoff, forge fetch failed.
- **Value** — determined.

jq treats absent and `null` identically in path expressions, so the
one-liners are unaffected; careful consumers distinguish via `has()`. This is
the JSON spelling of the table's `·`, and it generalizes the care
`working_tree` already takes to every field.

### Vocabulary and conventions

- **snake_case for every enum value** (`trees_match`, `azure_devops`,
  `changes_requested`). One convention, schema-enforced. Note
  `IntegrationReason`'s serde rename is shared with other surfaces; the
  change must be coordinated, not local to list.
- **Timestamps are RFC 3339 UTC** (`committed_at`), matching what forge APIs
  return, so times are uniform within the document. (Trade-off: unix ints
  subtract without a `fromdate` parse. If age-math ergonomics are judged to
  matter more, `committed_at_unix` — but pick one.)
- **Ordering is documented**: items appear in table order, and that order is
  a guarantee of the format.

### The schema ships

Every type already derives `JsonSchema`; the drift in `json_output.rs:55`
shows why leaving it unexported fails. Generate the schema into the repo
(e.g. `docs/list-schema.json`) and pin it with a sync test alongside
`test_docs_are_in_sync`, so wire vocabulary can no longer drift from
documentation. The hand-written field table in `list.md` stays for prose; the
schema pins the values.

### What this deliberately does not add

- **Streaming JSON (NDJSON)** — the progressive philosophy suggests it, but
  no consumer asks for it today. The envelope is designed so it ports (items
  are independent; the envelope could become a first/last line), which is all
  YAGNI permits.
- **A `removable` verdict** — exporting `wt remove`'s own integration verdict
  is attractive (one mechanism per guarantee), but removability also involves
  locks, dirty trees, and prompts; a boolean here would invite unsafe
  scripting. `default_branch.integration` carries the fact; the verdict stays
  in `wt remove`.
- **PR `title`/`body`** — the TODO stands, but now has a natural home (`pr`)
  when a consumer materializes.

## Migration

Maturing mode: external-interface breaks need justification. Finding 1 is a
correctness defect (silent unknown-as-empty in a format that drives removal
scripts) and finding 5 blocks ever versioning the format — together they
justify one break; the naming and vocabulary fixes ride along rather than
justifying their own.

- **Opt-in, nag, then flip.** Release N ships v2 behind a user-config
  acknowledgment: `[list] json-schema = 2` selects the new shape; unset
  emits v1 plus a per-process-deduped stderr nag naming both settings;
  an out-of-range value warns and degrades to schema 1, matching how
  config load treats a type error (a config problem never bricks a
  command, least of all the statusline). `= 1` is an explicit, silent,
  clock-bounded pin. Release N+k flips unset → v2 and turns `= 1` into a
  standard deprecation (`DEPRECATION_RULES` row; `wt config update`
  strips the key); release N+k+m removes it. The initial nag says "a
  future release" — N+k isn't knowable when N ships — but the flip and
  removal releases are named in the changelog the day the flip is
  scheduled, and the deprecation warning for `= 1` names its removal
  release from its first firing. Nobody's format changes *at* them: early
  adopters switch immediately, everyone else migrates per machine on their
  own schedule, and the final flip surprises only whoever ignored a full
  window of nags. This is the Rust-editions / `init.defaultBranch` shape,
  and it maps onto machinery the repo already has: warnings are
  per-process deduped, the statusline surface already calls
  `suppress_warnings()` (a per-prompt nag would corrupt prompts and reach
  no one — the same human sees it on their next interactive run), and the
  warning-iff-`wt config update`-would-change invariant is already
  test-pinned.
- **User config only.** A `wt.toml` knob would flip a whole team's format
  via `git pull`; per-machine migration is the point. Within user config
  the key resolves per-project like its `[list]` siblings, so a
  `[projects."…".list] json-schema` override works. The key governs both
  `wt list --format=json` and `wt list statusline --format=json`.
- **Why not a call-site pin or a bare cutover.** A `--format=json-v1` flag
  travels with the script, which is the right property for *published*
  cross-machine scripts — but this format's consumers are almost entirely
  machine-local (statusline configs, personal aliases), living next to the
  config that would govern them; and the residual cross-machine case can
  detect the version structurally (`type == "array"` vs the envelope's
  `schema`), so no pin is required. A bare cutover with one release of
  stderr notice is cheapest but breaks not-reliably-loudly:
  `jq '.[] | select(…)'` against the envelope iterates object values and
  often yields empty output with exit 0, mid-workday, in a forgotten
  prompt config. What stays rejected in any variant is an *indefinite*
  selector — a knob whose purpose is choosing between two live formats
  forever, which is a parallel implementation wearing config clothes and
  would freeze v1 into a permanently maintained second product.
- `wt list statusline --format=json` moves in the same release (same
  `JsonItem`); its consumers' path changes to `.items[0].display.statusline`.
- The sibling JSON surfaces (`wt switch --format=json`,
  `wt config state logs --format=json`) adopt the same conventions
  (snake_case values, absence rule) as they are next touched; they are small
  and flat enough not to need envelopes.

## Ripples beyond the serializer

The redesign is mostly a serialization change — `ListItem` already carries
the orthogonal facts (`counts`, `is_ancestor`, `would_conflict`, the
integration signals), and `MainState` is computed from them at render time.
Four things outside `json_output.rs` should change with it:

1. **Declare the JSON surfaces protected.** `CLAUDE.md`'s Project Status
   lists "output formatting" as flexible, while `list.md` invites scripting
   against `--format=json` — the format is de facto protected with de jure
   freedom to break it, which is how v1 accumulated compat duplicates
   (`repo_url`) without ever gaining a version. When `schema` lands, add
   "`--format=json` structures (schema-versioned)" to the protected list.

2. **Untangle `CiStatus`.** The one place the *internal* model carries the
   display collapse: fetchers map forge facts into a single enum at fetch
   time (`merge_state_status == "DIRTY"` → `Conflicts`, discarding
   mergeability as a fact; `Error` and `NoCI` ride in the same enum).
   Serializing `pr.mergeable` and `checks: null` requires `PrStatus` to
   carry the facts — `mergeable: Option<bool>`, pipeline status as its own
   optional, fetch failure as its own channel — with the
   one-color-folds-two-fields display becoming a render-time decision, where
   it belongs. The picker's preview tabs consume the same struct and get the
   same benefit.

3. **Generate the field docs from the schema.** The field tables in
   `src/cli/mod.rs`'s `after_long_help` are hand-maintained; the repo
   already pins docs with sync tests (`test_docs_are_in_sync`). Emit the
   schemars schema into the repo and sync-test it; the value-vocabulary
   lists in prose can then reference rather than restate it. This is what
   retires the `json_output.rs:55` drift class rather than fixing one
   instance of it.

4. **Write the conventions down once.** The absence rule, snake_case
   values, RFC 3339 timestamps, envelope-for-collections, and schema
   versioning apply to every `--format=json` surface. A short "JSON output
   conventions" note (in `CLAUDE.md` or `docs/CLAUDE.md`) keeps
   `wt switch --format=json`, `wt config state logs --format=json` (unix-int
   `modified_at` today), and any future surface from re-deriving them —
   each adopts as it is next touched, per the migration section.

Smaller, when touched: the internal `Main*` names (`MainState`, `JsonMain`)
carry the same glossary collision the JSON had — rename toward
`DefaultBranch*` opportunistically (internal APIs are flexible); and
`list.md` should state the item-ordering guarantee the table already
provides. Deliberately unchanged: the table's priority collapse (correct for
a one-glyph column), the gate model (it is what makes `null` truthful), and
the `[list] columns = ci` keyword (a display concept).

## Appendix: field-by-field mapping

Every current field and where it lands. "Dropped" always means the fact
survives elsewhere; no information is lost.

### Root

| Current | v2 |
|---------|-----|
| bare array | `items` inside the envelope |
| — | `schema` (new) |
| — | `repo.default_branch` (new — previously unexposed) |
| — | `collected` (new — what this run requested) |

### Identity and commit

| Current | v2 | Change |
|---------|-----|--------|
| `branch` | `branch` | remote rows: `"origin/feature"` → `branch: "feature"` + `remote: "origin"` |
| `kind` | dropped | `"worktree"` ≡ `worktree` present; `"branch"` ≡ absent; remote-ness moves to `remote` |
| `commit` | `head` | `null` for unborn branches (was `sha: ""`, `timestamp: 0`) |
| `commit.sha` | `head.sha` | |
| `commit.short_sha` | `head.short_sha` | |
| `commit.message` | `head.subject` | git's term for the first line |
| `commit.timestamp` | `head.committed_at` | unix int → RFC 3339 UTC |

### Worktree

| Current | v2 | Change |
|---------|-----|--------|
| `path` | `worktree.path` | |
| `is_main` | `worktree.main` | |
| `is_current` | `worktree.current` | |
| `is_previous` | `worktree.previous` | |
| `worktree.detached` | `worktree.detached` | |
| `worktree.state: "locked"` + `reason` | `worktree.locked: {reason}` | locked and prunable can now coexist |
| `worktree.state: "prunable"` + `reason` | `worktree.prunable: {reason}` | |
| `worktree.state: "branch_worktree_mismatch"` | `worktree.branch_mismatch: true` | |
| `worktree.state: "no_worktree"` | dropped | ≡ `worktree` absent |
| `working_tree` | `worktree.changes` | same null-when-uncomputed semantics |
| `working_tree.{staged,modified,untracked,renamed,deleted}` | `worktree.changes.{…}` | names unchanged |
| `working_tree.diff` | `worktree.changes.diff` | |
| `operation_state: "rebase"` / `"merge"` | `worktree.operation` | |
| `operation_state: "conflicts"` | `worktree.changes.conflicted` | file fact, no longer masks the operation |

### Default-branch relation (was `main` + `main_state`)

| Current | v2 | Change |
|---------|-----|--------|
| `main.ahead` / `main.behind` | `default_branch.ahead` / `.behind` | `null` = not determined |
| `main.diff` | `default_branch.diff` | |
| `main_state: "is_main"` | `worktree.main: true` | `default_branch` absent on that row |
| `main_state: "orphan"` | `default_branch.orphan: true` | |
| `main_state: "integrated"` | `default_branch.integration` present | |
| `main_state: "empty"` | `integration.reason: "same_commit"` + clean `changes` | cleanliness leaves the enum |
| `main_state: "same_commit"` | `integration.reason: "same_commit"` + dirty `changes` | |
| `main_state: "would_conflict"` | `default_branch.merge_conflicts: true` | no longer masked by priority |
| `main_state: "ahead"/"behind"/"diverged"` | dropped | ≡ the `ahead`/`behind` counts |
| `main_state` (the collapsed value itself) | `display.state` | kept for humans and quick scripts |
| `integration_reason` | `default_branch.integration.reason` | kebab → snake (`trees-match` → `trees_match`) |

### Upstream (was `remote`)

| Current | v2 | Change |
|---------|-----|--------|
| `remote.name` | `upstream.remote` | |
| `remote.branch` | `upstream.branch` | actual tracking ref, not the local-name guess |
| `remote.ahead` / `.behind` | `upstream.ahead` / `.behind` | |

### Forge (was `ci` + per-item `repo`)

| Current | v2 | Change |
|---------|-----|--------|
| `ci.number` | `pr.number` | |
| `ci.url` | `pr.url` | |
| `ci.review_state` | `pr.review` | |
| `ci.repo` | `pr.repo` | |
| `ci.repo_url` | dropped | ≡ `pr.repo.url` |
| `ci.status: "passed"/"running"/"failed"` | `checks.status` | pipeline outcomes only |
| `ci.status: "conflicts"` | `pr.mergeable: false` | forge `mergeStateStatus`, a PR fact |
| `ci.status: "no-ci"` | dropped | ≡ `checks` absent |
| `ci.status: "error"` | dropped | ≡ `checks: null` (fetch failed = undetermined) |
| `ci.source` | `checks.source` | |
| `ci.stale` | `checks.stale` | |
| `repo` (per item) | envelope `repo.forge` | once, not per row |
| `repo_url` (per item) | dropped | ≡ envelope `repo.forge.url` |

### Everything else

| Current | v2 | Change |
|---------|-----|--------|
| `url` | `dev_server.url` | |
| `url_active` | `dev_server.listening` | |
| `summary` | `summary` | |
| `vars` | `vars` | |
| `statusline` | `display.statusline` | |
| `symbols` | `display.symbols` | |
| `columns` | `display.columns` | |

### The documented jq examples, before and after

```console
# Current worktree path
jq -r '.[] | select(.is_current) | .path'
jq -r '.items[] | select(.worktree.current) | .worktree.path'

# Uncommitted changes
jq '.[] | select(.working_tree.modified)'
jq '.items[] | select(.worktree.changes.modified)'

# Merge conflicts in progress
jq '.[] | select(.operation_state == "conflicts")'
jq '.items[] | select(.worktree.changes.conflicted)'

# Ahead of the default branch
jq '.[] | select(.main.ahead > 0) | .branch'
jq '.items[] | select(.default_branch.ahead > 0) | .branch'

# Integrated (safe to remove)
jq '.[] | select(.main_state == "integrated" or .main_state == "empty") | .branch'
jq '.items[] | select(.display.state == "integrated" or .display.state == "empty") | .branch'

# Branches without worktrees
jq '.[] | select(.kind == "branch") | .branch'
jq '.items[] | select(.worktree | not) | .branch'

# Ahead of upstream (needs pushing)
jq '.[] | select(.remote.ahead > 0) | {branch, ahead: .remote.ahead}'
jq '.items[] | select(.upstream.ahead > 0) | {branch, ahead: .upstream.ahead}'

# Stale CI
jq '.[] | select(.ci.stale) | .branch'
jq '.items[] | select(.checks.stale) | .branch'
```

The integrated-branches query is the one place the collapsed vocabulary
remains the right tool, which is why `display.state` keeps it verbatim:
today's `"integrated"`/`"empty"` fold in a cleanliness guard (the expensive
integration checks only run on clean trees, and `"empty"` means
same-commit *and* clean), so the data-model spelling
`select(.default_branch.integration)` is not an exact swap — it would also
match a dirty same-commit branch, whose uncommitted work makes it unsafe to
remove. Precise consumers pair `default_branch.integration` with
`worktree.changes`; quick scripts keep the one-liner.

The absence rule still pays off underneath: a branch whose integration was
never determined (timeout, staleness, dirty tree skipping the check) carries
`integration: null`, which `select` treats as false — undetermined branches
fall out of removal queries by construction, where today they are
indistinguishable from "normal branch".

## Independent of the redesign

Actionable now, no compat cost:

- Fix the stale value list at `json_output.rs:55` (wrong case, missing
  `patch-id-match`).
- Document the absent-vs-unknown hazard in `list.md`'s field table until the
  redesign lands — today's docs assert absent `main_state` means "normal
  up-to-date branch", which is not the only cause.
