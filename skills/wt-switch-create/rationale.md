# wt-switch-create design rationale

Why the skill is create-then-reach: create the worktree with `wt`, enter it with
`EnterWorktree`, and when entry fails for any reason but a decision against it,
work in the worktree if it's reachable or escalate to the user to make it
reachable. Not the guard-heavy multi-route flow it replaced, and not the silent
absolute-paths fallback that read as a failure.

Every claim here was verified against primary sources (2026-06-11 to
2026-06-17): Claude Code 2.1.173, with the path-entry and working-directory
logic re-confirmed live and against the 2.1.177 binary, plus official docs at
code.claude.com/docs; and wt v0.57.0-16-g371d28662 (live runs in a scratch
repo). Entry — the confirmation, both `EnterWorktree` routes, and what each
leaves behind at exit — was re-run live on 2026-07-28 against Claude Code
2.1.220 and wt v0.69.2. Re-verify against current versions before relying on a
specific behavior; the *shape* of the argument should outlive the details.
Binary symbol names are deliberately omitted — they re-minify every build.

## The design

1. Create the worktree with `wt -C <repo> switch --create <branch> --no-cd
   --format=json` in Bash (omit `-C` for this repo). `wt` solves repo targeting
   (`-C` works from anywhere), existing-branch handling (rerun without
   `--create`), and machine-readable output (`.path` on stdout, status on
   stderr). Creating in another repo is fine; only *entering* the result is
   constrained.
2. Enter it with `EnterWorktree({path})` — the only way to re-root a Claude Code
   session, and it accepts only a worktree of the session's own repo. Entry
   that someone declined ends there, with the worktree left unentered (M2).
3. On any other refusal, the session can still work there iff the path sits
   inside a directory it's allowed in (an `additionalDirectories` entry). A
   single `cd <path>` discovers which: it sticks when reachable, resets when
   not. Reachable → work in place. Unreachable → escalate, because the agent
   can't enlarge that set itself: ask the user to add the repo or a parent
   (e.g. `~/workspace`) to `additionalDirectories`, or `/add-dir <path>`. A
   one-line, set-once handback, not a silent degrade.

Two independent harness facts underlie this — re-root is repo-scoped, and `cd`
persistence is working-directory-membership-scoped — detailed below.

## Why creation is unconditional

The recurring failure: handed a research or read-only task, the model decided
it didn't need isolation and skipped creation. Two wordings fed that. Scope's
"authorizes" read as permission, inviting a should-I test (user-level rules
that gate worktree creation behind an explicit request answer it "no"); an
ordering-only lead ("steps 1–3 come first") doesn't bind a model that decided
the steps don't apply. So the lead makes the invocation the explicit request
itself, and Scope states bounds rather than permission. Don't reintroduce
either wording.

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
  untracked files included — the mid-session carry-across in step 2.

## Claude Code behavior

A session works wherever its cwd is. Two mechanisms move it, and they *compose*:
`cd` moves the cwd (and so which repo `EnterWorktree` sees); `EnterWorktree`
re-roots within the repo the cwd is in. All verified live and against the
2.1.177 binary.

### M1 — `cd` (shell cwd)

Moves the shell cwd; the statusline and tool-path relativization
(`Write(foo/bar.py)`) follow it.

- **Gate:** the path must sit inside a configured working directory — the
  session's base cwd plus every entry in `permissions.additionalDirectories`
  (settings.json), `--add-dir` (launch), or `/add-dir` (mid-session).
- Inside → `cd` persists across Bash calls. Outside → the harness snaps it back
  and appends `Shell cwd was reset to <original>` to the result.
- **Repo-blind:** it checks only the path's location against that set, never
  which git repo owns the path. A different repo's worktree under `/tmp` is
  reachable when `/tmp` is configured.
- Within a *single* Bash call, `cd X && cmd` always works; the reset happens
  only *between* calls. (Subagent threads reset between every call.)

### M2 — `EnterWorktree({path})` (re-root)

Formally re-roots: sets the session's worktree home (tracked for exit) and its
cwd.

