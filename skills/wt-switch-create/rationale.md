# wt-switch-create design rationale

Why the skill is one route (`wt` in Bash → `EnterWorktree({path})`) plus one
error-driven fallback, and not the guard-heavy multi-route flow it replaced.
Every claim here was verified against primary sources on 2026-06-11:
Claude Code 2.1.173 (binary inspection + live tool calls + official docs at
code.claude.com/docs) and wt v0.57.0-16-g371d28662 (live runs in a scratch
repo). Re-verify against current versions before relying on a specific
behavior; the *shape* of the argument should outlive the details.

## The design

1. Create the worktree with `wt -C <repo> switch --create <branch> --no-cd
   --format=json` in Bash. `wt` already solves repo targeting (`-C`),
   existing-branch handling (rerun without `--create`), and machine-readable
   output (`.path` on stdout, status on stderr).
2. Re-root the session with `EnterWorktree({path})` — the only supported way
   to move a Claude Code session's root.
3. If (2) is rejected, work in the worktree via absolute paths. Every
   rejection is a graceful, side-effect-free error, so one attempt plus a
   fallback covers every environment without predicting which one we're in.

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
- `wt -C <repo>` works from an unrelated cwd, so the skill never needs `cd`
  to create a worktree in another repo.
- `wt remove`: dirty worktree → refuses (exit 1, hints `--force`); clean but
  unmerged commits → removes the worktree, keeps the branch, hints
  `wt remove -D`; clean and merged → removes worktree and branch.
- The git stash is per-repo, shared across worktrees: `git stash push -u` in
  one worktree pops cleanly in another via `git -C <path> stash pop`,
  untracked files included — the mid-session carry-across in step 2.

## Claude Code behavior

### `EnterWorktree({path})` accepts worktrunk's sibling layout (load-bearing)

On first entry from a session not already in a worktree, the path may be ANY
worktree registered in `git worktree list` of the current repo — there is no
`.claude/worktrees/` restriction. Binary: the managed-location check is
conditional (`requireManagedLocation: <already-in-worktree-session>`); the
restriction applies only when switching from inside a worktree session or
from a pinned agent. Live-tested: a background session entered
`projects.ew-path-test` (sibling layout) by path.

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

### `cd` is not a re-rooting mechanism

The harness resets a `cd` that lands outside the session's working
directories, appending `Shell cwd was reset to <original>` to the result —
reproduced twice. Within the working directories `cd` persists and
`EnterWorktree` *does* read the moved cwd (live: after `cd /tmp`,
`EnterWorktree({path})` failed with "the current directory is not in a git
repository"). So a target repo outside the working directories can never be
reached by `cd`, which is exactly the case where a repo argument matters.
The earlier revision's "some sessions pin the cwd" was a misdiagnosis of
this reset; subagent threads additionally reset cwd between every Bash call
(binary: "Agent threads always have their cwd reset between bash calls").

### Why not `EnterWorktree({name})` + the WorktreeCreate hook

The previous revision's primary route. Rejected for the skill — the hook
itself stays; it is how `isolation: "worktree"` agents get worktrunk
worktrees:

1. **Hard-fails on existing branches.** The hook runs `wt switch --create`,
   and a nonzero hook exit fails worktree creation outright — there is no
   git fallback (binary: "Other exit codes - worktree creation failed";
   docs: "the hook replaces the default git behavior"). That forced a
   second route for existing branches; `path` entry needs no second route.
2. **No repo targeting.** `EnterWorktree({name})` creates the worktree
   wherever the session is rooted; combined with the `cd` reset above, the
   name route can never reach another repo.
3. **Wrong exit-time semantics for durable worktrees.** Worktrees created by
   `name` are tracked for session-exit cleanup: when the session has no
   user-set title, a clean worktree (no changed files, no new commits) is
   *silently auto-removed* at exit (binary: auto-remove iff zero changes,
   zero commits, and no session title; message "Worktree removed (no
   changes)"). A worktrunk worktree the user asked for should outlive the
   session (`wt list`, later `wt merge`). Path-entered worktrees get exactly
   that: left in place, no prompt ("worktree at <path> left in place").
4. **Hook contract details leak into the skill.** stdout's last non-empty
   line must be an existing directory, etc. — irrelevant when the skill reads
   `.path` from `wt --format=json` directly.

### Why no pre-emptive guards

The old flow opened with "if already inside a worktree, reuse it", a
`cd`-then-`pwd` check, and a pinned-cwd branch — each a workaround for a
hypothesis about the harness, one of which (pinned cwd) was simply wrong.
Since every `EnterWorktree` rejection above is graceful and creates nothing,
attempting it and falling back on error covers all of those environments
with zero prediction. Step 2 creates the worktree before step 3 attempts
entry, so even a rejected entry leaves a usable worktree for the fallback.

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

- Cross-repo and pinned/already-in-worktree sessions cannot be re-rooted at
  all (Claude Code restriction, not a skill gap); the fallback does the task
  in the right worktree via absolute paths, which loses only the implicit
  default cwd — not any capability.
- `wt switch --create` is not idempotent. If that ever changes upstream
  (enter-if-exists), step 2's existing-branch retry collapses to nothing.
