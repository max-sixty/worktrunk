//! Unified PR/MR reference resolution (`pr:<number>` and `mr:<number>` syntax).
//!
//! This module provides shared infrastructure for resolving GitHub PRs and GitLab MRs.
//! Platform-specific implementations live in `pr_ref` and `mr_ref` modules.
//!
//! # Syntax
//!
//! The `pr:<number>` and `mr:<number>` prefixes are unambiguous because colons are
//! invalid in git branch names (git rejects them as "not a valid branch name").
//!
//! ```text
//! wt switch pr:101          # Switch to branch for GitHub PR #101
//! wt switch mr:42           # Switch to branch for GitLab MR !42
//! wt switch pr:101 --yes    # Skip approval prompts
//! ```
//!
//! **Invalid usage:**
//!
//! ```text
//! wt switch --create pr:101   # Error: PR/MR branch already exists
//! wt switch --base main pr:101  # Error: base is predetermined
//! ```
//!
//! # Resolution Flow
//!
//! Both PR and MR resolution follow the same pattern:
//!
//! ```text
//! pr:101 / mr:42
//!   │
//!   ▼
//! ┌─────────────────────────────────────────────────────────┐
//! │ Fetch metadata via CLI (gh api / glab mr view)          │
//! │   → branch name, source/target repos, URLs              │
//! └─────────────────────────────────────────────────────────┘
//!   │
//!   ├─── Same-repo ───▶ Branch exists in primary remote
//!   │     │
//!   │     └─▶ Use directly, standard switch
//!   │
//!   └─── Fork/Cross-repo ───▶ Branch exists in fork
//!         │
//!         ├─▶ Find remote for base/target repo (where refs live)
//!         └─▶ Set up push to fork URL
//! ```
//!
//! # Fork PR/MR Handling
//!
//! ## The Problem: PR/MR Refs Are Read-Only
//!
//! Both GitHub's `refs/pull/<N>/head` and GitLab's `refs/merge-requests/<N>/head`
//! are **read-only** and cannot be pushed to. These are managed by the platform:
//!
//! ```text
//! $ git push origin HEAD:refs/pull/101/head
//! ! [remote rejected] HEAD -> refs/pull/101/head (deny updating a hidden ref)
//! ```
//!
//! The only way to update a fork PR/MR is to push directly to the fork's branch.
//!
//! ## Push Strategy (No Remote Required)
//!
//! Git's `branch.<name>.pushRemote` config accepts a URL directly, not just a
//! named remote. This means we can set up push tracking without adding remotes:
//!
//! ```text
//! branch.feature.remote = upstream
//! branch.feature.merge = refs/pull/101/head  # or refs/merge-requests/42/head
//! branch.feature.pushRemote = git@github.com:contributor/repo.git
//! ```
//!
//! This configuration gives us:
//! - `git pull` fetches from the base repo's PR/MR ref (stays up to date)
//! - `git push` pushes to the fork URL (updates the PR/MR)
//! - No stray remotes cluttering `git remote -v`
//!
//! ## Local Branch Naming
//!
//! **The local branch name must match the fork's branch name** for `git push`
//! to work. With `push.default = current` (the common default), git pushes to
//! a same-named branch on the pushRemote. If the names differ, push fails.
//!
//! This means two fork PRs/MRs with the same branch name would conflict. The
//! `branch_tracks_ref()` check handles this by erroring if a branch exists
//! but tracks a different PR/MR.
//!
//! # Error Handling
//!
//! ## Not Found
//!
//! ```text
//! ✗ PR #101 not found
//! ✗ MR !42 not found
//! ```
//!
//! ## CLI Not Authenticated
//!
//! ```text
//! ✗ GitHub CLI not authenticated; run gh auth login
//! ✗ GitLab CLI not authenticated; run glab auth login
//! ```
//!
//! ## CLI Not Installed
//!
//! ```text
//! ✗ GitHub CLI (gh) not installed; install from https://cli.github.com/
//! ✗ GitLab CLI (glab) not installed; install from https://gitlab.com/gitlab-org/cli
//! ```
//!
//! ## --create Conflict
//!
//! ```text
//! ✗ Cannot create branch for pr:101 — PR already has branch feature-auth
//! ↳ To switch to it: wt switch pr:101
//! ```
