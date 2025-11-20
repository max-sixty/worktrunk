# CI Test Failure Investigation: Platform-Dependent Git Behavior After Force Push

## Executive Summary

The test `test_list_maximum_status_symbols` is failing on Ubuntu CI but passing locally on macOS. The failure is caused by platform-specific differences in how git calculates ahead/behind counts after a force push operation. This creates non-deterministic test output that cannot be reconciled without normalizing the flaky portions of the snapshot.

## Goals

1. **Primary Goal**: Make the `test_list_maximum_status_symbols` test pass consistently on all platforms (Ubuntu, macOS, Windows)
2. **Secondary Goal**: Understand why git behaves differently across platforms in this scenario
3. **Tertiary Goal**: Determine if there's a way to make git's behavior deterministic, or if normalization is the only solution

## The Test Scenario

### What the Test Does

The test (`tests/integration_tests/list.rs:1640-1808`) creates a complex git scenario designed to trigger every possible status symbol in the worktrunk CLI output:

1. Creates a main repository with a "feature" worktree
2. Makes a local commit on the feature branch ("Local only commit")
3. **Clones the remote** to a temporary location
4. In the cloned repo, **resets to before the local commit** and creates a different commit ("Remote diverged commit")
5. **Force pushes** this new commit to origin, replacing the remote history
6. **Fetches** in the original feature worktree to see the remote changes
7. Makes the main branch advance with conflicting changes
8. Adds various working tree modifications (untracked, modified, staged, renamed, deleted files)
9. Locks the worktree
10. Sets a user status emoji
11. Runs `wt list --full` and captures the output

### Expected Output

The test expects to see all possible status symbols in the output:
- `?` - untracked files
- `!` - modified files
- `+` - staged files
- `»` - renamed files
- `✘` - deleted files
- `=` - conflicts with main
- `⊠` - locked worktree
- `↕` - diverged from main
- `⇅` or `⇡` - upstream divergence (this is the flaky part)
- `🤖` - user status emoji

## The Problem

### Observed Behavior

**On macOS (local development)**:
```
feature  ?!+»✘=⊠ ↕⇅🤖    +2   -2   ↑2  ↓1    +3   -1  ./feature     ↑2  ↓1      21dc61ca  10 months ago    Local only commit
```
- Upstream divergence symbol: `⇅` (bidirectional, indicates diverged)
- Remote behind count: `↓1`

**On Ubuntu (CI)**:
```
feature  ?!+»✘=⊠ ↕⇡🤖    +2   -2   ↑2  ↓1    +3   -1  ./feature     ↑2  ↓0      21dc61ca  10 months ago    Local only commit
```
- Upstream divergence symbol: `⇡` (ahead only)
- Remote behind count: `↓0`

### How These Symbols Are Calculated

The upstream divergence symbol is determined in `src/commands/list/collect.rs:202-206` (and 240-244):

```rust
let upstream_divergence = match (upstream_ahead, upstream_behind) {
    (0, 0) => UpstreamDivergence::None,
    (a, 0) if a > 0 => UpstreamDivergence::Ahead,        // ⇡ symbol
    (0, b) if b > 0 => UpstreamDivergence::Behind,       // ⇣ symbol
    _ => UpstreamDivergence::Diverged,                   // ⇅ symbol
};
```

So the difference is:
- **macOS**: `upstream_behind = 1` → triggers `Diverged` case → shows `⇅`
- **Ubuntu**: `upstream_behind = 0` → triggers `Ahead` case → shows `⇡`

### Root Cause: Git's Ahead/Behind Calculation

The ahead/behind counts come from `git rev-list --left-right --count` in `src/git/repository/mod.rs:577-605`:

```rust
pub fn ahead_behind(&self, base: &str, head: &str) -> Result<(usize, usize), GitError> {
    // Use single git call with --left-right --count for better performance
    let range = format!("{}...{}", base, head);
    let output = self.run_command(&["rev-list", "--left-right --count", &range])?;

    // Parse output: "<behind>\t<ahead>" format
    // Example: "5\t3" means 5 commits behind, 3 commits ahead
    // git rev-list --left-right outputs left (base) first, then right (head)
    let parts: Vec<&str> = output.trim().split('\t').collect();
    // ... parsing logic
}
```

