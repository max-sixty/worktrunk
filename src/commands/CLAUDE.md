# Debugging Interactive Terminal Commands

When debugging TUI commands like `wt select`, use the `tmux-cli` skill (preferred) or MCP's `node-terminal` tools to test interactively.

## Debugging Workflow

### 1. Create Test Environment

```bash
cargo run --bin setup-select-test
```

This creates a reproducible test repo at `/tmp/wt-select-test/test-repo`.

### 2. Test Interactively

#### Option A: tmux-cli skill (preferred, if available)

Load the `tmux-cli` skill, then use the `tmux-cli` tool. Install if needed: `uv tool install claude-code-tools` (requires tmux).

```bash
# Launch shell in test repo
pane=$(tmux-cli launch "zsh")
tmux-cli send "cd /tmp/wt-select-test/test-repo" --pane=$pane
tmux-cli wait_idle --pane=$pane

# Run with debug logging
tmux-cli send "RUST_LOG=worktrunk=debug cargo run --quiet -- select 2> debug.log" --pane=$pane
tmux-cli wait_idle --pane=$pane

# Test interaction (e.g., select option 3)
tmux-cli send "3" --pane=$pane
tmux-cli wait_idle --pane=$pane

# Capture output
tmux-cli capture --pane=$pane
```

#### Option B: MCP node-terminal

MCP terminals use pseudo-TTY, not real terminals. If tests pass in MCP but users report issues, the bug is likely environment-specific. Always test on the actual problematic repository.

```typescript
// Create terminal and navigate to test repo
mcp__node-terminal__terminal_create({ sessionId: "test" })
mcp__node-terminal__terminal_write({ sessionId: "test", input: "cd /tmp/wt-select-test/test-repo" })
mcp__node-terminal__terminal_send_key({ sessionId: "test", key: "enter" })

// Run with debug logging
mcp__node-terminal__terminal_write({
  sessionId: "test",
  input: "RUST_LOG=worktrunk=debug cargo run --quiet -- select 2> debug.log"
})
mcp__node-terminal__terminal_send_key({ sessionId: "test", key: "enter" })

// Test the interaction
mcp__node-terminal__terminal_write({ sessionId: "test", input: "3" })
mcp__node-terminal__terminal_read({ sessionId: "test" })
```

### 3. Analyze Logs

```bash
tail -100 debug.log | grep -E "error|hang|stuck"
```

## Important Flags

- **`-C <path>`**: Set working directory (alternative to `cd`)
- **`--source`**: Use local source (only needed with installed `wt`, not with `cargo run`)

```bash
# Testing with cargo run (already uses local source):
cargo run --quiet -- -C /path/to/repo select

# Testing with installed wt:
wt --source -C /path/to/repo select
```

## Shell Completion for CLI Arguments

Branch and worktree arguments should include shell completion for better UX. Add completion helpers to CLI definitions:

```rust
/// Target branch (defaults to current)
#[arg(long, add = crate::completion::branch_value_completer())]
branch: Option<String>,
```

**Available completers:**
- `branch_value_completer()` - Completes with branch names
- `worktree_branch_completer()` - Completes with worktree paths and branch names

**Pattern:** All branch arguments should use `branch_value_completer()` for consistency with commands like `wt merge`, `wt switch --base`, `wt rebase`.

## CLI Help Text Placement

Help text has three levels:

1. **`about`** (single-line doc comment) → Short title after command name
2. **`long_about`** (multi-line doc comment, 1-2 sentences) → Brief summary before options
3. **`after_long_help`** → Examples and detailed docs after options

**Pattern for complex commands:**

```rust
/// Merge worktree into target branch
///
/// Commits, squashes, rebases, runs hooks, merges to target, and removes the worktree.
#[command(
    after_long_help = r#"## Examples

```console
wt merge
```
"#
)]
Merge { ... }
```

This renders as:
```
wt merge - Merge worktree into target branch

Commits, squashes, rebases, runs hooks, merges to target, and removes the worktree.

Usage: wt merge [OPTIONS] [TARGET]

Options:
  ...

## Examples
...
```

**Pattern for simple commands:**

```rust
/// Rebase onto target
Rebase { ... }
```

No `long_about` or `after_long_help` needed when the short description is self-explanatory.

**When to use `long_about`:** Add a 1-2 sentence summary when the short description doesn't convey the full behavior (e.g., `wt merge` does more than just "merge").

**Why:** Users see context before options for complex commands, but options stay near the top. Examples and detailed docs follow after.

## Command Documentation Guidelines

When writing or updating command docs in `docs/content/`, follow this structure and these principles. Load the `documentation` skill for additional guidance.

### Structure

Each command page should follow this order:

1. **Intro paragraph** — One or two sentences: what the command does and when to use it. Integrate key behavioral distinctions (e.g., "Switching to an existing worktree is just a directory change. With `--create`, hooks run.")
2. **Examples** — Common use cases with brief labels, immediately after intro
3. **Feature sections** — Deeper explanation of major features (e.g., "Creating worktrees", "Shortcuts")
4. **Hooks** — Brief summary with link to `/hook/` for details
5. **Technical details** — Implementation details like argument resolution, pushed to the bottom
6. **Command reference** — Auto-generated from `--help-page`, always last

### Writing Style

- **Indicative mood over imperative** — "The `--create` flag creates..." not "Use `--create` to create..."
- **Spaced em-dashes** — "instant — no stashing" not "instant—no stashing"
- **No second person** — Describe behavior, don't address the reader
- **Concrete examples** — Real commands, actual output, specific scenarios
- **Link to dedicated pages** — Don't duplicate content from `/hook/`, `/configuration/`, etc.

### What to Avoid

- AI-slop: series of headings with 3-5 bullets each
- Redundant content that duplicates other pages
- Technical details at the top (push Operation/Resolution sections down)
- Wrapper sections that just contain one subsection (remove "Operation" if it only contains "How Arguments Are Resolved")
- Presuming user intent — describe what the command does, not why users run it

### Example: Don't Presume Intent

```markdown
# Bad — presumes why users run the command
See which worktrees need attention.

# Good — describes what it does
Show all worktrees with their status.
```

Users run `wt list` for many reasons: checking status, finding a branch, remembering what they were working on, scripting. The intro should describe the command's behavior, not assume the user's goal.

### Example: Good Intro

```markdown
Navigate between worktrees or create new ones. Switching to an existing
worktree is just a directory change. With `--create`, a new branch and
worktree are created, and hooks run.
```

### Updating These Guidelines

As command docs are improved, update this section to capture new patterns that emerge.
