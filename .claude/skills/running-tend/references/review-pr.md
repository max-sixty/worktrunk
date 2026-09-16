# PR Review — Worktrunk Specifics

## Data-Loss Surface: Hold for Human Review

Worktrunk's worst failure is silently destroying a user's work. A change that
could cause that is not an agent's to merge: a force-flag bypass can read as
harmless and still discard committed work.

Hold a PR when its diff could make worktrunk destroy something it used to keep:
it adds a deletion, widens what an existing one can delete, or loosens a check
that gates one, such as the dirty-worktree check or the integration check that
lets `wt remove` delete a branch. Deletions include:

- `wt remove`, especially `-D` / `--force-delete` or `-f` / `--force`
- `git branch -D` / `-d`, `git worktree remove --force`
- `git reset --hard`, `git checkout -f`, `git clean` with `-f` / `-d` / `-x`
- `rm -rf`, `std::fs::remove_dir_all`, `std::fs::remove_file`
- shipped automation that runs the above: `plugins/*/hooks/hooks.json`,
  `hooks/hooks.json`, `hooks/wt.sh`, and skill or alias examples users copy

The hold is for what a user can't get back: a worktree, a repository, a branch,
uncommitted work, or a file worktrunk writes on their behalf (its own config and
state, an rc file, another tool's settings). A deletion reaches none of that when
everything under its target can be regenerated, or when it is confined to a
throwaway CI or development environment. The test is the contents rather than the
directory's age: `wt step promote` creates its staging directory and removes it
inside one operation, and in between the directory holds the user's only copy of
both worktrees' ignored files.

On a match:

1. Name the deletion in the review, and how the diff could make it destroy more.
2. Request review from @max-sixty.
3. Do not approve or authorize the merge, even if it looks acceptable.

## Review Criteria

**Idiomatic Rust and project conventions:**

- Does the code follow Rust idioms? (Iterator chains over manual loops, `?` over
  match-on-error, proper use of Option/Result, etc.)
- Are there unnecessary allocations, clones, or owned types where borrows would
  suffice?
- Does new code use `.expect()` or `.unwrap()` in functions returning `Result`?
  These should use `?` or `bail!` instead.

**Testing:**

- Do the tests follow the project's testing conventions (see tests/CLAUDE.md)?

**CLAUDE.md compliance:**

- Review the CLAUDE.md sections relevant to the changed code and flag
  deviations — code quality, error handling, command execution, data safety,
  system docstrings, etc.

**Documentation accuracy:**

When a PR changes behavior, check that related documentation still matches:

- Does `after_long_help` in `src/cli/mod.rs` and `src/cli/config.rs` still
  describe what the code does? (These are the primary sources for doc pages.)
- Do inline TOML comments in config examples match the actual behavior?
- If a new feature was added, does the relevant help text mention it?

**Duplication search patterns (Rust-specific):**

```bash
# For a new function, search for existing implementations
rg "fn detect.*provider|fn get.*platform|fn .*_provider" --type rust
# For code that iterates remotes and parses URLs
rg "all_remote_urls|remote_url|GitRemoteUrl::parse" --type rust
```

## Flake Tracking

When reporting flakes, use `worktrunk-bot` as the bot login for comment
deduplication:

```bash
LAST_COMMENT=$(gh issue view <issue-number> --json comments \
  --jq '[.comments[] | select(.author.login == "worktrunk-bot")] | last | {id: .url, createdAt: .createdAt}')
```
