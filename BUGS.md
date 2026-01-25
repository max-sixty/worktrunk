# Known Bugs

This document tracks bugs discovered through adversarial code review (2026-01-25).

## Priority Rankings

| Priority | Bug | Location | Impact |
|----------|-----|----------|--------|
| **HIGH** | Stash restore silent failure | `repository_ext.rs:404` | Work lost without clear recovery path |
| **HIGH** | `is_dirty()` misses skip-worktree | `working_tree.rs:117` | Silent data loss |
| **HIGH** | `paths_match()` symlink false negative | `resolve.rs:134` | Wrong "path occupied" errors on macOS |
| **MEDIUM** | `branch()` swallows errors | `working_tree.rs:95` | Confusing detached HEAD state |
| **MEDIUM** | CommandConfig serialization panic | `commands.rs:137` | Latent crash risk |
| **MEDIUM** | PowerShell block comment detection | `detection.rs:91` | Wrong detection state |
| **LOW** | Dot command (.) not detected | `detection.rs:146` | False "not installed" warning |
| **LOW** | Absolute path detection | `detection.rs:193` | False "not installed" warning |
| **LOW** | PowerShell iex alias not detected | `detection.rs:146` | False "not installed" warning |
| **LOW** | TOCTOU in squash | `step_commands.rs:285` | Rare unexpected commit content |
| **LOW** | Migration file double-dot | `deprecation.rs:477` | Cosmetic |
| **LOW** | `git_dir()` asymmetric canonicalization | `working_tree.rs:146` | Edge case path mismatch |

---

## HIGH Priority

### 1. Stash restore silent failure on conflicts

**File:** `src/commands/repository_ext.rs:404-434`

**Problem:** If `git stash pop` fails due to conflicts (not just command failure), the user gets only a warning "Failed to restore stash" but no diagnostic about WHY it failed. The stash is left in limbo.

**Scenario:**
1. User runs `wt merge` or push operation
2. Target worktree has staged changes that get stashed
3. After the operation, stash pop fails due to conflicts
4. User sees generic warning, work is stuck in stash

**User symptom:** Work lost in stash with confusing error message. User must manually recover via `git stash list` and `git stash pop`.

**Recommended fix:**
- Check if failure was due to conflicts specifically
- If conflicts, show the conflicting files
- Always show the stash ref clearly so users can recover

---

### 2. `is_dirty()` misses assume-unchanged and skip-worktree files

**File:** `src/git/repository/working_tree.rs:117-119`

```rust
pub fn is_dirty(&self) -> anyhow::Result<bool> {
    let stdout = self.run_command(&["status", "--porcelain"])?;
    Ok(!stdout.trim().is_empty())
}
```

**Problem:** Files marked with `git update-index --assume-unchanged` or `--skip-worktree` are hidden from `git status --porcelain`. This means:
- `is_dirty()` returns `false` even when local modifications exist
- `ensure_clean()` passes for worktrees with hidden local changes
- Operations like `wt merge` could proceed and lose these modifications during rebase/checkout

**Scenario:** User has local config modifications hidden via skip-worktree (common pattern for `.env` files in sparse checkouts). After `wt merge`, these modifications are silently lost.

**Recommended fix:** Add warning when assume-unchanged or skip-worktree files exist:
```bash
git ls-files -v | grep '^[a-z]'  # assume-unchanged
git ls-files -v | grep '^S'     # skip-worktree
```

---

### 3. `paths_match()` false negatives with symlinks

**File:** `src/commands/worktree/resolve.rs:134-138`

```rust
canonicalize(a).unwrap_or_else(|_| a.to_path_buf())
```

**Problem:** When comparing a computed worktree path (before creation) to an existing path:
- Path A exists → canonicalizes to `/private/var/tmp/foo`
- Path B doesn't exist → stays as `/var/tmp/foo`
- Comparison fails even though they're the same location

**Scenario:** On macOS with `/var` -> `/private/var` symlink, computed paths for new worktrees don't match existing paths from `git worktree list`.

**User symptom:** "path occupied" false alarms or "worktree not found" errors on macOS.

**Recommended fix:** When one path exists and the other doesn't, canonicalize the existing one and apply the same symlink resolution to the non-existent path (resolve parent directory symlinks).

---

## MEDIUM Priority

### 4. `branch()` caching swallows errors as detached HEAD

**File:** `src/git/repository/working_tree.rs:95-113`

```rust
self.run_command(&["branch", "--show-current"])
    .ok()  // <-- Swallows all errors
    .and_then(|s| { ... })
```

**Problem:** When `git branch --show-current` fails (corrupted `.git`, permission errors), the error is silently converted to `None` (as if detached HEAD) and cached permanently.

