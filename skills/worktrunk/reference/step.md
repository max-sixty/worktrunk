# wt step

Run individual operations. The building blocks of wt merge — commit, squash, rebase, push — plus standalone utilities.

## Examples

Commit with LLM-generated message:

```bash
$ wt step commit
```

Manual merge workflow with review between steps:

```bash
$ wt step commit
$ wt step squash
$ wt step rebase
$ wt step push
```

## Operations

- [`commit`](#wt-step-commit) — Stage and commit with [LLM-generated message](https://worktrunk.dev/llm-commits/)
- [`squash`](#wt-step-squash) — Squash all branch commits into one with [LLM-generated message](https://worktrunk.dev/llm-commits/)
- `rebase` — Rebase onto target branch
- `push` — Fast-forward target to current branch
- [`diff`](#wt-step-diff) — Show all changes since branching (committed, staged, unstaged, untracked)
- [`copy-ignored`](#wt-step-copy-ignored) — Copy gitignored files between worktrees
- [`eval`](#wt-step-eval) — [experimental] Evaluate a template expression
- [`for-each`](#wt-step-for-each) — [experimental] Run a command in every worktree
- [`promote`](#wt-step-promote) — [experimental] Swap a branch into the main worktree
- [`prune`](#wt-step-prune) — Remove worktrees and branches merged into the default branch
- [`relocate`](#wt-step-relocate) — [experimental] Move worktrees to expected paths
- [`<alias>`](#aliases) — [experimental] Run a configured command alias

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
  eval          [experimental] Evaluate a template expression
  for-each      [experimental] Run command in each worktree
  promote       [experimental] Swap a branch into the main worktree
  prune         [experimental] Remove worktrees merged into the default branch
  relocate      [experimental] Move worktrees to expected paths

Options:
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose...
          Verbose output (-v: hooks, templates; -vv: debug report)
```

# Subcommands

## wt step commit

Stage and commit with LLM-generated message.

See [LLM-generated commit messages](https://worktrunk.dev/llm-commits/) for configuration and prompt customization.

### Options

#### `--stage`

Controls what to stage before committing:

| Value | Behavior |
|-------|----------|
| `all` | Stage all changes including untracked files (default) |
| `tracked` | Stage only modified tracked files |
| `none` | Don't stage anything, commit only what's already staged |

```bash
$ wt step commit --stage=tracked
```

Configure the default in user config:

```toml
[commit]
stage = "tracked"
```

#### `--show-prompt`

Output the rendered LLM prompt to stdout without running the command. Useful for inspecting prompt templates or piping to other tools:

```bash
# Inspect the rendered prompt
$ wt step commit --show-prompt | less

# Pipe to a different LLM
$ wt step commit --show-prompt | llm -m gpt-5-nano
```

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

      --show-prompt
          Show prompt without running LLM

          Outputs the rendered prompt to stdout for debugging or manual piping.

  -h, --help
          Print help (see a summary with '-h')

Automation:
  -y, --yes
          Skip approval prompts

      --no-hooks
          Skip hooks

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose...
          Verbose output (-v: hooks, templates; -vv: debug report)
```

## wt step squash

Squash commits since branching. Stages changes and generates message with LLM.

See [LLM-generated commit messages](https://worktrunk.dev/llm-commits/) for configuration and prompt customization.

### Options

#### `--stage`

Controls what to stage before squashing:

| Value | Behavior |
|-------|----------|
| `all` | Stage all changes including untracked files (default) |
| `tracked` | Stage only modified tracked files |
| `none` | Don't stage anything, squash only committed changes |

```bash
$ wt step squash --stage=none
```

Configure the default in user config:

```toml
[commit]
stage = "tracked"
```

#### `--show-prompt`

Output the rendered LLM prompt to stdout without running the command. Useful for inspecting prompt templates or piping to other tools:

```bash
$ wt step squash --show-prompt | less
```

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

      --show-prompt
          Show prompt without running LLM

          Outputs the rendered prompt to stdout for debugging or manual piping.

  -h, --help
          Print help (see a summary with '-h')

Automation:
  -y, --yes
          Skip approval prompts

      --no-hooks
          Skip hooks

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose...
          Verbose output (-v: hooks, templates; -vv: debug report)
```

## wt step diff

Show all changes since branching. Includes committed, staged, unstaged, and untracked files.

This is what `wt merge` would include — a single diff against the merge base.

### Extra git diff arguments

Arguments after `--` are forwarded to `git diff`:

```bash
$ wt step diff -- --stat
$ wt step diff -- --name-only
$ wt step diff -- -- '*.rs'
```

The diff is pipeable to tools like `delta`:

```bash
$ wt step diff | delta
```

### How it works

Equivalent to:

```bash
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
  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose...
          Verbose output (-v: hooks, templates; -vv: debug report)
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

### What gets copied

All gitignored files are copied by default, except for built-in excluded directories: VCS metadata (`.bzr/`, `.hg/`, `.jj/`, `.pijul/`, `.sl/`, `.svn/`) and tool-state (`.conductor/`, `.entire/`, `.pi/`, `.worktrees/`). Tracked files are never touched.

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

### Common patterns

| Type | Patterns |
|------|----------|
| Dependencies | `node_modules/`, `.venv/`, `target/`, `vendor/`, `Pods/` |
| Build caches | `.cache/`, `.next/`, `.parcel-cache/`, `.turbo/` |
| Generated assets | Images, ML models, binaries too large for git |
| Environment files | `.env` (if not generated per-worktree) |

### Features

- Uses copy-on-write (reflink) when available for space-efficient copies
- Handles nested `.gitignore` files, global excludes, and `.git/info/exclude`
- Skips existing files by default (safe to re-run)
- `--force` overwrites existing files in the destination
- Always skips built-in excluded directories — VCS metadata (`.bzr/`, `.hg/`, `.jj/`, `.pijul/`, `.sl/`, `.svn/`) and tool-state (`.conductor/`, `.entire/`, `.pi/`, `.worktrees/`) — and nested worktrees

### Performance

Reflink copies share disk blocks until modified — no data is actually copied. For a 14GB `target/` directory:

| Command | Time |
|---------|------|
| `cp -R` (full copy) | 2m |
| `cp -Rc` / `wt step copy-ignored` | 20s |

Uses per-file reflink (like `cp -Rc`) — copy time scales with file count. On Unix, the process is automatically reniced to lowest priority (nice 19) so it yields to interactive work.

Use the `post-start` hook so the copy runs in the background. Use `pre-start` instead if subsequent hooks or `--execute` command need the copied files immediately.

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

- worktrunk copies all gitignored files by default; Claude Code requires `.worktreeinclude`
- worktrunk uses copy-on-write for large directories like `target/` — potentially 30x faster on macOS, 6x on Linux
- worktrunk runs as a configurable hook in the worktree lifecycle

### Command reference

```
wt step copy-ignored - Copy gitignored files to another worktree

Eliminates cold starts by copying build caches and dependencies.

Usage: wt step copy-ignored [OPTIONS]

Options:
      --from <FROM>
          Source worktree branch

          Defaults to main worktree.

      --to <TO>
          Destination worktree branch

          Defaults to current worktree.

      --dry-run
          Show what would be copied

      --force
          Overwrite existing files in destination

  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose...
          Verbose output (-v: hooks, templates; -vv: debug report)
```

## wt step eval

[experimental]

Evaluate a template expression. Prints the result to stdout for use in scripts and shell substitutions.

All [hook template variables and filters](https://worktrunk.dev/hook/#template-variables) are available.

### Examples

Get the port for the current branch:

```bash
$ wt step eval '{{ branch | hash_port }}'
16066
```

Use in shell substitution:

```bash
$ curl http://localhost:$(wt step eval '{{ branch | hash_port }}')/health
```

Combine multiple values:

```bash
$ wt step eval '{{ branch | hash_port }},{{ ("supabase-api-" ~ branch) | hash_port }}'
16066,16739
```

Use conditionals and filters:

```bash
$ wt step eval '{{ branch | sanitize_db }}'
feature_auth_oauth2_a1b
```

Show available template variables:

```bash
$ wt step eval --dry-run '{{ branch }}'
branch=feature/auth-oauth2
worktree_path=/home/user/projects/myapp-feature-auth-oauth2
...
Result: feature/auth-oauth2
```

Note: This command is experimental and may change in future versions.

### Command reference

```
wt step eval - [experimental] Evaluate a template expression

Prints the result to stdout for use in scripts and shell substitutions.

Usage: wt step eval [OPTIONS] <TEMPLATE>

Arguments:
  <TEMPLATE>
          Template expression to evaluate

Options:
      --dry-run
          Show template variables and expanded result

  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose...
          Verbose output (-v: hooks, templates; -vv: debug report)
```

## wt step for-each

[experimental]

Run command in each worktree. Executes sequentially with real-time output; continues on failure.

A summary of successes and failures is shown at the end. Context JSON is piped to stdin for scripts that need structured data.

### Template variables

All variables are shell-escaped. See [`wt hook` template variables](https://worktrunk.dev/hook/#template-variables) for the complete list and filters.

### Examples

Check status across all worktrees:

```bash
$ wt step for-each -- git status --short
```

Run npm install in all worktrees:

```bash
$ wt step for-each -- npm install
```

Use branch name in command:

```bash
$ wt step for-each -- "echo Branch: {{ branch }}"
```

Pull updates in worktrees with upstreams (skips others):

```bash
$ git fetch --prune && wt step for-each -- '[ "$(git rev-parse @{u} 2>/dev/null)" ] || exit 0; git pull --autostash'
```

Note: This command is experimental and may change in future versions.

### Command reference

```
wt step for-each - [experimental] Run command in each worktree

Executes sequentially with real-time output; continues on failure.

Usage: wt step for-each [OPTIONS] -- <ARGS>...

Arguments:
  <ARGS>...
          Command template (see --help for all variables)

Options:
  -h, --help
          Print help (see a summary with '-h')

Automation:
      --format <FORMAT>
          Output format (text, json)

          Possible values:
          - text: Human-readable text output
          - json: JSON output

          [default: text]

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose...
          Verbose output (-v: hooks, templates; -vv: debug report)
```

## wt step promote

[experimental]

Swap a branch into the main worktree. Exchanges branches and gitignored files between two worktrees.

**Experimental.** Use promote for temporary testing when the main worktree has special significance (Docker Compose, IDE configs, heavy build artifacts anchored to project root), and hooks & tools aren't yet set up to run on arbitrary worktrees. The idiomatic Worktrunk workflow does not use `promote`; instead each worktree has a full environment. `promote` is the only Worktrunk command which changes a branch in an existing worktree.

### Example

```bash
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

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose...
          Verbose output (-v: hooks, templates; -vv: debug report)
```

## wt step prune

[experimental]

Remove worktrees merged into the default branch.

Bulk-removes worktrees and branches that are integrated into the default branch, using the same criteria as `wt remove`'s branch cleanup. Stale worktree entries are cleaned up too.

In `wt list`, candidates show `_` (same commit) or `⊂` (content integrated). Run `--dry-run` to preview. See `wt remove --help` for the full integration criteria.

Locked worktrees and the main worktree are always skipped. The current worktree is removed last, triggering cd to the primary worktree. Pre-remove and post-remove hooks run for each removal.

### Min-age guard

Worktrees younger than `--min-age` (default: 1 hour) are skipped. This prevents removing a worktree just created from the default branch — it looks "merged" because its branch points at the same commit.

```bash
$ wt step prune --min-age=0s     # no age guard
$ wt step prune --min-age=2d     # skip worktrees younger than 2 days
```

### Examples

Preview what would be removed:

```bash
$ wt step prune --dry-run
```

Remove all merged worktrees:

```bash
$ wt step prune
```

### Command reference

```
wt step prune - [experimental] Remove worktrees merged into the default branch

Usage: wt step prune [OPTIONS]

Options:
      --dry-run
          Show what would be removed

      --min-age <MIN_AGE>
          Skip worktrees younger than this

          [default: 1h]

      --foreground
          Run removal in foreground (block until complete)

  -h, --help
          Print help (see a summary with '-h')

Automation:
  -y, --yes
          Skip approval prompts

      --format <FORMAT>
          Output format (text, json)

          Possible values:
          - text: Human-readable text output
          - json: JSON output

          [default: text]

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose...
          Verbose output (-v: hooks, templates; -vv: debug report)
```

## wt step relocate

[experimental]

Move worktrees to expected paths. Relocates worktrees whose path doesn't match the worktree-path template.

### Examples

Preview what would be moved:

```bash
$ wt step relocate --dry-run
```

Move all mismatched worktrees:

```bash
$ wt step relocate
```

Auto-commit and clobber blockers (never fails):

```bash
$ wt step relocate --commit --clobber
```

Move specific worktrees:

```bash
$ wt step relocate feature bugfix
```

### Swap handling

When worktrees are at each other's expected locations (e.g., `alpha` at
`repo.beta` and `beta` at `repo.alpha`), relocate automatically resolves
this by using a temporary location.

### Clobbering

With `--clobber`, non-worktree paths at target locations are moved to
`<path>.bak-<timestamp>` before relocating.

### Main worktree behavior

The main worktree can't be moved with `git worktree move`. Instead, relocate
switches it to the default branch and creates a new linked worktree at the
expected path. Untracked and gitignored files remain at the original location.

### Skipped worktrees

- **Dirty** (without `--commit`) — use `--commit` to auto-commit first
- **Locked** — unlock with `git worktree unlock`
- **Target blocked** (without `--clobber`) — use `--clobber` to backup blocker
- **Detached HEAD** — no branch to compute expected path

Note: This command is experimental and may change in future versions.

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

          Moves blocking paths to <path>.bak-<timestamp>.

  -h, --help
          Print help (see a summary with '-h')

Global Options:
  -C <path>
          Working directory for this command

      --config <path>
          User config file path

  -v, --verbose...
          Verbose output (-v: hooks, templates; -vv: debug report)
```

## Aliases [experimental]

Custom command templates configured in user config (`~/.config/worktrunk/config.toml`) or project config (`.config/wt.toml`). Aliases support the same [template variables](https://worktrunk.dev/hook/#template-variables) as hooks.

```toml
# .config/wt.toml
[aliases]
deploy = "make deploy BRANCH={{ branch }}"
port = "echo http://localhost:{{ branch | hash_port }}"
```

```bash
$ wt step deploy                            # run the alias
$ wt step deploy --dry-run                  # show expanded command
$ wt step deploy --var env=staging          # pass extra template variables
$ wt step deploy --yes                      # skip approval prompt
```

When defined in both user and project config, both run — user first, then project. Project-config aliases require [command approval](https://worktrunk.dev/hook/#wt-hook-approvals) on first run, same as project hooks. User-config aliases are trusted.

Inside an alias body, an inner `wt switch` (or `wt switch --create`) passes its `cd` through to the parent shell, so an alias wrapping `wt switch --create` lands the shell in the new worktree just like running it directly.

Alias names that match a built-in step command (`commit`, `squash`, etc.) are shadowed by the built-in and will never run.
