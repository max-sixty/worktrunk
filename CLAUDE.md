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

This confirms the program understood user intent and used their input correctly.

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

**Core Principle: Every user-facing message must have EITHER an emoji OR a gutter** for consistent visual separation.

```rust
// ✅ GOOD - standalone message with emoji
println!("{SUCCESS_EMOJI} {GREEN}Created worktree{GREEN:#}");

// ✅ GOOD - quoted content with gutter
print!("{}", format_with_gutter(&command));

// ✅ GOOD - section header with emoji, followed by gutter content
println!("{INFO_EMOJI} Global Config: {bold}{}{bold:#}", path.display());

// ❌ BAD - standalone message without emoji or gutter
println!("{dim}Operation declined{dim:#}");
```

### stdout vs stderr: Separation by Mode

**Core Principle: Different separation in interactive vs directive mode.**

**Interactive mode:**
- **stdout**: All worktrunk output (messages, errors, warnings, progress)
- **stderr**: Child process output (git, npm, user commands) + interactive prompts

**Directive mode (--internal flag for shell integration):**
- **stdout**: Only directives (`__WORKTRUNK_CD__`, `__WORKTRUNK_EXEC__`) - NUL-terminated
- **stderr**: All user-facing messages + child process output - streams in real-time

Use the output system (`output::success()`, `output::progress()`, etc.) to handle both modes automatically. Never write directly to stdout/stderr in command code.

```rust
// ✅ GOOD - use output system (handles both modes)
output::success("Branch created")?;
output::change_directory(&path)?;

// ❌ BAD - direct writes bypass output system
println!("Branch created");
writeln!(io::stderr(), "Progress...")?;
```

**Interactive prompts:** Flush stderr before blocking on stdin to prevent interleaving:
```rust
eprint!("💡 Allow and remember? [y/N] ");
stderr().flush()?;  // Ensures prompt is visible before blocking
io::stdin().read_line(&mut response)?;
```

**Child processes:** Redirect stdout to stderr for deterministic ordering:
```rust
let wrapped = format!("{{ {}; }} 1>&2", command);
Command::new("sh").arg("-c").arg(&wrapped).status()?;
```

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
- Collecting messages in a buffer
- Single success message for batch operations
- No progress before slow operations
- Progress without matching success

### Information Display: Show Once, Not Twice

**Core Principle: Show detailed context in progress messages, minimal confirmation in success messages.**

When operations have both progress and success messages:
- **Progress message**: Include ALL relevant details - what's being done, counts, stats, context
- **Success message**: MINIMAL - just confirm completion with reference info (hash, path)

```rust
// ✅ GOOD - detailed progress, minimal success
output::progress("🔄 Squashing 3 commits with working tree changes into 1 (5 files, +120, -45)...")?;
perform_squash()?;
output::success("✅ Squashed @ a1b2c3d")?;

// ❌ BAD - repeating detail in success message
output::progress("🔄 Squashing 3 commits into 1...")?;
perform_squash()?;
output::success("✅ Squashed 3 commits into 1 @ a1b2c3d")?;  // Redundant
```

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

**Avoid nested style resets** - Compose all attributes into a single style object:

```rust
// ❌ BAD - nested reset leaks color
"{WARNING}Text with {bold}nested{bold:#} styles{WARNING:#}"

// ✅ GOOD - compose styles together
let warning_bold = WARNING.bold();
"{WARNING}Text with {warning_bold}composed{warning_bold:#} styles{WARNING:#}"
```

**Reset all styles** with `anstyle::Reset`, not `{:#}` on empty `Style`:

```rust
// ❌ BAD - produces empty string
output.push_str(&format!("{:#}", Style::new()));

// ✅ GOOD - produces \x1b[0m reset code
output.push_str(&format!("{}", anstyle::Reset));
```

### Information Hierarchy & Styling

**Principle: Bold what answers the user's question, dim what provides context.**

Styled elements must maintain their surrounding color - compose the color with the style using `.bold()` or `.dimmed()`. Applying a style without color creates a leak.

