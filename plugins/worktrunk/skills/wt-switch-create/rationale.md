# wt-switch-create design rationale

The design of the skill's create-and-enter flow, and the verified harness
behavior behind it. The flow is error-driven; predictive guards and extra
routes were tried and cut.

Every claim here was verified against primary sources (2026-06-11 to
2026-06-17): Claude Code 2.1.173, with the path-entry and working-directory
logic re-confirmed live and against the 2.1.177 binary, plus official docs at
code.claude.com/docs; and wt v0.57.0-16-g371d28662 (live runs in a scratch
repo). Entry — the confirmation, both `EnterWorktree` routes, and what each
leaves behind at exit — was re-run live on 2026-07-28 against Claude Code
2.1.220 and wt v0.69.2. The confirmation hook and `cd`-before-entry for another
repo were verified live on 2026-09-16 against Claude Code 2.1.273 (#4149).
Re-verify against current versions before relying on a specific behavior; the
*shape* of the argument should outlive the details.
Binary symbol names are deliberately omitted — they re-minify every build.

## The design

1. `EnterWorktree({name})` for a new branch in this repo. It creates through
   worktrunk's `WorktreeCreate` hook (`wt switch --create`), so the result is
   an ordinary `wt` worktree, and it passes no `path`, which is what keeps M2's
   confirmation from firing. It fails on an existing branch and from a session
   that already entered a worktree, and it has no repo targeting.
2. Otherwise `wt -C <repo> switch --create <branch> --no-cd --format=json` in
   Bash, then, for another repo, `cd <repo>`, then `EnterWorktree({path})`.
   `wt` solves repo targeting (`-C` works from anywhere), existing-branch
   handling (rerun without `--create`), and machine-readable output (`.path`
   on stdout, status on stderr). Creating in another repo is fine; only
   *entering* the result is constrained, and both constraints read the
   repository from the cwd: the tool re-roots only within it (M2), and
   worktrunk's hook answers the confirmation only for that repository's
   worktrees ("The confirmation hook"). The `cd` satisfies both. Entry that someone declined
   ends there, with the worktree left unentered (M2).
3. On a tool error (or a denial with no user behind it), the session can still
   work there iff the path sits
   inside a directory it's allowed in (an `additionalDirectories` entry). A
   single `cd <path>` discovers which: it sticks when reachable, resets when
   not. Reachable → work in place. Unreachable → escalate, because the agent
   can't enlarge that set itself: ask the user to add the repo or a parent
   (e.g. `~/workspace`) to `additionalDirectories`, or `/add-dir <path>`. A
   one-line, set-once handback, not a silent degrade. A `cd <repo>` that
   resets reaches the same handback without attempting entry, which would only
   stop at a confirmation the hook can't answer and then fail.

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
  untracked files included — the mid-session carry-across in the skill's
  creation step.

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
  asking. A `PermissionRequest` hook's `allow` decision answers it in place of
  the click, and a session that can't prompt runs the same hooks and denies
  only when none decides (hooks docs, "PermissionRequest"). Worktrunk's hook
  is one ("The confirmation hook").
  `EnterWorktree({name})` passes no `path`, so it never asks.
- **Reading the failure:** step 3 splits tool errors from denials
  structurally rather than by parsing the denial's wording. The tool's own
  rejections are verbatim and graceful (`Cannot enter …`), so they key the
  recovery; a denial of the call defaults to being the user's answer, the
  no-user case an exception keyed on the denial saying the session couldn't
  prompt. The default falls on the safe side because denial texts don't
  classify reliably: a typed "no" arrives as the generic `The user doesn't
  want to proceed with this tool use`, naming neither the tool nor the
  confirmation, and in blind tests every wording that asked the agent to
  recognize it — branches labeled by who refused, "the denial reports a
  decision", even that string quoted as an example — sent the agent into the
  recovery, into the worktree the user had just declined. A recovery that
  follows a denial invites that routing, so the denial branch leads with
  stopping. The only decision that reaches step 3 is that answer — a
  `permissions.deny` entry for `EnterWorktree`, with or without an argument
  pattern, drops the tool from the session instead, so there is no call to
  deny.
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

### The confirmation hook

The plugin's `PermissionRequest` command runs the hidden
`wt config plugins claude approve-enter-worktree`, which reads the hook payload
and prints an `allow` decision when the call is `EnterWorktree` and its `path`
is a worktree of the repository the payload's `cwd` is in, at the path the
`worktree-path` template gives its branch. Anything else leaves stdout empty
and exits 1, which leaves the dialog (or, where no dialog can show, the denial)
in place. The tool's own validation still runs after an approval.

The rule extends Claude Code's exemption rather than overriding its check.
Claude Code enters a worktree under `.claude/worktrees/` without asking because
that location is its own convention, whichever name the model picked;
worktrunk's template location is the same kind of convention, and in the
default layout it holds every worktree `wt` creates. A worktree registered
anywhere else still asks, so a `git worktree add` into an arbitrary directory
(approvable in auto mode or by an allow rule) doesn't carry the permission root
there unconfirmed. The rule is not a boundary against a session that rewrites
the repository's own git config: `core.worktree`, `worktrunk.default-branch`,
and the remote URL all feed the template, and a session able to run those
commands can already write wherever an entry would let it.

The payload's `cwd` follows a shell `cd`, which is why step 3 `cd`s into
another repo before entering. The check lives in `wt` because it is
`is_worktree_at_expected_path`, the same test behind `wt list`'s
`branch_mismatch`; a script over `wt list --format=json` would inherit that
command's user config, where `list.full` adds CI fetches and LLM summaries
(33s measured on a 54-worktree repository) and `list.json-schema` and `list.branches` change
the output's shape. A `wt` too old to have the subcommand fails it, which
leaves the dialog as it was before the hook existed.

The approval shares one `hooks.json` command with the 💬 marker,
`wt … approve-enter-worktree || wt … marker set 💬`, because Claude Code runs
all matching hooks in parallel. A separate `EnterWorktree` entry would still
fire the catch-all marker hook, and the launch worktree would read 💬 while the
session works on. With one command, an approval skips the marker and every
other permission request sets it as before. The `permission_prompt`
notification can't set it either: Claude Code sends it only after a shown
prompt has waited about six seconds.

Verified live in `--permission-mode default` sessions: a same-repo entry, and a
cross-repo entry after `cd`, both reported "Allowed by PermissionRequest hook"
with no dialog, and the launch repo's marker still read 🤖 afterwards. The same
cross-repo call without the `cd` showed the dialog and set 💬, and so did a
worktree registered off the template path. An approved entry into the main
worktree, which sits at the default branch's expected path, was still refused
by the tool.

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

### What `EnterWorktree({name})` costs, and where it stops

The `name` route is worth having because it never asks M2's confirmation, with
or without worktrunk's hook, and it creates and enters in one call. The same
plugin hook backs `isolation: "worktree"` agents, so the worktree it produces is
the one `wt` would have made either way. Three properties come with it, all
verified live:

1. **An untouched worktree is cleaned up at exit, branch included.** With no
   changed files, no commits, and no user-set session title, the exiting
   session removes it through the plugin's `WorktreeRemove` hook, which is
   `wt remove`, so a clean fully-merged branch goes too. Verified end-to-end:
   `EnterWorktree({name: "probe"})`, then `/exit`, leaves neither `repo.probe`
   nor the `probe` branch; one untracked file is enough for exit to report
   "Keeping worktree…" and leave both. This is a feature at this scale — a
   research task that wrote nothing leaves nothing to prune — and it is the
   reason step 2's worktrees are not described as durable. Path-entered
   worktrees are always left in place ("worktree at <path> left in place").
2. **It hard-fails on an existing branch.** The hook runs `wt switch --create`,
   and a nonzero hook exit fails creation outright, with no git fallback
   (binary: "Other exit codes - worktree creation failed"; docs: "the hook
   replaces the default git behavior"). The failure is clean: `wt`'s own
   `✗ Branch <branch> already exists` surfaces and nothing is created.
3. **It only works on a session's first entry, and only in its own repo.** From
   a session already in a worktree it returns "Already in a worktree session.
   Pass `path` to switch into another existing worktree", and it has no `-C`.

Each failure names the route out, which is why step 3 needs no pre-check: try
the cheap call, read the error, fall back. The hook contract (stdout's last
non-empty line must be an existing directory) stays the hook's business, since
the skill reads only the tool result.

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

- Another repo is reachable only through `additionalDirectories` ("How they
  compose", above); outside it the skill escalates for the one-line config add
  rather than degrading to absolute-paths mode.
- A pinned or already-in-worktree session can't even re-enter a *same-repo*
  sibling worktree (the stricter `.claude/worktrees/` check); it lands in the
  same reachability test and the same escalation.
- Where the plugin's hooks don't run, or the worktree is off the
  `worktree-path` template (an existing branch whose worktree lives elsewhere,
  a detached worktree), step 3's entry asks the user to confirm, once per call,
  and a background session waits at it until someone attaches (M2).
  `permissions.allow` can't answer it.
- A subagent's `cd` doesn't carry to its next call, and it gets no reset notice
  (M1), so from a subagent step 3 can't enter another repo's worktree: the
  entry reaches the hook with the subagent's own repo as `cwd`.
- `wt switch --create` is not idempotent. If that ever changes upstream
  (enter-if-exists), step 3's existing-branch retry collapses to nothing, and
  the hook stops failing on an existing branch, which removes one of the two
  reasons step 3 exists.
