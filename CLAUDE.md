# Worktrunk Development Guidelines

> **Note**: This CLAUDE.md is just getting started. More guidelines will be added as patterns emerge.

## Project Status

**This project has no users yet and zero backward compatibility concerns.**

We are in **pre-release development** mode:
- Breaking changes are acceptable and expected
- No migration paths needed for config changes, API changes, or behavior changes
- Optimize for the best solution, not compatibility with previous versions
- Move fast and make bold improvements

When making decisions, prioritize:
1. **Best technical solution** over backward compatibility
2. **Clean design** over maintaining old patterns
3. **Modern conventions** over legacy approaches

Examples of acceptable breaking changes:
- Changing config file locations (e.g., moving from `~/Library/Application Support` to `~/.config`)
- Renaming commands or flags for clarity
- Changing output formats
- Replacing dependencies with better alternatives
- Restructuring the codebase

When the project reaches v1.0 or gains users, we'll adopt stability commitments. Until then, we're free to iterate rapidly.

## CLI Output Formatting Standards

### User Message Principles

**Core Principle: Acknowledge user-supplied arguments in output messages.**

When users provide explicit arguments (flags, options, values), the output should recognize and reflect those choices. This confirms the program understood their intent and used their input correctly.

**Examples:**

```rust
// User runs: wt switch --create feature --base=main
// ✅ GOOD - acknowledges the base branch
"Created new worktree for feature from main at /path/to/worktree"

// ❌ BAD - ignores the base argument
"Created new worktree for feature at /path/to/worktree"

// User runs: wt merge --squash
// ✅ GOOD - acknowledges squash mode
"Squashing 3 commits into 1..."

// ❌ BAD - doesn't mention squashing
"Merging commits..."
```

**Why this matters:**
- Builds confidence that arguments were parsed correctly
- Helps users understand what the command actually did
- Makes output more informative and traceable
- Prevents confusion about which options were applied

**Implementation pattern:**
- When formatting messages, include information from user-supplied arguments
- Don't just use defaults silently—show what was chosen
- For optional arguments, conditionally include them in the message

**Avoid redundant parenthesized content:**

Parenthesized text should add new information, not restate what's already said. If the parentheses just rephrase the main message in different words, remove them.

```rust
// ❌ BAD - parentheses restate "no changes"
"No changes after squashing 3 commits (commits resulted in no net changes)"

// ✅ GOOD - clear and concise
"No changes after squashing 3 commits"

// ✅ GOOD - parentheses add supplementary info (stats)
"Committing with default message... (3 files, +45, -12)"

// ✅ GOOD - parentheses explain why
"Worktree preserved (--no-remove)"
```

When reviewing messages, ask: "Does the parenthesized text add information, or just reword what's already clear?"

### The anstyle Ecosystem

All styling uses the **anstyle ecosystem** for composable, auto-detecting terminal output:

- **`anstream`**: Auto-detecting I/O streams (println!, eprintln! macros)
- **`anstyle`**: Core styling with inline pattern `{style}text{style:#}`
- **Color detection**: Respects NO_COLOR, CLICOLOR_FORCE, TTY detection

### Message Types

Six canonical message patterns with their emojis:

1. **Progress**: 🔄 + cyan text (operations in progress)
2. **Success**: ✅ + green text (successful completion)
3. **Errors**: ❌ + red text (failures, invalid states)
4. **Warnings**: 🟡 + yellow text (non-blocking issues)
5. **Hints**: 💡 + dimmed text (actionable suggestions, tips for user)
6. **Info**: ⚪ + text (neutral status, system feedback, metadata)
   - **NOT dimmed**: Primary status messages that answer the user's question
   - **Dimmed**: Supplementary metadata and contextual information

**Core Principle: Every user-facing message must have EITHER an emoji OR a gutter.**

This provides consistent visual separation and hierarchy:
- **Emoji**: For standalone messages (success, error, warning, hint, progress, info)
- **Gutter**: For multi-line quoted content (commands, config, code blocks)
- **Section headers**: Use emoji + header text, followed by gutter content
- **Exception**: Lines within multi-line blocks that already have an emoji on the first line