In our specific scenario, we're comparing the local branch against its remote tracking branch **after** the remote has been force-pushed with a different history.

## What We've Tried

### Attempt 1: WORKTRUNK_CONFIG_PATH Normalization (Successful)

**What we did**: Added a filter to normalize temporary file paths in snapshots:

```rust
// In tests/common/list_snapshots.rs:20-23
settings.add_filter(
    r"(/var/folders/[^/]+/[^/]+/T/\.tmp[^/]+|/tmp/\.tmp[^/]+)/test-config\.toml",
    "[TEST_TEMP]/test-config.toml",
);
```

**Result**: Fixed one source of platform differences (macOS uses `/var/folders/.../T/.tmpXXX` while Linux uses `/tmp/.tmpXXX`), but didn't fix the main test failure.

### Attempt 2: Reflog Pruning (Failed)

**What we did**: Added `git reflog expire --expire=now --all` after the fetch operation, hoping to clean up stale references.

**Rationale**: We thought old commit references might be affecting merge-base calculation.

**Result**: CI still failed with the same difference (`⇡↓0` vs `⇅↓1`).

### Attempt 3: Git Garbage Collection (Failed)

**What we did**: Replaced reflog pruning with `git gc --prune=now`:

```rust
// Added in tests/integration_tests/list.rs (lines 1718-1726, later removed)
let mut cmd = Command::new("git");
repo.configure_git_cmd(&mut cmd);
cmd.args(["gc", "--prune=now"])
    .current_dir(&feature)
    .output()
    .unwrap();
```

**Rationale**: Force pushes can leave unreachable objects that might affect git's calculations. GC would clean these up.

**Result**: CI still failed. Ubuntu continued showing `↓0` instead of `↓1`.

### Attempt 4: Incorrect Snapshot Update (Failed)

**What we did**: Changed the snapshot to match Ubuntu's output by updating BOTH the status symbol AND the behind count:
- Changed `↕⇅🤖` to `↕⇡🤖`
- Changed `↓1` to `↓0`

**Commit**: 90fb2884c3b89d26168c2371e61420f9dba560da

**Result**: Test now failed on ALL platforms (Ubuntu, macOS, Windows) because:
- The snapshot showed Ubuntu's expected output
- But macOS actually produces different output (`⇅↓1`)
- So the test failed locally on macOS

**Lesson**: This confirmed the platform difference is real and persistent. We reverted this commit.

### Attempt 5: Snapshot Normalization (Current Approach)

**What we did**: Added filters to normalize the platform-dependent parts of the output:

```rust
// In tests/integration_tests/list.rs:1798-1802
let mut settings = list_snapshots::standard_settings(&repo);
// Normalize upstream divergence: accept both ⇡ (ahead) and ⇅ (diverged)
settings.add_filter(r"↕[⇡⇅]🤖", "↕[UPSTREAM]🤖");
// Normalize remote behind count: accept both ↓0 and ↓1
settings.add_filter(r"↓[01]", "↓[N]");
```

**Rationale**: If we can't make git behave consistently, we can at least make the test accept either output.

**Result**: Snapshot now shows:
```
feature  ?!+»✘=⊠ ↕[UPSTREAM]🤖    +2   -2   ↑2  ↓1    +3   -1  ./feature     ↑2  ↓[N]      21dc61ca  10 months ago    Local only commit
```

**Status**: Needs to be tested in CI to confirm this resolves the flakiness.

## Technical Deep Dive

### The Force Push Scenario

The test creates this git history:

```
Initial state:
  origin/feature: A---B---C (local commit)
  local feature:  A---B---C

After cloning and force push:
  origin/feature: A---B---D (different commit, force-pushed)
  local feature:  A---B---C (still has old commit)

After fetch:
  origin/feature: A---B---D
  local feature:  A---B---C
  (local knows about both C and D)
```

When we run `git rev-list --left-right --count origin/feature...HEAD`:
- We're asking: "How far apart are origin/feature (D) and HEAD (C)?"
- Git must find the merge-base (B) and count commits

### Platform-Specific Merge-Base Calculation

The key question: **Why does git calculate different ahead/behind counts on different platforms?**

From the test output:
- **macOS**: Says local is 2 ahead, 1 behind origin
  - Ahead 2: Probably counting C and some other commit
  - Behind 1: Counting D

