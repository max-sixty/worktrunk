# Output System Architecture

## Shell Integration

Worktrunk uses file-based directive passing for shell integration:

1. Shell wrapper creates a temp file via `mktemp`
2. Shell wrapper sets `WORKTRUNK_DIRECTIVE_FILE` env var to the file path
3. wt writes shell commands (like `cd '/path'`) to that file
4. Shell wrapper sources the file after wt exits

When `WORKTRUNK_DIRECTIVE_FILE` is not set (direct binary call), commands execute
directly and shell integration hints are shown.

## Output Functions

The output system handles shell integration automatically. Just call output
functions — they do the right thing regardless of whether shell integration is
active.

```rust
// NEVER DO THIS - don't check mode in command code
if is_shell_integration_active() {
    // different behavior
}

// ALWAYS DO THIS - just call output functions
output::print(success_message("Created worktree"))?;
output::change_directory(&path)?;  // Writes to directive file if set, else no-op
```

**Output functions** (`src/output/global.rs`):

| Function | Destination | Purpose |
|----------|-------------|---------|
| `print(message)` | stderr | Status messages (use with formatting functions) |
| `blank()` | stderr | Visual separation |
| `stdout(content)` | stdout | Primary output (tables, JSON, pipeable) |
| `change_directory(path)` | directive file | Shell cd after wt exits |
| `execute(command)` | directive file | Shell command after wt exits |
| `flush()` | both | Flush buffers (call before interactive prompts) |
| `terminate_output()` | stderr | Reset ANSI state on stderr |
| `is_shell_integration_active()` | — | Check if directive file set (rarely needed) |

**Message formatting functions** (`worktrunk::styling`):

| Function | Symbol | Color |
|----------|--------|-------|
| `success_message()` | ✓ | green |
| `progress_message()` | ◎ | cyan |
| `info_message()` | ○ | — |
| `warning_message()` | ▲ | yellow |
| `hint_message()` | ↳ | dim |
| `error_message()` | ✗ | red |

## stdout vs stderr

**Decision principle:** If this command is piped, what should the receiving program get?

- **stdout** → Data for pipes, scripts, `eval` (tables, JSON, shell code)
- **stderr** → Status for the human watching (progress, success, errors, hints)
- **directive file** → Shell commands executed after wt exits (cd, exec)

Examples:
- `wt list` → table/JSON to stdout (for grep, jq, scripts)
- `wt config shell init` → shell code to stdout (for `eval`)
- `wt switch` → status messages only (nothing to pipe)

## Security

`WORKTRUNK_DIRECTIVE_FILE` is automatically removed from spawned subprocesses
(via `shell_exec::run()`). This prevents hooks from writing to the directive
file.

## Windows Compatibility (Git Bash / MSYS2)

On Windows with Git Bash, `mktemp` returns POSIX-style paths like `/tmp/tmp.xxx`.
The native Windows binary (`wt.exe`) needs a Windows path to write to the
directive file.

**No explicit path conversion is needed.** MSYS2 (which Git Bash uses)
automatically converts POSIX paths in environment variables when spawning native
Windows binaries. When the shell wrapper sets `WORKTRUNK_DIRECTIVE_FILE=/tmp/...`
and runs `wt.exe`, MSYS2 translates this to `C:\Users\...\Temp\...` before the
binary sees it.

See: https://www.msys2.org/docs/filesystem-paths/

This means the shell wrapper templates can use `$directive_file` directly without
calling `cygpath -w`. The conversion happens automatically in the MSYS2 runtime.

## Simplification Notes

The output system was originally more complex to handle shell integration
edge cases. After consolidation, the thin wrappers (`print`, `stdout`,
`blank`) are essentially `eprintln!`/`println!` + flush.

**What still provides value:**

- `change_directory()`, `execute()` — IPC with shell wrapper via directive file
- `terminate_output()` — ANSI reset when needed

**What could be further simplified:**

- `print()` → `eprintln!()` + flush (callers must remember to flush)
- `stdout()` → `println!()` + flush
- `blank()` → `eprintln!()` + flush

The abstraction cost is low, but if we wanted to reduce indirection, these
wrappers could be removed. The main value they provide is consistency (correct
stream, always flushing).

## Related

For message content and styling conventions, see `cli-output-formatting.md`.