```rust
// ✅ GOOD - standalone message with emoji
println!("{SUCCESS_EMOJI} {GREEN}Created worktree{GREEN:#}");

// ✅ GOOD - message with path on same line
let green_bold = GREEN.bold();
println!(
    "{SUCCESS_EMOJI} {GREEN}Created config file: {green_bold}{}{green_bold:#}",
    path.display()
);

// ✅ GOOD - info message for primary status (NOT dimmed - answers user's question)
output::info("All commands already approved")?;
output::info("Commands declined")?;

// ✅ GOOD - info message for supplementary metadata (dimmed)
let dim = AnstyleStyle::new().dimmed();
println!("{INFO_EMOJI} {dim}Showing 5 worktrees, 2 with changes...{dim:#}");

// ✅ GOOD - quoted content with gutter
print!("{}", format_with_gutter(&command));

// ✅ GOOD - section header with emoji, followed by gutter content
println!("{INFO_EMOJI} Global Config: {bold}{}{bold:#}", path.display());
print!("{}", format_toml(&contents, ""));

// ✅ GOOD - multi-line block with emoji on first line
println!("{WARNING_EMOJI} {WARNING}Permission required{WARNING:#}");
println!("Additional context on second line");  // Part of same block

// ❌ BAD - standalone message without emoji or gutter
println!("{dim}Operation declined{dim:#}");

// ❌ BAD - section header without emoji
println!("Config: {bold}{}{bold:#}", path);  // Should have INFO_EMOJI

// ❌ BAD - path on separate line without emoji
println!("{SUCCESS_EMOJI} {GREEN}Created config file{GREEN:#}");
println!("{dim}{}{dim:#}", path.display());  // Should be on same line as success message

// ❌ BAD - dimming primary status that answers user's question
output::info(format!("{dim}Commands declined{dim:#}"))?;  // Should NOT be dim
```

### stdout vs stderr: Separation by Source

**Core Principle: Separate output by who generates it, not by message type.**

- **stdout**: All worktrunk output (messages, directives, errors, warnings, progress)
- **stderr**: All child process output (git, npm, user commands)
- **Exception**: Interactive prompts use stderr to bypass shell wrapper's NUL-delimited parsing

**Why:** Simple reasoning (one decision point), better for piping (`wt list | jq`), child output never interferes with directives. Trade-off: violates Unix convention of errors→stderr, but our "errors" are structured program output, not crashes.

```rust
// ALL our output goes to stdout (including errors)
println!("{ERROR_EMOJI} {ERROR}Branch already exists{ERROR:#}");

// Interactive prompts go to stderr to bypass shell wrapper buffering
eprint!("{HINT_EMOJI} Allow and remember? [y/N] ");

// Redirect child processes to stderr
let wrapped = format!("{{ {}; }} 1>&2", command);
Command::new("sh").arg("-c").arg(&wrapped).status()?;
```

**Interactive prompts and stdin:**
- Prompts use stderr to bypass shell wrapper's NUL-delimited stdout parsing
- **CRITICAL**: Flush stderr before blocking on stdin to prevent interleaving:
  ```rust
  eprint!("💡 Allow and remember? [y/N] ");
  stderr().flush()?;  // Ensures prompt is visible before blocking

  let mut response = String::new();
  io::stdin().read_line(&mut response)?;
  ```
- Without flushing, buffered stderr can appear after the prompt is shown
- **Why stderr?** The shell wrapper (`templates/bash.sh`) uses `read -d ''` to parse NUL-delimited directives from stdout. Non-NUL-terminated output (like prompts) would get buffered in the shell's read buffer. Stderr bypasses this parsing and appears immediately.

### Temporal Locality: Output Should Be Close to Operations

**Core Principle: Output should appear immediately adjacent to the operations they describe.**

Output that appears far from its triggering operation breaks the user's mental model.

**Progress messages only for slow operations (>400ms):** Git operations, network requests, builds. Not for file checks or config reads.

**Pattern for sequential operations:**
```rust
for item in items {
    output::progress(format!("🔄 Removing {item}..."))?;
    perform_operation(item)?;
    output::success(format!("Removed {item}"))?;  // Immediate feedback
}
```

