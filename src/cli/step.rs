use clap::Subcommand;

/// Run individual operations
#[derive(Subcommand)]
pub enum StepCommand {
    /// Stage and commit with LLM-generated message
    #[command(
        after_long_help = r#"Stages all changes (including untracked files) and commits with an [LLM-generated message](@/llm-commits.md).

## Options

### `--stage`

Controls what to stage before committing:

| Value | Behavior |
|-------|----------|
| `all` | Stage all changes including untracked files (default) |
| `tracked` | Stage only modified tracked files |
| `none` | Don't stage anything, commit only what's already staged |

```console
wt step commit --stage=tracked
```

Configure the default in user config:

```toml
[commit]
stage = "tracked"
```

### `--show-prompt`

Output the rendered LLM prompt to stdout without running the command. Useful for inspecting prompt templates or piping to other tools:

```console
# Inspect the rendered prompt
wt step commit --show-prompt | less

# Pipe to a different LLM
wt step commit --show-prompt | llm -m gpt-5-nano
```
"#
    )]
    Commit {
        /// Skip approval prompts
        #[arg(short, long)]
        yes: bool,

        /// Skip hooks
        #[arg(long = "no-verify", action = clap::ArgAction::SetFalse, default_value_t = true)]
        verify: bool,

        /// What to stage before committing [default: all]
        #[arg(long)]
        stage: Option<crate::commands::commit::StageMode>,

        /// Show prompt without running LLM
        ///
        /// Outputs the rendered prompt to stdout for debugging or manual piping.
        #[arg(long)]
        show_prompt: bool,
    },

    /// Squash commits since branching
    ///
    /// Stages changes and generates message with LLM.
    #[command(
        after_long_help = r#"Stages all changes (including untracked files), then squashes all commits since diverging from the target branch into a single commit with an [LLM-generated message](@/llm-commits.md).

## Options

### `--stage`

Controls what to stage before squashing:

| Value | Behavior |
|-------|----------|
| `all` | Stage all changes including untracked files (default) |
| `tracked` | Stage only modified tracked files |
| `none` | Don't stage anything, squash only committed changes |

```console
wt step squash --stage=none
```

Configure the default in user config:

```toml
[commit]
stage = "tracked"
```

### `--show-prompt`

Output the rendered LLM prompt to stdout without running the command. Useful for inspecting prompt templates or piping to other tools:

```console
wt step squash --show-prompt | less
```
"#
    )]
    Squash {
        /// Target branch
        ///
        /// Defaults to default branch.
        #[arg(add = crate::completion::branch_value_completer())]
        target: Option<String>,

        /// Skip approval prompts
        #[arg(short, long)]
        yes: bool,

        /// Skip hooks
        #[arg(long = "no-verify", action = clap::ArgAction::SetFalse, default_value_t = true)]
        verify: bool,

        /// What to stage before committing [default: all]
        #[arg(long)]
        stage: Option<crate::commands::commit::StageMode>,

        /// Show prompt without running LLM
        ///
        /// Outputs the rendered prompt to stdout for debugging or manual piping.
        #[arg(long)]
        show_prompt: bool,
    },

    /// Fast-forward target to current branch
    #[command(
        after_long_help = r#"Updates the local target branch (e.g., `main`) to include current commits.

## Examples

```console
wt step push             # Fast-forward main to current branch
wt step push develop     # Fast-forward develop instead
```

Similar to `git push . HEAD:<target>`, but uses `receive.denyCurrentBranch=updateInstead` internally.
"#
    )]
    Push {
        /// Target branch
        ///
        /// Defaults to default branch.
        #[arg(add = crate::completion::branch_value_completer())]
        target: Option<String>,
    },

    /// Rebase onto target
    #[command(
        after_long_help = r#"Rebases the current branch onto the target branch. Conflicts abort immediately; use `git rebase --abort` to recover.

## Examples

```console
wt step rebase            # Rebase onto default branch
wt step rebase develop    # Rebase onto develop
```
"#
    )]
    Rebase {
        /// Target branch
        ///
        /// Defaults to default branch.
        #[arg(add = crate::completion::branch_value_completer())]
        target: Option<String>,
    },

    /// Show all changes since branching
    ///
    /// Includes committed, staged, unstaged, and untracked files.
    #[command(
        after_long_help = r#"Shows all changes that `wt merge` would include: committed, staged, unstaged, and untracked files — in a single diff against the merge base.

