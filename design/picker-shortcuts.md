# Design: more `wt switch` picker shortcuts, including custom template-bound ones

Status: proposal (no production code). This answers two questions about the
`wt switch` interactive picker:

1. Which additional built-in keyboard shortcuts are worth adding?
2. Can users define custom shortcuts that expand a template against the
   selected row, the way `[aliases]` and hooks already do?

## Summary of recommendations

- **Built-in shortcuts.** Add `alt-l` to reload the worktree list (pure
  worktrunk, no OS dependency). Add `alt-y` (copy branch) and `alt-o` (open the
  row's PR/MR URL) once a small cross-platform clipboard/open helper exists;
  worktrunk has none today, and that helper is the real cost, not the binds. All
  three sit on `alt+`, with the existing action binds (`alt-c`, `alt-r`, `alt-p`,
  `alt-1`…`7`) — see "Key convention".
- **Custom shortcuts.** Worth building, in a tightly scoped form:
  **user-config only**, under `[switch.picker.shortcuts]`, one key to one
  template, expanded against the selected row and run in the background while
  the picker stays open. User-config-only is the load-bearing decision. It isn't
  forced by the security model (a project shortcut could route through the same
  read-only approval the picker already uses for removal hooks); it's a scope
  choice. A project shortcut would run only if its exact command were already
  approved at a prior gate, which a picker-only command almost never is, so it
  would usually be an inert key the user can't fix mid-render. User-only drops
  that footgun and matches where keybindings already live. The feature fills a
  real gap: side-effects on a worktree you haven't switched to (copy, open, kick
  a build), which `[aliases]` can't reach because aliases run against the current
  worktree.
- **Execution.** Reuse the `alt-r` machinery: a new `run` verb on the existing
  `PickerCollector`, expanding via `expand_template(..., Posix, ...)` and
  spawning the command on a background thread. An `execute-silent` plus a hidden
  `wt` helper subcommand is a viable lighter-touch alternative; the tradeoff is
  spelled out below.

## How the picker binds keys today

`src/commands/picker/mod.rs` builds one `SkimOptionsBuilder.bind(vec![...])`
block (lines ~844-941). Current bindings:

| Key | Action |
|-----|--------|
| Enter | switch to the selected worktree |
| `alt-c` | create a worktree from the query |
| `alt-r` | remove the selected worktree/branch |
| `alt-p` | toggle the preview pane |
| `alt-1`…`alt-7` | jump to preview tab N |
| `tab` / `shift-tab` | cycle preview tabs |
| `ctrl-u` / `ctrl-d` | scroll the preview half a page |

Bare digits `1`-`7` are deliberately unbound so they flow into the query (a PR
number, or digits inside a branch name).

Three hard skim constraints shape every bind, documented in the bind block and
confirmed against skim 4.8 source (`src/tui/event.rs`):

- **Paren-free bodies.** skim parses an `execute-silent(BODY)` by splitting at
  the first `(` and trimming the trailing `)`, so `BODY` can contain no parens.
  `$(...)` and `$(( ))` are out.
- **`+`-free bodies.** skim splits an action chain on `+`, so `BODY` can't
  contain `+` (no `cmd1 && cmd2` via `+`).
- **Shell-agnostic bodies.** Binds run under the user's `$SHELL` (fish/zsh/sh),
  so only POSIX externals (`tr`, `mv`, `sh`) behave identically everywhere.

Two precedents matter for the custom-shortcut design:

- **`alt-r` routes a key into Rust through a fake `reload`.** The bind is
  `alt-r:reload(remove {})`. skim expands `{}` to the selected row's `output()`
  token, shell-quotes it, and hands `remove <token>` to
  `PickerCollector::invoke` (a `CommandCollector`). The collector does the
  removal off the event loop and streams the updated item list back. `reload` is
  the only skim action that calls into worktrunk Rust at key-press time without
  exiting the picker.
- **PR #3199 drives the cursor from Rust via `Action::Custom`.** `reload` resets
  the cursor to the top; the removal injects an `Action::Custom`
  (`reposition_cursor_action`) through skim's event sender that lands the cursor
  back on the removed row's slot once the reloaded rows settle. `Action`,
  `Event`, and `ActionCallback` are public in `skim::tui::event`, and a
  `Custom` callback gets `&mut App`.

The skim action vocabulary is large (~70 actions), but only a handful reach
worktrunk logic: `reload` (into a `CommandCollector`), `accept(label)` (exit,
then inspect `out.final_event`), and `execute` / `execute-silent` (run a shell
command, no Rust expansion). There is no bind-string syntax for `Action::Custom`
— a key cannot be bound to a Rust closure directly; it can only reach Rust
through `reload` or `accept`.

