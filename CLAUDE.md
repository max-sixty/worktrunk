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

Acceptable breaking changes: config locations, command/flag names, output formats, dependencies, codebase structure.

When the project reaches v1.0 or gains users, we'll adopt stability commitments. Until then, we're free to iterate rapidly.

## Code Quality

Claude commonly makes the mistake of adding `#[allow(dead_code)]` when writing code that isn't immediately used. Don't suppress the warning—either delete the code or add a TODO comment explaining when it will be used.

Example of escalating instead of suppressing:

```rust
// TODO(feature-name): Used by upcoming config validation
fn parse_config() { ... }
```

## Testing

### Running Tests

```bash
# Unit tests (fast, ~200 tests)
cargo test --lib --bins

# Integration tests (~360 tests, requires bash/zsh/fish shells)
cargo test --test integration

# Run all tests via pre-merge hook (recommended before committing)
cargo run -- beta run-hook pre-merge
```

The pre-merge hook runs the full test suite and is the recommended way to verify changes before committing.

### Claude Code Web Environment

When working in Claude Code web, run the setup script first:

```bash
./scripts/setup-claude-code-web.sh
```

This installs required shells (zsh, fish) and builds the project. The permission tests (`test_permission_error_prevents_save`, `test_approval_prompt_permission_error`) automatically skip when running as root, which is common in containerized environments.

### CLI Flag Descriptions

Keep the first line of flag and argument descriptions brief—aim for 3-6 words. Use parenthetical defaults sparingly, only when the default isn't obvious from context.

**Good examples:**
- `/// Skip approval prompts`
- `/// Show CI, conflicts, and full diffs`
- `/// Target branch (defaults to default branch)`
- `/// Branch, path, '@' (HEAD), '-' (previous), or '^' (main)`

**Bad examples (too verbose):**
- `/// Auto-approve project commands without saving approvals.`
- `/// Run as if worktrunk was started in <path> instead of the current working directory`
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

**Avoid redundant parenthesized content:** Parenthesized text should add new information, not restate what's already said.

```rust
// ❌ BAD - parentheses restate "no changes"
"No changes after squashing 3 commits (commits resulted in no net changes)"
// ✅ GOOD - clear and concise
"No changes after squashing 3 commits"
// ✅ GOOD - parentheses add supplementary info
"Committing with default message... (3 files, +45, -12)"
```

### Message Consistency Patterns

Use consistent punctuation and structure for related messages:

**Semicolon for qualifiers:** Separate the action from a qualifier/reason:
```rust
// Action; qualifier (flag)
"Removing feature in background; retaining branch (--no-delete-branch)"
"Commands approved; not saved (--force)"
```

**Ampersand for conjunctions:** Use `&` for combined actions:
```rust
// Action & additional action
"Removing feature & branch in background"
"Commands approved & saved to config"
```

**Explicit flag acknowledgment:** Show flags in parentheses when they change behavior:
```rust
// ✅ GOOD - shows the flag explicitly
"Removing feature in background; retaining branch (--no-delete-branch)"
// ❌ BAD - doesn't acknowledge user's explicit choice
"Removing feature in background; retaining branch"
```

**Parallel structure:** Related messages should follow the same pattern:
```rust
// ✅ GOOD - parallel structure distinguishes user choice from system decision
"Removing feature & branch in background"                                // Merged (will delete)
"Removing feature in background; retaining unmerged branch"              // Unmerged (system keeps)
"Removing feature in background; retaining branch (--no-delete-branch)"  // User flag (user keeps)
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
   - **NOT dimmed**: Primary status messages
   - **Dimmed**: Supplementary metadata

**Every user-facing message requires either an emoji or a gutter** for consistent visual separation.

### stdout vs stderr: Separation by Mode

**Interactive mode:**
- stdout: All worktrunk output (messages, errors, warnings, progress)
- stderr: Child process output (git, npm, user commands) + interactive prompts

**Directive mode** (--internal flag for shell integration):
- stdout: Only directives (`__WORKTRUNK_CD__`, `__WORKTRUNK_EXEC__`) - NUL-terminated
- stderr: All user-facing messages + child process output - streams in real-time

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
    output::progress(format!("🔄 Removing {item}..."))?;
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
output::progress("🔄 Squashing 3 commits with working tree changes into 1 (5 files, +120, -45)...")?;
perform_squash()?;
output::success("✅ Squashed @ a1b2c3d")?;
```

### Semantic Style Constants

Style constants defined in `src/styling.rs`:
- `ERROR`: Red (errors, conflicts)
- `WARNING`: Yellow (warnings)
- `HINT`: Dimmed (hints, secondary information)
- `CURRENT`: Magenta + bold (current worktree)
- `ADDITION`: Green (diffs, additions)
- `DELETION`: Red (diffs, deletions)

Emoji constants: `ERROR_EMOJI` (❌), `WARNING_EMOJI` (🟡), `HINT_EMOJI` (💡), `INFO_EMOJI` (⚪)

### Inline Formatting Pattern

Use anstyle's inline pattern `{style}text{style:#}` where `#` means reset:

```rust
use worktrunk::styling::{println, ERROR, ERROR_EMOJI, WARNING_EMOJI, HINT_EMOJI, AnstyleStyle};
use anstyle::{AnsiColor, Color};

let cyan = AnstyleStyle::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
println!("🔄 {cyan}Rebasing onto main...{cyan:#}");

println!("{ERROR_EMOJI} {ERROR}Working tree has uncommitted changes{ERROR:#}");
println!("{HINT_EMOJI} {HINT}Use 'wt list' to see all worktrees{HINT:#}");
```