**Bad - output decoupled from operations:**
```
🔄 Removing worktree for feature...
🔄 Removing worktree for bugfix...
                                    ← Long delay, no feedback
Removed worktree for feature        ← All output at the end
Removed worktree for bugfix
```

**Red flags:**
- Collecting messages in a buffer: `let mut messages = Vec::new()` → emit immediately instead
- Single success message for batch operations → show success after each item
- No progress before slow operations → user sees nothing, then sudden output
- Progress without matching success → always pair them

**Why this matters:** Immediate feedback builds confidence, failed operations are obvious, Ctrl+C interrupts don't leave uncertainty, matches how users think about sequential operations.

### Information Display: Show Once, Not Twice

**Core Principle: Show detailed context in progress messages, minimal confirmation in success messages.**

When operations have both progress and success messages:
- **Progress message**: Include ALL relevant details - what's being done, counts, stats, context
- **Success message**: MINIMAL - just confirm completion with reference info (hash, path)

This prevents redundant noise while giving users information when they need it most (before the operation runs). Think: "tell me everything before you start, just confirm when you're done."

**Good patterns:**

```rust
// Example 1: Squashing commits
output::progress("🔄 Squashing 3 commits with working tree changes into 1 (5 files, +120, -45)...")?;
perform_squash()?;
output::success("✅ Squashed @ a1b2c3d")?;  // Minimal - no repeated detail

// Example 2: Committing changes
output::progress("🔄 Committing changes... (3 files, +45, -12)")?;
perform_commit()?;
output::success("✅ Committed changes @ a1b2c3d")?;  // Just hash for reference

// Example 3: Creating worktree
output::progress("🔄 Creating worktree for feature-x...")?;
create_worktree()?;
output::success("✅ Created worktree, changed directory to /path/to/worktree")?;  // Just location
```

**Bad patterns:**

```rust
// ❌ Repeating detail in success message
output::progress("🔄 Squashing 3 commits into 1...")?;
perform_squash()?;
output::success("✅ Squashed 3 commits into 1 @ a1b2c3d")?;  // Redundant "3 commits"

// ❌ Repeating stats in success message
output::progress("🔄 Committing changes... (3 files, +45, -12)")?;
perform_commit()?;
output::success("✅ Committed changes (3 files, +45, -12) @ a1b2c3d")?;  // Stats already shown
```

**Rationale:**
- Users read progress messages to understand what's about to happen
- Success messages just confirm completion - details were already shown
- Reference info (hashes, paths) in success messages enable quick lookup
- Reduces visual noise and line count in output
- Makes output scannable: detailed context before, quick confirmation after

### Semantic Style Constants

**Style constants defined in `src/styling.rs`:**

- **`ERROR`**: Red (errors, conflicts)
- **`WARNING`**: Yellow (warnings)
- **`HINT`**: Dimmed (hints, secondary information)
- **`CURRENT`**: Magenta + bold (current worktree)
- **`ADDITION`**: Green (diffs, additions)
- **`DELETION`**: Red (diffs, deletions)

**Emoji constants:**

- **`ERROR_EMOJI`**: ❌ (use with ERROR style)
- **`WARNING_EMOJI`**: 🟡 (use with WARNING style)
- **`HINT_EMOJI`**: 💡 (use with HINT style)
- **`INFO_EMOJI`**: ⚪ (use with dimmed style)

### Inline Formatting Pattern

Use anstyle's inline pattern `{style}text{style:#}` where `#` means reset:

```rust
use worktrunk::styling::{println, ERROR, ERROR_EMOJI, WARNING, WARNING_EMOJI, HINT, HINT_EMOJI, AnstyleStyle};
use anstyle::{AnsiColor, Color};

// Progress
let cyan = AnstyleStyle::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
println!("🔄 {cyan}Rebasing onto main...{cyan:#}");

// Success
let green = AnstyleStyle::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));
println!("✅ {green}Merged to main{green:#}");

// Error - ALL our output goes to stdout
println!("{ERROR_EMOJI} {ERROR}Working tree has uncommitted changes{ERROR:#}");

// Warning - ALL our output goes to stdout
println!("{WARNING_EMOJI} {WARNING}Uncommitted changes detected{WARNING:#}");

// Hint
println!("{HINT_EMOJI} {HINT}Use 'wt list' to see all worktrees{HINT:#}");
```

