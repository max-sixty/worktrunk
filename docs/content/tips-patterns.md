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

Use [`wt step copy-ignored`](@/step.md#wt-step-copy-ignored) to copy gitignored files (caches, dependencies, `.env`) between worktrees:

```toml
[post-start]
copy = "wt step copy-ignored"
```

Use `post-create` instead if subsequent hooks or `--execute` command need the copied files immediately.

All gitignored files are copied by default. To limit what gets copied, create `.worktreeinclude` with patterns — files must be both gitignored and listed. See [`wt step copy-ignored`](@/step.md#wt-step-copy-ignored) for details.

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
<span class="cmd">wt list</span>
  <b>Branch</b>       <b>Status</b>        <b>HEAD±</b>    <b>main↕</b>  <b>Remote⇅</b>  <b>URL</b>                     <b>Commit</b>    <b>Age</b>
@ main           <span class=c>?</span> <span class=d>^</span><span class=d>⇅</span>                         <span class=g>⇡1</span>  <span class=d><span class=r>⇣1</span></span>  <span class=d>http://localhost:12107</span>  <span class=d>41ee0834</span>  <span class=d>4d</span>
+ feature-api  <span class=c>+</span>   <span class=d>↕</span><span class=d>⇡</span>     <span class=g>+54</span>   <span class=r>-5</span>   <span class=g>↑4</span>  <span class=d><span class=r>↓1</span></span>   <span class=g>⇡3</span>      <span class=d>http://localhost:10703</span>  <span class=d>6814f02a</span>  <span class=d>30m</span>
+ fix-auth         <span class=d>↕</span><span class=d>|</span>                <span class=g>↑2</span>  <span class=d><span class=r>↓1</span></span>     <span class=d>|</span>     <span class=d>http://localhost:16460</span>  <span class=d>b772e68b</span>  <span class=d>5h</span>

<span class=d>○</span> <span class=d>Showing 3 worktrees, 2 with changes, 2 ahead, 2 columns hidden</span>
{% end %}

<!-- END AUTO-GENERATED -->

Ports are deterministic — `fix-auth` always gets port 16460, regardless of which machine or when. The URL dims if the server isn't running.

## Database per worktree

Each worktree can have its own isolated database. Docker containers get unique names and ports:

```toml
[post-start]
db = """
docker run -d --rm \
  --name {{ repo }}-{{ branch | sanitize }}-postgres \
  -p {{ ('db-' ~ branch) | hash_port }}:5432 \
  -e POSTGRES_DB={{ branch | sanitize_db }} \
  -e POSTGRES_PASSWORD=dev \
  postgres:16
"""

[pre-remove]
db-stop = "docker stop {{ repo }}-{{ branch | sanitize }}-postgres 2>/dev/null || true"
```

The `('db-' ~ branch)` concatenation hashes differently than plain `branch`, so database and dev server ports don't collide.
Jinja2's operator precedence has pipe `|` with higher precedence than concatenation `~`, meaning expressions need parentheses to filter concatenated values.

The `sanitize_db` filter produces database-safe identifiers (lowercase, underscores, no leading digits, with a short hash suffix to avoid collisions and SQL reserved words).

Generate `.env.local` with the correct `DATABASE_URL` using a `post-create` hook:

```toml
[post-create]
env = """
cat > .env.local << EOF
DATABASE_URL=postgres://postgres:dev@localhost:{{ ('db-' ~ branch) | hash_port }}/{{ branch | sanitize_db }}
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

## Agent handoffs

Spawn a worktree with Claude running in the background:

**tmux** (new detached session):
```bash
tmux new-session -d -s fix-auth-bug "wt switch --create fix-auth-bug -x claude -- \
  'The login session expires after 5 minutes. Find the session timeout config and extend it to 24 hours.'"
```

**Zellij** (new pane in current session):
```bash
zellij run -- wt switch --create fix-auth-bug -x claude -- \
  'The login session expires after 5 minutes. Find the session timeout config and extend it to 24 hours.'
```

This lets one Claude session hand off work to another that runs in the background. Hooks run inside the multiplexer session/pane.

The [worktrunk skill](@/claude-code.md) includes guidance for Claude Code to execute this pattern. To enable it, request it explicitly ("spawn a parallel worktree for...") or add to `CLAUDE.md`:

```markdown
When I ask you to spawn parallel worktrees, use the agent handoff pattern
from the worktrunk skill.
```

## Tmux session per worktree

Each worktree gets its own tmux session with a multi-pane layout. Sessions are named after the branch for easy identification.

```toml
# .config/wt.toml
[post-create]
tmux = """
S="{{ branch | sanitize }}"
W="{{ worktree_path }}"
tmux new-session -d -s "$S" -c "$W" -n dev

# Create 4-pane layout: shell | backend / claude | frontend
tmux split-window -h -t "$S:dev" -c "$W"
tmux split-window -v -t "$S:dev.0" -c "$W"
tmux split-window -v -t "$S:dev.2" -c "$W"

# Start services in each pane
tmux send-keys -t "$S:dev.1" 'npm run backend' Enter
tmux send-keys -t "$S:dev.2" 'claude' Enter
tmux send-keys -t "$S:dev.3" 'npm run frontend' Enter

tmux select-pane -t "$S:dev.0"
echo "✓ Session '$S' — attach with: tmux attach -t $S"
"""

[pre-remove]
tmux = "tmux kill-session -t '{{ branch | sanitize }}' 2>/dev/null || true"
```

`pre-remove` stops all services when the worktree is removed.

To create a worktree and immediately attach:

```bash
wt switch --create feature -x 'tmux attach -t {{ branch | sanitize }}'
```

## Subdomain routing with Caddy
<!-- Hand-tested 2025-01-15 -->

Clean URLs like `http://feature-auth.myproject.lvh.me` without port numbers. Useful for cookies, CORS, and matching production URL structure.

**Prerequisites:** [Caddy](https://caddyserver.com/docs/install) (`brew install caddy`)

```toml
# .config/wt.toml
[post-start]
server = "npm run dev -- --port {{ branch | hash_port }}"
proxy = """
  curl -sf --max-time 0.5 http://localhost:2019/config/ || caddy start
  curl -sf http://localhost:2019/config/apps/http/servers/wt || \
    curl -sfX PUT http://localhost:2019/config/apps/http/servers/wt -H 'Content-Type: application/json' \
      -d '{"listen":[":8080"],"automatic_https":{"disable":true},"routes":[]}'
  curl -sf -X DELETE http://localhost:2019/id/wt:{{ repo }}:{{ branch | sanitize }} || true
  curl -sfX PUT http://localhost:2019/config/apps/http/servers/wt/routes/0 -H 'Content-Type: application/json' \
    -d '{"@id":"wt:{{ repo }}:{{ branch | sanitize }}","match":[{"host":["{{ branch | sanitize }}.{{ repo }}.lvh.me"]}],"handle":[{"handler":"reverse_proxy","upstreams":[{"dial":"127.0.0.1:{{ branch | hash_port }}"}]}]}'
"""

[pre-remove]
proxy = "curl -sf -X DELETE http://localhost:2019/id/wt:{{ repo }}:{{ branch | sanitize }} || true"

[list]
url = "http://{{ branch | sanitize }}.{{ repo }}.lvh.me:8080"
```

**How it works:**

1. `wt switch --create feature-auth` runs the `post-start` hook, starting the dev server on a deterministic port (`{{ branch | hash_port }}` → 16460)
2. The hook starts Caddy if needed and registers a route using the same port: `feature-auth.myproject` → `localhost:16460`
3. `lvh.me` is a public domain with wildcard DNS — `*.lvh.me` resolves to `127.0.0.1`
4. Visiting `http://feature-auth.myproject.lvh.me:8080`: Caddy matches the subdomain and proxies to the dev server

## Monitor hook logs

Follow background hook output in real-time:

```bash
tail -f "$(wt config state logs get --hook=user:post-start:server)"
```

The `--hook` format is `source:hook-type:name` — e.g., `project:post-start:build` for project-defined hooks. Use `wt config state logs get` to list all available logs.

Create an alias for frequent use:

```bash
alias wtlog='f() { tail -f "$(wt config state logs get --hook="$1")"; }; f'
```

## Bare repository layout

An alternative to the default sibling layout (`myproject.feature/`) uses a bare repository with worktrees as subdirectories:

```
myproject/
├── .git/       # bare repository
├── main/       # default branch
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
