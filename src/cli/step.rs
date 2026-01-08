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
    /// Updates the local target branch (e.g., main) to include current commits.
    /// Equivalent to `git push . HEAD:main`.
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

    /// Copy `.worktreeinclude` files to another worktree
    ///
    /// Copies files listed in `.worktreeinclude` that are also gitignored.
    /// Useful in post-create hooks to sync local config files
    /// (`.env`, IDE settings) to new worktrees. Skips symlinks and existing
    /// files.
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