- **Gate:** a worktree of **the repo the current cwd resolves to**, by session
  state:
  - **Plain / first-entry session** → any worktree *registered to that repo*
    (`git worktree list`), anywhere on disk; in a multi-repo workspace, also
    one registered to a repo nested inside it.
  - **Already in a worktree-session, or a pinned agent** → only under that
    repo's `.claude/worktrees/`; rejects even same-repo siblings.
  - **cwd outside any git repo** → refuses entirely.
- **Confirmation:** the safety check runs on the two facts the call carries —
  a `path` argument, and a target outside the project's `.claude/worktrees/` —
  and asks before anything else runs, the dialog reading "permission-root
  relocation to `<path>` — a model-supplied worktree outside
  .claude/worktrees/". Yes/no only: no always-allow, nothing persisted after a
  yes, and `permissions.allow` entries for `EnterWorktree` (bare, `(*)`, or a
  path glob) don't suppress it. It asks wherever the session can prompt
  (`default`, `acceptEdits`, `auto`); `bypassPermissions` allows without
  asking, and a session that can't prompt denies without asking.
  `EnterWorktree({name})` passes no `path`, so it never asks.
- **Reading the failure:** step 3 routes on whether the denial reports a
  decision or an inability to ask. A decision stops the flow; an inability
  decides nothing, so it takes the recovery, which is what an unattended
  session wants. The distinction has to come from the result text, because a
  typed "no" arrives as a bare denial naming neither the tool nor a reason:
  given branches labeled by who refused, a blind agent reasons its way into the
  recovery and works in the worktree the user just declined. The only decision
  that reaches step 3 is that answer — a `permissions.deny` entry for
  `EnterWorktree`, with or without an argument pattern, drops the tool from the
  session instead, so there is no call to deny.
- Rejections are graceful and side-effect-free (nothing is created). The three
  the skill's own flow produces, verbatim (others exist — a worktree locked by
  another running session, the main working tree, a prunable registration):
  - registered-check: `Cannot enter worktree: <path> is not a registered
    worktree of <repo>. Run 'git -C <repo> worktree list' …`
  - managed-location: `Cannot enter worktree: <path> is not under
    <repo>/.claude/worktrees. Switching from this session is limited to
    worktrees managed by Claude Code …`
  - no repo: `Cannot enter an existing worktree: the current directory is not in
    a git repository.`

The repo is read from the cwd, so `EnterWorktree` never moves you to a different
repo on its own — it re-roots within whatever repo you're already standing in.
To re-root into *another* repo, `cd` into it first, then `EnterWorktree`.
Verified: from a worktrunk session, `cd` into a prql worktree under `/tmp`, then
`EnterWorktree` re-rooted within prql.

### How they compose

Whether you can work in — or re-root into — a repo reduces to whether your cwd
can be there, which is `cd`'s gate:

| Target | cwd reachable? | Result |
|---|---|---|
| Same repo (incl. its sibling worktrees) | always | `EnterWorktree` re-roots directly |
| Another repo under a configured dir (`~/workspace`, `/tmp`) | yes | `cd` in → working there; from a plain session, `EnterWorktree` re-roots within it too |
| Another repo outside every configured dir | no | unreachable — add it (or a parent) to `additionalDirectories`, or `/add-dir` |
| Outside any repo | n/a | no re-root possible |

`additionalDirectories` is the one master gate: once a repo, or a parent like
`~/workspace`, is in it, the session can `cd` into that repo's worktrees and both
work there and re-root within them. This is load-bearing for the same-repo path
too: a sibling worktree is registered to the repo, so `EnterWorktree` from a
plain session re-roots into the sibling `wt` creates. The agent **cannot**
enlarge that set itself (`/add-dir` is user-typed; the only automatic add is a
narrow symlink-resolving-to-the-same-cwd fixup), so a repo reachable by neither
the cwd nor config is a genuine handback to the user.

### Why `--no-cd`

The Bash tool is not a bare shell: Claude Code replays the user's shell startup
from a snapshot, so a user who installed wt shell integration runs the `wt`
wrapper function inside the tool. wt then runs with integration active, and a
plain `wt switch` hands the wrapper a cd directive that moves the tool's cwd — a
second, untracked re-root racing `EnterWorktree`. `--no-cd` skips the directive,
so `EnterWorktree` stays the single re-root. Verified: without `--no-cd`,
`wt switch <branch>` moved the session and the new cwd persisted to the next
Bash call. Where the user never installed integration (a fresh shell, CI) the
wrapper is absent and wt cannot cd regardless, so `--no-cd` is load-bearing on an
integrated machine and a no-op elsewhere. Don't drop it.

