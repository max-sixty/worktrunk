---
name: release
description: Worktrunk release workflow. Use when user asks to "do a release", "release a new version", "cut a release", or wants to publish a new version to crates.io and GitHub.
---

# Release Workflow

## Steps

1. **Run tests**: `cargo run -- hook pre-merge --yes`
2. **Check current version**: Read `version` in `Cargo.toml`
3. **Review commits**: Check commits since last release to understand scope of changes
4. **Credit contributors**: Check for external contributors with `git log v<last-version>..HEAD --format="%an <%ae>" | sort -u` and credit them in changelog entries (see below)
5. **Confirm release type with user**: Present changes summary and ask user to confirm patch/minor/major (see below)
6. **Update CHANGELOG**: Add `## X.Y.Z` section at top with changes (see MANDATORY verification below)
7. **Bump version**: Update `version` in `Cargo.toml`, run `cargo check` to update `Cargo.lock`
8. **Commit**: `git add -A && git commit -m "Release vX.Y.Z"`
9. **Merge to main**: `wt merge --no-remove` (rebases onto main, pushes, keeps worktree)
10. **Tag and push**: `git tag vX.Y.Z && git push origin vX.Y.Z`
11. **Wait for release workflow**: `gh run watch <run-id> --exit-status`

The tag push triggers the release workflow which builds binaries and publishes to crates.io, Homebrew, and winget automatically.

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

**Finding reporters:** Check PR bodies for "Fixes #N", "Closes #N", or issue references. Then look up the issue author with `gh issue view N --json author`.

**When to credit:**
- Bug reports with clear reproduction steps
- Feature requests that shaped the implementation
- Performance reports with measurements (like "takes 15s")
- Users who helped diagnose issues through discussion

Skip credit for: issues opened by the repo owner, trivial reports, or issues that were substantially different from what was implemented.

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

Also check: Are there any user-facing changes in the commits that are NOT covered by these entries?

Report format:
- Entry: [entry text]
  Status: ✅ Accurate / ⚠️ Needs revision / ❌ Incorrect
  Evidence: [what you found in the diff]
  Suggested fix: [if needed]
```

**Do not finalize the changelog until the subagent confirms all entries are accurate.**

**If verification finds problems:** Escalate to the user. Show them the subagent's findings and ask how to proceed. Don't attempt to resolve ambiguous changelog entries autonomously — the user knows the intent behind their changes better than you do.

## Confirm Release Type

**Before proceeding with changelog and version bump, confirm the release type with the user.**

After reviewing commits, present:
1. Current version (e.g., `0.2.0`)
2. Brief summary of changes (new features, bug fixes, breaking changes)
3. Your recommendation for release type with reasoning
4. The three options: patch, minor, major

Use `AskUserQuestion` to get explicit confirmation. Example:

```
Current version: 0.2.0
Changes since v0.2.0:
- Added `state clear` command (new feature)
- Added `previous-branch` state key (new feature)
- No breaking changes

Recommendation: Minor release (0.3.0) — new features, no breaking changes
```

**Do not proceed until user confirms the release type.** The user may have context about upcoming changes or preferences that affect versioning.

## Version Guidelines

- **Second digit** (0.1.0 → 0.2.0): Backward incompatible changes
- **Third digit** (0.1.0 → 0.1.1): Everything else

Current project status: early release, breaking changes acceptable, optimize for best solution over compatibility.

## Troubleshooting

### Release workflow fails after tag push

If the workflow fails (e.g., cargo publish error), fix the issue, then recreate the tag:

```bash
gh release delete vX.Y.Z --yes           # Delete GitHub release
git push origin :refs/tags/vX.Y.Z        # Delete remote tag
git tag -d vX.Y.Z                        # Delete local tag
git tag vX.Y.Z && git push origin vX.Y.Z # Recreate and push
```

The new tag will trigger a fresh workflow run with the fixed code.
