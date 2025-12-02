# Worktrunk Development Guidelines

> **Note**: This CLAUDE.md is just getting started. More guidelines will be added as patterns emerge.

## Project Status

**This project was released very recently and has very few backward compatibility concerns.**

We are in **early release** mode:
- Breaking changes are generally acceptable
- Optimize for the best solution, not compatibility with previous versions
- No Rust library compatibility concerns (this is a CLI tool only)

**CLI deprecation policy:** When renaming or removing CLI options, retain the old option with a deprecation warning if there's no code complication cost. If supporting both old and new would add complexity, just make the breaking change.

When making decisions, prioritize:
1. **Best technical solution** over backward compatibility
2. **Clean design** over maintaining old patterns
3. **Modern conventions** over legacy approaches

Acceptable breaking changes: config locations, output formats, dependencies, codebase structure. CLI flag changes can be breaking if deprecation warnings would complicate the code.

## Code Quality

Claude commonly makes the mistake of adding `#[allow(dead_code)]` when writing code that isn't immediately used. Don't suppress the warning—either delete the code or add a TODO comment explaining when it will be used.

Example of escalating instead of suppressing:

```rust
// TODO(feature-name): Used by upcoming config validation
fn parse_config() { ... }
```

## Terminology

Use consistent terminology in documentation, help text, and code comments:

- **main worktree** — when referring to the primary worktree (the original git directory)
- **default branch** — when referring to the branch (main, master, etc.)

Avoid mixing: "main/default branch worktree" is confusing. Use "main worktree" for worktrees and "default branch" for branches.

## Testing

### Running Tests

```console
# Unit tests (fast, ~210 tests)
cargo test --lib --bins

# Integration tests without shell tests (~370 tests, no external dependencies)
cargo test --test integration

# Integration tests WITH shell tests (~420 tests, requires bash/zsh/fish)
cargo test --test integration --features shell-integration-tests

# Run all tests via pre-merge hook (recommended before committing)
cargo run -- step pre-merge --force
```

The pre-merge hook runs the full test suite and is the recommended way to verify changes before committing.

**Shell integration tests** require bash, zsh, and fish. On Linux, run `./dev/setup-claude-code-web.sh` to install them.

### Claude Code Web Environment

When working in Claude Code web, run the setup script first:

```console
./dev/setup-claude-code-web.sh
```

This installs required shells (zsh, fish) and builds the project. The permission tests (`test_permission_error_prevents_save`, `test_approval_prompt_permission_error`) automatically skip when running as root, which is common in containerized environments.

### CLI Flag Descriptions

Keep the first line of flag and argument descriptions brief—aim for 3-6 words. Use parenthetical defaults sparingly, only when the default isn't obvious from context.

**Good examples:**
- `/// Skip approval prompts`
- `/// Show CI, conflicts, and full diffs`
- `/// Target branch (defaults to default branch)`

**Bad examples (too verbose):**
- `/// Auto-approve project commands without saving approvals.`
- `/// Show CI status, conflict detection, and complete diff statistics`

The help text should be scannable. Users reading `wt switch --help` need to quickly understand what each flag does without parsing long sentences.

## CLI Output Formatting Standards

### User Message Principles

Output messages should acknowledge user-supplied arguments (flags, options, values) by reflecting those choices in the message text.

```rust
// User runs: wt switch --create feature --base=main
// ✅ GOOD - acknowledges the base branch
"Created new worktree for feature from main at /path/to/worktree"
// ❌ BAD - ignores the base argument
"Created new worktree for feature at /path/to/worktree"
```

**Avoid second-person ("you", "your"):** Messages should describe actions and state, not address the user directly.

```rust
// ❌ BAD - second-person
"Use 'wt merge' to rebase your changes onto main"
// ✅ GOOD - describes the action
"Use 'wt merge' to rebase onto main"

// ❌ BAD - second-person
"Add one line to your shell config"
// ✅ GOOD - refers to the thing
"Add one line to the shell config"
```

