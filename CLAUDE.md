# Worktrunk Development Guidelines

> **Note**: This CLAUDE.md is just getting started. More guidelines will be added as patterns emerge.

## CLI Output Formatting Standards

### The anstyle Ecosystem

All styling uses the **anstyle ecosystem** for composable, auto-detecting terminal output:

- **`anstream`**: Auto-detecting I/O streams (println!, eprintln! macros)
- **`anstyle`**: Core styling with inline pattern `{style}text{style:#}`
- **Color detection**: Respects NO_COLOR, CLICOLOR_FORCE, TTY detection

### Message Types

Three canonical message patterns with their emojis:

1. **Errors**: ❌ + red text
2. **Warnings**: 🟡 + yellow text
3. **Hints**: 💡 + dimmed text

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

### Inline Formatting Pattern

Use anstyle's inline pattern `{style}text{style:#}` where `#` means reset:

```rust
use worktrunk::styling::{eprintln, println, ERROR, ERROR_EMOJI, WARNING, WARNING_EMOJI, HINT, HINT_EMOJI};

// Simple error
eprintln!("{ERROR_EMOJI} {ERROR}Working tree has uncommitted changes{ERROR:#}");

// Warning
eprintln!("{WARNING_EMOJI} {WARNING}Uncommitted changes detected{WARNING:#}");

// Hint
println!("{HINT_EMOJI} {HINT}Use 'wt list' to see all worktrees{HINT:#}");
```

### Composing Styles

Compose styles using anstyle methods (`.bold()`, `.fg_color()`, etc.):

```rust
use worktrunk::styling::{eprintln, AnstyleStyle, ERROR, WARNING};

// Error with bold branch name
let error_bold = ERROR.bold();
eprintln!("❌ Branch '{error_bold}{branch}{error_bold:#}' already exists");

// Warning with bold
let warning_bold = WARNING.bold();
eprintln!("🟡 {warning_bold}{message}{warning_bold:#}");

// Just bold (no color)
let bold = AnstyleStyle::new().bold();
println!("Switched to worktree: {bold}{branch}{bold:#}");
```

### Branch Name Formatting

**Always format branch names in bold** when they appear in messages:

```rust
use worktrunk::styling::{AnstyleStyle, ERROR};

// Good - bold branch name in error
let error_bold = ERROR.bold();
eprintln!("❌ Branch '{error_bold}{branch}{error_bold:#}' already exists");

// Good - bold in regular message
let bold = AnstyleStyle::new().bold();
println!("Switched to worktree: {bold}{branch}{bold:#}");

// Bad - plain branch name
println!("Switched to worktree: {branch}")
```

### Color Detection

Colors automatically adjust based on environment:
- Respects `NO_COLOR` (disables)
- Respects `CLICOLOR_FORCE` / `FORCE_COLOR` (enables)
- Auto-detects TTY (colors only on terminals)

All handled automatically by `anstream` macros.

### Design Principles

- **Inline over wrappers** - Use `{style}text{style:#}` pattern, not wrapper functions
- **Composition over special cases** - Use `.bold()`, `.fg_color()`, not `format_X_with_Y()`
- **Semantic constants** - Use `ERROR`, `WARNING`, not raw colors
- **YAGNI for presentation** - Most output needs no styling
- **Minimal output** - Only use formatting where it adds clarity
- **Unicode-aware** - Width calculations respect emoji and CJK characters (via `StyledLine`)
- **Graceful degradation** - Must work without color support

### Complete Examples

```rust
use worktrunk::styling::{
    eprintln, println, AnstyleStyle,
    ERROR, ERROR_EMOJI, WARNING, WARNING_EMOJI, HINT, HINT_EMOJI
};
use anstyle::Style;

// Simple error
eprintln!("{ERROR_EMOJI} {ERROR}Working tree has uncommitted changes{ERROR:#}");

// Error with bold branch name
let error_bold = ERROR.bold();
eprintln!("{ERROR_EMOJI} Branch '{error_bold}{branch}{error_bold:#}' already exists");

// Warning with bold
let warning_bold = WARNING.bold();
eprintln!("{WARNING_EMOJI} {warning_bold}Uncommitted changes detected{warning_bold:#}");

// Hint
println!("{HINT_EMOJI} {HINT}Use 'wt list' to see all worktrees{HINT:#}");

// Bold branch name in regular message
let bold = Style::new().bold();
println!("Switched to worktree: {bold}{branch}{bold:#}");

// Complex multi-part error
let error_bold = ERROR.bold();
eprintln!("{ERROR_EMOJI} Not a fast-forward from '{error_bold}{target_branch}{error_bold:#}' to HEAD");

// Dimmed secondary info
let dim = Style::new().dimmed();
println!("  {dim}Path: {path}{dim:#}");
```

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
