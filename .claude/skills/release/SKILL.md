---
name: release
description: Worktrunk release workflow. Use when user asks to "do a release", "release a new version", "cut a release", or wants to publish a new version to crates.io and GitHub.
---

# Release Workflow

## Steps

1. **Run tests**: `cargo run -- hook pre-merge --force`
2. **Check current version**: Read `version` in `Cargo.toml` to determine next version
3. **Review CHANGELOG**: Check commits since last release cover notable changes
4. **Update CHANGELOG**: Add `## X.Y.Z` section at top with changes
5. **Bump version**: Update `version` in `Cargo.toml`, run `cargo check` to update `Cargo.lock`
6. **Commit**: `git add -A && git commit -m "Release vX.Y.Z"`
7. **Merge to main**: `wt merge --no-remove` (rebases onto main, pushes, keeps worktree)
8. **Tag and push**: `git tag vX.Y.Z && git push origin vX.Y.Z`
9. **Wait for release workflow**: `gh run watch <run-id> --exit-status`
10. **Update Homebrew**: `./dev/update-homebrew.sh` (requires sibling `homebrew-worktrunk` checkout)

The tag push triggers the release workflow which builds binaries and publishes to crates.io. The Homebrew script fetches SHA256 hashes from the release assets and updates the formula.

## CHANGELOG Review

Check commits since last release for missing entries:

```bash
git log v<last-version>..HEAD --oneline
```

Notable changes to document:
- New features or commands
- User-visible behavior changes
- Bug fixes users might encounter
- Breaking changes

Skip: internal refactors, doc-only changes, test additions (unless user-facing like shell completion tests).

## Version Guidelines

- **Patch** (0.1.x → 0.1.y): Bug fixes only
- **Minor** (0.x.0 → 0.y.0): New features, non-breaking changes
- **Major** (x.0.0 → y.0.0): Breaking changes (rare in early development)

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
