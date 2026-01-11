use clap::Subcommand;

/// Run individual operations
#[derive(Subcommand)]
pub enum StepCommand {
    /// Commit changes with LLM commit message
    ///
    /// Stages working tree changes and commits with an LLM-generated message.
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
    /// Stages working tree changes, squashes all commits since diverging from target into one, generates message with LLM.
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
    ///
    /// Updates the local target branch (e.g., `main`) to include current commits.
    /// Similar to `git push . HEAD:<target>` but uses
    /// `receive.denyCurrentBranch=updateInstead` internally.
    Push {
        /// Target branch
        ///
        /// Defaults to default branch.
        #[arg(add = crate::completion::branch_value_completer())]
        target: Option<String>,
    },

    /// Rebase onto target
    Rebase {
        /// Target branch
        ///
        /// Defaults to default branch.
        #[arg(add = crate::completion::branch_value_completer())]
        target: Option<String>,
    },

    /// Copy gitignored files to another worktree
    ///
    /// Copies gitignored files to another worktree. By default copies all
    /// gitignored files; use `.worktreeinclude` to limit what gets copied.
    /// Useful in post-create hooks to sync local config files (`.env`, IDE
    /// settings) to new worktrees. Skips symlinks and existing files.
    #[command(
        after_long_help = r#"Git worktrees share the repository but not untracked files. This command copies gitignored files to another worktree, eliminating cold starts.

## Setup

Add to your project config:

```toml
# .config/wt.toml
[post-create]
copy = "wt step copy-ignored"
```

All gitignored files are copied by default, as if `.worktreeinclude` contained `**`. To copy only specific patterns, create a `.worktreeinclude` file using gitignore syntax:

```gitignore
# .worktreeinclude — optional, limits what gets copied
.env
node_modules/
target/
.cache/
```

## What gets copied

Only gitignored files are copied — tracked files are never touched. If `.worktreeinclude` exists, files must match **both** `.worktreeinclude` **and** be gitignored.

## Common patterns

| Type | Patterns |
|------|----------|
| Dependencies | `node_modules/`, `.venv/`, `target/`, `vendor/`, `Pods/` |
| Build caches | `.cache/`, `.next/`, `.parcel-cache/`, `.turbo/` |
| Generated assets | Images, ML models, binaries too large for git |
| Environment files | `.env` (if not generated per-worktree) |

## Features

- Uses copy-on-write (reflink) when available for instant, space-efficient copies
- Handles nested `.gitignore` files, global excludes, and `.git/info/exclude`
- Skips existing files (safe to re-run)
- Skips symlinks and `.git` entries

## Language-specific notes

### Rust

The `target/` directory is huge (often 1-10GB). Copying with reflink cuts first build from ~68s to ~3s by reusing compiled dependencies.

### Node.js

`node_modules/` is large but mostly static. If the project has no native dependencies, symlinks are even faster:

```toml
[post-create]
deps = "ln -sf {{ main_worktree_path }}/node_modules ."
```

### Python

Virtual environments contain absolute paths and can't be copied. Use `uv sync` instead — it's fast enough that copying isn't worth it.
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
    },

    /// \[experimental\] Run command in each worktree
    #[command(
        after_long_help = r#"Executes a command sequentially in every worktree with real-time output. Continues on failure and shows a summary at the end.

Context JSON is piped to stdin for scripts that need structured data.

## Template variables

All variables are shell-escaped:

| Variable | Description |
|----------|-------------|
| `{{ branch }}` | Branch name (raw, e.g., `feature/auth`) |
| `{{ branch \| sanitize }}` | Branch name with `/` and `\` replaced by `-` |
| `{{ repo }}` | Repository directory name (e.g., `myproject`) |
| `{{ repo_path }}` | Absolute path to repository root |
| `{{ worktree_name }}` | Worktree directory name |
| `{{ worktree_path }}` | Absolute path to current worktree |
| `{{ main_worktree_path }}` | Default branch worktree path |
| `{{ commit }}` | Current HEAD commit SHA (full) |
| `{{ short_commit }}` | Current HEAD commit SHA (7 chars) |
| `{{ default_branch }}` | Default branch name (e.g., "main") |
| `{{ remote }}` | Primary remote name (e.g., "origin") |
| `{{ remote_url }}` | Primary remote URL |
| `{{ upstream }}` | Upstream tracking branch, if configured |

**Deprecated:** `repo_root` (use `repo_path`), `worktree` (use `worktree_path`), `main_worktree` (use `repo`).

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
}