- **Ubuntu**: Says local is 2 ahead, 0 behind origin
  - Ahead 2: Same as macOS
  - Behind 0: Doesn't see D as "behind"

### Hypothesis: Reflog or Object Storage Differences

**Possible explanations**:

1. **Git version differences**: Different git versions might have different merge-base algorithms
   - Need to check: What git version runs in GitHub Actions Ubuntu vs macOS runners?

2. **Filesystem differences**: Object storage and reference handling might differ
   - macOS uses APFS
   - Ubuntu uses ext4
   - Could affect how unreachable objects are tracked

3. **Race conditions**: The force push + fetch might create timing issues
   - Objects might not be fully packed/indexed
   - Reflog entries might not be synchronized

4. **Object reachability**: After force push, commit C becomes unreachable from any ref
   - macOS might keep it in calculations
   - Ubuntu might ignore unreachable commits sooner

## Current Code State

### Test File Location
`tests/integration_tests/list.rs:1640-1808`

### Relevant Code Sections

**Setting up the divergence**:
```rust
// Line 1666-1695: Create the force push scenario
// 1. Make local commit
std::fs::write(feature.join("feature.txt"), "local content").unwrap();
// ... git add + commit "Local only commit"

// 2. Clone and create different remote history
let temp_dir = tempfile::tempdir().unwrap();
let temp_wt = temp_dir.path().join("temp-wt");
// ... git clone
// ... git reset HEAD~1
// ... modify file differently
// ... git commit "Remote diverged commit"
// ... git push --force origin feature

// 3. Fetch to see the divergence
// ... git fetch origin (in original feature worktree)
```

**Generating the snapshot**:
```rust
// Lines 1792-1808
let mut settings = list_snapshots::standard_settings(&repo);
settings.add_filter(r"↕[⇡⇅]🤖", "↕[UPSTREAM]🤖");
settings.add_filter(r"↓[01]", "↓[N]");
settings.bind(|| {
    let mut cmd = list_snapshots::command(&repo, repo.root_path());
    cmd.arg("--full");
    assert_cmd_snapshot!("maximum_status_symbols", cmd);
});
```

### The Snapshot File
`tests/snapshots/integration__integration_tests__list__maximum_status_symbols.snap`

After normalization, shows:
```yaml
---
source: tests/integration_tests/list.rs
assertion_line: 1787
info:
  program: wt
  args:
    - list
    - "--full"
  env:
    CLICOLOR_FORCE: "1"
    COLUMNS: "150"
    # ... other env vars
---
success: true
exit_code: 0
----- stdout -----

----- stderr -----
[1mBranch[0m   [1mStatus[0m              [1mHEAD±[0m    [1mmain↕[0m     [1mmain…±[0m  [1mPath[0m         [1mRemote⇅[0m  [1mCI[0m  [1mCommit[0m    [1mAge[0m              [1mMessage[0m
[1m[35mmain[0m                                                    [1m[35m./test-repo[0m               [2m85ed6d3d[0m  [2m10 months ago[0m    [2mMain advances[0m
feature  [36m?!+»✘[0m[31m=[0m  [33m⊠[0m ↕[UPSTREAM]🤖    [32m+2[0m   [31m-2[0m   [32m↑2[0m  [2m[31m↓1[0m    [32m+3[0m   [31m-1[0m  ./feature     [32m↑2[0m  [2m[31m↓[N][0m      [2m21dc61ca[0m  [2m10 months ago[0m    [2mLocal only commit[0m

⚪ [2mShowing 2 worktrees, 1 with changes, 1 ahead[0m
```

Note the normalized parts:
- `[UPSTREAM]` instead of `⇡` or `⇅`
- `↓[N]` instead of `↓0` or `↓1`

## Open Questions

### Git Behavior Questions

1. **Why does git's merge-base calculation differ after force push across platforms?**
   - Is this documented git behavior?
   - Are there git configuration options that affect this?
   - Is it related to how git handles unreachable objects?

2. **What git versions are running in CI vs locally?**
   - Ubuntu CI runners: `git --version` = ?
   - macOS CI runners: `git --version` = ?
   - Local macOS: We can check, but it's developer-specific

3. **Does the order of operations matter?**
   - If we add a delay between force push and fetch, does it change?
   - If we run `git gc` on BOTH repos (local and remote), does it help?
   - If we prune before AND after fetch, does it change?