**Avoid redundant parenthesized content:** Parenthesized text should add new information, not restate what's already said.

```rust
// ❌ BAD - parentheses restate "no changes"
"No changes after squashing 3 commits (commits resulted in no net changes)"
// ✅ GOOD - clear and concise
"No changes after squashing 3 commits"
// ✅ GOOD - parentheses add supplementary info
"Committing with default message... (3 files, +45, -12)"
```

**Avoid pronouns with cross-message referents:** Hints appear as separate messages from errors. Don't use pronouns like "it" that refer to something mentioned in the error message.

```rust
// ❌ BAD - "it" refers to branch name in error message
// Error: "Branch 'feature' not found"
// Hint:  "Use --create to create it"
// ✅ GOOD - self-contained hint
// Error: "Branch 'feature' not found"
// Hint:  "Use --create to create a new branch"
```

### Message Consistency Patterns

Use consistent punctuation and structure for related messages:

**Semicolon for qualifiers:** Separate the action from a qualifier/reason:
```rust
// Action; qualifier (flag)
"Removing feature worktree in background; retaining branch (--no-delete-branch)"
"Commands approved; not saved (--force)"
```

**Ampersand for conjunctions:** Use `&` for combined actions:
```rust
// Action & additional action
"Removing feature worktree & branch in background"
"Commands approved & saved to config"
```

**Explicit flag acknowledgment:** Show flags in parentheses when they change behavior:
```rust
// ✅ GOOD - shows the flag explicitly
"Removing feature worktree in background; retaining branch (--no-delete-branch)"
// ❌ BAD - doesn't acknowledge user's explicit choice
"Removing feature worktree in background; retaining branch"
```

**Parallel structure:** Related messages should follow the same pattern:
```rust
// ✅ GOOD - parallel structure with integration reason explaining branch deletion
// Both wt merge and wt remove show integration reason when branch is deleted
"Removing feature worktree & branch in background (already in main)"       // Branch is integrated (ancestor)
"Removing feature worktree & branch in background (files match main)"      // Branch is integrated (squash/rebase)
"Removing feature worktree & branch in background (all changes in main)"   // Branch is integrated (squash + main advanced)
"Removing feature worktree in background; retaining unmerged branch"        // Unmerged (system keeps)
"Removing feature worktree in background; retaining branch (--no-delete-branch)"  // User flag (user keeps)
```

**Compute decisions once:** For background operations, check conditions upfront, show the message, then pass the decision explicitly rather than re-checking in background scripts:
```rust
// ✅ GOOD - check once, pass decision
let should_delete = check_if_merged();
show_message_based_on(should_delete);
spawn_background(build_command(should_delete));

// ❌ BAD - check twice (once for message, again in background script)
let is_merged = check_if_merged();
show_message_based_on(is_merged);
spawn_background(build_command_that_checks_merge_again());  // Duplicate check!
```

### Message Types

Six canonical message patterns with their emojis:

1. **Progress**: 🔄 (operations in progress)
2. **Success**: ✅ (successful completion)
3. **Errors**: ❌ (failures, invalid states)
4. **Warnings**: 🟡 (non-blocking issues)
5. **Hints**: 💡 (actionable suggestions, tips for user)
6. **Info**: ⚪ (neutral status, system feedback, metadata)

**Output functions automatically add emoji AND semantic color.** Callers provide content with optional inner styling (like `<bold>`):

```rust
// Simple message - function adds emoji + color
output::success("Created worktree")?;
output::hint("Run 'wt config' to configure")?;

// With inner styling - use cformat! for bold/dim within the message
output::success(cformat!("Created worktree for <bold>{branch}</>"))?;
output::warning(cformat!("Branch <bold>{name}</> not found"))?;
```

**Semantic colors added automatically:**
- `success()` → green
- `progress()` → cyan
- `hint()` → dimmed
- `warning()` → yellow
- `info()` → no color (neutral status)

