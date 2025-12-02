+++
title = "Concepts"
weight = 3
+++

## Why git worktrees?

When working with multiple AI agents (or multiple tasks), you have a few options:

| Approach | Pros | Cons |
|----------|------|------|
| **One working tree, many branches** | Simple setup | Agents step on each other, can't use git for staging/committing |
| **Multiple clones** | Full isolation | Slow to set up, drift out of sync |
| **Git worktrees** | Isolation + shared history | Requires management |

Git worktrees give you multiple directories backed by a single `.git` directory. Each worktree has its own branch and working tree, but shares the repository history and refs.

## Why Worktrunk?

Git's built-in `worktree` commands require remembering worktree locations and composing git + `cd` commands. Worktrunk bundles creation, navigation, status, and cleanup into simple commands.

### Comparison

| Task | Worktrunk | Plain git |
|------|-----------|-----------|
| Switch worktrees | `wt switch feature` | `cd ../repo.feature` |
| Create + start Claude | `wt switch -c -x claude feature` | `git worktree add -b feature ../repo.feature main && cd ../repo.feature && claude` |
| Clean up | `wt remove` | `cd ../repo && git worktree remove ../repo.feature && git branch -d feature` |
| List | `wt list` (with diffstats & status) | `git worktree list` (just names & paths) |
| List with CI status | `wt list --full` | N/A |

### Local merging with `wt merge`

`wt merge` handles the full merge workflow: stage, commit, squash, rebase, merge, cleanup. Includes LLM commit messages, project hooks, and flags for skipping steps.

| Task | Worktrunk | Plain git |
|------|-----------|-----------|
| Merge + clean up | `wt merge` | `git add -A && git reset --soft $(git merge-base HEAD main) && git diff --staged \| llm "..." \| git commit -F - && git rebase main && cargo test && cd ../repo && git merge --ff-only feature && git worktree remove ../repo.feature && git branch -d feature && cargo install --path .` |

<!-- ⚠️ AUTO-GENERATED-HTML from tests/snapshots/integration__integration_tests__merge__readme_example_complex.snap — edit source to update -->

{% terminal() %}
<span class="prompt">$</span> wt merge
🔄 <span style='color:var(--cyan,#0aa)'>Squashing 3 commits into a single commit <span style='color:var(--bright-black,#555)'>(3 files, <span style='color:var(--green,#0a0)'>+33</span></span></span><span style='color:var(--bright-black,#555)'>)</span>...
🔄 <span style='color:var(--cyan,#0aa)'>Generating squash commit message...</span>
<span style='background:var(--bright-white,#fff)'> </span>  <b>feat(auth): Implement JWT authentication system</b>
<span style='background:var(--bright-white,#fff)'> </span>
<span style='background:var(--bright-white,#fff)'> </span>  Add comprehensive JWT token handling including validation, refresh logic,
<span style='background:var(--bright-white,#fff)'> </span>  and authentication tests. This establishes the foundation for secure
<span style='background:var(--bright-white,#fff)'> </span>  API authentication.
<span style='background:var(--bright-white,#fff)'> </span>
<span style='background:var(--bright-white,#fff)'> </span>  - Implement token refresh mechanism with expiry handling
<span style='background:var(--bright-white,#fff)'> </span>  - Add JWT encoding/decoding with signature verification
<span style='background:var(--bright-white,#fff)'> </span>  - Create test suite covering all authentication flows
✅ <span style='color:var(--green,#0a0)'>Squashed @ <span style='opacity:0.67'>95c3316</span></span>
🔄 <span style='color:var(--cyan,#0aa)'>Running pre-merge <b>test</b>:</span>
<span style='background:var(--bright-white,#fff)'> </span>  <span style='opacity:0.67'><span style='color:var(--blue,#00a)'>cargo</span></span><span style='opacity:0.67'> test</span>
    Finished test [unoptimized + debuginfo] target(s) in 0.12s
     Running unittests src/lib.rs (target/debug/deps/worktrunk-abc123)

running 18 tests
test auth::tests::test_jwt_decode ... ok
test auth::tests::test_jwt_encode ... ok
test auth::tests::test_token_refresh ... ok
test auth::tests::test_token_validation ... ok

test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s
🔄 <span style='color:var(--cyan,#0aa)'>Running pre-merge <b>lint</b>:</span>
<span style='background:var(--bright-white,#fff)'> </span>  <span style='opacity:0.67'><span style='color:var(--blue,#00a)'>cargo</span></span><span style='opacity:0.67'> clippy</span>
    Checking worktrunk v0.1.0
    Finished dev [unoptimized + debuginfo] target(s) in 1.23s
🔄 <span style='color:var(--cyan,#0aa)'>Merging 1 commit to <b>main</b> @ <span style='opacity:0.67'>95c3316</span> (no rebase needed)</span>
<span style='background:var(--bright-white,#fff)'> </span>  * <span style='color:var(--yellow,#a60)'>95c3316</span> feat(auth): Implement JWT authentication system
<span style='background:var(--bright-white,#fff)'> </span>   auth.rs      |  8 <span style='color:var(--green,#0a0)'>++++++++</span>
<span style='background:var(--bright-white,#fff)'> </span>   auth_test.rs | 17 <span style='color:var(--green,#0a0)'>+++++++++++++++++</span>
<span style='background:var(--bright-white,#fff)'> </span>   jwt.rs       |  8 <span style='color:var(--green,#0a0)'>++++++++</span>
<span style='background:var(--bright-white,#fff)'> </span>   3 files changed, 33 insertions(+)
✅ <span style='color:var(--green,#0a0)'>Merged to <b>main</b> <span style='color:var(--bright-black,#555)'>(1 commit, 3 files, +33</span></span><span style='color:var(--bright-black,#555)'>)</span>
🔄 <span style='color:var(--cyan,#0aa)'>Removing <b>feature-auth</b> worktree &amp; branch in background (already in main)</span>
🔄 <span style='color:var(--cyan,#0aa)'>Running post-merge <b>install</b>:</span>
<span style='background:var(--bright-white,#fff)'> </span>  <span style='opacity:0.67'><span style='color:var(--blue,#00a)'>cargo</span></span><span style='opacity:0.67'> install </span><span style='opacity:0.67'><span style='color:var(--cyan,#0aa)'>--path</span></span><span style='opacity:0.67'> .</span>
  Installing worktrunk v0.1.0
   Compiling worktrunk v0.1.0
    Finished release [optimized] target(s) in 2.34s
  Installing ~/.cargo/bin/wt
   Installed package `worktrunk v0.1.0` (executable `wt`)
{% end %}

<!-- END AUTO-GENERATED -->

### What Worktrunk adds

- **Branch-based navigation**: Address worktrees by branch name, not path
- **Consistent directory naming**: Predictable locations for all worktrees
- **Lifecycle hooks**: Run commands on create, start, pre-merge, post-merge
- **Unified status**: See changes, commits, CI status across all worktrees
- **Safe cleanup**: Validates changes are merged before deleting branches

## Worktree addressing

Worktrunk uses **path-first lookup** when resolving arguments:

1. Compute the expected path for the argument (using the configured path template)
2. If a worktree exists at that path, use it (regardless of what branch it's on)
3. Otherwise, treat the argument as a branch name

This means `wt switch foo` will switch to `repo.foo/` even if that worktree is on a different branch.