### Composing Styles

Compose styles using anstyle methods (`.bold()`, `.fg_color()`, etc.). **In messages (not tables), always bold branch names:**

```rust
use worktrunk::styling::{println, AnstyleStyle, ERROR};

// Error message with bold branch name
let error_bold = ERROR.bold();
println!("❌ Branch '{error_bold}{branch}{error_bold:#}' already exists");

// Success message with bold branch name
let bold = AnstyleStyle::new().bold();
println!("Switched to worktree: {bold}{branch}{bold:#}");
```

Tables (`wt list`) use conditional styling for branch names to indicate worktree state (current/primary/other), not bold.

**CRITICAL: Avoid nested style resets** - When composing styles, apply all attributes to a single style object rather than nesting different styles. Nested resets can leak colors:

```rust
// ❌ BAD - nested reset can leak color
"{WARNING}Text with {bold}nested{bold:#} styles{WARNING:#}"
// When {bold:#} resets, it also resets WARNING color!

// ✅ GOOD - compose styles together
let warning_bold = WARNING.bold();
"{WARNING}Text with {warning_bold}composed{warning_bold:#} styles{WARNING:#}"
```

**CRITICAL: Resetting all styles** - To reset ALL ANSI attributes at once, use `anstyle::Reset`, NOT `{:#}` on an empty `Style`:

```rust
// ❌ BAD - produces empty string, NO reset!
output.push_str(&format!("{:#}", Style::new()));

// ✅ GOOD - produces \x1b[0m reset code
output.push_str(&format!("{}", anstyle::Reset));
```

**Why this matters:** `{:#}` only resets when used on a style with attributes. Using it on `Style::new()` (empty style) produces an empty string, causing color bleeding into subsequent output. This was the root cause of color leaking from gutter-formatted commands into child process output.

### Information Hierarchy & Styling

**Principle: Bold what answers the user's question, dim what provides context.**

Style based on **user intent**, not data type. In messages, branch names are always bold. When a path is part of an action phrase (e.g., "changed directory to {path}"), it's bold because it answers "where?". When shown as supplementary metadata on a separate line (e.g., "Path: ..."), it's dimmed. Commit hashes are always dimmed in their surrounding color (reference info).

**Parenthesized suffixes do NOT need to maintain surrounding color.** Parenthesized content (e.g., `(no squashing needed)`, `(3 files, +45, -12)`) can be unstyled/default color even within colored messages. These are supplementary details that don't need color emphasis.

Styled elements (except parenthesized suffixes) must maintain their surrounding color. Don't apply just `{bold}` or `{dim}` to elements in colored messages - compose the color with the style using `.bold()` or `.dimmed()` on the color style. Applying a style without color creates a color leak - the styled element loses its context color and appears in default terminal colors (black/white).

```rust
// WRONG - styled element loses surrounding color
let green = AnstyleStyle::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));
let bold = AnstyleStyle::new().bold();
println!("✅ {green}Message {bold}{path}{bold:#}{green:#}");  // Path will be black/white!

// RIGHT - styled element maintains surrounding color
let green = AnstyleStyle::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));
let green_bold = green.bold();
println!("✅ {green}Message {green_bold}{path}{green_bold:#}");

// Using semantic constants (preferred pattern)
let green_bold = GREEN.bold();
println!("✅ {GREEN}Created worktree, changed directory to {green_bold}{}{green_bold:#}", path.display());

// Commit hash as reference info - dimmed in surrounding color
let green_dim = GREEN.dimmed();
println!("✅ {GREEN}Committed changes @ {green_dim}{hash}{green_dim:#}{GREEN:#}");

// CRITICAL: Styled elements in the MIDDLE of messages
// When a styled element appears mid-message with text after it, re-establish the outer color
// because {styled:#} resets ALL attributes including the outer color
let green_bold = GREEN.bold();
println!("✅ {GREEN}Already on {green_bold}{branch}{green_bold:#}{GREEN}, nothing to merge{GREEN:#}");
//                                                      ^^^^^^^ Re-establish GREEN here!
// Without re-establishing GREEN, ", nothing to merge" would appear in default color (black/white)

// Parenthesized suffixes - unstyled even in colored messages
let cyan_dim = CYAN.dimmed();
println!("🔄 {CYAN}Merging to main @ {cyan_dim}{hash}{cyan_dim:#}{CYAN:#} (no squashing needed)");

// Path as supplementary metadata (separate line) - dimmed in unstyled context
let dim = AnstyleStyle::new().dimmed();
println!("✅ {GREEN}Created worktree{GREEN:#}\n{dim}Path: {}{dim:#}", path.display());

// Element in unstyled message - just bold
let bold = AnstyleStyle::new().bold();
println!("Global Config: {bold}{}{bold:#}", path.display());
```

