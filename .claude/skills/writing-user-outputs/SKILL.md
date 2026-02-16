---
name: writing-user-outputs
description: CLI output formatting standards for worktrunk. Use when writing user-facing messages, error handling, progress output, hints, warnings, or working with the output system.
---

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
eprintln!("{}", success_message("Created worktree"));
output::change_directory(&path)?;  // Writes to directive file if set, else no-op
```

**Printing output:**

Use `eprintln!` and `println!` from `worktrunk::styling` (re-exported from
`anstream` for automatic color support and TTY detection):

```rust
use worktrunk::styling::{eprintln, println, stderr};

// Status messages to stderr
eprintln!("{}", success_message("Created worktree"));

// Primary output to stdout (tables, JSON, pipeable)
println!("{}", table_output);

// Flush before interactive prompts
stderr().flush()?;
```

**Shell integration functions** (`src/output/global.rs`):

| Function | Purpose |
|----------|---------|
| `change_directory(path)` | Shell cd after wt exits (writes to directive file if set) |
| `execute(command)` | Shell command after wt exits |
| `terminate_output()` | Reset ANSI state on stderr |
| `is_shell_integration_active()` | Check if directive file set (rarely needed) |
| `pre_hook_display_path(path)` | Compute display path for pre-hooks |
| `post_hook_display_path(path)` | Compute display path for post-hooks |

**Message formatting functions** (`worktrunk::styling`):

| Function | Symbol | Color |
|----------|--------|-------|
| `success_message()` | ✓ | green |
| `progress_message()` | ◎ | cyan |
| `info_message()` | ○ | symbol dim, text plain |
| `warning_message()` | ▲ | yellow |
| `hint_message()` | ↳ | dim |
| `error_message()` | ✗ | red |
| `prompt_message()` | ❯ | cyan |

**Section headings** (`worktrunk::styling`):

```rust
use worktrunk::styling::format_heading;

// Plain heading
format_heading("BINARIES", None)  // => "BINARIES" (cyan)

// Heading with suffix
format_heading("USER CONFIG", Some("~/.config/wt.toml"))
// => "USER CONFIG  ~/.config/wt.toml" (title cyan, suffix plain)
```

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
(via `shell_exec::Cmd`). This prevents hooks from writing to the directive
file.

## Windows Compatibility (Git Bash / MSYS2)

On Windows with Git Bash, `mktemp` returns POSIX-style paths like `/tmp/tmp.xxx`.
The native Windows binary (`wt.exe`) needs a Windows path to write to the
directive file.

**No explicit path conversion is needed.** MSYS2 automatically converts POSIX
paths in environment variables when spawning native Windows binaries — shell
wrappers can use `$directive_file` directly. See:
https://www.msys2.org/docs/filesystem-paths/

---

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

**Avoid "you/your" pronouns:** Messages should refer to things directly, not
address the user. Imperatives like "Run", "Use", "Add" are fine — they're
concise CLI idiom.

```rust
// BAD - "Use 'wt merge' to rebase your changes onto main"
// GOOD - "Use 'wt merge' to rebase onto main"
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

**Show path when hooks run in a different directory:** When hooks run in a
worktree other than the user's current (or eventual) location, show the path.
Use the appropriate helper function:

1. **Pre-hooks and manual `wt hook`** — User is at cwd, no cd happens.
   Use `output::pre_hook_display_path(hooks_run_at)`.
   Examples: pre-commit, pre-merge, pre-remove, manual `wt hook post-merge`.

2. **Post-hooks** — User will cd to destination if shell integration is active.
   Use `output::post_hook_display_path(destination)`.
   Examples: post-create, post-switch, post-start, post-merge (after removal).

