---
name: review-pr
description: Reviews a pull request for idiomatic Rust, project conventions, and code quality. Use when asked to review a PR or when running as an automated PR reviewer.
argument-hint: "[PR number]"
metadata:
  internal: true
---

# Worktrunk PR Review

Load `/cd-review` for the generic review workflow (pre-flight, posting
mechanics, inline suggestions, thread resolution, CI monitoring). This file
adds worktrunk-specific review criteria.

Review a pull request to worktrunk, a Rust CLI tool for managing git worktrees.

**PR to review:** $ARGUMENTS

## Worktrunk Review Criteria

In addition to the generic review checks, evaluate:

**Idiomatic Rust and project conventions:**

- Does the code follow Rust idioms? (Iterator chains over manual loops, `?` over
  match-on-error, proper use of Option/Result, etc.)
- Does it follow the project's conventions documented in CLAUDE.md? (Cmd for
  shell commands, error handling with anyhow, accessor naming conventions, etc.)
- Are there unnecessary allocations, clones, or owned types where borrows would
  suffice?
- Does new code use `.expect()` or `.unwrap()` in functions returning `Result`?
  These should use `?` or `bail!` instead.

**Testing:**

- Do the tests follow the project's testing conventions (see tests/CLAUDE.md)?

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

## CI Monitoring

After approving or staying silent, monitor CI using `/running-in-ci` (includes
worktrunk-specific codecov/patch polling).

## Flake Tracking

When reporting flakes, use `worktrunk-bot` as the bot login for comment
deduplication:

```bash
LAST_COMMENT=$(gh issue view <issue-number> --json comments \
  --jq '[.comments[] | select(.author.login == "worktrunk-bot")] | last | {id: .url, createdAt: .createdAt}')
```

## Token Assignment

All actions use `WORKTRUNK_BOT_TOKEN` for consistent identity. The merge
restriction (ruleset) is the security boundary, not token scoping.
