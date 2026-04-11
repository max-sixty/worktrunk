# Spec: Unified Hook/Alias Execution

## Problem

Hooks and aliases share most infrastructure (config schema, template engine, context
building, approval system) but diverge at execution time into two separate code paths
with different capabilities. Aliases miss signal forwarding, ANSI reset, command logging
via `Cmd`, concurrent pipeline steps, and lazy `vars.*` expansion. Hooks miss directive
file pass-through for shell integration.

Three consumers of shell execution exist today:

| Consumer | Function | Via `Cmd`? | Signal fwd | Directive file | Pipeline steps |
|----------|----------|-----------|------------|----------------|----------------|
| Hooks (foreground) | `execute_command_in_worktree` | Yes | Yes | Scrubbed | Full |
| Aliases | `run_command_streaming` | No | No | Pass-through | Flattened |
| `for-each` | `run_command_streaming` | No | No | Scrubbed | N/A |

## End state

A single leaf execution function replaces both `execute_command_in_worktree` and
`run_command_streaming`. All three consumers call it.

```
execute_command_in_worktree(options)
├── Hooks (foreground pre-*, post-* with --foreground)
├── Aliases (wt step <name>)
└── for-each (wt step for-each)
```

The background pipeline runner (`run_pipeline.rs`) keeps its own spawning logic — it
redirects to log files and runs detached from the terminal.

### Unified capabilities

| Capability | Before | After |
|---|---|---|
| Signal forwarding | Hooks only | All three |
| ANSI reset before child output | Hooks only | All three |
| `Cmd` builder (tracing, logging) | Hooks only | All three |
| Pipeline steps (serial + concurrent) | Hooks only | Hooks + aliases |
| Lazy `vars.*` expansion | Hooks only | Hooks + aliases |
| Directive file pass-through | Aliases only | Foreground hooks + aliases |

### Leaf executor interface

```rust
struct CommandExecOptions<'a> {
    working_dir: &'a Path,
    stdin_json: Option<&'a str>,
    directive_file: Option<&'a Path>,  // None = scrub env var
    command_log_label: Option<&'a str>,
}
```

## Decisions

### Directive file: foreground hooks pass through

Foreground hooks (all pre-\* hooks; post-\* with `--foreground`) pass the parent
shell's directive file to child processes. Background hooks continue to scrub — they
outlive the parent shell, so the directive file would be stale.

Foreground hooks have the same trust profile as aliases: the hook body is already
arbitrary shell that can `cd`/`rm`/`exec` anything. Letting it write a `cd` directive
is strictly less powerful than what it can already do.

**Effect**: `wt switch --create` inside a pre-start hook body lands the shell in the new
worktree (currently drops the `cd`).

### Aliases: no command logging

Alias runs do not log to `.git/wt/logs/`. Logging exists for background hooks where
output is invisible. Aliases are always foreground — the user sees output directly.

### Announcements: separate format, aliases gain pipeline summary

Hooks and aliases keep distinct announcement styles:

```
Hook:    Running post-merge user:foo @ /path
Alias:   Running alias deploy: install; build, lint
```

When aliases gain pipeline support, the announcement shows the pipeline structure so the
user sees what's about to run.

### `for-each`: unified leaf executor

`for-each` switches from `run_command_streaming` to the unified executor, gaining signal
forwarding and ANSI reset. Iteration logic (multi-worktree, continue-on-failure summary)
stays in `for_each.rs`. Directive file stays scrubbed — `for-each` runs in other
worktrees, not the current one.

### Config schema: stays separate

`[hooks]` and `[aliases]` remain separate config sections. The execution unification is
internal. Config stays separate because the *meaning* differs (event-driven vs
user-invoked), even though the mechanism becomes shared.

## What stays separate

- **Background execution**: Aliases are always foreground. Background spawning
  (`spawn_hook_pipeline`, `run_pipeline.rs`) remains hook-only.
- **`--var` on aliases vs hooks**: Both already support `--var`. No change needed.
- **`--dry-run`**: Both already support it. No change needed.
- **Name filters**: Hook name filters (`user:foo`, `project:bar`) operate within a hook
  type. Alias name selects the whole `CommandConfig`. No convergence needed.
- **Announcement format**: Hooks and aliases use different announcement functions (see
  Decisions above).