## Extra git diff arguments

Arguments after `--` are forwarded to `git diff`:

```console
wt step diff -- --stat
wt step diff -- --name-only
wt step diff -- -- '*.rs'
```

The diff is pipeable to tools like `delta`:

```console
wt step diff | delta
```

## How it works

Equivalent to:

```console
GIT_INDEX_FILE=/tmp/idx git read-tree --empty
GIT_INDEX_FILE=/tmp/idx git add --intent-to-add .
GIT_INDEX_FILE=/tmp/idx git diff $(git merge-base HEAD $(wt config state default-branch))
```

`git diff` ignores untracked files. `git add --intent-to-add .` registers them in the index without staging their content, making them visible to `git diff`. This runs against a temporary empty index so the real one is never modified.
"#
    )]
    Diff {
        /// Target branch
        ///
        /// Defaults to default branch.
        #[arg(add = crate::completion::branch_value_completer())]
        target: Option<String>,

        /// Extra arguments forwarded to `git diff`
        #[arg(last = true)]
        extra_args: Vec<String>,
    },

    /// Copy gitignored files to another worktree
    ///
    /// Eliminates cold starts by copying build caches and dependencies.
    #[command(
        after_long_help = r#"Git worktrees share the repository but not untracked files. This command copies gitignored files to another worktree, eliminating cold starts.

## Setup

Add to the project config:

```toml
# .config/wt.toml
[post-start]
copy = "wt step copy-ignored"
```

## What gets copied

All gitignored files are copied by default. Tracked files are never touched.

To limit what gets copied, create `.worktreeinclude` with gitignore-style patterns. Files must be **both** gitignored **and** in `.worktreeinclude`:

```text
# .worktreeinclude
.env
node_modules/
target/
```

## Common patterns

| Type | Patterns |
|------|----------|
| Dependencies | `node_modules/`, `.venv/`, `target/`, `vendor/`, `Pods/` |
| Build caches | `.cache/`, `.next/`, `.parcel-cache/`, `.turbo/` |
| Generated assets | Images, ML models, binaries too large for git |
| Environment files | `.env` (if not generated per-worktree) |

## Features

- Uses copy-on-write (reflink) when available for space-efficient copies
- Handles nested `.gitignore` files, global excludes, and `.git/info/exclude`
- Skips existing files by default (safe to re-run)
- `--force` overwrites existing files in the destination
- Skips `.git` entries and other worktrees

## Performance

Reflink copies share disk blocks until modified — no data is actually copied. For a 14GB `target/` directory:

| Command | Time |
|---------|------|
| `cp -R` (full copy) | 2m |
| `cp -Rc` / `wt step copy-ignored` | 20s |

Uses per-file reflink (like `cp -Rc`) — copy time scales with file count.

Use the `post-start` hook so the copy runs in the background. Use `post-create` instead if subsequent hooks or `--execute` command need the copied files immediately.

## Language-specific notes

### Rust

The `target/` directory is huge (often 1-10GB). Copying with reflink cuts first build from ~68s to ~3s by reusing compiled dependencies.

### Node.js

`node_modules/` is large but mostly static. If the project has no native dependencies, symlinks are even faster:

```toml
[post-create]
deps = "ln -sf {{ primary_worktree_path }}/node_modules ."
```

### Python

Virtual environments contain absolute paths and can't be copied. Use `uv sync` instead — it's fast enough that copying isn't worth it.

## Behavior vs Claude Code on desktop

The `.worktreeinclude` pattern is shared with [Claude Code on desktop](https://code.claude.com/docs/en/desktop), which copies matching files when creating worktrees. Differences:

- worktrunk copies all gitignored files by default; Claude Code requires `.worktreeinclude`
- worktrunk uses copy-on-write for large directories like `target/` — potentially 30x faster on macOS, 6x on Linux
- worktrunk runs as a configurable hook in the worktree lifecycle
"#
    )]
    CopyIgnored {
        /// Source worktree branch
        ///
        /// Defaults to main worktree.
        #[arg(long, add = crate::completion::worktree_only_completer())]
        from: Option<String>,

        /// Destination worktree branch
        ///
        /// Defaults to current worktree.
        #[arg(long, add = crate::completion::worktree_only_completer())]
        to: Option<String>,

        /// Show what would be copied
        #[arg(long)]
        dry_run: bool,

        /// Overwrite existing files in destination
        #[arg(long)]
        force: bool,
    },

    /// \[experimental\] Run command in each worktree
    ///
    /// Executes sequentially with real-time output; continues on failure.
    #[command(
        after_long_help = r#"Executes a command sequentially in every worktree with real-time output. Continues on failure and shows a summary at the end.