**Every user-facing message requires either an emoji or a gutter** for consistent visual separation.

### Blank Line Principles

- **No leading/trailing blanks** - Start immediately, end cleanly
- **One blank after blocks** - Separate multi-line content (gutter blocks, sections)
- **One blank after prompts** - Separate user input from results
- **Never double blanks** - One blank line maximum between elements

### stdout vs stderr

**Both modes write all messages to stderr.** stdout is reserved for structured data:

- **stdout**: JSON output (`--format=json`), shell scripts (directive mode)
- **stderr**: All user-facing messages (progress, success, errors, hints, gutter, etc.)

**Directive mode** additionally emits a shell script to stdout at the end.

Use the output system (`output::success()`, `output::progress()`, etc.) to handle both modes automatically. Never write directly to stdout/stderr in command code.

```rust
// ✅ GOOD - use output system (handles both modes)
output::success("Branch created")?;

// ❌ BAD - direct writes bypass output system
println!("Branch created");
```

Interactive prompts must flush stderr before blocking on stdin:
```rust
eprint!("💡 Allow and remember? [y/N] ");
stderr().flush()?;
io::stdin().read_line(&mut response)?;
```

### Temporal Locality: Output Should Be Close to Operations

Output should appear immediately adjacent to the operations it describes. Progress messages apply only to slow operations (>400ms): git operations, network requests, builds.

Sequential operations should show immediate feedback:
```rust
for item in items {
    output::progress(format!("Removing {item}..."))?;
    perform_operation(item)?;
    output::success(format!("Removed {item}"))?;  // Immediate feedback
}
```

Bad example (output decoupled from operations):
```
🔄 Removing worktree for feature...
🔄 Removing worktree for bugfix...
                                    ← Long delay, no feedback
Removed worktree for feature        ← All output at the end
Removed worktree for bugfix
```

Signs of poor temporal locality: collecting messages in a buffer, single success message for batch operations, no progress before slow operations.

### Information Display: Show Once, Not Twice