**Visual hierarchy patterns:**

| Element | Primary (answers question) | Secondary (provides context) |
|---------|---------------------------|------------------------------|
| Branch names | **Bold** (always) | **Bold** (always) |
| File paths | **Bold** (standalone or in action phrase) | **Dim** (supplementary metadata) |
| Config values | Normal | **Dim** |
| Metadata | Dim | **Dim** |

### Indentation Policy

**Core Principle: No manual indentation for secondary information.**

Styling (bold, dim, color) already provides visual hierarchy. Don't add redundant indentation.

```rust
// Good - dimming provides hierarchy
println!("✅ Created {bold}{branch}{bold:#}");
println!("{dim}Path: {}{dim:#}", path.display());

// Bad - unnecessary indent
println!("  {dim}Path: {}{dim:#}", path.display());
```

For quoted content (commands, config), use `format_with_gutter()` instead of manual indents.

### Color Detection

Colors automatically adjust based on environment:
- Respects `NO_COLOR` (disables)
- Respects `CLICOLOR_FORCE` / `FORCE_COLOR` (enables)
- Auto-detects TTY (colors only on terminals)

All handled automatically by `anstream` macros.

**CRITICAL: Always use styled print macros** - Import `print`, `println`, `eprint`, `eprintln` from `worktrunk::styling`, NOT the standard library versions. The styled versions use `anstream` for proper color detection and reset handling. Using standard macros bypasses color management and can cause leaks:

```rust
// ❌ BAD - uses standard library macro, bypasses anstream
eprintln!("{}", styled_text);

// ✅ GOOD - import and use anstream-wrapped version
use worktrunk::styling::eprintln;
eprintln!("{}", styled_text);
```

### Design Principles

- **Inline over wrappers** - Use `{style}text{style:#}` pattern, not wrapper functions
- **Composition over special cases** - Use `.bold()`, `.fg_color()`, not `format_X_with_Y()`
- **Semantic constants** - Use `ERROR`, `WARNING`, not raw colors
- **YAGNI for presentation** - Most output needs no styling
- **Minimal output** - Only use formatting where it adds clarity
- **Unicode-aware** - Width calculations respect emoji and CJK characters (via `StyledLine`)
- **Graceful degradation** - Must work without color support

### StyledLine API

For complex table formatting with proper width calculations, use `StyledLine`:

```rust
use worktrunk::styling::StyledLine;
use anstyle::{AnsiColor, Color, Style};

let mut line = StyledLine::new();
let dim = Style::new().dimmed();
let cyan = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));

line.push_styled("Branch", dim);
line.push_raw("  ");
line.push_styled("main", cyan);

println!("{}", line.render());
```

See `src/commands/list/render.rs` for advanced usage.

### Gutter Formatting for Quoted Content

Use `format_with_gutter()` for quoted content (commands, config). The gutter is a visual separator (colored background) at column 0 - no additional indentation needed.

**CRITICAL: Gutter content must be raw external output without our styling additions.**

When displaying external command output (git errors, shell output, etc.) in gutters:
- **DO**: Pass the raw output string directly
- **DON'T**: Add our emojis (❌, 🟡, etc.) or styling to the content
- **WHY**: Gutters are for quoting external sources verbatim - our visual additions belong in our own messages, not in quotes

```rust
use worktrunk::styling::format_with_gutter;
use worktrunk::git::GitError;

// ✅ GOOD - raw git output in gutter
let raw_error = match &error {
    GitError::CommandFailed(msg) => msg.as_str(),  // Extract raw string
    _ => &error.to_string(),
};
super::gutter(format_with_gutter(raw_error, "", None))?;

// ❌ BAD - includes our formatting in gutter
super::gutter(format_with_gutter(&error.to_string(), "", None))?;  // Adds ❌ emoji

// ✅ GOOD - command output
print!("{}", format_with_gutter(&command));
```

