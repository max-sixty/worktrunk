# CLI Output Formatting Standards

> For output system architecture (shell integration, stdout vs stderr, output
> functions API), see `output-system-architecture.md`.

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

**Avoid "you/your" pronouns:** Messages should refer to things directly, not
address the user. Imperatives like "Run", "Use", "Add" are fine — they're
concise CLI idiom.

```rust
// BAD - possessive pronoun
"Use 'wt merge' to rebase your changes onto main"
// GOOD - refers to the thing directly
"Use 'wt merge' to rebase onto main"

// BAD - possessive pronoun
"Add one line to your shell config"
// GOOD - refers to the thing directly
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

**Two types of parenthesized content with different styling:**

1. **Stats parentheses → Gray** (`[90m` bright-black): Supplementary numerical
   info that could be omitted without losing meaning.
   ```
   ✓ Merged to main (1 commit, 1 file, +1)
   ◎ Squashing 2 commits into a single commit (2 files, +2)...
   ```

2. **Reason parentheses → Message color**: Explains WHY an action is happening;
   integral to understanding.
   ```
   ◎ Removing feature worktree & branch in background (same commit as main, _)
   ```

Stats are truly optional context. Reasons answer "why is this safe/happening?"
and belong with the main message. Symbols within reason parentheses still render
in their native styling (see "Symbol styling" below).

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
"Commands approved; not saved (--yes)"
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
// Target branch is bold; symbol uses its standard styling (dim for _ and ⊂)
"Removing feature worktree & branch in background (same commit as <bold>main</>, <dim>_</>)"        // SameCommit
"Removing feature worktree & branch in background (ancestor of <bold>main</>, <dim>⊂</>)"           // Ancestor (main moved past)
"Removing feature worktree & branch in background (no added changes on <bold>main</>, <dim>⊂</>)"   // NoAddedChanges (empty 3-dot diff)
"Removing feature worktree & branch in background (tree matches <bold>main</>, <dim>⊂</>)"          // TreesMatch (squash/rebase)
"Removing feature worktree & branch in background (all changes in <bold>main</>, <dim>⊂</>)"        // MergeAddsNothing (squash + main advanced)
"Removing feature worktree in background; retaining unmerged branch"                         // Unmerged (system keeps)
"Removing feature worktree in background; retaining branch (--no-delete-branch)"             // User flag (user keeps)
```

**Symbol styling:** Symbols are atomic with their color — the styling is part of
the symbol's identity, not a presentation choice. Each symbol has a defined
appearance that must be preserved in all contexts:

- `_` and `⊂` — dim (integration/safe-to-delete indicators)
- `+N` and `-N` — green/red (diff indicators)

When a symbol appears in a colored message (cyan progress, green success), close
the message color before the symbol so it renders in its native styling. This
requires breaking out of the message color and reopening it after the symbol.
See `FlagNote` in `src/output/handlers.rs` for the implementation pattern.

**Comma + "but" + em-dash for limitations:** When stating an outcome with a
limitation and its reason:

```rust
// Outcome, but limitation — reason
"Worktree for feature @ ~/repo.feature, but cannot change directory — shell integration not installed"
```

This pattern:
- States what succeeded (worktree exists at path)
- Uses "but" to introduce what didn't work (cannot cd)
- Uses em-dash to explain why (shell integration status)

See `compute_shell_warning_reason()` in `src/output/handlers.rs` for the
complete spec of shell integration warning messages and hints

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

See `output-system-architecture.md` for the API. This section covers when to use
each type.

**Success vs Info:** Success (✓) means something was created or changed. Info
(○) acknowledges state without changing anything.

| Success ✓                               | Info ○                                |
| --------------------------------------- | ------------------------------------- |
| "Created worktree for feature"          | "Switched to worktree for feature"    |
| "Created new worktree for feature"      | "Already on worktree for feature"     |
| "Commands approved & saved"             | "All commands already approved"       |

**Hint vs Info:** Hints suggest user action. Info acknowledges what happened.

| Hint ↳                        | Info ○                                |
| ----------------------------- | ------------------------------------- |
| "Run `wt merge` to continue"  | "Already up to date with main"        |
| "Use `--yes` to override"     | "Skipping hooks (--no-verify)"        |
| "Branch can be deleted"       | "Worktree preserved (main worktree)"  |

**Command suggestions in hints:** Use "To X, run Y" pattern. End with the
command for easy copying:

```rust
// GOOD - command at end for easy copying
"To delete the unmerged branch, run wt remove feature -D"
"To rebase onto main, run wt step rebase or wt merge"

// GOOD - when user needs to modify their command
"To switch to the remote branch, remove --create; run wt switch feature"

// BAD - command without context
"wt remove feature -D deletes unmerged branches"

// BAD - command not at end (hard to copy)
"Run wt switch feature (without --create) to switch to the remote branch"
```

**Multiple suggestions in one hint:** When combining suggestions with semicolons,
put the more commonly needed command last for easy terminal copying:

```rust
// GOOD - common action (create) last, easy to select and copy
"To list branches, run wt list --branches; to create a new branch, run wt switch feature --create"

// BAD - common action buried, harder to copy
"To create a new branch, run wt switch feature --create; to list branches, run wt list --branches"
```

Use `suggest_command()` from `worktrunk::styling` for proper shell escaping.

**Every user-facing message requires either a symbol or a gutter.**

**Section titles** (experimental): For sectioned output (`wt hook show`,
`wt config show`), use `<cyan>SECTION TITLE</>`.

## Blank Line Principles

**Core principle:** When presenting the user with text to read and consider, add
spacing for readability. When piping output (stdout), keep output dense for
parsing.

Specific rules:

- **No leading/trailing blanks** — Start immediately, end cleanly
- **One blank after prompts** — Separate user input from results
- **Never double blanks** — One blank line maximum between elements

## Output System

Use `output::` functions for consistency. See `output-system-architecture.md`
for stdout vs stderr decisions and simplification notes.

```rust
// Preferred - consistent routing and flushing
output::print(success_message("Branch created"))?;

// Acceptable for simple cases - just remember to flush if needed
eprintln!("{}", success_message("Branch created"));
```

**Note:** The output wrappers are thin (`eprintln!` + flush). The main value is
consistency, not complex logic. See "Simplification Notes" in
`output-system-architecture.md`.

**Interactive prompts** must flush stderr before blocking on stdin:

```rust
eprint!("❯ Allow and remember? [y/N] ");
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
◎ Removing worktree for feature...
◎ Removing worktree for bugfix...
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

Only three `anstyle` constants exist for table rendering (`src/styling/constants.rs`):

- `ADDITION`: Green (diffs)
- `DELETION`: Red (diffs)
- `GUTTER`: BrightWhite background

For everything else, use `cformat!` tags.

## Styling in Command Code

Use `output::print()` with formatting functions. Use `cformat!` for inner
styling:

```rust
output::print(success_message(cformat!("Created <bold>{branch}</> from <bold>{base}</>")))?;
output::print(hint_message(cformat!("Run <bright-black>wt merge</> to continue")))?;
```

**color-print tags:** `<bold>`, `<dim>`, `<bright-black>`, `<red>`, `<green>`,
`<yellow>`, `<cyan>`, `<magenta>`

**Branch names** should be bolded in messages.

**Symbol constants in cformat!:** For messages that bypass output:: functions
(e.g., `GitError` Display impl), use symbol constants directly:

```rust
cformat!("{ERROR_SYMBOL} <red>Branch <bold>{branch}</> not found</>")
```

## Commands and Branches in Messages

Never quote commands or branch names. Use styling to make them stand out:

- **In normal font context**: Use `<bold>` for commands and branches
- **In hints**: Use `<bright-black>` for commands and data values (paths,
  branches). Avoid `<bold>` inside hints — the closing `[22m` resets both bold
  AND dim, so text after `</bold>` loses dim styling.

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

## Design Principles

- **`cformat!` for styling** — Never manual escape codes (`\x1b[...`)
- **`output::` for printing** — Preferred for consistency; direct `println!`/`eprintln!` acceptable
- **YAGNI** — Most output needs no styling
- **Graceful degradation** — Colors auto-adjust (NO_COLOR, TTY detection)
- **Unicode-aware** — Width calculations respect symbols and CJK (via `StyledLine`)

**StyledLine** for table rendering with proper width calculations:

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

## Gutter Formatting

Use gutter for **quoted content** (git output, commit messages, config to copy,
hook commands being displayed):

- `format_bash_with_gutter()` — shell commands (dimmed + syntax highlighting)
- `format_with_gutter()` — other content

**Gutter vs Table:** Tables for structured app data; gutter for quoting external
content.

**Gutter vs Hints:** Command suggestions in hints use inline `<bright-black>`,
not gutter. Gutter is for displaying content (what will execute, config to
copy); hints suggest what the user should run.

## Newline Convention

**Core principle:** All formatting functions return content WITHOUT trailing
newlines. Callers handle element separation.

This applies to:
- Message functions: `error_message()`, `success_message()`, `hint_message()`, etc.
- Gutter functions: `format_with_gutter()`, `format_bash_with_gutter()`

**With `output::print()`:** Adds trailing newline automatically (uses `eprintln!`).

```rust
output::print(progress_message("Merging..."))?;
output::print(format_with_gutter(&log, None))?;
```

**In Display impls:** Use explicit newlines for element separation.

```rust
// Pattern: leading \n separates from previous element
write!(f, "{}", error_message(...))?;           // first element, no leading \n
write!(f, "\n{}", format_with_gutter(...))?;    // gutter, separated by \n
write!(f, "\n{}", hint_message(...))            // hint, separated by \n

// For blank line between elements, add extra \n
write!(f, "\n{}\n", format_with_gutter(...))?;  // trailing \n creates blank line
write!(f, "\n{}", hint_message(...))            // hint after blank line
```

**Don't add trailing `\n` to content:**

```rust
// GOOD - output::print adds newline
output::print(progress_message("Merging..."))?;

// BAD - double newline
output::print(progress_message("Merging...\n"))?;
```

**Avoid bullets — use gutter instead:**

```rust
// BAD - bullet list
let mut warning = String::from("Some git operations failed:");
for error in &errors {
    warning.push_str(&format!("\n  - {}: {}", name, msg));
}
output::print(warning_message(warning))?;

// GOOD - gutter formatting
let error_lines: Vec<String> = errors
    .iter()
    .map(|e| cformat!("<bold>{}</>: {}", e.name, e.msg))
    .collect();
let warning = format!(
    "Some git operations failed:\n{}",
    format_with_gutter(&error_lines.join("\n"), None)
);
output::print(warning_message(warning))?;
```

## Error Formatting

**Single-line errors** with variables are fine:

```rust
// GOOD - single-line with path variable
.map_err(|e| format!("Failed to read {}: {}", format_path_for_display(path), e))?

// GOOD - using .context() for simple errors
std::fs::read_to_string(&path).context("Failed to read config")?
```

**Multi-line external output** (git, hooks, LLM) needs gutter:

1. Show the command that was run (with arguments)
2. Put multi-line output in a gutter

```
✗ Commit generation command 'llm --model claude' failed
   ┃ Error: [Errno 8] nodename nor servname provided

// NOT: ✗ ... failed: LLM command failed: Error: [Errno 8]...
```

See `format_error_block()` in `src/git/error.rs`.

## Table Column Alignment

- **Text columns** (Branch, Path): left-aligned
- **Numeric columns** (HEAD±, main↕): right-aligned

## Snapshot Testing

Every command output must have snapshot tests (`tests/integration_tests/`).
See `tests/integration_tests/remove.rs` for the standard pattern using
`setup_snapshot_settings()`, `make_snapshot_cmd()`, and `assert_cmd_snapshot!()`.

Cover success/error states, with/without data, and flag variations.