```rust
// Pre-hooks: user is at cwd, no cd happens
run_hook_with_filter(..., crate::output::pre_hook_display_path(ctx.worktree_path))?;

// Post-hooks: user will cd to destination if shell integration active
ctx.spawn_post_start_commands(crate::output::post_hook_display_path(&destination))?;
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

## Heading Case

Use **sentence case** for help text headings: "Configuration files", "JSON output", "LLM commit messages".

## Message Consistency Patterns

Use consistent punctuation and structure for related messages.

**Ampersand for combined actions:** Use `&` when a single operation does
multiple things:

```rust
"Removing feature worktree & branch in background"
"Commands approved & saved to config"
```

**Semicolon for joining clauses:** Use semicolons to connect related information:

```rust
"Removing feature worktree in background; retaining branch (--no-delete-branch)"
"Branch unmerged; to delete, run <bright-black>wt remove -D</>"  // hint uses bright-black
"{tool} not authenticated; run <bold>{tool} auth login</>"       // warning uses bold
```

**Explicit flag acknowledgment:** Show flags in parentheses when they change
behavior:

```rust
// GOOD - shows the flag explicitly
"Removing feature worktree in background; retaining branch (--no-delete-branch)"
// BAD - doesn't acknowledge user's explicit choice
"Removing feature worktree in background; retaining branch"
```

**Flag locality:** Place flag indicators adjacent to the concept they modify.
Flags should appear immediately after the noun/action they affect, not at the
end of the message:

```rust
// GOOD - (--force) is adjacent to "worktree" which it modifies
"Removing feature worktree (--force) & branch in background (same commit as main, _)"
// BAD - (--force) at end, disconnected from the worktree removal it enables
"Removing feature worktree & branch in background (same commit as main, _) (--force)"
```

This principle ensures readers can immediately understand what each annotation
modifies.

**Parallel structure:** Related messages should follow the same pattern:

```rust
// GOOD - parallel structure with integration reason explaining branch deletion
// Target branch is bold; symbol uses its standard styling (dim for _ and ⊂)
"Removing feature worktree & branch in background (same commit as <bold>main</>, <dim>_</>)"  // Integrated
"Removing feature worktree in background; retaining unmerged branch"                          // Unmerged
"Removing feature worktree in background; retaining branch (--no-delete-branch)"              // User flag
```

**Symbol styling:** Symbols are atomic with their color — the styling is part of
the symbol's identity, not a presentation choice. Each symbol has a defined
appearance that must be preserved in all contexts:

- `_` and `⊂` — dim (integration/safe-to-delete indicators)
- `+N` and `-N` — green/red (diff indicators)

When a symbol appears in a colored message (cyan progress, green success), close
the message color before the symbol so it renders in its native styling. This
requires breaking out of the message color and reopening it after the symbol.
See `FlagNote` in `src/output/handlers.rs` for an example — it handles flag
acknowledgment notes (like integration reasons) with proper color transitions
via `after_cyan()` and `after_green()` methods.

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

See `compute_shell_warning_reason()` in `src/output/shell_integration.rs` for the
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

## Warning Ordering

**Core principle:** Warnings about state discovered during evaluation appear
**before** the action message that follows from that evaluation.

When a command evaluates state, discovers something unexpected, and proceeds
anyway, the warning should come first:

```
▲ Branch-worktree mismatch; expected feature @ ~/workspace/project.feature ⚑
◎ Removing feature worktree & branch in background (same commit as main, _)
```

Not:

```
◎ Removing feature worktree & branch in background (same commit as main, _)
▲ Branch-worktree mismatch; expected feature @ ~/workspace/project.feature ⚑
```

Warnings that result from the action itself (something failed during execution)
naturally come after the action.

## Message Types

**Success vs Info:** Success (✓) means something was created or changed. Info
(○) acknowledges state without changing anything.

| Success ✓                               | Info ○                                |
| --------------------------------------- | ------------------------------------- |
| "Created worktree for feature"          | "Switched to worktree for feature"    |
| "Created new worktree for feature"      | "Already on worktree for feature"     |
| "Commands approved & saved"             | "All commands already approved"       |

**Hint vs Info:** Hints suggest user action or provide additional non-essential
context (supplementary details the user doesn't need but may find useful). Info
acknowledges state without changing anything.

| Hint ↳                                          | Info ○                                |
| ------------------------------------------------ | ------------------------------------- |
| "To continue, run `wt merge`"                    | "Already up to date with main"        |
| "Commit or stash changes first"                  | "Skipping hooks (--no-verify)"        |
| "Branch can be deleted"                           | "Worktree preserved (main worktree)"  |
| "Failed command, exit code 128:"                   |                                       |

**Warning placement:** When something unexpected happens, warn somewhere. Where
depends on the nature of the issue:

```
Is it unexpected?
├── No → Silent (e.g., gh not installed when no GitHub remote)
└── Yes → Warn somewhere:
    ├── Immediate impact OR temporary → Inline (warning_message or in-band indicator)
    ├── Persists until user action → wt config show (can be checked later)
    └── Not user-fixable → log::warn! (developer diagnostics)
