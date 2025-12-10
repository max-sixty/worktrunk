# CLI Output Formatting Standards

## User Message Principles

Output messages should acknowledge user-supplied arguments (flags, options,
values) by reflecting those choices in the message text.

```rust
// User runs: wt switch --create feature --base=main
// GOOD - acknowledges the base branch
"Created new worktree for feature from main at /path/to/worktree"
// BAD - ignores the base argument
"Created new worktree for feature at /path/to/worktree"
```

**Avoid second-person ("you", "your"):** Messages should describe actions and
state, not address the user directly.

```rust
// BAD - second-person
"Use 'wt merge' to rebase your changes onto main"
// GOOD - describes the action
"Use 'wt merge' to rebase onto main"

// BAD - second-person
"Add one line to your shell config"
// GOOD - refers to the thing
"Add one line to the shell config"
```

**Avoid redundant parenthesized content:** Parenthesized text should add new
information, not restate what's already said.

```rust
// BAD - parentheses restate "no changes"
"No changes after squashing 3 commits (commits resulted in no net changes)"
// GOOD - clear and concise
"No changes after squashing 3 commits"
// GOOD - parentheses add supplementary info
"Committing with default message... (3 files, +45, -12)"
```

**Avoid pronouns with cross-message referents:** Hints appear as separate
messages from errors. Don't use pronouns like "it" that refer to something
mentioned in the error message.

```rust
// BAD - "it" refers to branch name in error message
// Error: "Branch 'feature' not found"
// Hint:  "Use --create to create it"
// GOOD - self-contained hint
// Error: "Branch 'feature' not found"
// Hint:  "Use --create to create a new branch"
```

## Message Consistency Patterns

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

**Explicit flag acknowledgment:** Show flags in parentheses when they change
behavior:

```rust
// GOOD - shows the flag explicitly
"Removing feature worktree in background; retaining branch (--no-delete-branch)"
// BAD - doesn't acknowledge user's explicit choice
"Removing feature worktree in background; retaining branch"
```

**Parallel structure:** Related messages should follow the same pattern:

```rust
// GOOD - parallel structure with integration reason explaining branch deletion
// Both wt merge and wt remove show integration reason when branch is deleted
"Removing feature worktree & branch in background (already in main)"       // Branch is integrated (ancestor)
"Removing feature worktree & branch in background (files match main)"      // Branch is integrated (squash/rebase)
"Removing feature worktree & branch in background (all changes in main)"   // Branch is integrated (squash + main advanced)
"Removing feature worktree in background; retaining unmerged branch"        // Unmerged (system keeps)
"Removing feature worktree in background; retaining branch (--no-delete-branch)"  // User flag (user keeps)
```

**Compute decisions once:** For background operations, check conditions upfront,
show the message, then pass the decision explicitly rather than re-checking in
background scripts:

```rust
// GOOD - check once, pass decision
let should_delete = check_if_merged();
show_message_based_on(should_delete);
spawn_background(build_command(should_delete));

// BAD - check twice (once for message, again in background script)
let is_merged = check_if_merged();
show_message_based_on(is_merged);
spawn_background(build_command_that_checks_merge_again());  // Duplicate check!
```

## Message Types

Seven canonical message patterns with their emojis:

1. **Progress**: 🔄 (operations in progress)
2. **Success**: ✅ (successful completion)
3. **Errors**: ❌ (failures, invalid states)
4. **Warnings**: 🟡 (non-blocking issues)
5. **Hints**: 💡 (actionable — user could/should do something)
6. **Info**: ⚪ (status — acknowledging state or user choices, no action needed)
7. **Prompts**: ❓ (questions requiring user input)

**Hint vs Info decision:** If the message suggests the user take an action, it's
a hint. If it's acknowledging what happened (including flag effects), it's info.

| Hint 💡                       | Info ⚪                               |
| ----------------------------- | ------------------------------------- |
| "Run `wt merge` to continue"  | "Already up to date with main"        |
| "Use `--force` to override"   | "Skipping hooks (--no-verify)"        |
| "Branch can be deleted"       | "Worktree preserved (main worktree)"  |

**Message formatting functions** add emoji AND semantic color. Callers provide
content with optional inner styling (like `<bold>`), then pass to
`output::print()`:

```rust
// Simple message - formatting function adds emoji + color
output::print(success_message("Created worktree"))?;
output::print(hint_message("Run 'wt config' to configure"))?;

// With inner styling - use cformat! for bold/dim within the message
output::print(success_message(cformat!("Created worktree for <bold>{branch}</>")))?;
output::print(warning_message(cformat!("Branch <bold>{name}</> not found")))?;
```

**Semantic colors from formatting functions:**