Progress messages should include all relevant details (what's being done, counts, stats, context). Success messages should be minimal, confirming completion with reference info (hash, path).

```rust
// ✅ GOOD - detailed progress, minimal success
output::progress("Squashing 3 commits & working tree changes into a single commit (5 files, +60)...")?;
perform_squash()?;
output::success("Squashed @ a1b2c3d")?;
```

### Style Constants

Style constants in `src/styling/constants.rs` (minimal set for programmatic styling):
- `ADDITION`: Green (diffs, additions) - used in table rendering
- `DELETION`: Red (diffs, deletions) - used in table rendering
- `GUTTER`: BrightWhite background (quoted content)

Emoji constants: `PROGRESS_EMOJI` (🔄), `SUCCESS_EMOJI` (✅), `ERROR_EMOJI` (❌), `WARNING_EMOJI` (🟡), `HINT_EMOJI` (💡), `INFO_EMOJI` (⚪)

For all other styling, use color-print tags in `cformat!`: `<red>`, `<green>`, `<yellow>`, `<cyan>`, `<dim>`, `<bold>`, `<bright-black>`

### Styling in Command Code

Use `output::` functions with `cformat!` for styled content. The output function adds the emoji + semantic color, and `cformat!` handles inner styling:

```rust
// ✅ GOOD - output:: handles emoji + outer color, cformat! handles inner styling
output::success(cformat!("Created <bold>{branch}</> from <bold>{base}</>"))?;
output::warning(cformat!("Branch <bold>{name}</> has <dim>uncommitted changes</>"))?;
output::hint(cformat!("Run <bright-black>wt merge</> to continue"))?;

// ✅ GOOD - plain strings work too (no inner styling needed)
output::progress("Rebasing onto main...")?;
output::hint("No changes to commit")?;
```

**Available color-print tags:** `<bold>`, `<dim>`, `<bright-black>`, `<red>`, `<green>`, `<yellow>`, `<cyan>`, `<magenta>`

**Emoji constants in cformat!:** Use `{ERROR_EMOJI}`, `{HINT_EMOJI}`, etc. for messages that bypass output:: functions (e.g., GitError Display impl):

```rust
cformat!("{ERROR_EMOJI} <red>Branch <bold>{branch}</> not found</>")
```

Branch names in messages should be bolded. Tables (`wt list`) use `StyledLine` with conditional styling for branch names.

### Commands and Branches in Messages

Never quote commands or branch names. Use styling to make them stand out:

- **In normal font context**: Use `<bold>` for commands and branches
- **In hints**: Use `<bright-black>` for commands (hint() already applies dimming—no explicit `<dim>` needed)

```rust
// ✅ GOOD - bold in normal context
output::info(cformat!("Use <bold>wt merge</> to continue"))?;

// ✅ GOOD - bright-black for commands in hints
output::hint(cformat!("Run <bright-black>wt list</> to see worktrees"))?;

// ✅ GOOD - plain hint without commands
output::hint("No changes to commit")?;

// ❌ BAD - quoted commands
output::hint("Run 'wt list' to see worktrees")?;
```

### Color Detection

Colors automatically adjust based on environment (NO_COLOR, CLICOLOR_FORCE, TTY detection). When using `output::` functions, this is handled automatically.

For direct terminal I/O (rare - mainly internal output system code), import print macros from `worktrunk::styling`:

```rust
use worktrunk::styling::eprintln;  // Auto-detects color support
```

### Design Principles

- **color-print for all styling** - Use `cformat!` with HTML-like tags (`<green>`, `<bold>`, etc.). Only use anstyle for `StyledLine` table rendering.
- **output:: functions over direct printing** - Use output:: for user messages, which auto-adds emoji + semantic color
- **cformat! for inner styling** - Use `<bold>`, `<dim>` tags within output:: calls
- **Never manual escape codes** - No `\x1b[...` in code
- **YAGNI for presentation** - Most output needs no styling
- **Unicode-aware** - Width calculations respect emoji and CJK characters (via `StyledLine`)
- **Graceful degradation** - Must work without color support

### StyledLine API

For complex table formatting with proper width calculations, use `StyledLine`:

```rust
use worktrunk::styling::StyledLine;
use anstyle::{AnsiColor, Color, Style};

let mut line = StyledLine::new();
line.push_styled("Branch", Style::new().dimmed());
line.push_raw("  ");
line.push_styled("main", Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan))));
println!("{}", line.render());
```

See `src/commands/list/render.rs` for advanced usage.

### Gutter Formatting for Quoted Content

Use `format_with_gutter()` for quoted content. Gutter content displays external output (git errors, command output) in a visually distinct block.

```rust
// Show warning message, then external error in gutter
output::warning(cformat!("Could not delete branch <bold>{branch_name}</>"))?;
output::gutter(format_with_gutter(&e.to_string(), "", None))?;
```

**Linebreaks:** Gutter content requires a single newline before it, never double newlines. Output functions (`progress()`, `success()`, etc.) use `println!()` internally, adding a trailing newline. Messages passed to these functions should not include `\n`:

```rust
// ✅ GOOD - no trailing \n
output::progress("Merging...")?;
output::gutter(format_with_gutter(&log, "", None))?;

// ❌ BAD - trailing \n creates blank line
output::progress("Merging...\n")?;
```

### Error Message Formatting

**Single-line errors with variables are fine:**
```rust
// ✅ GOOD - single-line with path variable
.map_err(|e| format!("Failed to read {}: {}", format_path_for_display(path), e))?

// ✅ GOOD - using .context() for simple errors
std::fs::read_to_string(&path).context("Failed to read config")?
```

**Multi-line external output needs gutter formatting:**

When external commands (git, npm, LLM tools, hooks) produce multi-line stderr, use gutter formatting:
1. **Show the command that was run** - Include the full command with arguments so users can debug
2. **Put multi-line output in a gutter** - Don't embed raw stderr inline in the message

Use the `format_error_block` helper in `src/git/error.rs` or follow its pattern:

```rust
// ✅ GOOD - command shown in header, multi-line error in gutter
❌ Commit generation command 'llm --model claude' failed
   ┃ Error: [Errno 8] nodename nor servname provided, or not known

// ❌ BAD - multi-line error embedded inline
❌ Commit generation command 'llm' failed: LLM command failed: Error: [Errno 8]...
```

**Implementation:** See `format_error_block()` in `src/git/error.rs` for the pattern, and `LlmCommandFailed` variant for an example.

### Table Column Alignment

**Principle: Headers and values align consistently within each column type.**

Column alignment follows standard tabular data conventions:

1. **Text columns** (Branch, Path, Message, Commit):
   - Headers: Left-aligned
   - Values: Left-aligned

2. **Diff/numeric columns** (HEAD±, main↕, main…±, Remote⇅):
   - Headers: Right-aligned
   - Values: Right-aligned

**Why:** Right-aligning numeric data allows visual scanning by magnitude (rightmost digits align vertically). Left-aligning text data prioritizes readability from the start. Matching header and value alignment within each column creates a consistent visual grid.

**Implementation:** Headers are positioned within their column width using the same alignment strategy as their values (render.rs).

### Snapshot Testing Requirement

Every command output must have a snapshot test (`tests/integration_tests/`). See `tests/integration_tests/remove.rs` for the standard pattern using `setup_snapshot_settings()`, `make_snapshot_cmd()`, and `assert_cmd_snapshot!()`.

Cover success/error states, with/without data, and flag variations.

## Output System Architecture

### Two Output Modes

Worktrunk supports two output modes, selected once at program startup:
1. **Interactive Mode** - Human-friendly output with colors, emojis, and hints
2. **Directive Mode** - Shell script on stdout (at end), user messages on stderr

Both modes write all messages to stderr. stdout is reserved for structured data (JSON, shell scripts).

The mode is determined at initialization in `main()` and never changes during execution.

### The Cardinal Rule: Never Check Mode in Command Code

Command code must never check which output mode is active. The output system uses enum dispatch - commands call output functions without knowing the mode.

```rust
// ❌ NEVER DO THIS
if mode == OutputMode::Interactive {
    println!("✅ Success!");
}

// ✅ ALWAYS DO THIS
output::success("Success!")?;
```

Decide once at the edge (`main()`), initialize globally, trust internally:

```rust
// In main.rs - the only place that knows about modes
output::initialize(if internal { OutputMode::Directive } else { OutputMode::Interactive });

// Everywhere else - just use the output functions
output::success("Created worktree")?;
output::change_directory(&path)?;
```

### Available Output Functions

The output module (`src/output/global.rs`) provides:

- `success(message)` - Successful completion (✅, both modes)
- `progress(message)` - Operations in progress (🔄, both modes)
- `info(message)` - Neutral status/metadata (⚪, both modes)
- `warning(message)` - Non-blocking issues (🟡, both modes)
- `hint(message)` - Actionable suggestions (💡, both modes)
- `print(message)` - Write message as-is (caller formats, both modes)
- `shell_integration_hint(message)` - Shell integration hints (💡, suppressed in directive)
- `gutter(content)` - Gutter-formatted content (use with `format_with_gutter()`)
- `blank()` - Blank line for visual separation
- `data(content)` - Structured data output without emoji (JSON, for piping)
- `table(content)` - Table/UI output to stderr
- `change_directory(path)` - Request directory change
- `execute(command)` - Execute command or buffer for shell script
- `flush()` - Flush output buffers
- `flush_for_stderr_prompt()` - Flush before interactive prompts
- `terminate_output()` - Emit shell script in directive mode (no-op in interactive)

For the complete API, see `src/output/global.rs`.

### Adding New Output Functions

Add the function to both handlers, add dispatch in `global.rs`, never add mode parameters. This maintains one canonical path: commands have ONE code path that works for both modes.

### Architectural Constraint: --internal Commands Must Use Output System

Commands supporting `--internal` must never use direct print macros - use output system functions to prevent directive leaks. Enforced by `tests/output_system_guard.rs`.

## Command Execution Principles

### Real-time Output Streaming

Command output must stream in real-time. Never buffer external command output.

```rust
// ✅ GOOD - streaming
for line in reader.lines() {
    println!("{}", line);
    stdout().flush();
}
// ❌ BAD - buffering
let lines: Vec<_> = reader.lines().collect();
```

## Background Operation Logs

### Unified Logging Location

All background operation logs are centralized in `.git/wt-logs/` (main worktree's git directory):

- **Post-start commands**: `{branch}-post-start-{command}.log`
- **Background removal**: `{branch}-remove.log`

Examples (where command names are from config):
- `feature-post-start-npm.log`
- `bugfix-remove.log`

### Log Behavior

- **Centralized**: All logs go to main worktree's `.git/wt-logs/`, shared across all worktrees
- **Overwrites**: Same operation on same branch overwrites previous log (prevents accumulation)
- **Not tracked**: Logs are in `.git/` directory, which git doesn't track
- **Manual cleanup**: Stale logs (from deleted branches) persist but are bounded by branch count

Users can clean up old logs manually or use a git hook. No automatic cleanup is provided.

## Testing Guidelines

### Timing Tests: Long Timeouts with Fast Polling

**Core principle:** Use long timeouts (5+ seconds) for reliability on slow CI, but poll frequently (10-50ms) so tests complete quickly when things work.

This achieves both goals:
- **No flaky failures** on slow machines - generous timeout accommodates worst-case
- **Fast tests** on normal machines - frequent polling means no unnecessary waiting

```rust
// ✅ GOOD: Long timeout, fast polling
let timeout = Duration::from_secs(5);
let poll_interval = Duration::from_millis(10);
let start = Instant::now();
while start.elapsed() < timeout {
    if condition_met() { break; }
    thread::sleep(poll_interval);
}

// ❌ BAD: Fixed sleep (always slow, might still fail)
thread::sleep(Duration::from_millis(500));
assert!(condition_met());

// ❌ BAD: Short timeout (flaky on slow CI)
let timeout = Duration::from_millis(100);
```

Use the helpers in `tests/common/mod.rs`:

```rust
use crate::common::{wait_for_file, wait_for_file_count};

// ✅ Poll for file existence with 5+ second timeout
wait_for_file(&log_file, Duration::from_secs(5));

// ✅ Poll for multiple files
wait_for_file_count(&log_dir, "log", 3, Duration::from_secs(5));
```

These use exponential backoff (10ms → 500ms cap) for fast initial checks that back off on slow CI.

**Exception - testing absence:** When verifying something did NOT happen, polling doesn't work. Use a fixed 500ms+ sleep:

```rust
thread::sleep(Duration::from_millis(500));
assert!(!marker_file.exists(), "Command should NOT have run");
```

### Testing with --execute Commands

Use `--force` to skip interactive prompts in tests. Don't pipe input to stdin.

## Benchmarks

For detailed benchmark documentation, see `benches/CLAUDE.md`.

### Quick Start

```console
# Run fast synthetic benchmarks (skip slow ones)
cargo bench --bench list -- --skip cold --skip real

# Run specific benchmark
cargo bench --bench list bench_list_by_worktree_count
cargo bench --bench completion
```

Real repo benchmarks clone rust-lang/rust (~2-5 min first run, cached thereafter). Skip during normal development with `--skip real`.

## JSON Output Format

Use `wt list --format=json` for structured data access. See `wt list --help` for complete field documentation, status variants, and query examples.

## Worktree Model

- Worktrees are **addressed by branch name**, not by filesystem path.
- Each worktree should map to **exactly one branch**.
- We **never retarget an existing worktree** to a different branch; instead create/switch/remove worktrees.