### Why not `EnterWorktree({name})` + the WorktreeCreate hook

A worktree could also be created by `EnterWorktree({name})`, which the worktrunk
plugin routes through its `WorktreeCreate` hook — landing at the same `wt`
sibling path, in `wt list`, and past M2's confirmation. Rejected for the skill
all the same; the hook itself stays, as it is how `isolation: "worktree"`
agents get worktrunk worktrees:

1. **Hard-fails on existing branches.** The hook runs `wt switch --create`,
   and a nonzero hook exit fails worktree creation outright — there is no
   git fallback (binary: "Other exit codes - worktree creation failed";
   docs: "the hook replaces the default git behavior"). That forced a
   second route for existing branches; `path` entry needs no second route.
2. **No repo targeting.** `EnterWorktree({name})` creates the worktree wherever
   the session is rooted; it can't make one in another repo, which `wt -C` does
   trivially in step 1.
3. **Deletes an untouched worktree, and its branch, at exit.** Worktrees
   created by `name` are tracked for session-exit cleanup: with no changed
   files, no new commits, and no user-set session title, exit removes the
   worktree silently — through the plugin's `WorktreeRemove` hook, which is
   `wt remove`, so a clean branch that is fully merged goes with it. Verified
   end-to-end: `EnterWorktree({name: "probe"})`, then `/exit`, leaves neither
   `repo.probe` nor the `probe` branch; the same session with one untracked
   file in the worktree exits with "Keeping worktree…" and leaves both in
   place. A read-only or research task is the case that ends untouched, and it
   is a case the skill deliberately gives a worktree that outlives the session
   (`wt list`, later `wt merge`). Path-entered worktrees get exactly that:
   left in place, no prompt ("worktree at <path> left in place").
4. **Hook contract details leak into the skill.** stdout's last non-empty
   line must be an existing directory, etc. — irrelevant when the skill reads
   `.path` from `wt --format=json` directly.

### Why escalate instead of grinding through absolute paths

The original incident worked a cross-repo task via absolute paths from a session
rooted elsewhere: every `cd` into the worktree reset, so each command needed an
absolute prefix, and the session never gained the worktree cwd. It produced
correct output but read as a failure. File tools (absolute paths) and `git -C`
are cwd-independent, so the work is *possible* that way — but it is friction the
user shouldn't absorb when the fix is one line of config.

So when the worktree is unreachable, the skill escalates with the concrete fix
rather than degrading silently. This is not a predictive guard: the skill
doesn't refuse to create cross-repo and doesn't guess reachability. It creates,
attempts entry, and lets a single `cd` reveal reachability — the escalation
fires only on an actual reset. Cheap to attempt, and the handback is actionable
and durable (a `~/workspace` entry, set once, covers every future cross-repo
task).

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

- `EnterWorktree` re-roots only within the repo the cwd is in, and the cwd can't
  `cd` outside the session's `additionalDirectories` (Claude Code restrictions,
  not skill gaps). So another repo is reachable only when it, or a parent like
  `~/workspace`, is in `additionalDirectories` — then the session `cd`s in and
  works there (and, from a plain session, can re-root within it). Otherwise the
  skill escalates for a one-line config add rather than degrading to
  absolute-paths mode. The agent can't enlarge the set itself (`/add-dir` is
  user-typed), so this handback is irreducible — but set once, it is permanent.
- A pinned or already-in-worktree session can't even re-enter a *same-repo*
  sibling worktree (the stricter `.claude/worktrees/` check); it lands in the
  same reachability test and the same escalation.
- Entry asks the user to confirm once per invocation, in any session that can
  prompt (M2). The skill absorbs that rather than routing around it: `{name}`
  buys the keystroke back at the cost above, and pointing a project's
  `worktree-path` into `.claude/worktrees/` satisfies the check but leaves
  worktrunk's default layout and puts `wt` and Claude Code in one directory, a
  combination we haven't run. Removing the prompt is upstream's to do, by
  asking once per repo rather than once per call.
- `wt switch --create` is not idempotent. If that ever changes upstream
  (enter-if-exists), step 2's existing-branch retry collapses to nothing.