### Composing Styles

Compose styles using anstyle methods (`.bold()`, `.fg_color()`, etc.). Branch names in messages (not tables) should be bolded. Tables (`wt list`) use conditional styling for branch names to indicate worktree state (current/primary/other).

Nested style resets leak color. Compose all attributes into a single style object:

```rust
// ❌ BAD - nested reset leaks color
"{WARNING}Text with {bold}nested{bold:#} styles{WARNING:#}"
// ✅ GOOD - compose styles together
let warning_bold = WARNING.bold();
"{WARNING}Text with {warning_bold}composed{warning_bold:#} styles{WARNING:#}"
```

Styled elements must maintain their surrounding color. Compose the color with the style to avoid leaking:

```rust
// ❌ WRONG - styled element loses surrounding color
let bold = AnstyleStyle::new().bold();
println!("✅ {GREEN}Message {bold}{path}{bold:#}{GREEN:#}");  // Path will be black/white!
// ✅ RIGHT - compose color with style
let green_bold = GREEN.bold();
println!("✅ {GREEN}Created worktree at {green_bold}{path}{green_bold:#}{GREEN:#}");
```

### Color Detection

Colors automatically adjust based on environment (NO_COLOR, CLICOLOR_FORCE, TTY detection) via `anstream` macros.

Styled print macros must be imported from `worktrunk::styling`, not stdlib:

```rust
// ❌ BAD - uses standard library macro, bypasses anstream
eprintln!("{}", styled_text);
// ✅ GOOD - import and use anstream-wrapped version
use worktrunk::styling::eprintln;
eprintln!("{}", styled_text);
```

### Design Principles

- **Use the ecosystem, not manual escape codes** - Use `anstyle` for colors, `osc8` for hyperlinks, `strip-ansi-escapes` for stripping. Never manually write ANSI codes (`\x1b[...`)
- **Inline over wrappers** - Use `{style}text{style:#}` pattern, not wrapper functions
- **Composition over special cases** - Use `.bold()`, `.fg_color()`, not `format_X_with_Y()`
- **Semantic constants** - Use `ERROR`, `WARNING`, not raw colors
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

Use `format_with_gutter()` for quoted content. Gutter content must be raw external output without our styling additions (emojis, colors).

```rust
// ✅ GOOD - raw git output in gutter
let raw_error = match &error {
    GitError::CommandFailed(msg) => msg.as_str(),
    _ => &error.to_string(),
};
super::gutter(format_with_gutter(raw_error, "", None))?;

// ❌ BAD - includes our formatting (❌ emoji)
super::gutter(format_with_gutter(&error.to_string(), "", None))?;
```

**Linebreaks:** Gutter content requires a single newline before it, never double newlines. Output functions (`progress()`, `success()`, etc.) use `println!()` internally, adding a trailing newline. Messages passed to these functions should not include `\n`:

```rust
// ✅ GOOD - no trailing \n
output::progress(format!("{CYAN}Merging...{CYAN:#}"))?;
output::gutter(format_with_gutter(&log, "", None))?;

// ❌ BAD - trailing \n creates blank line
output::progress(format!("{CYAN}Merging...{CYAN:#}\n"))?;
```

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

Every command output must have a snapshot test (`tests/integration_tests/`). Use this pattern:

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
- `hint(message)` - Actionable suggestions (💡, interactive only, suppressed in directive)
- `change_directory(path)` - Request directory change (directive) or store for execution (interactive)
- `execute(command)` - Execute command (interactive) or emit directive (directive mode)
- `flush()` - Flush output buffers

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

Use `wt list --format=json` for structured data access. The output is an array of objects with `type: "worktree" | "branch"`.

### Common Fields (all objects)

- `type`: "worktree" | "branch"
- `head_sha`, `timestamp`, `commit_message`: commit info
- `ahead`, `behind`: commits ahead/behind main branch
- `branch_diff`: `{added, deleted}` - line diff vs main
- `upstream_remote`, `upstream_ahead`, `upstream_behind`: remote tracking status
- `pr_status`: PR/CI status object (null if not available)
- `status_symbols`: structured status object with two maps:
  - `status`: variant names for queryability (e.g., "MatchesMain", "Ahead")
  - `status_symbols`: display symbols (e.g., "≡", "↑")

### Worktree-Specific Fields

- `path`: absolute path to worktree
- `branch`: branch name (null if detached)
- `bare`, `detached`: boolean flags
- `locked`, `prunable`: reason strings (null if not applicable)
- `working_tree_diff`: `{added, deleted}` - uncommitted changes
- `working_tree_diff_with_main`: `{added, deleted}` or null (null = not computed, `{0,0}` = matches main exactly)
- `worktree_state`: "rebase" | "merge" | null - git operation in progress
- `is_primary`: boolean - is main worktree

### Branch-Specific Fields

- `name`: branch name

### JSON Field Documentation

**For complete field documentation and additional examples, see:**
- `wt list --help` - authoritative source of truth
- README.md - copy of help text for quick reference

### Display Fields (json-pretty format only)

ANSI-formatted strings for human-readable output:
- `commits_display`, `branch_diff_display` (branches only)
- `upstream_display`, `ci_status_display` (optional)
- `status_display`: rendered status symbols + user status
- `working_diff_display` (worktrees only, optional)

## Worktree Model

- Worktrees are **addressed by branch name**, not by filesystem path.
- Each worktree should map to **exactly one branch**.
- We **never retarget an existing worktree** to a different branch; instead create/switch/remove worktrees.