**Example output:**
```
🔄 Executing (post-create):
  npm install

🟡 Could not delete branch feature-x
  error: the branch 'feature-x' is not fully merged
  hint: If you are sure you want to delete it, run 'git branch -D feature-x'
```

The colored space at column 0 provides visual separation from surrounding text. Content starts at column 3 (gutter + 2 spaces) to align with emoji messages where the emoji (2 columns) + space (1 column) also starts content at column 3.

## Output System Architecture

### Two Output Modes

Worktrunk supports two output modes, selected once at program startup:

1. **Interactive Mode** - Human-friendly output with colors, emojis, and hints
2. **Directive Mode** - Machine-readable NUL-terminated directives for shell integration

The mode is determined at initialization in `main()` and never changes during execution.

### The Cardinal Rule: Never Check Mode in Command Code

**CRITICAL: Command code must NEVER check which output mode is active.**

The output system uses enum dispatch with a global context. Commands call output functions (`output::success()`, `output::change_directory()`, etc.) without knowing or caring which mode is active. The output system dispatches to the appropriate handler.

**Bad - mode conditionals scattered through commands:**
```rust
// ❌ NEVER DO THIS
use crate::output::OutputMode;

fn some_command(mode: OutputMode) {
    if mode == OutputMode::Interactive {
        println!("✅ Success!");
    } else {
        println!("Success!\0");
    }
}
```

**Good - use the output system:**
```rust
// ✅ ALWAYS DO THIS
use crate::output;

fn some_command() {
    output::success("Success!")?;
    // The output system handles formatting for both modes
}
```

### How It Works

The output system implements the "trust boundaries" principle:

1. **Decide once at the edge** - `main()` determines mode from CLI flags
2. **Initialize globally** - `output::initialize(mode)` sets up the handler
3. **Trust internally** - Commands just call output functions
4. **Dispatch handles adaptation** - Enum dispatch routes to appropriate handler

```rust
// In main.rs - the only place that knows about modes
let mode = if internal {
    OutputMode::Directive
} else {
    OutputMode::Interactive
};
output::initialize(mode);

// Everywhere else - just use the output functions
output::success("Created worktree")?;
output::change_directory(&path)?;
output::execute("git pull")?;
```

### Available Output Functions

The output module (`src/output/global.rs`) provides these functions:

- `success(message)` - Emit success messages (✅, both modes)
- `progress(message)` - Emit progress updates (🔄, both modes)
- `info(message)` - Emit info messages (⚪, both modes)
- `warning(message)` - Emit warning messages (🟡, both modes)
- `hint(message)` - Emit hint messages (💡, interactive only, suppressed in directive)
- `change_directory(path)` - Request directory change (directive) or store for execution (interactive)
- `execute(command)` - Execute command (interactive) or emit directive (directive mode)
- `flush()` - Flush output buffers

**When to use each function:**
- `success()` - Successful completion (e.g., "✅ Committed changes")
- `progress()` - Operations in progress (e.g., "🔄 Squashing commits...")
- `info()` - Neutral status/metadata (e.g., "⚪ No changes detected")
- `warning()` - Non-blocking issues (e.g., "🟡 Uncommitted changes detected")
- `hint()` - Actionable suggestions for users (e.g., "💡 Run 'wt config help'")

For the complete API, see `src/output/global.rs`.

### Adding New Output Functions

When adding new output capabilities:

1. Add the function to both `InteractiveOutput` and `DirectiveOutput` handlers
2. Add dispatch in `global.rs` - route to both handlers via enum match
3. Never add mode parameters - the handlers already know their mode

**Example pattern:**
```rust
// In interactive.rs
pub fn warning(&mut self, message: String) -> io::Result<()> {
    println!("{WARNING_EMOJI} {WARNING}{message}{WARNING:#}");
    Ok(())
}

// In global.rs - dispatch to both handlers
pub fn warning(message: impl Into<String>) -> io::Result<()> {
    OUTPUT_CONTEXT.with(|ctx| {
        let msg = message.into();
        match &mut *ctx.borrow_mut() {
            OutputHandler::Interactive(i) => i.warning(msg),
            OutputHandler::Directive(d) => d.warning(msg),
        }
    })
}
```

