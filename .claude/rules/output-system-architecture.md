# Output System Architecture

## Two Output Modes

Worktrunk supports two output modes, selected once at program startup:

1. **Interactive Mode** — Human-friendly output with colors, emojis, and hints
2. **Directive Mode** — Shell script on stdout (at end), user messages on stderr

Both modes write all messages to stderr. stdout is reserved for structured data
(JSON, shell scripts).

The mode is determined at initialization in `main()` and never changes during
execution.

## The Cardinal Rule: Never Check Mode in Command Code

Command code must never check which output mode is active. The output system
uses enum dispatch — commands call output functions without knowing the mode.

```rust
// NEVER DO THIS
if mode == OutputMode::Interactive {
    println!("✅ Success!");
}

// ALWAYS DO THIS
output::print(success_message("Success!"))?;
```

Decide once at the edge (`main()`), initialize globally, trust internally:

```rust
// In main.rs - the only place that knows about modes
let output_mode = match cli.internal {
    Some(shell) => output::OutputMode::Directive(shell),
    None => output::OutputMode::Interactive,
};
output::initialize(output_mode);

// Everywhere else - just use the output functions
output::print(success_message("Created worktree"))?;
output::change_directory(&path)?;
```

## Available Output Functions

The output module (`src/output/global.rs`) provides:

- `print(message)` — Write message as-is (use with message formatting functions)
- `shell_integration_hint(message)` — Shell integration hints (💡, suppressed in
  directive)
- `gutter(content)` — Gutter-formatted content (use with `format_with_gutter()`)
- `blank()` — Blank line for visual separation
- `data(content)` — Structured data output without emoji (JSON, for piping)
- `table(content)` — Table/UI output to stderr
- `change_directory(path)` — Request directory change
- `execute(command)` — Execute command or buffer for shell script
- `flush()` — Flush output buffers
- `flush_for_stderr_prompt()` — Flush before interactive prompts
- `terminate_output()` — Emit shell script in directive mode (no-op in
  interactive)

**Message formatting functions** (from `worktrunk::styling`):

- `success_message(content)` — ✅ green
- `progress_message(content)` — 🔄 cyan
- `info_message(content)` — ⚪ no color
- `warning_message(content)` — 🟡 yellow
- `hint_message(content)` — 💡 dimmed
- `error_message(content)` — ❌ red

For the complete API, see `src/output/global.rs` and `src/styling/constants.rs`.

## Adding New Output Functions

Add the function to both handlers, add dispatch in `global.rs`, never add mode
parameters. This maintains one canonical path: commands have ONE code path that
works for both modes.

## Architectural Constraint: --internal Commands Must Use Output System

Commands supporting `--internal` must never use direct print macros — use output
system functions to prevent directive leaks. Enforced by
`tests/output_system_guard.rs`.
