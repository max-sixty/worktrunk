# wt-switch-create design rationale

Why the skill is one same-repo route (`wt` in Bash → `EnterWorktree({path})`)
with a single up-front split for cross-repo tasks and an error-driven fallback,
and not the guard-heavy multi-route flow it replaced.
Every claim here was verified against primary sources (2026-06-11 to
2026-06-14): Claude Code 2.1.173, the path-entry logic re-confirmed against the
2.1.177 binary (`EnterWorktree`'s `LI6` validator and its call sites) plus live
tool calls and official docs at code.claude.com/docs; and wt
v0.57.0-16-g371d28662 (live runs in a scratch repo). Re-verify against current
versions before relying on a specific behavior; the *shape* of the argument
should outlive the details.

## The design

1. Settle the repo before creating anything. A session re-roots only within
   its own repo, so a task in another repo can't be entered from here; hand it
   to a session rooted in that repo. The rest assumes a same-repo task.
2. Create the worktree with `wt switch --create <branch> --no-cd --format=json`
   in Bash. `wt` already solves existing-branch handling (rerun without
   `--create`) and machine-readable output (`.path` on stdout, status on
   stderr).
3. Re-root the session with `EnterWorktree({path})` — the only supported way
   to move a Claude Code session's root.
4. If (3) is rejected (a same-repo edge case: already in a worktree session, or
   a pinned agent), work in the worktree via absolute paths. The rejection is
   graceful and side-effect-free, so attempt-then-fallback covers it without
   prediction.

## wt CLI and git behavior (all live-tested)

- `wt switch --create <branch>` exits 1 with `✗ Branch <branch> already
  exists` whenever the branch exists, with or without a worktree. Its own
  hint names the fix: rerun without `--create`.
- `wt switch <branch>` (no `--create`) exits 0 for an existing branch: it
  creates the worktree if missing (`"action":"created","created_branch":false`)
  or re-enters it (`"action":"existing"`). Per `wt switch --help`: "Without
  --create, the branch must already exist."
- Every `--format=json` variant carries `path` (absolute). Only the JSON goes
  to stdout; all human-readable status, including hook output, goes to stderr —
  which is what makes `.path` extraction safe.
- `wt remove`: dirty worktree → refuses (exit 1, hints `--force`); clean but
  unmerged commits → removes the worktree, keeps the branch, hints
  `wt remove -D`; clean and merged → removes worktree and branch.
- The git stash is per-repo, shared across worktrees: `git stash push -u` in
  one worktree pops cleanly in another via `git -C <path> stash pop`,
  untracked files included — the mid-session carry-across in step 3.

## Claude Code behavior

### `EnterWorktree({path})` accepts worktrunk's sibling layout (load-bearing)

The path validator (`LI6` in the 2.1.177 binary) derives the repo from the cwd
(`T = h$(u_())`; no repo argument), and applies a `requireManagedLocation` flag
the *caller* sets from session state: `requireManagedLocation: q != null`, where
`q` is the active worktree session (`z$()`). Pinned agents force it `true`. That
one flag picks which check runs, which is the whole reason the two cross-repo
rejections read differently:

- **`false`** (plain session, no active worktree session): the target must be in
  `git worktree list` of the cwd's repo, else *"is not a registered worktree of
  <repo>"*. Sibling layout passes — a fresh session enters its sibling worktree
  by path.
- **`true`** (already in a worktree session, or a pinned agent): the target must
  be under `<repo>/.claude/worktrees/` (`_F_ = join(repo, ".claude",
  "worktrees")`), else *"not under …/.claude/worktrees … managed by Claude
  Code"*. Sibling layout fails — a worktree session can't re-enter a worktrunk
  sibling worktree.

worktrunk's `WorktreeCreate` hook writes the sibling layout but never meets this
check: the hook runs only on create (`{name}`), the validator only on entry
(`{path}`). Both branches reject cross-repo regardless — no cwd has a worktree
of another repo in its `git worktree list`, and `.claude/worktrees/` is
per-repo. All four cells (plain / worktree-session × sibling / cross-repo), plus
the no-repo refusal, reproduced live against 2.1.177.

### Every rejection is graceful and side-effect-free

This is what lets the skill replace predictive guards with try-then-fallback.
Observed live, verbatim:

- Cross-repo: `Cannot enter worktree: <path> is not a registered worktree of
  <repo>. Run 'git -C <repo> worktree list' to see registered worktrees.`
- Nesting by name: ``Already in a worktree session. Pass `path` to switch
  into another existing worktree, or use ExitWorktree to leave this one
  before creating a new worktree.``
- Not in a repo: `Cannot enter an existing worktree: the current directory is
  not in a git repository.`
- Removing a path-entered worktree: `This session entered an existing worktree
  (<path>); it was not created by EnterWorktree, so this tool will not remove
  it. Use action: "keep" to return to <original cwd>…`

Binary-confirmed (not live-run): pinned agents (subagent `isolation:
"worktree"` or explicit cwd) can't create by `name` at all and require `path`;
their `path` entry is restricted to `.claude/worktrees/`, so sibling-layout
worktrees are rejected there too — same fallback applies.

### `cd` is a separate axis from re-root, gated by working-directory membership

Re-root (`EnterWorktree`) is scoped by repo, above. `cd` persistence is a
different gate: the session carries its primary cwd plus an
`additionalWorkingDirectories` set, and the harness resets any `cd` that lands
outside it, appending `Shell cwd was reset to <original>` to the result.
Reproduced both ways against 2.1.177: `cd /tmp` persisted (a working directory),
while `cd` into a sibling worktree or another repo reset.

So `cd` succeeds anywhere inside the working-directory set and never re-roots;
it only resets when stepping outside. `/add-dir <path>` (or the
`additionalDirectories` setting) enlarges the set, which makes `cd` into another
repo's worktree persist — the fix for *working* cross-repo in place, distinct
from re-rooting (the session's repo identity is unchanged either way). `/add-dir`
is a user-typed command, not an agent tool, so the agent can't enlarge the set
itself.

`EnterWorktree` reads the moved cwd: after `cd /tmp`, `EnterWorktree({path})`
failed with "the current directory is not in a git repository" (the no-repo
branch above). The earlier revision's "some sessions pin the cwd" was a
misdiagnosis of this reset; subagent threads additionally reset cwd between every
Bash call (binary: "Agent threads always have their cwd reset between bash
calls").

### Why not `EnterWorktree({name})` + the WorktreeCreate hook

The previous revision's primary route. Rejected for the skill — the hook
itself stays; it is how `isolation: "worktree"` agents get worktrunk
worktrees:

1. **Hard-fails on existing branches.** The hook runs `wt switch --create`,
   and a nonzero hook exit fails worktree creation outright — there is no
   git fallback (binary: "Other exit codes - worktree creation failed";
   docs: "the hook replaces the default git behavior"). That forced a
   second route for existing branches; `path` entry needs no second route.
2. **Wrong exit-time semantics for durable worktrees.** Worktrees created by
   `name` are tracked for session-exit cleanup: when the session has no
   user-set title, a clean worktree (no changed files, no new commits) is
   *silently auto-removed* at exit (binary: auto-remove iff zero changes,
   zero commits, and no session title; message "Worktree removed (no
   changes)"). A worktrunk worktree the user asked for should outlive the
   session (`wt list`, later `wt merge`). Path-entered worktrees get exactly
   that: left in place, no prompt ("worktree at <path> left in place").
3. **Hook contract details leak into the skill.** stdout's last non-empty
   line must be an existing directory, etc. — irrelevant when the skill reads
   `.path` from `wt --format=json` directly.

### One guard (cross-repo), and no others

Guard a case when the check is cheap and deterministic *and* the fallback is
worse, not merely different. Try-then-fallback when the state is opaque *and*
the fallback is fine. The removed guards and the kept one split exactly on
this.

The old flow opened with "if already inside a worktree, reuse it", a
`cd`-then-`pwd` check, and a pinned-cwd branch — each a workaround for a
hypothesis about opaque harness state, one of which (pinned cwd) was simply
wrong, and each landing in the same place a failed `EnterWorktree` lands
anyway. Both tests fail: the state is unknowable without trying, and the
fallback is fine. So they go, and step 4 just attempts entry and falls back.

The cross-repo split (step 2) passes both tests. The check is a path
comparison the agent already has the inputs for, with no harness state to
predict. And the fallback is genuinely worse: a substantial task run through
absolute paths breaks the skill's one promise and pays per-command friction,
and the right answer isn't a fallback at all but a different tool (a session
rooted in the target repo) that only an up-front decision can reach. Trying
first can't discover that — it just creates a worktree no one can enter.

## The hooks.json pipefail wrapper (agent-isolation path, not this skill)

`WorktreeCreate` pipes `jq | xargs wt | jq`; without `pipefail` the trailing
`jq` exits 0 on empty input and swallows a `wt` failure, so Claude Code saw a
"successful" hook with no path. Hook commands are spawned with an empty args
array and `shell: true` (binary), i.e. `/bin/sh -c` on Unix — bash 3.2 on
macOS but dash on many Linuxes. dash rejects `set -o pipefail` fatally
(`set` is a POSIX special builtin; no dash release through 0.5.12 supports
pipefail — only post-0.5.12 upstream git and distro backports such as
Debian's 0.5.12-7). And `/bin/sh -c` is evidently not universal: one user's
hooks ran under fish (worktrunk PR #2962), which has no shell options at
all. Hence the explicit `bash -c 'set -o pipefail; …'` wrapper. Verified
end-to-end: success prints the path and exits 0; an existing-branch failure
exits 1 with empty stdout.

## Known limits (deliberate)

- A session can't re-root across repos (Claude Code restriction, not a skill
  gap). The skill routes this before creating anything (step 2): substantial
  work to a session *started* in that repo (the only re-root), and work wanted
  in this conversation to `/add-dir <repo>` so `cd` there persists. Both beat
  the silent absolute-paths mode — every `cd` reset, an absolute prefix per
  command, no worktree cwd — which is what's left when neither lever is used.
  A non-interactive session has only that mode (re-root is impossible and
  `/add-dir` is user-typed), so a cross-repo task is best provisioned at launch:
  start the session in the target repo.
- Pinned and already-in-worktree sessions can't re-root into a sibling-layout
  worktree either; that surfaces as the step-4 rejection. There's no cross-repo
  handoff to make here: the worktree is already in the session's own repo, so
  absolute paths in place lose only the cwd convenience, not the right repo. (A
  pinned agent couldn't spawn a fresh rooted session anyway.)
- `wt switch --create` is not idempotent. If that ever changes upstream
  (enter-if-exists), step 3's existing-branch retry collapses to nothing.
