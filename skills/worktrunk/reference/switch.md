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
$ wt switch https://github.com/owner/repo/pull/123   # ...or paste the PR's URL
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
$ wt switch -                           # Back to previous
$ wt switch ^                           # Default branch worktree
$ wt switch --create fix --base=@       # Branch from current HEAD
$ wt switch --create fix --base=pr:123  # Branch from PR #123's head
$ wt switch pr:123                      # PR #123's branch
$ wt switch mr:101                      # MR !101's branch
```

Shortcuts also apply to `--base`. For a fork PR/MR, the head commit is fetched and used as the base SHA without creating a tracking branch.

## Interactive picker

When called without arguments, `wt switch` opens an interactive picker to browse and select worktrees with live preview. The candidate set widens with `--branches` (local branches without worktrees), `--remotes` (remote branches), and `--prs` (open PRs/MRs — see below).

The CI column shows each row's PR/MR CI and review status, the same as [`wt list --full`](https://worktrunk.dev/list/).

**Keybindings:**

| Key | Action |
|-----|--------|
| `↑`/`↓` | Navigate worktree list |
| (type) | Filter worktrees |
| `Enter` | Switch to selected worktree |
| `Alt-c` | Create new worktree named as entered text |
| `Alt-x` | Remove selected worktree/branch |
| `Alt-y` | Copy selected branch name to the clipboard |
| `Alt-o` | Open the selected row's PR/MR URL in the browser |
| `Alt-r` | Refresh the list (pick up worktrees created elsewhere) |
| `Esc` | Cancel |
| `Alt-1`–`Alt-7` | Jump to a preview tab |
| `Tab`/`Shift-Tab` | Cycle preview tabs forward/backward |
| `Alt-p` | Toggle preview panel |
| `Ctrl-u`/`Ctrl-d` | Scroll preview up/down |

`Alt-o` is a no-op on a row with no PR/MR (or whose status hasn't loaded yet).

Plain digits go to the filter, so a branch name containing a number can be typed directly; the preview tabs move to `Alt`.

Typing a gutter sigil filters by row kind: `+` narrows to linked worktrees and `@` to the current worktree. The other sigils don't filter cleanly — `^` and `|` are skim's prefix-anchor and OR query operators (so `^` matches every row and `|` none), and `/` matches most rows because every worktree path contains it.

**Preview tabs:**

1. **HEAD±** — Diff of uncommitted changes
2. **log** — Recent commits; commits already on the default branch have dimmed hashes
3. **main…±** — Diff of changes since the merge-base with the default branch
4. **remote⇅** — Ahead/behind diff vs upstream tracking branch
5. **summary** — LLM-generated branch summary; requires `[list] summary = true` and [`commit.generation`](https://worktrunk.dev/config/#commit)
6. **pr** — The selected row's PR/MR, for any row whose branch has one
7. **comments** — The PR/MR's comment thread, fetched from the forge on `--prs` rows

On narrow previews the tab bar compacts to digits — only the active tab keeps its label — so every `Alt-N` accelerator stays visible.

**Pager configuration:** The preview panel pipes diff output through git's pager. Override in user config:

```toml
[switch.picker]
pager = "delta --paging=never --width=$COLUMNS"
```

## Pull requests and merge requests

The `pr:<number>` / `mr:<number>` shortcut and the PR/MR's web URL both resolve to its branch. For same-repo PRs/MRs, worktrunk switches to the branch directly. For fork PRs/MRs, it fetches the ref (`refs/pull/N/head` or `refs/merge-requests/N/head`) and configures `pushRemote` to the fork URL.

```bash
$ wt switch pr:101                                  # GitHub PR #101
$ wt switch https://github.com/owner/repo/pull/101  # ...the same PR, by URL
$ wt switch mr:101                                  # GitLab MR !101
$ wt switch https://gitlab.com/owner/repo/-/merge_requests/101  # ...the same MR, by URL
$ wt switch --prs                                   # Browse open PRs/MRs in the picker
```

Both work anywhere a branch is accepted, including `--base`. The `--create` flag cannot be used with a PR/MR reference since the branch already exists.

If the PR or MR is on a fork, the local branch uses its branch name directly, so `git push` works normally. A pre-existing local branch with that name tracking something else requires renaming first.

The `--prs` flag adds the repository's open PRs (GitHub) or MRs (GitLab) to the interactive picker. Each row resolves to the same `pr:`/`mr:` shortcut, so selecting one fetches the ref and switches to its branch. A `--prs` row has no local worktree, so its `pr` and `comments` preview tabs load the PR/MR's metadata and comments from the forge in the background. The `log` tab uses a local `git log` — graph and merge-base dimming included — whenever the head commit is already in the object store (a same-repo PR off a fetched remote), falling back to a flat forge-fetched commit list otherwise.

Requires `gh` (GitHub), `glab` (GitLab), or an equivalent CLI installed and authenticated; see [forge platform](https://worktrunk.dev/config/#forge-platform) for Gitea, Azure DevOps, and other supported platforms.

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
          Branch name, shortcut, or PR/MR URL

          Opens interactive picker if omitted. Shortcuts: ^ (default branch), - (previous), @
          (current), pr:{N} (GitHub PR), mr:{N} (GitLab MR)

  [EXECUTE_ARGS]...
          Additional arguments for --execute command (after --)

          Arguments after -- are appended to the execute command. Each argument is expanded for
          templates, then POSIX shell-escaped.

Options:
  -c, --create
          Create a new branch

  -b, --base <BASE>
          Base branch

          Defaults to default branch. Supports the same shortcuts as the branch argument: ^, @, -,
          pr:{N}, mr:{N}.

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

          Template example: -x code -- '{{ worktree_path }}' opens VS Code at the worktree, -x tmux
          -- new -s '{{ branch | sanitize }}' starts a tmux session named after the branch.

      --clobber
          Remove stale paths at target

      --no-cd
          Skip directory change after switching

          Hooks still run normally. Useful when hooks handle navigation (e.g., tmux workflows) or
          for CI/automation. Use --cd to override.

  -h, --help
          Print help (see a summary with '-h')

Picker Options:
      --branches
          Include branches without worktrees

      --remotes
          Include remote branches

      --prs
          Include open PRs/MRs

Automation:
      --no-hooks
          Skip hooks

      --format <FORMAT>
          Output format

          JSON prints structured result to stdout. Designed for tool integration (e.g., Claude Code
          WorktreeCreate hooks).

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
