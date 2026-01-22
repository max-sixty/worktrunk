# `wt step relocate` Specification

Move worktrees to their expected paths based on the `worktree-path` template.

## Flags

| Flag | Purpose |
|------|---------|
| `--dry-run` | Show what would be moved without moving |
| `--commit` | Auto-commit dirty worktrees with LLM-generated messages before relocating |
| `--clobber` | Move non-worktree paths out of the way (to `<path>.bak-<timestamp>`) |
| `[branches...]` | Specific branches to relocate (default: all mismatched) |

## Invariant

**`--commit --clobber` should never fail** (except for truly unrecoverable errors like disk full, permissions denied).

## Failure Cases

The command should **only skip/fail** when:

| Condition | Without Flag | With Flag |
|-----------|--------------|-----------|
| Dirty worktree | Skip with warning | `--commit`: auto-commit, then move |
| Locked worktree | Skip with warning | User must `git worktree unlock` manually |
| Non-worktree at target | Skip with warning | `--clobber`: move to `<path>.bak-<timestamp>` |
| Main worktree with non-default branch | Special handling | Create new wt, switch main to default |

**Swap/cycle scenarios are NOT failure cases** — they must be handled automatically.

## Target Classification

For each mismatched worktree, classify its target path:

| Classification | Description | Action |
|----------------|-------------|--------|
| `Empty` | Target doesn't exist | Move directly |
| `Worktree` | Target is another worktree we're relocating | Coordinate via dependency graph |
| `Blocked` | Target exists but is NOT a worktree we're moving | Skip or clobber |

## Algorithm

```
RELOCATE(worktrees, commit, clobber):
    # Phase 1: Gather candidates
    mismatched = []
    for wt in worktrees:
        expected = compute_expected_path(wt.branch)
        if wt.path != expected:
            mismatched.append((wt, expected))

    if empty(mismatched):
        print "All worktrees at expected paths"
        return

    # Phase 2: Pre-check and handle blockers
    for (wt, expected) in mismatched:
        if wt.locked:
            skip(wt, "locked")
            continue

        if wt.dirty:
            if commit:
                auto_commit(wt)
            else:
                skip(wt, "dirty — use --commit")
                continue

        target_status = classify_target(expected, mismatched)

        if target_status == Blocked:
            if clobber:
                move_to_backup(expected)  # → expected.bak-<timestamp>
            else:
                skip(wt, "target exists — use --clobber")
                continue

    # Phase 3: Build dependency graph
    # Edge A→B means "A's target is currently occupied by worktree B"
    graph = build_dependency_graph(remaining_mismatched)

    # Phase 4: Process in topological order
    while graph has nodes:
        # Find nodes with no blockers (target is empty or already moved)
        ready = nodes where target is empty

        if ready is empty:
            # Cycle detected — break it with temp location
            cycle_node = pick_any(graph.nodes)
            move_to_temp(cycle_node)
            continue

        for node in ready:
            move_worktree(node.source, node.target)
            remove_from_graph(node)

    # Phase 5: Move any temp-relocated worktrees to final locations
    for wt in temp_relocated:
        move_worktree(wt.temp_path, wt.final_path)
```

## Scenarios

### Simple Mismatch
```
Before:  feature @ ~/wrong-location
After:   feature @ ~/repo.feature
```
Direct move — target is empty.

### Swap (2-cycle)
```
Before:
  alpha @ repo.beta    (wants repo.alpha)
  beta  @ repo.alpha   (wants repo.beta)

Algorithm:
  1. Build graph: alpha→beta, beta→alpha (cycle)
  2. No ready nodes — break cycle
  3. Move alpha → .wt-relocate-temp/alpha
  4. Now beta's target (repo.beta) is empty
  5. Move beta → repo.beta
  6. Move alpha from temp → repo.alpha

After:
  alpha @ repo.alpha ✓
  beta  @ repo.beta  ✓
```