### Why This Matters

This maintains "one canonical path": commands have ONE code path that works for both modes (LOW CARDINALITY), not mode conditionals scattered throughout 50+ functions (HIGH CARDINALITY). Mode-specific behavior is encapsulated in two handler files. Same pattern as logging frameworks (`log`, `tracing`).

**Red Flag:** "I need to check if we're in interactive mode" - This is always wrong. Either the behavior should be the same (just do it), or different (add a new output function). Never check the mode in commands.

### Architectural Constraint: --internal Commands Must Use Output System

**CRITICAL: Commands that support `--internal` mode must NEVER use direct print macros (`print!()`, `println!()`, `eprint!()`, `eprintln!()`).**

Using direct prints bypasses the output system and causes directive leaks - directives become visible to users.

**Restricted files** (enforced by `tests/output_system_guard.rs`):
- `src/commands/worktree.rs` (switch, remove)
- `src/commands/merge.rs`

**Always use output system:**
```rust
// ❌ NEVER in --internal commands
println!("🔄 Starting operation...");

// ✅ ALWAYS
crate::output::progress("🔄 Starting operation...")?;
```

**Enforcement:** The `check_output_system_usage` test (`cargo test check_output_system_usage`) verifies compliance. Integration tests catch directive leaks via `tests/integration_tests/shell_wrapper.rs`.

## Command Execution Principles

### Real-time Output Streaming

**CRITICAL: Command output must stream through in real-time. Never buffer external command output.**

When executing external commands (git, npm, user scripts, etc.):

- **Stream immediately**: Output from child processes must appear as it's generated
- **No buffering**: Do NOT collect output into buffers before displaying
- **Real-time feedback**: Users must see progress as it happens, not after completion

**Why this matters:**
- Long-running commands (npm install, cargo build) provide progress indicators
- Users need to see what's happening in real-time for debugging
- Buffering breaks interactive commands and progress bars
- Commands may run for minutes - buffering until completion is unacceptable

**Good - streaming output:**
```rust
// Read and write line-by-line as data arrives
for line in reader.lines() {
    println!("{}", line);
    stdout().flush();
}
```

**Bad - buffering output:**
```rust
// ❌ NEVER DO THIS - waits until command completes
let lines: Vec<_> = reader.lines().collect();
for line in lines {
    println!("{}", line);
}
```

**Implementation constraint:**
Any solution to output ordering problems must maintain real-time streaming. If you need deterministic ordering, solve it at the pipe level (e.g., redirect stderr to stdout in the shell), not by buffering in Rust.

## Testing Guidelines

### Testing with --execute Commands

When testing commands that require confirmation (e.g., `wt switch --execute "..."`), use the `--force` flag to skip the interactive prompt:

```bash
# Good - skips confirmation prompt for testing
wt switch --create feature --execute "echo test" --force

# Bad - DO NOT pipe 'yes' to stdin, this crashes Claude
echo yes | wt switch --create feature --execute "echo test"
```

**Why `--force`?**
- Non-interactive testing requires automated approval
- Piping input to stdin interferes with Claude's I/O handling
- `--force` provides explicit, testable behavior

## Benchmarks

### Running Benchmarks Selectively

Some benchmarks are expensive (clone large repos, run for extended periods). Use Criterion's selective execution to control which benchmarks run:

```bash
# Run all benchmarks (includes expensive ones)
cargo bench

# Run only fast benchmarks by name (exclude expensive ones)
cargo bench --bench list bench_list_by_worktree_count
cargo bench --bench list bench_list_by_repo_profile
cargo bench --bench list bench_sequential_vs_parallel
cargo bench --bench completion

# Run a specific benchmark suite
cargo bench --bench completion
```

**Expensive benchmarks:**
- `bench_list_real_repo` - Clones rust-lang/rust repo (~2-5 min first run, cached in `target/bench-repos/`)

**Default workflow:** Skip expensive benchmarks during normal development. Run them explicitly when benchmarking performance on realistic repos.
