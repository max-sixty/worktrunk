# wt step

Run individual operations. The building blocks of wt merge — commit, squash, rebase, push — plus standalone utilities.

## Examples

Commit with LLM-generated message:

```console
$ wt step commit
◎ Generating commit message and committing changes... (2 files, +26)
  feat(validation): add input validation utilities
✓ Committed changes @ a1b2c3d
```

Manual merge workflow with review between steps:

```console
$ wt step commit
$ wt step squash
$ wt step rebase
$ wt step push
```

## Operations

- [`commit`](#wt-step-commit) — Stage and commit with [LLM-generated message](https://worktrunk.dev/llm-commits/)
- [`squash`](#wt-step-squash) — Squash all branch commits into one with [LLM-generated message](https://worktrunk.dev/llm-commits/)
- [`rebase`](#wt-step-rebase) — Rebase onto target branch
- [`push`](#wt-step-push) — Fast-forward target to current branch
- [`diff`](#wt-step-diff) — Show all changes since branching (committed, staged, unstaged, untracked)
- [`copy-ignored`](#wt-step-copy-ignored) — Copy gitignored files between worktrees
- [`eval`](#wt-step-eval) — Evaluate a template expression
- [`for-each`](#wt-step-for-each) — Run a command in every worktree
- [`promote`](#wt-step-promote) — [experimental] Swap a branch into the main worktree
- [`prune`](#wt-step-prune) — Remove worktrees and branches merged into the default branch
- [`relocate`](#wt-step-relocate) — [experimental] Move worktrees to expected paths
- [`tether`](#wt-step-tether) — [experimental] Run a command; kill its whole process tree when its worktree is removed
- [`<alias>`](https://worktrunk.dev/extending/#aliases) — Run a configured command alias

## Command reference

```
wt step - Run individual operations

The building blocks of wt merge — commit, squash, rebase, push — plus standalone utilities.

Usage: wt step [OPTIONS] <COMMAND>

Commands:
  commit        Stage and commit with LLM-generated message
  squash        Squash commits since branching
  rebase        Rebase onto target
  push          Fast-forward target to current branch
  diff          Show all changes since branching
  copy-ignored  Copy gitignored files to another worktree
  eval          Evaluate a template expression
  for-each      Run command in each worktree
  promote       [experimental] Swap a branch into the main worktree
  prune         Remove worktrees and branches merged into the default branch
  relocate      [experimental] Move worktrees to expected paths
  tether        [experimental] Run a command; kill its whole process tree when its worktree is
                removed

Options:
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

## wt step commit

Stage and commit with LLM-generated message.

See [LLM-generated commit messages](https://worktrunk.dev/llm-commits/) for configuration and prompt customization. Without a `[commit.generation]` command configured, the commit still happens — the message is built from the staged file names instead (`Changes to README.md`).

### Operating on another worktree

`--branch` commits in another worktree's branch without leaving the current one:

```console
$ wt step commit --branch feature
```

The branch must have a checked-out worktree. `--branch` re-roots the whole command: staging, hooks, and the commit all happen there. It has no effect on `--dry-run`, which always previews the current worktree.

### Hooks

`pre-commit` hooks run before the commit and abort it on failure; `post-commit` hooks run after it, in the background with their output logged. `--no-hooks` skips both. See [`wt hook`](https://worktrunk.dev/hook/).

### Options

#### Staging

Controls what to stage before committing:

| Value | Behavior |
|-------|----------|
| `all` | Stage all changes including untracked files (default) |
| `tracked` | Stage only modified tracked files |
| `none` | Don't stage anything, commit only what's already staged |

```console
$ wt step commit --stage=tracked
```

Configure the default in user config:

```toml
[commit]
stage = "tracked"
```

#### Dry run

Render the prompt, print the LLM command, generate the message, and exit without staging, running hooks, or committing:

```console
$ wt step commit --dry-run
```

Three sections are printed: the rendered prompt, the shell command that would invoke the LLM, and the message returned. The LLM call still happens — only the commit is skipped.

### Command reference

```
wt step commit - Stage and commit with LLM-generated message

Usage: wt step commit [OPTIONS]

Options:
  -b, --branch <BRANCH>
          Branch to operate on (defaults to current worktree)

      --stage <STAGE>
          What to stage before committing [default: all]

          Possible values:
          - all:     Stage everything: untracked files + unstaged tracked changes
          - tracked: Stage tracked changes only (like git add -u)
          - none:    Stage nothing, commit only what's already in the index

      --dry-run
          Preview prompt, command, and generated message without committing

  -h, --help
          Print help (see a summary with '-h')

Automation:
      --no-hooks
          Skip hooks

      --format <FORMAT>
          Output format

          JSON prints structured result to stdout after the commit completes.

          [default: text]
          [possible values: text, json]

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

## wt step squash

Squash commits since branching. Stages changes and generates message with LLM.

See [LLM-generated commit messages](https://worktrunk.dev/llm-commits/) for configuration and prompt customization. Without a `[commit.generation]` command configured, the squash still happens — the message lists the squashed commits' subjects under `Squash commits from <branch>` instead.

### Hooks

`pre-commit` hooks run before the squash commit and abort it on failure; `post-commit` hooks run after it, in the background with their output logged. `--no-hooks` skips both. See [`wt hook`](https://worktrunk.dev/hook/).

### Options

#### Staging

Controls what to stage before squashing:

| Value | Behavior |
|-------|----------|
| `all` | Stage all changes including untracked files (default) |
| `tracked` | Stage only modified tracked files |
| `none` | Don't stage anything, squash only committed changes |

```console
$ wt step squash --stage=none
```

Configure the default in user config:

```toml
[commit]
stage = "tracked"
```

#### Dry run

Render the prompt, print the LLM command, generate the squash message, and exit without resetting, running hooks, or committing:

```console
$ wt step squash --dry-run
```

Three sections are printed: the rendered prompt, the shell command that would invoke the LLM, and the message returned. The LLM call still happens — only the squash and commit are skipped.

### Command reference

```
wt step squash - Squash commits since branching

Stages changes and generates message with LLM.

Usage: wt step squash [OPTIONS] [TARGET]

Arguments:
  [TARGET]
          Target branch

          Defaults to default branch.

Options:
      --stage <STAGE>
          What to stage before committing [default: all]

          Possible values:
          - all:     Stage everything: untracked files + unstaged tracked changes
          - tracked: Stage tracked changes only (like git add -u)
          - none:    Stage nothing, commit only what's already in the index

      --dry-run
          Preview prompt, command, and generated message without squashing

  -h, --help
          Print help (see a summary with '-h')

Automation:
      --no-hooks
          Skip hooks

      --format <FORMAT>
          Output format

          JSON prints structured result to stdout after the squash completes.

          [default: text]
          [possible values: text, json]

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

## wt step rebase

Rebase onto target.

A rebase puts the branch's commits on top of the target, which is what [`wt step push`](#wt-step-push) needs — a push fast-forwards only if the target is an ancestor of the branch. `wt merge` runs this step as part of its pipeline; on its own it brings a branch up to date with a target that has moved, without merging into it.

The target is any commit: a branch, a tag, a SHA.

### Examples

```console
$ wt step rebase            # Rebase onto default branch
$ wt step rebase develop    # Rebase onto develop
$ wt step rebase v1.2.0     # Rebase onto a tag
```

### Outcomes

The first matching row wins:

| Branch and target | Result |
|-------------------|--------|
| The target is already an ancestor of the branch, with no merge commit in between | Nothing runs — `Already up to date` |
| The branch is an ancestor of the target, so it has no commits of its own | `Fast-forwarded to <target>` |
| Otherwise | The branch's commits replay onto the target's tip — refused outright if the two share no history |

A branch that merged the target into itself still rebases: the target is its ancestor, but the merge commit in between keeps the first row from applying.

When the target's local ref lags its upstream, the rows are measured against that upstream, which the result then names in place of the argument. [`wt merge`](https://worktrunk.dev/merge/) covers why.

### Conflicts

A conflicting commit leaves the rebase open rather than undoing it. The worktree keeps git's conflict markers, and the ways out are `git rebase --continue` once the conflict is resolved, `git rebase --skip`, or `git rebase --abort`. Until the rebase is settled, `wt step rebase`, `wt step squash`, `wt step push`, and `wt merge` refuse to run — as they do while any other git operation is open, a conflicted `git merge` included.

### Command reference

```
wt step rebase - Rebase onto target

Usage: wt step rebase [OPTIONS] [TARGET]

Arguments:
  [TARGET]
          Target branch, tag, or commit

          Defaults to default branch.

Options:
  -h, --help
          Print help (see a summary with '-h')

Automation:
      --format <FORMAT>
          Output format

          JSON prints structured result to stdout after the rebase completes.

          [default: text]
          [possible values: text, json]

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

## wt step push

Fast-forward target to current branch.

Despite the name, no commits leave the repository. The target branch's ref moves forward locally, and a worktree holding that branch is updated along with it. Publishing is a separate `git push` to the remote afterward.

The target is a branch, and must already be an ancestor of the current branch. One that has moved ahead is refused, and there is no force variant — [`wt step rebase`](#wt-step-rebase) puts the branch back on top of it first.

### Examples

```console
$ wt step push             # Fast-forward main to current branch
$ wt step push develop     # Fast-forward develop instead
$ wt step push --no-ff     # Merge commit instead of a fast-forward
```

### Target worktree

When the target branch has a worktree of its own, that worktree's files move to the new commits too. Uncommitted changes there never move: the update carries any file the push doesn't touch — staged or not — exactly where it is, and a change touching a file the push does change is refused upfront, naming the file. If the sync can't be applied for any reason — a conflicting file appearing in the race window after the check, or a busy index — the update is rolled back whole, leaving branch and worktree as they were.

A worktree that is still registered but whose directory is gone is refused as well, since nothing can be synced into it — `git worktree prune` clears the registration.

### Command reference

```
wt step push - Fast-forward target to current branch

Usage: wt step push [OPTIONS] [TARGET]

Arguments:
  [TARGET]
          Target branch

          Defaults to default branch.

Options:
      --no-ff
          Create a merge commit (no fast-forward)

  -h, --help
          Print help (see a summary with '-h')

Automation:
      --format <FORMAT>
          Output format

          JSON prints structured result to stdout after the push completes.

          [default: text]
          [possible values: text, json]

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

## wt step diff

Show all changes since branching. Includes committed, staged, unstaged, and untracked files.

This is what `wt merge` would include — a single diff against the merge base.

### Operating on another worktree

`--branch` diffs another worktree's branch without leaving the current one:

```console
$ wt step diff --branch feature
```

The branch must have a checked-out worktree.

### Extra git diff arguments

Arguments after `--` are forwarded to `git diff`:

```console
$ wt step diff -- --stat
$ wt step diff -- --name-only
$ wt step diff -- -- '*.rs'
```

The diff is pipeable to tools like `delta`:

```console
$ wt step diff | delta
```

### How it works

Equivalent to:

```console
$ cp "$(git rev-parse --git-dir)/index" /tmp/idx
$ GIT_INDEX_FILE=/tmp/idx git add --intent-to-add .
$ GIT_INDEX_FILE=/tmp/idx git diff $(git merge-base HEAD $(wt config state default-branch))
```

`git diff` ignores untracked files. `git add --intent-to-add .` registers them in the index without staging their content, making them visible to `git diff`. This runs against a copy of the real index so the original is never modified.

### Command reference

```
wt step diff - Show all changes since branching

Includes committed, staged, unstaged, and untracked files.

Usage: wt step diff [OPTIONS] [TARGET] [-- <EXTRA_ARGS>...]

Arguments:
  [TARGET]
          Target branch

          Defaults to default branch.

  [EXTRA_ARGS]...
          Extra arguments forwarded to git diff

Options:
  -b, --branch <BRANCH>
          Branch to operate on (defaults to current worktree)

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

## wt step copy-ignored

Copy gitignored files to another worktree. Eliminates cold starts by copying build caches and dependencies.

### Setup

Add to the project config:

```toml
# .config/wt.toml
[post-start]
copy = "wt step copy-ignored"
```

### Choosing source and destination

By default the copy runs from the primary worktree into the current one — what a `post-start` hook needs, since the new worktree is where the hook runs. `--from` and `--to` name either end by branch, so a copy can run between two worktrees from anywhere:

```console
$ wt step copy-ignored --from main --to feature   # between two named worktrees
$ wt step copy-ignored --from feature             # from feature into the current worktree
```

A branch named by `--from` or `--to` must have a worktree.

### What gets copied

All gitignored files are copied by default, except for built-in excluded directories: VCS metadata (`.bzr/`, `.hg/`, `.jj/`, `.pijul/`, `.sl/`, `.svn/`), tool-state (`.conductor/`, `.entire/`, `.worktrees/`), and nested worktrees. Tracked files are never touched. Discovery handles nested `.gitignore` files, global excludes, and `.git/info/exclude`. Existing files in the destination are skipped, so re-running is safe; `--force` overwrites them.

To limit what gets copied further, create `.worktreeinclude` with gitignore-style patterns. Files must be **both** gitignored **and** in `.worktreeinclude`:

```text
# .worktreeinclude
.env
node_modules/
target/
```

After `.worktreeinclude` selects entries, you can add more gitignore-style excludes in user config, per-project user overrides, or project config:

```toml
[step.copy-ignored]
exclude = [".cache/", ".turbo/"]
```

To copy nothing unless `.worktreeinclude` exists — matching Claude Code desktop, where the file is required — pass `--require-include`:

```console
$ wt step copy-ignored --require-include
```

Without `.worktreeinclude`, the command is a no-op (it reports that nothing was copied and why). With the file present, only matching files copy as above. To apply this across every repository, put the flag in a user-config hook: `post-start = "wt step copy-ignored --require-include"`.

### Common patterns

| Type | Patterns |
|------|----------|
| Dependencies | `node_modules/`, `.venv/`, `target/`, `vendor/`, `Pods/` |
| Build caches | `.cache/`, `.next/`, `.parcel-cache/`, `.turbo/` |
| Generated assets | Images, ML models, binaries too large for git |
| Environment files | `.env` (if not generated per-worktree) |

### Performance

Reflink copies share disk blocks until modified — no data is actually copied. For a 14GB `target/` directory:

| Command | Time |
|---------|------|
| `cp -R` (full copy) | 2m |
| `cp -Rc` / `wt step copy-ignored` | 20s |

Uses per-file reflink (like `cp -Rc`) — copy time scales with file count.

Use the `post-start` hook so the copy runs in the background. Use `pre-start` instead if subsequent hooks or `--execute` command need the copied files immediately.

### Background-hook priority (experimental)

When invoked from a background hook pipeline (`post-*` hooks), `wt step copy-ignored` self-lowers its CPU and I/O priority — `taskpolicy -b` on macOS, `nice -n 19` plus `ionice -c 3` on Linux — so it yields to interactive work. Foreground callers (`pre-*` hooks, direct interactive use) run at normal priority so the user isn't waiting on a throttled copy.

wt signals background-hook context by exporting `WORKTRUNK_FOREGROUND=-1` into every detached hook pipeline; `copy-ignored` inspects that variable on entry. The variable name is experimental and may change.

### Language-specific notes

#### Rust

The `target/` directory is huge (often 1-10GB). Copying with reflink cuts first build from ~68s to ~3s by reusing compiled dependencies.

#### Node.js

`node_modules/` is large but mostly static. If the project has no native dependencies, symlinks are even faster:

```toml
[pre-start]
deps = "ln -sf {{ primary_worktree_path }}/node_modules ."
```

#### Python

Virtual environments contain absolute paths and can't be copied. Use `uv sync` instead — it's fast enough that copying isn't worth it.

### Behavior vs Claude Code on desktop

The `.worktreeinclude` pattern is shared with [Claude Code on desktop](https://code.claude.com/docs/en/desktop), which copies matching files when creating worktrees. Differences:

- worktrunk copies all gitignored files by default; Claude Code requires `.worktreeinclude`. Pass `--require-include` to match Claude Code (copy nothing without `.worktreeinclude`)
- worktrunk uses copy-on-write for large directories like `target/` (see Performance above)
- worktrunk runs as a configurable hook in the worktree lifecycle

### Command reference

```
wt step copy-ignored - Copy gitignored files to another worktree

Eliminates cold starts by copying build caches and dependencies.

Usage: wt step copy-ignored [OPTIONS]

Options:
      --from <FROM>
          Source worktree branch

          Defaults to primary worktree.

      --to <TO>
          Destination worktree branch

          Defaults to current worktree.

      --dry-run
          Show what would be copied

      --force
          Overwrite existing files in destination

      --require-include
          Require .worktreeinclude to copy anything

  -h, --help
          Print help (see a summary with '-h')

Automation:
      --format <FORMAT>
          Output format

          JSON prints structured result to stdout after the copy completes.

          [default: text]
          [possible values: text, json]

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

## wt step eval

Evaluate a template expression. Prints the result to stdout for use in scripts and shell substitutions.

All [hook template variables and filters](https://worktrunk.dev/hook/#template-variables) are available.

### Examples

Get the port for the current branch:

```console
$ wt step eval '{{ branch | hash_port }}'
16066
```

Use in shell substitution:

```console
$ curl http://localhost:$(wt step eval '{{ branch | hash_port }}')/health
```

Combine multiple values:

```console
$ wt step eval '{{ branch | hash_port }},{{ ("supabase-api-" ~ branch) | hash_port }}'
16066,16739
```

Use conditionals and filters:

```console
$ wt step eval '{{ branch | sanitize_db }}'
feature_auth_oauth2_a1b
```

List the available template variables with `-v` (alongside the expansion, on stderr). The real block prints every variable in scope; this one is abridged:

```console
$ wt step eval -v '{{ branch }}'
○ eval template variables:
  branch                = feature/auth
  worktree_path         = /home/user/code/myproject.feature-auth
  …
  cwd                   = /home/user/code/myproject.feature-auth
○ eval source
  {{ branch }}
○ eval result
  feature/auth

feature/auth
```

### Command reference

```
wt step eval - Evaluate a template expression

Prints the result to stdout for use in scripts and shell substitutions.

Usage: wt step eval [OPTIONS] <TEMPLATE>

Arguments:
  <TEMPLATE>
          Template expression to evaluate

Options:
  -h, --help
          Print help (see a summary with '-h')

Automation:
      --format <FORMAT>
          Output format

          JSON prints {name, template, result} to stdout instead of the bare result.

          [default: text]
          [possible values: text, json]

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

## wt step for-each

Run command in each worktree. Executes sequentially with real-time output; continues past command failures.

A summary of successes and failures is shown at the end. A template-expansion error (a malformed `{{ … }}` argument) aborts the whole run; only command failures are tolerated and reported. Context JSON — a flat object of every template variable — is piped to stdin for scripts that need structured data.

### Arguments

Arguments after `--` are the program and its arguments — run directly, no shell.

```console
$ wt step for-each -- git status --short
$ wt step for-each -- npm install
```

For pipes, redirects, variables, or globs, wrap in `sh -c`:

```console
$ wt step for-each -- sh -c 'git status | wc -l'
$ wt step for-each -- sh -c 'echo $HOME && git pull'
```

### Template variables

Variables substitute into each argv element before exec. See [`wt hook` template variables](https://worktrunk.dev/hook/#template-variables) for the complete list and filters.

```console
$ wt step for-each -- echo 'Branch: {{ branch }}'
```

Each element is expanded fresh in every worktree, so `{{ branch }}` is that worktree's branch. An alias wrapping for-each renders templates earlier, in the invoking worktree; [deferring expansion in an alias](https://worktrunk.dev/extending/#deferring-expansion-to-a-nested-wt-command) shows how to keep a variable per-worktree.

### Examples

Pull updates in worktrees with upstreams (skips others):

```console
$ git fetch --prune && wt step for-each -- sh -c '[ "$(git rev-parse @{u} 2>/dev/null)" ] || exit 0; git pull --autostash'
```

### Command reference

```
wt step for-each - Run command in each worktree

Executes sequentially with real-time output; continues past command failures.

Usage: wt step for-each [OPTIONS] -- <ARGS>...

Arguments:
  <ARGS>...
          Command template (see --help for all variables)

Options:
      --format <FORMAT>
          Output format

          [default: text]
          [possible values: text, json]

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

## wt step promote

[experimental]

Swap a branch into the main worktree. Exchanges branches and gitignored files between two worktrees.

**Experimental.** Use promote for temporary testing when the main worktree has special significance (Docker Compose, IDE configs, heavy build artifacts anchored to project root), and hooks & tools aren't yet set up to run on arbitrary worktrees. The idiomatic Worktrunk workflow does not use `promote`; instead each worktree has a full environment. `promote` is the only Worktrunk command which changes a branch in an existing worktree.

### Example

```console
# from ~/project (main worktree)
$ wt step promote feature
```

Before:

```
  Branch   Path
@ main     ~/project
+ feature  ~/project.feature
```

After:

```
  Branch   Path
@ feature  ~/project
+ main     ~/project.feature
```

To restore: `wt step promote main` from anywhere, or just `wt step promote` from the main worktree.

Without an argument, promotes the current branch — or restores the default branch if run from the main worktree.

### Requirements

- Both worktrees must be clean
- The branch must have an existing worktree

### Gitignored files

Gitignored files (build artifacts, `node_modules/`, `.env`) are swapped along with the branches so each worktree keeps the artifacts that belong to its branch. Files are discovered using the same mechanism as [`copy-ignored`](#wt-step-copy-ignored) and can be filtered with `.worktreeinclude`.

The swap uses `rename()` for each entry — fast regardless of entry size, since only filesystem metadata changes. If the worktree is on a different filesystem from `.git/`, it falls back to reflink copy.

### Command reference

```
wt step promote - [experimental] Swap a branch into the main worktree

Exchanges branches and gitignored files between two worktrees.

Usage: wt step promote [OPTIONS] [BRANCH]

Arguments:
  [BRANCH]
          Branch to promote to main worktree

          Defaults to current branch, or default branch from main worktree.

Options:
  -h, --help
          Print help (see a summary with '-h')

Automation:
      --format <FORMAT>
          Output format

          JSON prints structured result to stdout after the promote completes. The mismatch warning
          still appears on stderr in JSON mode (safety signal).

          [default: text]
          [possible values: text, json]

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

## wt step prune

Remove worktrees and branches merged into the default branch.

Bulk-removes worktrees and branches that are integrated into the default branch, using the same criteria as `wt remove`'s branch cleanup. Stale worktree entries are cleaned up too.

In `wt list`, candidates show `_` (same commit) or `⊂` (content integrated). Run `--dry-run` to preview. See `wt remove --help` for the full integration criteria.

Locked worktrees and the main worktree are always skipped. The current worktree is removed last, triggering cd to the primary worktree. Pre-remove and post-remove hooks run for each removal; a candidate whose hooks include an unapproved project command is skipped with `(approval required)` (pre-approve with `wt config approvals add`, or pass `--yes`).

### Min-age guard

Worktrees and branches younger than `--min-age` (default: 1 day) are skipped. This prevents removing a worktree just created from the default branch — it looks "merged" because its branch points at the same commit.

```console
$ wt step prune --min-age=0s     # no age guard
$ wt step prune --min-age=2d     # skip worktrees younger than 2 days
```

### JSON output

`--format=json` prints one object per candidate to stdout. The two modes report different things, and name their fields accordingly: a live run reports `branch_outcome`, the executed outcome, using the vocabulary [`wt remove`](https://worktrunk.dev/remove/#json-output) documents; `--dry-run` reports `branch_deleted`, its prediction of whether the removal would take the branch, since it runs nothing to have an outcome. A dry run also carries `reason` and `target` (why the candidate qualifies, and what it was measured against).

### Examples

Preview what would be removed:

```console
$ wt step prune --dry-run
```

Remove all merged worktrees:

```console
$ wt step prune
```

### Command reference

```
wt step prune - Remove worktrees and branches merged into the default branch

Usage: wt step prune [OPTIONS]

Options:
      --dry-run
          Show what would be removed

      --min-age <MIN_AGE>
          Skip worktrees and branches younger than this

          [default: 1d]

      --foreground
          Run removal in foreground (block until complete)

      --format <FORMAT>
          Output format

          [default: text]
          [possible values: text, json]

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

## wt step relocate

[experimental]

Move worktrees to expected paths. Relocates worktrees whose path doesn't match the worktree-path template.

### Examples

Preview what would be moved:

```console
$ wt step relocate --dry-run
```

Move all mismatched worktrees:

```console
$ wt step relocate
```

Auto-commit and clobber blockers (never fails):

```console
$ wt step relocate --commit --clobber
```

Move specific worktrees:

```console
$ wt step relocate feature bugfix
```

### Swap handling

When worktrees are at each other's expected locations (e.g., `alpha` at
`repo.beta` and `beta` at `repo.alpha`), relocate automatically resolves
this by using a temporary location.

### Clobbering

With `--clobber`, non-worktree paths at target locations are moved to
`<path>.bak.<timestamp>` before relocating. If that name is already taken,
the move counts up (`…-2`, `…-3`, …) until it finds a free name, so an
existing backup is never overwritten.

### Main worktree behavior

The main worktree can't be moved with `git worktree move`. Instead, relocate
switches it to the default branch and creates a new linked worktree at the
expected path. Untracked and gitignored files remain at the original location.

### Dirty worktrees

Linked worktrees relocate as-is — `git worktree move` carries uncommitted
changes along. Only the main worktree skips when dirty (its `git switch`
refuses), unless `--commit` is passed.

### Skipped worktrees

- **Dirty main worktree** (without `--commit`) — use `--commit` to auto-commit first
- **Locked** — unlock with `git worktree unlock`
- **Target blocked** (without `--clobber`) — use `--clobber` to backup blocker
- **Detached HEAD** — no branch to compute expected path

### Command reference

```
wt step relocate - [experimental] Move worktrees to expected paths

Relocates worktrees whose path doesn't match the worktree-path template.

Usage: wt step relocate [OPTIONS] [BRANCHES]...

Arguments:
  [BRANCHES]...
          Worktrees to relocate (defaults to all mismatched)

Options:
      --dry-run
          Show what would be moved

      --commit
          Commit uncommitted changes before relocating

      --clobber
          Backup non-worktree paths at target locations

          Moves blocking paths to <path>.bak.<timestamp>. If that name is taken, counts up (…-2, …-3
          , …) to a free name.

  -h, --help
          Print help (see a summary with '-h')

Automation:
      --format <FORMAT>
          Output format

          JSON prints structured result to stdout after the relocate completes.

          [default: text]
          [possible values: text, json]

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

## wt step tether

[experimental]

Run a command; kill its whole process tree when its worktree is removed. Teardown is automatic and needs no pre-remove hook; the group gets SIGTERM then SIGKILL.

### Why

A `post-start` hook to start a long-lived process and a `pre-remove` hook to
stop it is usually enough. But `pre-remove` only runs when worktrunk removes
the worktree, so a `git worktree remove`, an `rm -rf`, or a crashed hook skips
it. Across enough worktree churn some process is bound to outlive its worktree,
and with no cleanup these leaks accumulate (on macOS they eventually saturate
`fseventsd`). `tether` removes the need for a `pre-remove`: it ties the
command's lifetime to the worktree and kills the whole process group once the
worktree is gone.

### Arguments

Arguments after `--` are the program and its arguments, run directly, no shell.

```console
$ wt step tether -- npm run dev
```

For pipes, redirects, variables, or globs, wrap in `sh -c`:

```console
$ wt step tether -- sh -c 'PORT=$P npm run dev | tee dev.log'
```

To run the command from a subdirectory, pass the global `-C` flag (teardown
still watches the worktree root, so a server launched with a relative `-C` is
torn down with the worktree):

```console
$ wt step tether -C frontend -- npm run dev
```

### Examples

Run a dev server, torn down automatically when the worktree goes away:

```toml
# .config/wt.toml
[post-start]
server = "wt step tether -- npm run dev -- --port {{ branch | hash_port }}"
```

### Command reference

```
wt step tether - [experimental] Run a command; kill its whole process tree when its worktree is removed

Teardown is automatic and needs no pre-remove hook; the group gets SIGTERM then SIGKILL.

Usage: wt step tether [OPTIONS] -- <COMMAND>...

Arguments:
  <COMMAND>...
          Command to run (after --, run directly, no shell)

Options:
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