### Chain (3-cycle)
```
Before:
  A @ repo.B   (wants repo.A)
  B @ repo.C   (wants repo.B)
  C @ repo.A   (wants repo.C)

Algorithm:
  1. Build graph: A→B→C→A (cycle)
  2. No ready nodes — break cycle by moving A to temp
  3. Now C's target (repo.A) is empty → move C
  4. Now B's target (repo.C) is empty → move B
  5. Move A from temp → repo.A

After:
  A @ repo.A ✓
  B @ repo.B ✓
  C @ repo.C ✓
```

### Mixed: Some Ready, Some Cyclic
```
Before:
  feature @ wrong-path     (wants repo.feature)     # target empty
  alpha   @ repo.beta      (wants repo.alpha)       # cycle with beta
  beta    @ repo.alpha     (wants repo.beta)        # cycle with alpha

Algorithm:
  1. feature's target is empty → move immediately
  2. alpha↔beta cycle → break with temp
  3. Move alpha → temp
  4. Move beta → repo.beta
  5. Move alpha → repo.alpha
```

### Clobber: Non-worktree at Target
```
Before:
  feature @ wrong-path
  repo.feature exists as regular directory (not a worktree)

Without --clobber:
  ▲ Skipping feature (target exists: ~/repo.feature)

With --clobber:
  1. Move ~/repo.feature → ~/repo.feature.bak-20250121-143022
  2. Move feature → ~/repo.feature
  ✓ Relocated feature (backed up existing ~/repo.feature)
```

### Main Worktree
```
Before:
  main worktree (repo root) has branch "feature" checked out

Algorithm:
  1. Cannot use `git worktree move` on main worktree
  2. Create new worktree: git worktree add repo.feature feature
  3. Switch main to default: git checkout main
  4. Result: feature now at repo.feature, main worktree on main branch
```

## Temp Location

Use `.git/wt-relocate-tmp/` inside the main worktree's git directory:
- Guaranteed to be on same filesystem (for atomic moves)
- Inside .git so not visible to user
- Cleaned up after successful relocation

## Output

```
# Dry run
◎ Would relocate alpha: ~/repo.beta → ~/repo.alpha
◎ Would relocate beta: ~/repo.alpha → ~/repo.beta
○ Would relocate 2 worktrees (dry run)

# Actual run with swap
◎ Relocating alpha to temporary location...
✓ Relocated beta: ~/repo.alpha → ~/repo.beta
✓ Relocated alpha: ~/repo.beta → ~/repo.alpha

✓ Relocated 2 worktrees

# With clobber
◎ Backing up ~/repo.feature → ~/repo.feature.bak-20250121-143022
✓ Relocated feature: ~/wrong-path → ~/repo.feature

✓ Relocated 1 worktree
```

## Implementation Notes

1. **Dependency graph**: Use `HashMap<PathBuf, Branch>` to map target paths to the worktree currently there
2. **Cycle detection**: Standard graph algorithm — if no nodes have in-degree 0, there's a cycle
3. **Temp location**: Create inside `.git/wt-relocate-tmp/` to ensure same filesystem
4. **Backup naming**: `<original>.bak-<YYYYMMDD-HHMMSS>` for uniqueness
5. **Atomicity**: Each move is atomic (`git worktree move`), but the overall operation is not — if interrupted, some worktrees may be in temp. Recovery: re-run `wt step relocate`

## Test Cases

1. `test_relocate_no_mismatches` — all at correct locations
2. `test_relocate_single_mismatch` — simple case, target empty
3. `test_relocate_swap` — 2-cycle, both relocated successfully
4. `test_relocate_chain` — 3-cycle
5. `test_relocate_mixed_ready_and_cycle` — some ready, some cyclic
6. `test_relocate_clobber_directory` — non-worktree at target, --clobber moves it
7. `test_relocate_clobber_file` — file at target
8. `test_relocate_no_clobber_skips` — without --clobber, skips blocked
9. `test_relocate_dirty_without_commit` — skips dirty
10. `test_relocate_dirty_with_commit` — commits then moves
11. `test_relocate_locked_worktree` — always skipped
12. `test_relocate_main_worktree` — create + switch
13. `test_relocate_dry_run` — shows plan without executing
14. `test_relocate_specific_branches` — only relocate named branches
15. `test_relocate_commit_clobber_never_fails` — comprehensive test with all edge cases