Context JSON is piped to stdin for scripts that need structured data.

## Template variables

All variables are shell-escaped. See [`wt hook` template variables](@/hook.md#template-variables) for the complete list and filters.

## Examples

Check status across all worktrees:

```console
wt step for-each -- git status --short
```

Run npm install in all worktrees:

```console
wt step for-each -- npm install
```

Use branch name in command:

```console
wt step for-each -- "echo Branch: {{ branch }}"
```

Pull updates in worktrees with upstreams (skips others):

```console
git fetch --prune && wt step for-each -- '[ "$(git rev-parse @{u} 2>/dev/null)" ] || exit 0; git pull --autostash'
```

Note: This command is experimental and may change in future versions.
"#
    )]
    ForEach {
        /// Command template (see --help for all variables)
        #[arg(required = true, last = true, num_args = 1..)]
        args: Vec<String>,
    },

    /// \[experimental\] Put a branch into the main worktree
    ///
    /// The displaced branch moves to the promoted branch's worktree.
    #[command(
        after_long_help = r#"**Experimental.** Use promote for temporary testing when the main worktree has special significance (Docker Compose, IDE configs, heavy build artifacts anchored to project root), and hooks & tools aren't yet set up to run on arbitrary worktrees. The idiomatic Worktrunk workflow does not use `promote`; instead each worktree has a full environment. `promote` is the only Worktrunk command which changes a branch in an existing worktree.

## Example

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

## Requirements

- Both worktrees must be clean
- The branch must have an existing worktree
"#
    )]
    Promote {
        /// Branch to promote to main worktree
        ///
        /// Defaults to current branch, or default branch from main worktree.
        #[arg(add = crate::completion::worktree_only_completer())]
        branch: Option<String>,
    },

    /// \[experimental\] Move worktrees to expected paths
    ///
    /// Relocates worktrees whose path doesn't match the `worktree-path` template.
    #[command(
        after_long_help = r#"Moves worktrees to match the configured `worktree-path` template.

## Examples

Preview what would be moved:

```console
wt step relocate --dry-run
```

Move all mismatched worktrees:

```console
wt step relocate
```

Auto-commit and clobber blockers (never fails):

```console
wt step relocate --commit --clobber
```

Move specific worktrees:

```console
wt step relocate feature bugfix
```

## Swap handling

When worktrees are at each other's expected locations (e.g., `alpha` at
`repo.beta` and `beta` at `repo.alpha`), relocate automatically resolves
this by using a temporary location.

## Clobbering

With `--clobber`, non-worktree paths at target locations are moved to
`<path>.bak-<timestamp>` before relocating.

## Main worktree behavior

The main worktree can't be moved with `git worktree move`. Instead, relocate
switches it to the default branch and creates a new linked worktree at the
expected path. Untracked and gitignored files remain at the original location.

## Skipped worktrees

- **Dirty** (without `--commit`) — use `--commit` to auto-commit first
- **Locked** — unlock with `git worktree unlock`
- **Target blocked** (without `--clobber`) — use `--clobber` to backup blocker
- **Detached HEAD** — no branch to compute expected path

Note: This command is experimental and may change in future versions.
"#
    )]
    Relocate {
        /// Worktrees to relocate (defaults to all mismatched)
        #[arg(add = crate::completion::worktree_only_completer())]
        branches: Vec<String>,

        /// Show what would be moved
        #[arg(long)]
        dry_run: bool,

        /// Commit uncommitted changes before relocating
        #[arg(long)]
        commit: bool,

        /// Backup non-worktree paths at target locations
        ///
        /// Moves blocking paths to `<path>.bak-<timestamp>`.
        #[arg(long)]
        clobber: bool,
    },
}
