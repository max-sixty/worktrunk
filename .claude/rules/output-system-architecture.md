# Output System Architecture

## Shell Integration

Worktrunk uses file-based directive passing for shell integration:

1. Shell wrapper creates a temp file via `mktemp`
2. Shell wrapper sets `WORKTRUNK_DIRECTIVE_FILE` env var to the file path
3. wt writes shell commands (like `cd '/path'`) to that file
4. Shell wrapper sources the file after wt exits

When `WORKTRUNK_DIRECTIVE_FILE` is not set (direct binary call), commands execute
directly and shell integration hints are shown.

## The Cardinal Rule: Never Check Mode in Command Code

Command code must never check whether shell integration is active. The output
system handles this automatically — commands call output functions without
knowing the mode.

```rust
// NEVER DO THIS
if is_shell_integration_active() {
    // different behavior
}

// ALWAYS DO THIS
output::print(success_message("Success!"))?;
output::change_directory(&path)?;
```

State is lazily initialized on first use — just call output functions anywhere:

```rust
output::print(success_message("Created worktree"))?;
output::change_directory(&path)?;
```

## Available Output Functions

The output module (`src/output/global.rs`) provides:

- `print(message)` — Status message to stderr (use with message formatting functions)
- `shell_integration_hint(message)` — Shell integration hints (↳, suppressed when
  shell integration is active)
- `gutter(content)` — Gutter-formatted content to stderr
- `blank()` — Blank line to stderr
- `data(content)` — Structured data to stdout (JSON)
- `table(content)` — Primary output to stdout (table data, pipeable)
- `change_directory(path)` — Request directory change (writes to directive file
  if set)
- `execute(command)` — Execute command directly or write to directive file
- `flush()` — Flush output buffers
- `flush_for_stderr_prompt()` — Flush before interactive prompts
- `terminate_output()` — Reset ANSI state on stderr
- `is_shell_integration_active()` — Check if directive file is set (rarely needed)

**Message formatting functions** (from `worktrunk::styling`):

- `success_message(content)` — ✓ green
- `progress_message(content)` — ◎ cyan
- `info_message(content)` — ○ no color
- `warning_message(content)` — ▲ yellow
- `hint_message(content)` — ↳ dimmed
- `error_message(content)` — ✗ red

For the complete API, see `src/output/global.rs` and `src/styling/constants.rs`.

## Security: Protecting the Directive File

The `WORKTRUNK_DIRECTIVE_FILE` environment variable is automatically removed from all
spawned subprocesses. This prevents hooks from discovering and writing to the
directive file.

The `shell_exec::run()` function (the canonical way to run external commands)
handles this automatically. Other spawn sites must explicitly call
`.env_remove(DIRECTIVE_FILE_ENV_VAR)`.

## Adding New Output Functions

Add the function to `global.rs`. The pattern:
- **Primary output** (data the command produces) → stdout via `table()` or `data()`
- **Status messages** (progress, success, errors) → stderr via `print()`
- **Directives** (cd, exec) → directive file via `change_directory()`, `execute()`

This separation makes commands pipeable: `wt list | grep feature` works because
the table goes to stdout while progress/warnings go to stderr.
