# PR Review — Worktrunk Specifics

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