**User symptom:** User sees "detached HEAD" state for a worktree that actually has a branch, but has filesystem issues. The actual error is completely swallowed.

**Recommended fix:** Propagate the error instead of converting to `None`. At minimum, log the error.

---

### 5. CommandConfig serialization panic (latent)

**File:** `src/config/commands.rs:137`

```rust
let key = cmd.name.as_ref().unwrap();  // Panics if name is None
```

**Problem:** When serializing a `CommandConfig` that contains multiple unnamed commands (which can happen after `merge_append()` of two single-string hook configs), the code panics.

**Scenario:**
1. User config: `post-start = "echo global"`
2. Project config: `post-start = "echo project"`
3. Merged config has 2 unnamed commands
4. If this merged config is ever serialized → panic

**Current impact:** None - merged hooks are only used transiently for execution.

**Future risk:** If code evolves to serialize merged configs, it will crash with "called `Option::unwrap()` on a `None` value".

**Recommended fix:** Either document that merged CommandConfigs must not be serialized, OR fix serialization to generate synthetic names for unnamed commands.

---

### 6. PowerShell block comment false positive

**File:** `src/shell/detection.rs:91-93`

```rust
if trimmed.starts_with('#') {
    return false;
}
```

**Problem:** PowerShell block comments `<# ... #>` are NOT filtered. Only `#` at line start is checked. A line like:
```powershell
<# Invoke-Expression (wt config shell init powershell) #>
```
Is incorrectly detected as active shell integration when it's actually commented out.

**User symptom:** `wt config show` reports "shell integration installed" when it's actually commented out.

**Recommended fix:** Add block comment detection for PowerShell.

---

## LOW Priority

### 7. Dot command (.) not detected as shell integration

**File:** `src/shell/detection.rs:146-149`

**Problem:** The POSIX `.` command (equivalent to `source`) is not detected:
```bash
. <(wt config shell init bash)
```

**User symptom:** Users with POSIX-style config see "shell integration not installed" warning when it IS installed.

**Status:** Documented as "CONFIRMED FALSE NEGATIVE" in tests.

---

### 8. Absolute path invocations not detected

**File:** `src/shell/detection.rs:193`

**Problem:** `/` not in allowed preceding characters:
```bash
eval "$(/usr/local/bin/wt config shell init bash)"
```

**User symptom:** Users invoking via absolute path see misleading "not installed" warnings.

**Status:** Documented as "CONFIRMED FALSE NEGATIVE" in tests.

---

### 9. PowerShell iex alias not detected

**File:** `src/shell/detection.rs:146-149`

**Problem:** Only `Invoke-Expression` checked, not the common `iex` shorthand:
```powershell
iex (wt config shell init powershell)
```

**User symptom:** PowerShell users with `iex` shorthand see "not installed" warning.

**Status:** Documented in tests.

---

### 10. TOCTOU in squash between reset and commit

**File:** `src/commands/step_commands.rs:285-302`

**Problem:** Between `git reset --soft` and `git commit`, another process could modify the staging area.

**User symptom:** Rare: squash commit may include unexpected changes if external process modifies staging during operation.

**Likelihood:** Very low - requires precise timing from external process.

---

### 11. Migration file path double-dot (cosmetic)

**File:** `src/config/deprecation.rs:477`

**Problem:** For config files without extensions:
- `no_extension` → `no_extension..new`
- `.hidden` → `.hidden..new`

**User symptom:** Slightly confusing migration file name, but works correctly.

---

### 12. `git_dir()` asymmetric canonicalization

**File:** `src/git/repository/working_tree.rs:146-156`

**Problem:** Only canonicalizes relative paths, not absolute. Creates inconsistency with `root()` which always canonicalizes.

**User symptom:** On macOS with symlinks, `is_linked()` could return wrong result in edge cases.

---

## Acceptable By Design

These issues were investigated but determined to be intentional design decisions:

### TOCTOU in wt merge cleanup

**File:** `src/commands/merge.rs:247-276`

Files can be added between `ensure_clean()` and `remove_worktree()`. This is handled correctly by using `force_worktree: false` - git's own check provides the safety net. If new files appear, removal fails with a clear error suggesting `wt remove --force`.

### Background removal 1-second race

**File:** `src/output/handlers.rs`

During the 1-second delay before background removal, commits could theoretically be added. The race window is extremely narrow and the design is intentional (timing workaround for shell cd).

### `worktree_at_path()` using normalize instead of canonicalize

**File:** `src/git/repository/worktrees.rs:77-89`

Uses lexical normalization instead of canonicalization because canonicalize fails on non-existent paths. The tradeoff is documented in code comments.

### Root-level repository panic

**File:** `src/git/repository/mod.rs:374`

Would panic for repos at filesystem root (`/.git`). Acceptable because no one creates repos at filesystem root.