```

**Inline warnings** for issues affecting the current command:

| Issue | Why inline |
|-------|------------|
| Rate limit during CI fetch | Temporary — won't be there next time |
| Network timeout | Temporary — retry might work |
| Hook failed during operation | Immediate impact on this command |

**`wt config show`** for issues that persist until the user fixes them. These
don't need to interrupt every command — users can check diagnostics when
investigating:

| Issue | Why config show |
|-------|-----------------|
| `gh` not authenticated | User runs `gh auth login` |
| Shell integration misconfigured | User updates shell config |
| Config syntax errors | User fixes config file |

**`log::warn!()`** for issues users cannot fix. These help developers debug but
shouldn't clutter user output:

| Issue | Why log::warn! |
|-------|----------------|
| JSON parse error (API changed) | Requires code fix |
| Internal invariant violated | Developer bug |

**Command suggestions in hints:** When a hint includes a runnable command, use
"To X, run Y" pattern. End with the command for easy copying:

```rust
// GOOD - command at end for easy copying
"To delete the unmerged branch, run wt remove feature -D"
"To rebase onto main, run wt step rebase or wt merge"

// GOOD - recovery command after shadowing a remote branch
"To switch to the remote branch, delete this branch and run without --create: wt remove feature && wt switch feature"

// BAD - command without context
"wt remove feature -D deletes unmerged branches"

// BAD - command not at end (hard to copy)
"Run wt switch feature (without --create) to switch to the remote branch"
```

For general action guidance without a specific command, direct imperatives are
clearer:

```rust
// GOOD - direct imperative for general guidance
"Commit or stash changes first"
"Run from inside a worktree, or specify a branch name"

// VERBOSE - "To proceed" adds nothing
"To proceed, commit or stash changes first"
```

**Description + command in single message:** For warnings/errors that include a
recovery command, join with semicolon. Use `<bold>` for commands in
warnings/errors (only hints use `<bright-black>`):

```rust
// Warning with inline recovery command (bold for commands)
warning_message("Failed to restore stash; run <bold>git stash pop {ref}</> to restore manually")
warning_message("{tool} not authenticated; run <bold>{tool} auth login</>")

// For longer suggestions, use separate hint message (bright-black for commands)
warning_message("Failed to restore stash")
hint_message("To restore manually, run <bright-black>git stash pop {ref}</>")
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

**Section titles:** For sectioned output (`wt hook show`, `wt config show`), use
`format_heading()` from `worktrunk::styling` (documented above).

## Blank Line Principles

**Core principle:** When presenting the user with text to read and consider, add
spacing for readability. When piping output (stdout), keep output dense for
parsing.

Specific rules:

- **No leading/trailing blanks** — Start immediately, end cleanly
- **Blank before prompts, not after** — Signal "pause, something interactive is
  happening" before the prompt; once the user responds, output flows continuously
- **One blank between phases** — When a sub-operation completes and a different
  operation begins, add a blank line to visually separate them
- **Never double blanks** — One blank line maximum between elements
- **Hints attach to their subject** — Never put a blank line between a hint and
  the message it elaborates on. Hints (↳) are subordinate — they belong directly
  below their parent message with no gap.

  ```
  // GOOD - hint directly follows its subject
  ↳ fish: Not configured shell extension
  ↳ To configure, run wt config shell install

  // BAD - blank line detaches hint from subject
  ↳ fish: Not configured shell extension

  ↳ To configure, run wt config shell install
  ```

