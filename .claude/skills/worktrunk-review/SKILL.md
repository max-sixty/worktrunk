---
name: worktrunk-review
description: Reviews a pull request for idiomatic Rust, project conventions, and code quality. Use when asked to review a PR or when running as an automated PR reviewer.
argument-hint: "[PR number]"
---

# Worktrunk PR Review

Review a pull request to worktrunk, a Rust CLI tool for managing git worktrees.

**PR to review:** $ARGUMENTS

## Setup

Load these skills first:

1. `/reviewing-code` — systematic review checklist (design review, universal
   principles, completeness)
2. `/developing-rust` — Rust idioms and patterns

Then read CLAUDE.md (project root) to understand project-specific conventions.

## Instructions

1. Read the PR diff with `gh pr diff <number>`.
2. Read the changed files in full (not just the diff) to understand context.
3. Follow the `reviewing-code` skill's structure: design review first, then
   tactical checklist.

## What to review

**Idiomatic Rust and project conventions:**

- Does the code follow Rust idioms? (Iterator chains over manual loops, `?` over
  match-on-error, proper use of Option/Result, etc.)
- Does it follow the project's conventions documented in CLAUDE.md? (Cmd for
  shell commands, error handling with anyhow, accessor naming conventions, etc.)
- Are there unnecessary allocations, clones, or owned types where borrows would
  suffice?

**Code quality:**

- Is the code clear and well-structured?
- Are there simpler ways to express the same logic?
- Does it avoid unnecessary complexity, feature flags, or compatibility layers?

**Correctness:**

- Are there edge cases that aren't handled?
- Could the changes break existing functionality?
- Are error messages helpful and consistent with the project style?

**Testing:**

- Are the changes adequately tested?
- Do the tests follow the project's testing conventions (see tests/CLAUDE.md)?

## How to provide feedback

- Use inline comments for specific code issues.
- Use `gh pr comment` for a top-level summary.
- Be constructive and explain *why* something should change, not just *what*.
- Distinguish between suggestions (nice to have) and issues (should fix).
- Don't nitpick formatting — that's what linters are for.
