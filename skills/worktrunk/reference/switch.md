# wt switch

Switch to a worktree; create if needed.

Worktrees are addressed by branch name; paths are computed from a configurable template. Unlike `git switch`, this navigates between worktrees rather than changing branches in place.

## Examples

```bash
$ wt switch feature-auth           # Switch to worktree
$ wt switch -                      # Previous worktree (like cd -)
$ wt switch --create new-feature   # Create new branch and worktree
$ wt switch --create hotfix --base production
$ wt switch pr:123                 # Switch to PR #123's branch
```

## Creating a branch

The `--create` flag creates a new branch from `--base` — the default branch unless specified. Without `--create`, the branch must already exist. Switching to a remote branch (e.g., `wt switch feature` when only `origin/feature` exists) creates a local tracking branch.

## Creating worktrees

If the branch already has a worktree, `wt switch` changes directories to it. Otherwise, it creates one:

1. Runs [pre-switch hooks](https://worktrunk.dev/hook/#hook-types), blocking until complete
2. Creates worktree at configured path
3. Switches to new directory
4. Runs [pre-start hooks](https://worktrunk.dev/hook/#hook-types), blocking until complete
5. Spawns [post-start](https://worktrunk.dev/hook/#hook-types) and [post-switch hooks](https://worktrunk.dev/hook/#hook-types) in the background

```bash
$ wt switch feature                        # Existing branch → creates worktree
$ wt switch --create feature               # New branch and worktree
$ wt switch --create fix --base release    # New branch from release
$ wt switch --create temp --no-hooks       # Skip hooks
```

## Shortcuts

| Shortcut | Meaning |
|----------|---------|
| `^` | Default branch (`main`/`master`) |
| `@` | Current branch/worktree |
| `-` | Previous worktree (like `cd -`) |
| `pr:{N}` | GitHub PR #N's branch |
| `mr:{N}` | GitLab MR !N's branch |

```bash
$ wt switch -                      # Back to previous
$ wt switch ^                      # Default branch worktree
$ wt switch --create fix --base=@  # Branch from current HEAD
$ wt switch pr:123                 # PR #123's branch
$ wt switch mr:101                 # MR !101's branch
```

## Interactive picker

When called without arguments, `wt switch` opens an interactive picker to browse and select worktrees with live preview.

**Keybindings:**

| Key | Action |
|-----|--------|
| `↑`/`↓` | Navigate worktree list |
| (type) | Filter worktrees |
| `Enter` | Switch to selected worktree |
| `Alt-c` | Create new worktree named as entered text |
| `Esc` | Cancel |
| `1`–`5` | Switch preview tab |
| `Alt-p` | Toggle preview panel |
| `Ctrl-u`/`Ctrl-d` | Scroll preview up/down |

**Preview tabs** — toggle with number keys:

1. **HEAD±** — Diff of uncommitted changes
2. **log** — Recent commits; commits already on the default branch have dimmed hashes
3. **main…±** — Diff of changes since the merge-base with the default branch
4. **remote⇅** — Ahead/behind diff vs upstream tracking branch
5. **summary** — LLM-generated branch summary; requires `[list] summary = true` and `[commit.generation]`

**Pager configuration:** The preview panel pipes diff output through git's pager. Override in user config:

```toml
[switch.picker]
pager = "delta --paging=never --width=$COLUMNS"
```

Available on Unix only (macOS, Linux). On Windows, use `wt list` or `wt switch <branch>` directly.

## Pull requests and merge requests

The `pr:<number>` and `mr:<number>` shortcuts resolve a GitHub PR or GitLab MR to its branch. For same-repo PRs/MRs, worktrunk switches to the branch directly. For fork PRs/MRs, it fetches the ref (`refs/pull/N/head` or `refs/merge-requests/N/head`) and configures `pushRemote` to the fork URL.

```bash
$ wt switch pr:101                 # GitHub PR #101
$ wt switch mr:101                 # GitLab MR !101
```

Requires `gh` (GitHub) or `glab` (GitLab) CLI to be installed and authenticated. The `--create` flag cannot be used with `pr:`/`mr:` syntax since the branch already exists.

**Forks:** The local branch uses the PR/MR's branch name directly (e.g., `feature-fix`), so `git push` works normally. If a local branch with that name already exists tracking something else, rename it first.

## When wt switch fails

- **Branch doesn't exist** — Use `--create`, or check `wt list --branches`
- **Path occupied** — Another worktree is at the target path; switch to it or remove it
- **Stale directory** — Use `--clobber` to remove a non-worktree directory at the target path

To change which branch a worktree is on, use `git switch` inside that worktree.

## Command reference

```
wt switch - Switch to a worktree; create if needed

Usage: wt switch [OPTIONS] [BRANCH] [-- <EXECUTE_ARGS>...]

Arguments:
  [BRANCH]
          Branch name or shortcut

          Opens interactive picker if omitted. Shortcuts: '^' (default branch), '-' (previous), '@'
          (current), 'pr:{N}' (GitHub PR), 'mr:{N}' (GitLab MR)

  [EXECUTE_ARGS]...
          Additional arguments for --execute command (after --)

          Arguments after -- are appended to the execute command. Each argument is expanded for
          templates, then POSIX shell-escaped.

Options:
  -c, --create
          Create a new branch

  -b, --base <BASE>
          Base branch

          Defaults to default branch.

  -x, --execute <EXECUTE>
          Command to run after switch

          Replaces the wt process with the command after switching, giving it full terminal control.
          Useful for launching editors, AI agents, or other interactive tools.

          Supports hook template variables ({{ branch }}, {{ worktree_path }}, etc.) and filters. {{
          base }} and {{ base_worktree_path }} require --create.

          Especially useful with shell aliases:

            alias wsc='wt switch --create -x claude'
            wsc feature-branch -- 'Fix GH #322'

          Then wsc feature-branch creates the worktree and launches Claude Code. Arguments after --
          are passed to the command, so wsc feature -- 'Fix GH #322' runs claude 'Fix GH #322',
          starting Claude with a prompt.

          Template example: -x 'code {{ worktree_path }}' opens VS Code at the worktree, -x 'tmux
          new -s {{ branch | sanitize }}' starts a tmux session named after the branch.

      --clobber
          Remove stale paths at target

      --no-cd
          Skip directory change after switching

          Hooks still run normally. Useful when hooks handle navigation (e.g., tmux workflows) or
          for CI/automation. Use --cd to override.

          In picker mode (no branch argument), prints the selected branch name and exits without
          switching. Useful for scripting.

  -h, --help
          Print help (see a summary with '-h')

Picker Options:
      --branches
          Include branches without worktrees

      --remotes
          Include remote branches

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