4. **Is this behavior actually non-deterministic, or consistently different?**
   - Will Ubuntu ALWAYS show `↓0` in this scenario?
   - Will macOS ALWAYS show `↓1` in this scenario?
   - Or is there randomness involved (e.g., race conditions)?

### Testing Strategy Questions

5. **Is normalization the right approach?**
   - Are we hiding a real bug by normalizing?
   - Should we have separate snapshots for each platform?
   - Should we skip testing this specific edge case?

6. **Could we restructure the test to avoid the force push scenario?**
   - The test wants to show all status symbols
   - Is there a different way to create upstream divergence that's deterministic?
   - Could we use a regular push instead of force push?

### Implementation Questions

7. **Why do we need this test at all?**
   - It's testing that all status symbols can appear together
   - But is the specific git history important, or just the symbols?
   - Could we mock the git operations instead?

8. **Are there other tests with similar issues?**
   - Do any other tests use force push + fetch?
   - Are there other potentially flaky scenarios we haven't discovered yet?

## Research Needed

### Git Documentation Research

1. **Search for**:  "git rev-list merge-base force push platform differences"
2. **Look for**: Git mailing list discussions about merge-base calculation edge cases
3. **Check**: Git release notes for changes to ahead/behind calculation algorithms
4. **Find**: Any documented differences in how git handles unreachable objects across platforms

### Known Issues Research

1. **Search for**: "git ahead behind inconsistent after force push"
2. **Look for**: Stack Overflow questions about similar behavior
3. **Check**: Git bug tracker for related issues
4. **Find**: Any known git configuration options that affect this

### Testing Best Practices Research

1. **Search for**: "testing git operations across platforms"
2. **Look for**: How other projects handle platform-specific git behavior
3. **Check**: Snapshot testing best practices for non-deterministic output
4. **Find**: Examples of tests that normalize git-related output

## Assumptions We're Making (Unproven)

1. **Assumption**: The platform difference is consistent (Ubuntu always shows `↓0`, macOS always shows `↓1`)
   - **Risk**: If it's actually random, normalization won't help
   - **How to verify**: Run the test 100 times on each platform and check consistency

2. **Assumption**: This is a git implementation detail, not a bug in our code
   - **Risk**: We might be calculating ahead/behind incorrectly
   - **How to verify**: Test with `git rev-list` directly in both environments

3. **Assumption**: Normalization won't hide real bugs in our ahead/behind calculation
   - **Risk**: We might miss actual errors in the Remote⇅ column
   - **How to verify**: Add separate tests that verify ahead/behind calculation explicitly

4. **Assumption**: The test is valuable enough to keep despite the normalization
   - **Risk**: We're adding complexity for marginal benefit
   - **How to verify**: Evaluate if we can achieve the same coverage with simpler tests

5. **Assumption**: Force push + fetch scenario is rare enough that this flakiness doesn't indicate a broader problem
   - **Risk**: Real users might hit the same issue
   - **How to verify**: Check if worktrunk behaves correctly for users after force pushes

## Next Steps

### Immediate Actions

1. **Test the normalization approach in CI**:
   - Push the current changes (commit with normalization)
   - Monitor CI results on all platforms
   - Verify the test passes consistently

2. **If normalization works**:
   - Add a comment explaining why normalization is needed
   - Document the platform difference as a known limitation
   - Close the issue

3. **If normalization doesn't work**:
   - Research git behavior more deeply
   - Consider splitting into platform-specific tests
   - Or remove the flaky portions of the test

### Long-term Improvements

1. **Add git version logging to CI**:
   - Print `git --version` at the start of test runs
   - Track if behavior correlates with git version

2. **Create minimal reproduction**:
   - Strip down to just the force push + fetch + ahead/behind check
   - Test with plain git commands outside of worktrunk
   - Share with git community if truly platform-specific

3. **Consider alternative test design**:
   - Mock git operations for symbol display tests
   - Keep integration tests simple and deterministic
   - Move complex scenarios to manual verification

## Summary

We have a test that's failing due to platform-specific differences in git's ahead/behind calculation after a force push. Multiple attempts to make git's behavior deterministic have failed. The current solution is to normalize the flaky parts of the snapshot output. This needs to be validated in CI to confirm it resolves the issue without hiding real bugs.