- `success_message()` → green
- `progress_message()` → cyan
- `hint_message()` → dimmed
- `warning_message()` → yellow
- `info_message()` → no color (neutral status)
- `error_message()` → red

**Every user-facing message requires either an emoji or a gutter** for
consistent visual separation.

**Section titles** (experimental): For output with distinct sections (like
`wt hook show`, `wt config show`), use cyan uppercase text without emoji:
`<cyan>SECTION TITLE</>`. This distinguishes organizational headers from status
messages. Currently being trialed — expand to other commands if it works well.

## Blank Line Principles

- **No leading/trailing blanks** — Start immediately, end cleanly
- **One blank after blocks** — Separate multi-line content (gutter blocks,
  sections)
- **One blank after prompts** — Separate user input from results
- **Never double blanks** — One blank line maximum between elements

## stdout vs stderr

**Both modes write all messages to stderr.** stdout is reserved for structured
data:

- **stdout**: JSON output (`--format=json`), shell scripts (directive mode)
- **stderr**: All user-facing messages (progress, success, errors, hints,
  gutter, etc.)

**Directive mode** additionally emits a shell script to stdout at the end.

Use the output system (`output::print()` with message formatting functions) to
handle both modes automatically. Never write directly to stdout/stderr in
command code.

```rust
// GOOD - use output system (handles both modes)
output::print(success_message("Branch created"))?;

// BAD - direct writes bypass output system
println!("Branch created");
```

Interactive prompts must flush stderr before blocking on stdin:

```rust
eprint!("❓ Allow and remember? [y/N] ");
stderr().flush()?;
io::stdin().read_line(&mut response)?;
```

## Temporal Locality: Output Should Be Close to Operations

Output should appear immediately adjacent to the operations it describes.
Progress messages apply only to slow operations (>400ms): git operations,
network requests, builds.

Sequential operations should show immediate feedback:

```rust
for item in items {
    output::print(progress_message(format!("Removing {item}...")))?;
    perform_operation(item)?;
    output::print(success_message(format!("Removed {item}")))?;  // Immediate feedback
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

Signs of poor temporal locality: collecting messages in a buffer, single success
message for batch operations, no progress before slow operations.

## Information Display: Show Once, Not Twice

Progress messages should include all relevant details (what's being done,
counts, stats, context). Success messages should be minimal, confirming
completion with reference info (hash, path).

```rust
// GOOD - detailed progress, minimal success
output::print(progress_message("Squashing 3 commits & working tree changes into a single commit (5 files, +60)..."))?;
perform_squash()?;
output::print(success_message("Squashed @ a1b2c3d"))?;
```

## Style Constants

Style constants in `src/styling/constants.rs` (minimal set for programmatic
styling):

- `ADDITION`: Green (diffs, additions) — used in table rendering
- `DELETION`: Red (diffs, deletions) — used in table rendering
- `GUTTER`: BrightWhite background (quoted content)

Emoji constants: `PROGRESS_EMOJI` (🔄), `SUCCESS_EMOJI` (✅), `ERROR_EMOJI` (❌),
`WARNING_EMOJI` (🟡), `HINT_EMOJI` (💡), `INFO_EMOJI` (⚪), `PROMPT_EMOJI` (❓)

For all other styling, use color-print tags in `cformat!`: `<red>`, `<green>`,
`<yellow>`, `<cyan>`, `<dim>`, `<bold>`, `<bright-black>`

## Styling in Command Code

Use `output::print()` with message formatting functions. The formatting function
adds the emoji + semantic color, and `cformat!` handles inner styling:

```rust
// GOOD - formatting function handles emoji + outer color, cformat! handles inner styling
output::print(success_message(cformat!("Created <bold>{branch}</> from <bold>{base}</>")))?;
output::print(warning_message(cformat!("Branch <bold>{name}</> has <dim>uncommitted changes</>")))?;
output::print(hint_message(cformat!("Run <bright-black>wt merge</> to continue")))?;

// GOOD - plain strings work too (no inner styling needed)
output::print(progress_message("Rebasing onto main..."))?;
output::print(hint_message("No changes to commit"))?;
```

**Available color-print tags:** `<bold>`, `<dim>`, `<bright-black>`, `<red>`,
`<green>`, `<yellow>`, `<cyan>`, `<magenta>`

**Emoji constants in cformat!:** Use `{ERROR_EMOJI}`, `{HINT_EMOJI}`, etc. for
messages that bypass output:: functions (e.g., GitError Display impl):

```rust
cformat!("{ERROR_EMOJI} <red>Branch <bold>{branch}</> not found</>")
```

Branch names in messages should be bolded. Tables (`wt list`) use `StyledLine`
with conditional styling for branch names.

## Commands and Branches in Messages

Never quote commands or branch names. Use styling to make them stand out:

- **In normal font context**: Use `<bold>` for commands and branches
- **In hints**: Use `<bright-black>` for commands (hint() already applies
  dimming — no explicit `<dim>` needed)

```rust
// GOOD - bold in normal context
output::print(info_message(cformat!("Use <bold>wt merge</> to continue")))?;