**Prompt spacing:** A blank line before the prompt signals "something different
is about to happen" and gives the user's eye a natural stopping point before they
need to read and respond. No blank line after — the user's input ends the
interactive moment and subsequent output flows naturally from that decision.

```
◎ Detecting available LLM tools...

❯ Configure claude for commit messages? [y/N/?] y
✓ Added to user config:
   ┃ [commit.generation]
   ┃ command = "..."
↳ View config: wt config show

▲ Auto-staging 1 untracked path:
   ┃ a
◎ Generating commit message...
```

**Phase separation:** A "phase" is a logically distinct operation. The blank
line between the hint and warning above signals "config setup is done, now we're
doing the main command workflow."

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
    eprintln!("{}", progress_message(format!("Removing {item}...")));
    perform_operation(item)?;
    eprintln!("{}", success_message(format!("Removed {item}")));  // Immediate feedback
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
eprintln!("{}", progress_message("Squashing 3 commits & working tree changes into a single commit (5 files, +60)..."));
perform_squash()?;
eprintln!("{}", success_message("Squashed @ a1b2c3d"));
```

## Style Constants

Only three `anstyle` constants exist for table rendering (`src/styling/constants.rs`):

- `ADDITION`: Green (diffs)
- `DELETION`: Red (diffs)
- `GUTTER`: BrightWhite background

For everything else, use `cformat!` tags.

## Styling in Command Code

Use `eprintln!` with formatting functions. Use `cformat!` for inner styling:

```rust
eprintln!("{}", success_message(cformat!("Created <bold>{branch}</> from <bold>{base}</>")));
eprintln!("{}", hint_message(cformat!("Run <bright-black>wt merge</> to continue")));
```

**color-print tags:** `<bold>`, `<dim>`, `<bright-black>`, `<red>`, `<green>`,
`<yellow>`, `<cyan>`, `<magenta>`

**Branch names and status values** should be bolded in messages.

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
eprintln!("{}", info_message(cformat!("Use <bold>wt merge</> to continue")));
// GOOD - bright-black for commands in hints
eprintln!("{}", hint_message(cformat!("Run <bright-black>wt list</> to see worktrees")));
// BAD - quoted commands
eprintln!("{}", hint_message("Run 'wt list' to see worktrees"));
```

## Design Principles

- **`cformat!` for styling** — Never manual escape codes (`\x1b[...`)
- **`cformat!` variables are safe** — Tags like `<bold>` are processed at compile
  time only. Runtime variable values are NOT interpreted as markup, so user
  content (branch names, commit messages, paths, shell commands with `<`/`>`
  redirects) can be interpolated directly without escaping. Do NOT escape
  `<`/`>` in variables — it adds extra chars.
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

## Documentation Examples

Use consistent examples throughout all documentation, help text, and config
templates.

### Canonical example setup

| Element | Value | Notes |
|---------|-------|-------|
| Repo directory | `myproject` | Generic placeholder |
| Repo path | `~/code/myproject` | Realistic dev path |
| Branch | `feature/auth` | Shows sanitize filter |
| Worktree path | `~/code/myproject.feature-auth` | Result of default template |

### Template variable examples

Use the canonical values from the table above in all examples:

```
{{ repo }}           — Repository directory name (e.g., `myproject`)
{{ branch }}         — Branch name (e.g., `feature/auth`)
{{ worktree_path }}  — Absolute path to worktree (e.g., `/path/to/myproject.feature-auth`)
```

In TOML comments:
```toml
#   {{ repo }}           - Repository directory name (e.g., "myproject")
#   {{ branch }}         - Raw branch name (e.g., "feature/auth")
#   {{ worktree_name }}  - Worktree directory name (e.g., "myproject.feature-auth")
```

### Worktree path examples

When showing worktree-path template examples:

```toml
# Default — siblings in parent directory
# Creates: ~/code/myproject.feature-auth
worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

# Inside the repository
# Creates: ~/code/myproject/.worktrees/feature-auth
worktree-path = ".worktrees/{{ branch | sanitize }}"
```

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

**With `eprintln!`:** Adds trailing newline automatically.