## Part 1: built-in shortcuts

### What peer pickers bind, and what maps to worktrunk

fzf, lazygit, and gh-dash converge on a small set of row-level actions:
`y`/`Y` to copy (branch name, path, hash, URL), `o` to open a URL in the
browser, and `ctrl-r` / `R` to reload the list. Most of their other binds are
file-picker or git-internal actions with no worktree analogue.

Mapping to worktrunk, three are genuinely useful and don't already exist:

1. **Copy the selected branch name.** Paste into an issue, a commit message, a
   chat. The single most common "I just want the name" action.
2. **Open the selected row's PR/MR URL.** A worktree row whose branch has a PR,
   or a `--prs` row, carries a URL (`PrStatus.url`). Opening it for review or to
   check CI is a natural follow-on to spotting it in the picker.
3. **Reload the worktree list.** Branches and worktrees appear from outside the
   session (a teammate pushes, a parallel agent creates one). A reload re-runs
   `collect` without reopening the picker.

Renaming and relocating are deliberately excluded: they need planning and
confirmation that doesn't fit a single keystroke, and `wt step relocate` already
covers them.

### Key convention

Discrete actions live on `alt+`. The existing action binds already follow this
(`alt-c` create, `alt-r` remove, `alt-p` toggle preview, `alt-1`…`7` tabs), so
new actions join them rather than reaching for `ctrl+`. The remaining keys are
navigation, not actions, and keep their conventional bindings: `Enter` accepts
(switches), `tab` / `shift-tab` cycle the preview tabs, and `ctrl-u` / `ctrl-d`
scroll the preview half a page (the vim/less convention). Bare digits `1`-`7`
stay reserved for the query.

So the new built-ins are `alt+`:

| Action | Key | Notes |
|--------|-----|-------|
| Copy branch name | `alt-y` | "yank" |
| Open PR/MR URL | `alt-o` | no-op when the row has no URL |
| Reload list | `alt-l` | "reload"; `alt-r` is taken (remove) |

### The cost: worktrunk has no clipboard or open abstraction

`grep` across `src/` finds no `$EDITOR`, no clipboard (`pbcopy`/`xclip`/
`clip.exe`), and no browser-open (`open`/`xdg-open`/`start`) helper. Copy and
open therefore need a small cross-platform module before they can be built-ins
that work without configuration. Reload needs none of that, so it's the
low-friction first win.

Two ways to deliver copy and open:

- **Build the helper.** A ~30-line `clipboard_copy(text)` / `open_url(url)`
  module dispatching on `cfg!(target_os = ...)`. Then `alt-y` and `alt-o` are
  real built-ins.
- **Ship them as default custom shortcuts** (Part 2), templates like
  `pbcopy <<< {{ branch }}` and `open {{ pr_url }}`. Zero new Rust, but not
  cross-platform out of the box, and bare-clipboard commands differ per OS.

