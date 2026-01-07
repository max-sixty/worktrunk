+++
title = "Tips & Patterns"
weight = 24

[extra]
group = "Reference"
+++

Practical recipes for common Worktrunk workflows.

## Alias for new worktree + agent

Create a worktree and launch Claude in one command:

```bash
alias wsc='wt switch --create --execute=claude'
wsc new-feature                       # Creates worktree, runs hooks, launches Claude
wsc feature -- 'Fix GH #322'          # Runs `claude 'Fix GH #322'`
```

## Eliminate cold starts

`post-create` hooks install deps and copy caches. On macOS, use copy-on-write for instant cache cloning:

```toml
[post-create]
"cache" = "cp -c -r ../.cache .cache"  # APFS clones (instant, no disk space)
"install" = "npm ci"
```

See [Designing effective hooks](@/hook.md#designing-effective-hooks) for cross-platform CoW commands, language-specific tips, and more patterns.

## Dev server per worktree

Each worktree runs its own dev server on a deterministic port. The `hash_port` filter generates a stable port (10000-19999) from the branch name:

```toml
# .config/wt.toml
[post-start]
server = "npm run dev -- --port {{ branch | hash_port }}"

[list]
url = "http://localhost:{{ branch | hash_port }}"

[pre-remove]
server = "lsof -ti :{{ branch | hash_port }} | xargs kill 2>/dev/null || true"
```

The URL column in `wt list` shows each worktree's dev server:

<!-- ⚠️ AUTO-GENERATED-HTML from tests/snapshots/integration__integration_tests__list__tips_dev_server_workflow.snap — edit source to update -->

{% terminal() %}
<span class="prompt">$</span> <span class="cmd">wt list</span>
  <b>Branch</b>       <b>Status</b>        <b>HEAD±</b>    <b>main↕</b>  <b>Remote⇅</b>  <b>URL</b>                     <b>Commit</b>    <b>Age</b>
@ main           <span class=c>?</span> <span class=d>^</span><span class=d>⇅</span>                         <span class=g>⇡1</span>  <span class=d><span class=r>⇣1</span></span>  <span class=d>http://localhost:12107</span>  <span class=d>6088adb3</span>  <span class=d>4d</span>
+ feature-api  <span class=c>+</span>   <span class=d>↕</span><span class=d>⇡</span>     <span class=g>+54</span>   <span class=r>-5</span>   <span class=g>↑4</span>  <span class=d><span class=r>↓1</span></span>   <span class=g>⇡3</span>      <span class=d>http://localhost:10703</span>  <span class=d>ec97decc</span>  <span class=d>30m</span>
+ fix-auth         <span class=d>↕</span><span class=d>|</span>                <span class=g>↑2</span>  <span class=d><span class=r>↓1</span></span>     <span class=d>|</span>     <span class=d>http://localhost:16460</span>  <span class=d>127407de</span>  <span class=d>5h</span>

<span class=d>○</span> <span class=d>Showing 3 worktrees, 2 with changes, 2 ahead, 2 columns hidden</span>
{% end %}

<!-- END AUTO-GENERATED -->

Ports are deterministic — `fix-auth` always gets port 16460, regardless of which machine or when. The URL dims if the server isn't running.

For subdomain-based routing (useful for cookies and CORS), use `lvh.me` which resolves to 127.0.0.1:

```toml
[post-start]
server = "npm run dev -- --host {{ branch | sanitize }}.lvh.me --port {{ branch | hash_port }}"
```

## Database per worktree

Each worktree can have its own isolated database. Docker containers get unique names and ports:

```toml
[post-start]
db = """
docker run -d --rm \
  --name {{ repo }}-{{ branch | sanitize }}-postgres \
  -p {{ ('db-' ~ branch) | hash_port }}:5432 \
  -e POSTGRES_DB={{ repo }} \
  -e POSTGRES_PASSWORD=dev \
  postgres:16
"""

[pre-remove]
db-stop = "docker stop {{ repo }}-{{ branch | sanitize }}-postgres 2>/dev/null || true"
```

The `('db-' ~ branch)` concatenation hashes differently than plain `branch`, so database and dev server ports don't collide.
Jinja2's operator precedence has pipe `|` with higher precedence than concatenation `~`, meaning expressions need parentheses to filter concatenated values.

Generate `.env.local` with the correct `DATABASE_URL` using a `post-create` hook:

```toml
[post-create]
env = """
cat > .env.local << EOF
DATABASE_URL=postgres://postgres:dev@localhost:{{ ('db-' ~ branch) | hash_port }}/{{ repo }}
DEV_PORT={{ branch | hash_port }}
EOF
"""
```

## Local CI gate

`pre-merge` hooks run before merging. Failures abort the merge:

```toml
[pre-merge]
"lint" = "uv run ruff check"
"test" = "uv run pytest"
```

This catches issues locally before pushing — like running CI locally.

## Track agent status

Custom emoji markers show agent state in `wt list`. The Claude Code plugin sets these automatically:

```
+ feature-api      ↑  🤖              ↑1      ./repo.feature-api
+ review-ui      ? ↑  💬              ↑1      ./repo.review-ui
```

- `🤖` — Claude is working
- `💬` — Claude is waiting for input

Set status manually for any workflow:

```bash
wt config state marker set "🚧"                   # Current branch
wt config state marker set "✅" --branch feature  # Specific branch
git config worktrunk.state.feature.marker '{"marker":"💬","set_at":0}'  # Direct
```

See [Claude Code Integration](@/claude-code.md#installation) for plugin installation.

## Monitor CI across branches

```bash
wt list --full --branches
```

Shows PR/CI status for all branches, including those without worktrees. CI indicators are clickable links to the PR page.

## JSON API

```bash
wt list --format=json
```

Structured output for dashboards, statuslines, and scripts. See [`wt list`](@/list.md) for query examples.

## Reuse `default-branch`

Worktrunk maintains useful state. Default branch [detection](@/config.md#wt-config-state-default-branch), for instance, means scripts work on any repo — no need to hardcode `main` or `master`:

```bash
git rebase $(wt config state default-branch)
```

## Task runners in hooks

Reference Taskfile/Justfile/Makefile in hooks:

```toml
[post-create]
"setup" = "task install"

[pre-merge]
"validate" = "just test lint"
```

## Shortcuts

Special arguments work across all commands—see [`wt switch`](@/switch.md#shortcuts) for the full list.

```bash
wt switch --create hotfix --base=@       # Branch from current HEAD
wt switch -                              # Switch to previous worktree
wt remove @                              # Remove current worktree
```

## Stacked branches

Branch from current HEAD instead of the default branch:

```bash
wt switch --create feature-part2 --base=@
```

Creates a worktree that builds on the current branch's changes.

## Bare repository layout

An alternative to the default sibling layout (`myproject.feature/`) uses a bare repository with worktrees as subdirectories:

```
myproject/
├── .git/       # bare repository
├── main/       # main branch
├── feature/    # feature branch
└── bugfix/     # bugfix branch
```

Setup:

```bash
git clone --bare <url> myproject/.git
cd myproject
```

Configure worktrunk to create worktrees as subdirectories:

```toml
# ~/.config/worktrunk/config.toml
worktree-path = "{{ branch | sanitize }}"
```

Create the first worktree:

```bash
wt switch --create main
```

Now `wt switch --create feature` creates `myproject/feature/`.