```rust
// ❌ WRONG - styled element loses surrounding color
let bold = AnstyleStyle::new().bold();
println!("✅ {GREEN}Message {bold}{path}{bold:#}{GREEN:#}");  // Path will be black/white!

// ✅ RIGHT - compose color with style
let green_bold = GREEN.bold();
println!("✅ {GREEN}Created worktree, changed directory to {green_bold}{}{green_bold:#}");

// Re-establish outer color after styled elements mid-message
let green_bold = GREEN.bold();
println!("✅ {GREEN}Already on {green_bold}{branch}{green_bold:#}{GREEN}, nothing to merge{GREEN:#}");
//                                                      ^^^^^^^ Re-establish GREEN
```

### Indentation Policy

No manual indentation - styling provides hierarchy. For quoted content, use `format_with_gutter()`.

### Color Detection

Colors automatically adjust based on environment (NO_COLOR, CLICOLOR_FORCE, TTY detection) via `anstream` macros.

**Always use styled print macros** - Import from `worktrunk::styling`, not stdlib:

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

Use `format_with_gutter()` for quoted content. Gutter content must be raw external output without our styling additions (emojis, colors).

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


### Snapshot Testing Requirement

Every command output must have a snapshot test (`tests/integration_tests/`).

**Pattern:**
```rust
use crate::common::{make_snapshot_cmd, setup_snapshot_settings, TestRepo};
use insta_cmd::assert_cmd_snapshot;

fn snapshot_command(test_name: &str, repo: &TestRepo, args: &[&str]) {
    let settings = setup_snapshot_settings(repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd(repo, "beta", &[], None);
        cmd.arg("command-name").args(args);
        assert_cmd_snapshot!(test_name, cmd);
    });
}

#[test]
fn test_command_success() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");
    snapshot_command("command_success", &repo, &[]);
}

#[test]
fn test_command_no_data() {
    let repo = TestRepo::new();
    snapshot_command("command_no_data", &repo, &[]);
}
```

Cover success/error states, with/without data, and flag variations.

## Output System Architecture

### Two Output Modes

Worktrunk supports two output modes, selected once at program startup:

1. **Interactive Mode** - Human-friendly output with colors, emojis, and hints
2. **Directive Mode** - Machine-readable NUL-terminated directives for shell integration

The mode is determined at initialization in `main()` and never changes during execution.

### The Cardinal Rule: Never Check Mode in Command Code

Command code must never check which output mode is active. The output system uses enum dispatch - commands call output functions without knowing the mode.

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

Decide once at the edge (`main()`), initialize globally, trust internally.

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
- `hint()` - Actionable suggestions for users (e.g., "💡 Run 'wt config --help'")

For the complete API, see `src/output/global.rs`.

### Adding New Output Functions

Add the function to both handlers, add dispatch in `global.rs`, never add mode parameters.

This maintains one canonical path: commands have ONE code path that works for both modes. Never check the mode in commands.

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

## Testing Guidelines

### Testing with --execute Commands

Use `--force` to skip interactive prompts in tests. Don't pipe input to stdin.

## Benchmarks

### Running Benchmarks Selectively

Run specific benchmarks by name to skip expensive ones:
```bash
cargo bench --bench list bench_list_by_worktree_count
cargo bench --bench completion
```

`bench_list_real_repo` clones rust-lang/rust (~2-5 min first run). Skip during normal development.

## JSON Output Format

Use `wt list --format=json --full` for structured data access. Objects have:

**Core fields:**
- `type`: "worktree" | "not_worktree"
- `worktree.path`, `worktree.branch`, `worktree.head`
- `timestamp`, `commit_message`, `is_primary`

**Diff stats:** `ahead`/`behind`, `working_tree_diff`, `branch_diff`, `working_tree_diff_with_main` (each: `{added, deleted}`)

**Remote tracking:** `upstream_remote`, `upstream_ahead`, `upstream_behind`

**State:** `has_conflicts`, `worktree_state` (null | "bisect" | "rebase" | etc.), `pr_status`, `ci_status`, `is_stale`

Query: `jq '.[] | select(.worktree.branch == "main") | {path: .worktree.path, ahead, behind}'`