Recommendation: build the helper for copy (it's tiny and universal) and gate
open on the same helper. Reload ships independently and first.

### Implementing reload

`alt-l:reload(refresh)` routes `refresh` into `PickerCollector::invoke`, which
re-runs `collect::collect` against a fresh `Repository` and streams the new item
list back, the same shape as the `alt-r` removal path. This is more than a
one-liner: the startup flow builds the collect pipeline once and the handler
holds the only live `tx`. A reload re-enters that pipeline. Moderate effort,
self-contained.

## Part 2: custom template shortcuts

### The gap they fill

`[aliases]` already lets a user run a template, but always against the **current**
worktree (`build_hook_context` reads the invoking worktree's repo/branch/path).
The picker is about **other** worktrees. A custom picker shortcut runs a template
against the **selected** row without first switching to it: copy that branch's
name, open that worktree's diff in a GUI editor, kick a CI run for that branch.
Nothing today covers acting on a worktree you're only pointing at.

### Config schema

`[switch.picker]` already exists (`SwitchPickerConfig` in
`src/config/user/sections.rs`), holding the picker's `pager` preference. Add a
shortcuts map beside it:

```toml
[switch.picker.shortcuts]
alt-e = "code {{ worktree_path }}"
alt-b = "gh workflow run ci.yml --ref {{ branch }}"
alt-d = "git -C {{ worktree_path }} difftool {{ default_branch }}"
```

The keys avoid the built-ins from Part 1; a shortcut on a key that's already a
built-in (or `alt-y` / `alt-o` from Part 1) is rejected at load (see "Key
validation").

The Rust shape mirrors `[aliases]` (`BTreeMap<String, CommandConfig>`) and
`[list]` custom-columns (`BTreeMap<String, String>`):

```rust
// In SwitchPickerConfig
#[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
pub shortcuts: BTreeMap<String, String>,
```

Keys are skim key names (`alt-y`, `alt-o`). Values are minijinja templates,
the same dialect as aliases and hooks. Start with a bare `key = "template"` map
rather than an array of tables with descriptions; it matches the existing
precedent and leaves room to grow if the controls line later needs labels.

### User config only

This is the central decision, and it follows from the approval model rather than
being imposed on top of it.

worktrunk already splits trust by config source (`src/commands/alias.rs`
docstring, `HookSource` in `src/commands/hook_filter.rs`): **user-config
commands are trusted and skip approval; project-config commands require approval**
keyed by project id in `~/.config/worktrunk/approvals.toml`. The picker honors
this read-only — `approved_removal_plan` runs a removal's project hooks only when
they're already approved (`HookPlan::approve_readonly`, no prompt) and drops the
rest, because the picker can't show an approval prompt mid-render.

A project-config shortcut is technically buildable. It would route through
`approve_readonly` exactly as removal hooks do: run only when its exact command
string is already in `approvals.toml`, drop otherwise, no prompt. So the security
invariant holds either way; the question is whether project shortcuts earn their
keep, and they don't:

- Approval is keyed by the exact command string and granted at operation gates
  (`wt merge`, `wt remove`, `wt switch --create`). A picker-only shortcut command
  is unlikely to match anything approved at one of those gates, so in practice
  `approve_readonly` drops it and the key does nothing, with no explanation.
- The user can't fix that from the picker, because it can't prompt mid-render.
  The only way to approve a picker shortcut would be a new approval surface
  (a one-line modal in the picker, or auto-inheriting a prior gate's approvals),
  which is real new machinery for a feature whose value is convenience.

So user-only is a deliberate scope choice, not a technical necessity: it trades
away project-shipped shortcuts (which would usually be inert anyway) for a
feature with no dead keys and no new approval surface. It also matches where
keybindings already live: `[switch.picker]`, `worktree-path`, and `[list]`
custom-columns are all user-only (a field declared on `UserConfig` but not
`ProjectConfig` is rejected in project config by the unknown-field check). A
project that wants to suggest shortcuts documents them; it doesn't ship them as
executable config.

Because the picker never loads project-config shortcuts, the "Project Commands
Run Only After Approval" invariant is satisfied structurally: there is no project
command in this path to gate.

### Template variables

A shortcut template expands against the selected row. The picker row carries a
frozen `Arc<ListItem>` snapshot plus a live `pr_status` slot
(`src/commands/picker/items.rs`). The variables that are reliably present when a
key fires are the synchronous, row-identity fields; async fields (ahead/behind,
upstream) are unset until their task lands and shouldn't be promised.

Recommended set, reusing the names `expand_template` already defines
(`src/config/expansion.rs`):

| Variable | Source | Worktree row | `--prs` row | Branch-only row |
|----------|--------|:---:|:---:|:---:|
| `branch` | `ListItem.branch` | ✓ | ✓ | ✓ |
| `commit` | `ListItem.head` (full SHA) | ✓ | ✓ | ✓ |
| `short_commit` | `ListItem.short_sha` | ✓ | ✓ | ✓ |
| `worktree_path` | `WorktreeData.path` | ✓ | — | — |
| `worktree_name` | basename of path | ✓ | — | — |
| `default_branch`, `repo`, `owner`, `remote`, `remote_url` | repo metadata | ✓ | ✓ | ✓ |
| `pr_number`, `pr_url` | live `pr_status` / `--prs` entry | async | always | — |

`pr_number` / `pr_url` are async on worktree rows: the picker reads them from the
live `pr_status` slot, populated only after the CI fetch lands. They're always
present on `--prs` rows (whose token is `pr:N` / `mr:N`). `worktree_path` /
`worktree_name` are absent on a branch-only row.

An absent variable is not a silent empty argument. minijinja runs in `SemiStrict`
mode, so a bare `{{ pr_url }}` on a row without one raises an undefined-value
error and the command doesn't run (logged, not executed). A guard turns that hard
error into a clean no-op: `{% if pr_url %}open {{ pr_url }}{% endif %}`. So
guards are about graceful skipping, not safety.

Validation reuses the existing system by adding a new
`ValidationScope::PickerShortcut` variant to `src/config/expansion.rs` (the enum
has `Hook` / `SwitchExecute` / `Alias` today): its available set is `base_vars()`
plus `pr_number` / `pr_url`. `validate_template` then runs at config load, so a
typo like `{{ brnach }}` is a config error listing the available variables, not a
dead key discovered at runtime. Detecting an *unguarded* row-specific variable
(a `{{ pr_url }}` not wrapped in `{% if %}`) is out of reach for
`undeclared_variables`, which can't see guard structure; the SemiStrict runtime
error above is the backstop.

### Execution mechanism

The constraint: the expanded command is arbitrary shell (parens, `&&`, pipes),
so it can never go into a skim bind body. Expansion must happen in worktrunk Rust
at key-press time, against the current selection. Three routes reach Rust from a
key; weighed against the skim constraints:

**(A) `reload` into the `PickerCollector` — recommended.** Add a `run` verb
beside `remove`. The bind is `format!("{key}:reload(run {id} {{}})")`. skim
hands `run <id> <token>` to `invoke`, which looks the row's variables up by
`output()` token in a side table (built at skeleton time alongside
`shared_items`, so no cross-thread `downcast` — the same reason `alt-r` decodes
`output()` instead of downcasting), expands via
`expand_template(template, &vars, ShellEscapeMode::Posix, repo, name)`, and spawns
the command on a background thread, exactly as `do_removal` spawns git work. One
expansion site, full already-resolved row context (including the live
`pr_status` slot), no subprocess, no network.

Three wrinkles:
- `reload` resets the cursor to the top, so the run injects the PR #3199
  `reposition_cursor_action` with the target set to the current row. For an
  unchanged list the matcher has already settled, so the reposition lands on its
  first or second arm rather than running the full settle loop. There is still a
  re-render cycle and a possible brief cursor flash on every press, the same
  correction `alt-r` makes once per removal. Whether that flash is acceptable for
  a key a user might hit repeatedly is the central cost of route (A) and the main
  reason route (B) exists; it should be confirmed on a real picker before
  committing, not assumed away.
- A background command's stdout/stderr would corrupt skim's frame, so it routes
  to a per-shortcut log under `.git/wt/logs/`, keyed by the shortcut key through
  the existing `HookLog` `sanitize_for_filename` rule (which already owns log
  naming and avoids collisions). Copy and open produce no output anyway.
- Failures are silent by default: a non-zero exit lands in the log, not on the
  frame, because the picker owns the terminal. Surfacing it (a one-line marker in
  the header or preview) is an open question, not a blocker; fire-and-forget with
  a log is the honest default and matches how background hooks already behave.

**(B) `execute-silent` into a hidden `wt` helper — viable alternative.** Bind
`{key}:execute-silent(wt switch --picker-run {id} {})`. The body is paren-free
and `+`-free, so it satisfies the skim constraints, and `execute-silent` keeps
its output off the frame (the `alt-N` tab binds rely on the same containment).
The helper re-derives the row context from the `output()` token (a path, branch,
or `pr:N`) in a fresh process and expands there, the way `wt step eval` already
expands standalone. Upside: no `reload`, no cursor reset, no list re-stream, a
lighter touch on the render loop. Downside: a `wt` subprocess per key-press, and
token-only context — `pr_url` for a worktree row isn't in the token, so the
helper would re-fetch (reaching the network from a key-press) or be limited to
`--prs` rows where the token is `pr:N`. `execute-silent` is fire-and-forget in
skim 4.x (the race that broke an earlier `alt-r` attempt), which is fine here
because a side-effect feeds nothing back into skim.

**(C) `Action::Custom` — unsuitable as the entry point.** A `Custom` callback
can't run a shell command (it holds `&mut App` on the render loop), and no
bind-string maps a key to it. It's the right tool for the cursor reposition in
(A), not for launching the command.

Recommendation: **(A)**. It keeps a single expansion path with full context and
reuses three pieces the picker already maintains (`PickerCollector::invoke`, the
background-thread spawn, `reposition_cursor_action`). (B) is the fallback if the
`reload` round-trip per key-press proves too heavy in practice.

Whichever route, the spawn is a new in-process command site, so per CLAUDE.md it
constructs a `CommandTrace` (start before spawn, `complete`/`fail` after) so it
shows up in `wt-perf timeline` rather than as an unattributed gap. The trace is
created and resolved on the background thread that runs the command, the same
threading as `do_removal`, with the shortcut key as its context label.

### Semantics: run and stay, side-effects only

One canonical behavior: expand against the selected row, run in the background,
the picker stays open, output to a log. This fits what custom shortcuts are for
— copy, open, notify, kick a build — and it's the 90% case peer tools bind the
same way.

A shortcut runs against the single selected row; the picker is single-select
(`multi(false)`). Multi-row actions belong in a dedicated `wt` command, not a
picker shortcut.

The behavior deliberately does **not** cover:

- **Switching.** That's Enter. A background command can't redirect the parent
  shell's directory anyway; only the `accept` path drives the cd directive.
- **Interactive foreground commands** (an editor in the same terminal). skim owns
  the terminal while the picker is open, so a blocking TUI command can't share
  it. GUI editors that fork (`code`, `subl`) work because they return
  immediately.
- **Mutating the worktree set.** A shortcut that removes or creates a worktree
  leaves the picker's list stale: route (A) re-streams the list it already has,
  and there's no bespoke list-surgery like `alt-r` performs. The list refreshes
  on the next `alt-l` reload. No runtime guard intercepts this: the command is
  the user's own, the same trust as a `[aliases]` entry that worktrunk already
  runs unguarded, so detecting and blocking "looks like a mutation" would be
  inconsistent special-casing. Removal has a dedicated key (`alt-r`); a custom
  shortcut is for side-effects that don't change which rows exist.

### Data safety and signal handling

- **Data safety.** Custom shortcuts run arbitrary user commands, but only
  user-authored ones from `~/.config`, the same trust boundary as a user alias.
  No project code runs. A shortcut that does something destructive is the user's
  own command, exactly as `[aliases]` already allows.
- **Signal handling.** The command runs through `shell_exec::Cmd`, so a Ctrl-C
  forwards to the child via the existing `signal_hook` handler. Because the
  command is background and the picker continues, the foreground loop-abort
  policy (which governs hook and alias pipelines) doesn't apply; there is no loop
  to break.

### Key validation

At config load, reject a shortcut key that collides with a built-in bind
(`enter`, `alt-c`, `alt-r`, `alt-p`, `alt-y`, `alt-o`, `alt-l`, `alt-1`…`alt-7`,
`tab`, `shift-tab`, `ctrl-u`, `ctrl-d`) or with a bare digit (which must reach
the query). A collision is a config error naming the conflict, not a
last-writer-wins surprise.

## Phasing

1. **`alt-l` reload.** Pure worktrunk, no OS dependency, immediately useful in
   multi-agent and external-change workflows. Exercises the `reload`-verb plumbing
   that custom shortcuts reuse.
2. **Clipboard/open helper + `alt-y` / `alt-o` built-ins.** Small
   cross-platform module; the two highest-frequency row actions.
3. **Custom shortcuts.** `[switch.picker.shortcuts]`, `ValidationScope::PickerShortcut`,
   the `run` verb, key validation. Ship copy/open as documented examples so the
   feature has a concrete starting point.

## Open questions

1. **Route (A) vs (B).** Is the per-key-press `reload` round-trip of (A)
   acceptable, or is the lighter `execute-silent` helper of (B) worth its
   subprocess-per-press and token-only context? (A) is the recommendation; this
   is the one call a reviewer might overturn.
2. **`pr_url` on worktree rows.** Promise it best-effort (unset until the CI
   fetch lands, guarded by `{% if %}`), or restrict URL-dependent shortcuts to
   `--prs` rows where it's always present?
3. **Visual feedback.** Should a fired shortcut flash a one-line confirmation, or
   stay silent? skim has no footer slot (a prior finding), so feedback would have
   to ride the preview or header.

## Appendix: implementation map

| Concern | File | Anchor |
|---------|------|--------|
| Bind block, `alt-r` reload, reposition action | `src/commands/picker/mod.rs` | `.bind(vec![...])`, `PickerCollector::invoke`, `reposition_cursor_action` |
| Row data reachable at fire time | `src/commands/picker/items.rs` | `WorktreeSkimItem`, `output()` |
| `--prs` row token and fields | `src/commands/picker/prs.rs` | `PrSkimItem` |
| Template expansion + scopes | `src/config/expansion.rs` | `expand_template`, `validate_template`, `ValidationScope`, `vars_available_in` |
| Variable names and sources | `src/config/expansion.rs` | `ACTIVE_VARS`, `REPO_VARS` |
| Approval / trust model | `src/commands/hook_plan.rs`, `src/commands/hook_filter.rs`, `src/commands/command_approval.rs` | `approve_readonly`, `HookSource` |
| Picker's read-only approval use | `src/commands/picker/mod.rs` | `approved_removal_plan` |
| Config section home | `src/config/user/sections.rs` | `SwitchPickerConfig` |
| User-config docs + regen | `src/cli/mod.rs` | `USER_CONFIG` block; then `cargo test --test integration test_docs_are_in_sync` |