```rust
eprintln!("{}", progress_message("Merging..."));
eprintln!("{}", format_with_gutter(&log, None));
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
// GOOD - eprintln! adds newline
eprintln!("{}", progress_message("Merging..."));

// BAD - double newline
eprintln!("{}", progress_message("Merging...\n"));
```

**Avoid bullets — use gutter instead.** Instead of `"\n  - {}: {}"` bullet
formatting, use `format_with_gutter()` to present lists.

## Error Formatting

### Error Message Structure

Error and warning messages should communicate four things:

1. **What happened** — The actual state or outcome
2. **What was expected** — The correct or desired state
3. **The impact** — Why this matters (optional for obvious cases)
4. **How to resolve** — What the user should do (can be a separate hint message)

```rust
// GOOD - states actual, expected, and impact in main message
"Shell probe: wt is binary at /path, not function — won't auto-cd"
//                 ^^^^^^^^^^^^^^^   ^^^^^^^^^^^^   ^^^^^^^^^^^^^^
//                 actual            expected       impact
// Resolution in separate hint: "Restart shell to activate"

// GOOD - actual vs expected with resolution inline
"Config file has 3 errors, expected valid TOML; run wt config validate for details"
//              ^^^^^^^    ^^^^^^^^                  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
//              actual     expected                  resolution

// BAD - only states actual, no expected or impact
"Shell probe: wt resolves to binary at /path"
// Missing: what should it be instead? why does it matter?

// BAD - vague, no actionable information
"Shell integration problem detected"
// Missing: what's wrong? what should it be? what to do?
```

When the expected state is obvious from context, it can be implied:

```rust
// OK - expected state (file should exist) is obvious
"Config file not found at ~/.config/wt/config.toml"

// OK - expected state (should succeed) is obvious
"Failed to read config: permission denied"
```

**Diagnostic messages** (like `wt config show`) should follow this pattern
especially carefully — users read diagnostics to understand what's wrong.

### Single vs Multi-line

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

See `src/git/error.rs` for examples of this pattern in `GitError` Display impls.

## Verbose Output (`-v` and `-vv`)

**`-v` (verbose):** User-facing diagnostic output. Must follow these guidelines.
Shows template expansions and other details users might need for debugging config.

Format for template expansion:
```
○ Expanding name
 ┃ template → result
```

- **Info message** for header (`○` symbol, "Expanding" + bold name)
- **Gutter** for quoted content (template → result)
- Arrow `→` is dim
- For multiline: template lines, dim `→` on its own line, result lines

**`-vv` (debug):** Developer-facing logging output. MAY violate these guidelines.
Uses `log::debug!()` with structured format for deep debugging. Not intended for
regular users.

## Path Formatting

**All user-facing paths must use `format_path_for_display()`** from
`worktrunk::path`. This function replaces home directory prefixes with `~` for
readability (e.g., `/Users/alex/projects/repo` → `~/projects/repo`).

```rust
use worktrunk::path::format_path_for_display;

// GOOD - uses format_path_for_display
eprintln!("{}", success_message(cformat!(
    "Created worktree at {}",
    format_path_for_display(&worktree_path)
)));

// GOOD - error messages too
.map_err(|e| format!("Failed to read {}: {}", format_path_for_display(path), e))?

// BAD - raw path.display()
eprintln!("{}", success_message(format!(
    "Created worktree at {}",
    worktree_path.display()  // Shows /Users/alex/... instead of ~/...
)));
```

**Applies to:**
- Success/info/warning/error messages
- Hints suggesting paths
- Progress messages
- Dry-run previews

**Exceptions:**
- Debug logging (`log::debug!`) — full paths help debugging
- Paths passed to git commands — must be real paths

## Table Column Alignment

- **Text columns** (Branch, Path): left-aligned
- **Numeric columns** (HEAD±, main↕): right-aligned

## Snapshot Testing

Every command output must have snapshot tests (`tests/integration_tests/`).
See `tests/integration_tests/remove.rs` for the standard pattern using
`setup_snapshot_settings()`, `make_snapshot_cmd()`, and `assert_cmd_snapshot!()`.

Cover success/error states, with/without data, and flag variations.
