---
name: release
description: Worktrunk release workflow. Use when user asks to "do a release", "release a new version", "cut a release", or wants to publish a new version to crates.io and GitHub.
metadata:
  internal: true
---

# Release Workflow

## Steps

1. **Sync the release branch with `main`**: The release worktree's branch lags `main` between releases — it's only reset to `main` after a release lands. Cutting from a stale branch silently drops everything merged since. Fast-forward to the tip of `main` before anything else:
   ```bash
   git fetch origin
   git merge --ff-only origin/main
   ```
   `--ff-only` advances the branch when it's a strict ancestor of `origin/main` and **fails** (rather than creating a merge commit or discarding work) if it has diverged — reconcile manually before continuing. This is the release-branch equivalent of `wt up`, spelled out because the `up` alias rebases each branch onto its own upstream (`origin/release`), not `main`.
2. **Run tests**: `cargo run -- hook pre-merge --yes`
3. **Check current version**: Read `version` in `Cargo.toml`
4. **Review commits**: Check commits since last release to understand scope of changes. Audit the cumulative diff for the data-loss surface (see [Data-Loss Surface Review](#data-loss-surface-review)) before proceeding.
5. **Check library API compatibility**: Run `cargo semver-checks check-release -p worktrunk` (install with `cargo install cargo-semver-checks --locked` if missing). If it reports breaking changes, the bump must be minor (pre-1.0) or major (post-1.0). See "Library API Compatibility" below.
6. **Credit contributors**: Check for external PR authors and issue reporters (see "Credit External Contributors" and "Credit Issue Reporters" below)
7. **Determine release type**: Pick the bump from the changes (including semver-checks result). Ask the user only if the choice is genuinely ambiguous (see below).
8. **Bump version** (must run on a clean tree — before editing CHANGELOG):
   ```bash
   cargo release X.Y.Z -p worktrunk -x --no-publish --no-push --no-tag --no-verify --no-confirm && cargo check
   ```
   This bumps `Cargo.toml` and `Cargo.lock`, then auto-commits. We'll reset this commit in step 10 to fold in the CHANGELOG.
9. **Update CHANGELOG**: Add `## X.Y.Z` section at top with changes (see MANDATORY verification below)
10. **Commit**: Reset the auto-commit from step 8, stage everything, and create the final release commit:
    ```bash
    git reset --soft HEAD~1 && git add -A && git commit -m "Release vX.Y.Z"
    ```
11. **Re-check `main` for drift, then merge**: A long release session lets `main` advance after the changelog was written. `/gpk` squash-merges onto current `main`, so the *code* stays complete, but the changelog silently misses anything that landed in between (a closely-related change is the easy miss — e.g. a follow-up that reworks a feature this release already documents). Before merging, re-fetch and diff against the `main` tip you reviewed in step 4:
    ```bash
    git fetch origin
    git log <step-4-main-tip>..origin/main --oneline   # commits that landed since the changelog was written
    ```
    Fold any user-facing commit into the CHANGELOG and amend the release commit before continuing. Then `/gpk` — opens a PR, waits for CI, merges via PR (preserves worktree).
12. **Tag the merge commit and push**: After `/gpk` squash-merges, the local branch HEAD is not the commit on main. Tag the PR's merge commit explicitly so the tag is reachable from main:
    ```bash
    MERGE_SHA=$(gh pr view --json mergeCommit --jq '.mergeCommit.oid')
    git tag vX.Y.Z "$MERGE_SHA" && git push origin vX.Y.Z
    ```
13. **Wait for the release workflow**: The tag push triggers `release.yaml`. Launch a ci-reporter agent to monitor the run through to completion (avoid `gh run watch` — it can hang); the run ID comes from:
    ```bash
    gh run list --workflow=release.yaml --event=push --branch=vX.Y.Z --limit 1 --json databaseId --jq '.[0].databaseId'
    ```

`release.yaml` builds binaries and publishes to crates.io, Homebrew, and winget automatically.

## Data-Loss Surface Review

Worktrunk's worst failure is silently destroying a user's work. The per-PR review (`running-tend`, "Data-Loss Surface") is the first gate; the release is the second, where the whole diff since the last release is visible at once.

This review optimizes for recall: find every change that could touch the deletion surface, accept false positives, then adjudicate each candidate. Missing one real loss path costs far more than reviewing a false alarm.

A keyword grep alone is insufficient. It finds only what someone thought to pattern-match, and an agent handed the grep anchors on it and inherits its blind spots. So fan out independent finders, most of them without the grep, and analyze every candidate they surface.

### Find: independent finders, recall over precision

Launch 3-5 finder subagents in parallel over the cumulative diff (`git log v<last-version>..HEAD -p`) and the code it touches, including anything that calls into or is called by the changed code. Each works independently: do not let them share findings during this phase, and do not collapse them onto one method. Each returns candidate locations with a one-line reason, erring toward over-reporting.

Give each a distinct charter. At least two receive no grep and no keyword list, so they reason from the code instead of pattern-matching:

- **Behavioral (no grep):** read each change and ask what it could destroy: files, branches, worktrees, uncommitted or untracked work, committed-but-unpushed commits. Flag any change whose worst case is lost user data.
- **Blast-radius (no grep):** for every function and file the diff touches, trace callers and callees. Flag any path that can now reach a destructive primitive (branch deletion, worktree removal, filesystem removal, history rewrite), even when that call sits outside the diff.
- **Automation diff (no grep):** compare shipped automation before and after: `plugins/*/hooks/hooks.json`, `hooks/hooks.json`, `hooks/wt.sh`, and skill or alias examples users copy. Flag any new or altered invocation that removes or force-overwrites anything.
- **Keyword scan (grep):** one finder runs the pattern filter over the full diff as a cross-check, never as the primary method. It takes no pathspec; a path allowlist would recreate the blind spots the no-grep finders exist to avoid (the densest destructive code, the trash sweep in `src/commands/process.rs`, sits outside `src/git/`).
  ```bash
  git log v<last-version>..HEAD -p \
    | grep -nE -- '--force-delete|--force| -D| -f |branch -[dD]|worktree remove|reset --hard|checkout -f|clean -[fdx]|remove_dir_all|remove_file|rm -rf'
  ```

### Analyze: adjudicate each candidate

Pool the candidates, dedupe, and analyze each against the data-safety invariants in `CLAUDE.md` and the FAQ "What can Worktrunk delete?" inventory: does it preserve data on failure, require explicit consent for destructive ops, and avoid silent side-effect deletion? Mark each real risk / acceptable / needs change, with the reasoning.

Surface the full adjudicated list and get explicit sign-off before tagging. Do not tag a release with an unresolved deletion-surface candidate, even if it looks acceptable.

## CHANGELOG Review

Check commits since last release for missing entries:

```bash
git log v<last-version>..HEAD --oneline
```

**IMPORTANT: Don't trust commit messages.** Commit messages often undersell or misdescribe changes. For any commit that might be user-facing:

1. Run `git show <commit> --stat` to see what files changed
2. If it touches user-facing code (commands, CLI, output), read the actual diff
3. Look for changes bundled together — a "rename flag" commit might also add new features

Common patterns where commit messages mislead:
- "Refactor X" commits that also change behavior
- "Rename flag" commits that add new functionality
- "Fix Y" commits that also improve error messages or add hints
- CI/test commits that include production code fixes

Notable changes to document:
- New features or commands
- User-visible behavior changes
- Bug fixes users might encounter

**Section order:** Improved, Fixed, Documentation, Internal. Documentation is for help text, web docs, and terminology improvements. Internal is for selected notable internal changes (not everything).

**Within each section, order by impact:**
1. Breaking/behavior changes (affect existing users' workflows)
2. New user-facing features and commands
3. Performance improvements users will notice
4. Minor enhancements and display changes
5. Niche/platform-specific improvements (Nix, Windows-only, etc.)
6. Developer/internal tooling exposed to users

**Breaking changes:** Note inline with the entry, not as a separate section:

```markdown
- **Feature name**: Description. (Breaking: old behavior no longer supported)
```

Skip: internal refactors, test additions (unless user-facing like shell completion tests).

### Length and tone

**Combine related bullets.** Several PRs that share a theme — e.g. three perf changes that together account for one user-visible speedup — belong in one bullet, not three. The reader cares about the net change, not the PR boundaries. Cite all the PRs in the trailing `([#a](...), [#b](...), [#c](...))` list.

**Be brief.** Each bullet should communicate the user-visible change in 1–3 sentences. Internal-section bullets in particular should be terse — usually one sentence. Drop the "why we did it this way" details unless they materially affect how the user thinks about the change. Code examples and exhaustive `Cmd::stream` / `OnceCell` / `DashMap`-style internals usually don't belong; they live in the PR description.

**No editorial framing.** Describe what changed, not what was wrong with the previous decision in subjective terms. Avoid words like "sledgehammer", "ugly", "noisy", "wrong" applied to past code. State the prior behavior neutrally and the new behavior plainly.

**Good:** "Removed `.pi/` from the default excludes list; users who need it can add it via `[step.copy-ignored]`."
**Bad:** "Removed `.pi/` — a sledgehammer fix from an unrelated debugging session that has no place as a project-agnostic default."

### Credit External Contributors

For any changelog entry where an external contributor (not the repo owner) authored the commit, add credit with their GitHub username:

```markdown
- **Feature name**: Description. ([#123](https://github.com/user/repo/pull/123), thanks @contributor)
```

Find external contributors:
```bash
git log v<last-version>..HEAD --format="%an <%ae>" | sort -u
```

Then for each external contributor's commit, find their GitHub username from the commit (usually in the email or PR).

### Credit Issue Reporters

When a fix or feature addresses a user-reported issue *in this repo*, thank the reporter — not just the PR author. Users who take time to report bugs, request features, or provide reproduction steps deserve recognition. (Don't credit reporters from upstream/external repos — only issues filed here.)

```markdown
- **Feature name**: Description. ([#456](https://github.com/user/repo/pull/456), thanks @reporter for reporting)
```

For fixes that reference issues:

```markdown
- **Bug fix**: Description. Fixes [#123](https://github.com/user/repo/issues/123). (thanks @reporter)
```

**Finding reporters — do ALL three steps:**

Issues may have been filed months before the fix. Bug reports also appear as PR comments, not just issues. These steps are complementary; each catches things the others miss.

1. **Extract every issue/PR reference from every commit** (PRIMARY):
   ```bash
   git log v<last-version>..HEAD --format="%B" | grep -oE '#[0-9]+' | sort -un
   ```
   For **each** referenced number: run `gh issue view N --json title,author,state`. This catches issues filed months ago — the most commonly missed credits.

2. **Check PR comments for bug reports** (catches reports that never became issues):
   For feature PRs referenced in commits, check comment threads for users reporting problems:
   ```bash
   gh pr view NNN --json comments --jq '.comments[] | "\(.author.login): \(.body[:150])"'
   ```

3. **Survey every issue opened or closed since last release** (catches unreferenced matches):
   ```bash
   git log -1 --format=%cs v<last-version>
   gh issue list --state all --search "created:>=<date>" --json number,title,author --limit 100
   gh issue list --state closed --search "closed:>=<date>" --json number,title,author --limit 100
   ```
   Cross-reference every title against changes in this release.

**When to credit:**
- Bug reports with clear reproduction steps (in issues OR PR comments)
- Feature requests that shaped the implementation
- Performance reports with measurements (like "takes 15s")
- Users who helped diagnose issues through discussion

Skip credit for: issues opened by the repo owner, trivial reports, or issues that were substantially different from what was implemented.

### Link Significant Features to Docs

For major features with dedicated documentation, include a docs link. Use full URLs so links work from GitHub releases:

```markdown
- **Hook system**: Shell commands that run at key points in worktree lifecycle. [Docs](https://worktrunk.dev/hook/) ([#234](https://github.com/user/repo/pull/234), thanks @contributor for the suggestion)
```

Link when there's substantial documentation the user would benefit from reading — new commands, feature pages, or Tips & Patterns sections. Skip for minor improvements.

### MANDATORY: Verify Each Changelog Entry

**After drafting changelog entries, you MUST spawn a subagent to verify each bullet point is accurate.** This is non-negotiable — changelog mistakes are a recurring problem.

The subagent should:
1. Take the list of drafted changelog entries
2. For each entry, find the commit(s) it describes and read the actual diff
3. Verify the entry accurately describes what changed
4. Check for missing changes that should be documented
5. Report any inaccuracies or omissions

**Subagent prompt template:**

```
Verify these changelog entries for version X.Y.Z are accurate.

Previous version: [e.g., v0.1.9]
Commits to check: git log v<previous>..HEAD

Entries to verify:
[paste drafted entries]

For EACH entry:
1. Find the relevant commit(s) using git log and git show
2. Read the actual diff, not just the commit message
3. Confirm the entry accurately describes the user-facing change
4. Flag if the entry overstates, understates, or misdescribes the change

Also check:
- Are there user-facing changes NOT covered by these entries?
- Verify each "thanks @..." attribution (right person, right role — author vs reporter)

Report format:
- Entry: [entry text]
  Status: ✅ Accurate / ⚠️ Needs revision / ❌ Incorrect
  Evidence: [what you found in the diff]
  Suggested fix: [if needed]
```

**Do not finalize the changelog until the subagent confirms all entries are accurate.**

**If verification finds problems:** Escalate to the user. Show them the subagent's findings and ask how to proceed. Don't attempt to resolve ambiguous changelog entries autonomously — the user knows the intent behind their changes better than you do.

## Determine Release Type

Pick the bump from the changes. Skip the question when only one level is plausible; ask only when the choice is genuinely ambiguous.

**Proceed without asking when the answer is obvious:**

- `cargo semver-checks` reports breaking changes → minor (pre-1.0; patch is disallowed, major has no basis pre-1.0 in a maturing project). Inform the user and continue: "Bumping minor — semver-checks reported N breaking changes; patch is disallowed pre-1.0."
- Only bug fixes and internal commits since the last release, no new features, no semver breakage → patch. Inform the user with the same shape.

**Ask only when genuinely ambiguous** — e.g., new features landed with no semver breakage (patch vs. minor is a judgment call), or a borderline case where the user's read of the changes may differ. Use `AskUserQuestion` with the actual choice (patch vs. minor); don't add novelty options like skipping a version number.

When asking, present:
1. Current version (e.g., `0.2.0`)
2. Brief summary of changes (new features, bug fixes, breaking changes)
3. Your recommendation with reasoning

Example:

```
Current version: 0.2.0
Changes since v0.2.0:
- Added `state clear` command (new feature)
- Added `previous-branch` state key (new feature)
- No breaking changes

Recommendation: Minor release (0.3.0) — new features, no breaking changes
```

## Version Guidelines

- **Second digit** (0.1.0 → 0.2.0): Backward incompatible changes
- **Third digit** (0.1.0 → 0.1.1): Everything else

Current project status: early release, breaking changes acceptable, optimize for best solution over compatibility.

## Library API Compatibility

Worktrunk is primarily a CLI, but it also publishes a library crate (`[lib]` in `Cargo.toml`) that downstream crates depend on. `cargo-semver-checks` compares the current public API against the last version published to crates.io and flags semver violations.

```bash
cargo semver-checks check-release -p worktrunk
```

Interpreting results:

- **No issues reported**: any bump level is valid from the library's perspective. Choose based on CLI changes and new features.
- **Breaking changes reported**: while pre-1.0, these require at minimum a minor bump (e.g., 0.37.0 → 0.38.0). A patch release is not allowed.
- **Tool fails to run** (e.g., missing baseline): likely the crate hasn't been published yet or the registry cache is stale. Try `cargo semver-checks check-release -p worktrunk --baseline-version <last-published>`.

This check validates the chosen bump — it doesn't distinguish patch vs. minor when no breakage exists. Continue using the commit review to decide between patch (fixes only) and minor (new features).