// GOOD - bright-black for commands in hints
output::print(hint_message(cformat!("Run <bright-black>wt list</> to see worktrees")))?;

// GOOD - plain hint without commands
output::print(hint_message("No changes to commit"))?;

// BAD - quoted commands
output::print(hint_message("Run 'wt list' to see worktrees"))?;
```

## Color Detection

Colors automatically adjust based on environment (NO_COLOR, CLICOLOR_FORCE, TTY
detection). When using `output::` functions, this is handled automatically.

For direct terminal I/O (rare — mainly internal output system code), import
print macros from `worktrunk::styling`:

```rust
use worktrunk::styling::eprintln;  // Auto-detects color support
```

## Design Principles

- **color-print for all styling** — Use `cformat!` with HTML-like tags
  (`<green>`, `<bold>`, etc.). Only use anstyle for `StyledLine` table
  rendering.
- **output:: functions over direct printing** — Use output:: for user messages,
  which auto-adds emoji + semantic color
- **cformat! for inner styling** — Use `<bold>`, `<dim>` tags within output::
  calls
- **Never manual escape codes** — No `\x1b[...` in code
- **YAGNI for presentation** — Most output needs no styling
- **Unicode-aware** — Width calculations respect emoji and CJK characters (via
  `StyledLine`)
- **Graceful degradation** — Must work without color support

## StyledLine API

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

## Gutter Formatting for Quoted Content

Use `format_with_gutter()` for **external data** — anything read from outside
the application: git output, shell commands, commit messages, config values,
cached data. Gutter visually quotes this content to distinguish it from
application-generated output.

**Gutter vs Table:** Tabular data structured by the application should use
`output::table()` with markdown formatting via `render_markdown_in_help()`.
Tables are artifacts; gutter is for quoting external data.

**Linebreaks:** Gutter requires a single newline before it, never double.
`output::print()` adds a trailing newline, so messages should not include `\n`:

```rust
// GOOD - no trailing \n
output::print(progress_message("Merging..."))?;
output::gutter(format_with_gutter(&log, "", None))?;

// BAD - trailing \n creates blank line
output::print(progress_message("Merging...\n"))?;
```

## Error Message Formatting

**Single-line errors with variables are fine:**

```rust
// GOOD - single-line with path variable
.map_err(|e| format!("Failed to read {}: {}", format_path_for_display(path), e))?

// GOOD - using .context() for simple errors
std::fs::read_to_string(&path).context("Failed to read config")?
```

**Multi-line external output needs gutter formatting:**

When external commands (git, npm, LLM tools, hooks) produce multi-line stderr,
use gutter formatting:

1. **Show the command that was run** — Include the full command with arguments
   so users can debug
2. **Put multi-line output in a gutter** — Don't embed raw stderr inline in the
   message

Use the `format_error_block` helper in `src/git/error.rs` or follow its pattern:

```rust
// GOOD - command shown in header, multi-line error in gutter
❌ Commit generation command 'llm --model claude' failed
   ┃ Error: [Errno 8] nodename nor servname provided, or not known

// BAD - multi-line error embedded inline
❌ Commit generation command 'llm' failed: LLM command failed: Error: [Errno 8]...
```

**Implementation:** See `format_error_block()` in `src/git/error.rs` for the
pattern, and `LlmCommandFailed` variant for an example.

## Table Column Alignment

**Principle: Headers and values align consistently within each column type.**

Column alignment follows standard tabular data conventions:

1. **Text columns** (Branch, Path, Message, Commit):
   - Headers: Left-aligned
   - Values: Left-aligned

2. **Diff/numeric columns** (HEAD±, main↕, main…±, Remote⇅):
   - Headers: Right-aligned
   - Values: Right-aligned

**Why:** Right-aligning numeric data allows visual scanning by magnitude
(rightmost digits align vertically). Left-aligning text data prioritizes
readability from the start. Matching header and value alignment within each
column creates a consistent visual grid.

**Implementation:** Headers are positioned within their column width using the
same alignment strategy as their values (render.rs).

## Snapshot Testing Requirement

Every command output must have a snapshot test (`tests/integration_tests/`). See
`tests/integration_tests/remove.rs` for the standard pattern using
`setup_snapshot_settings()`, `make_snapshot_cmd()`, and `assert_cmd_snapshot!()`.

Cover success/error states, with/without data, and flag variations.
