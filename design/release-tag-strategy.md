# Design: decouple the release tag from main's moving tip

Status: proposal (no production code). This answers one question about the
release workflow (`.claude/skills/release/SKILL.md`, steps 1, 11, 12):

> Should a release tag a fixed, prepared commit, instead of waiting for the tip
> of `main` to become "complete"?

Short answer: yes. The current design couples the release to `main`'s tip, and
under concurrent merges "complete" becomes a target the changelog can never
catch. Two repo constraints (squash-only merges, `git describe`-based version
strings) bound how to decouple it. This document works through the mechanism,
the constraints, what mature Rust tooling does, and the candidate fixes, then
recommends one and shows the step rewrites.

## Summary of recommendations

- **Tag a fixed commit, scope the changelog to a fixed range.** Define the
  release as the commit range `(last-tag, cut-from]` chosen when the release
  branch is cut. The changelog covers exactly that range. Commits that land
  afterward are the *next* release, not undocumented members of this one. This
  is what `cargo-release` and `release-plz` both do, and the inverse of the
  current "wait for the tip to settle" model.
- **Primary: merge the release PR with a merge commit and tag the prepared
  release commit** (option A below). This is `release-plz`'s default and
  recommended strategy. It converges with zero release-time coordination:
  concurrent merges roll into the next release by construction. It costs one
  repo setting (`allow_merge_commit`) and one merge commit per release on
  `main`, a departure from the repo's pure squash-linear history.
- **Alternative: a short merge freeze plus tag-at-merge** (option B below).
  Keeps pure squash-linear history and changes no settings. It costs a coordination
  window: hold other user-facing merges from when the release PR opens until the
  tag is pushed (about the release PR's CI duration), and reconcile the changelog
  to `main` once before opening the PR.
- **The decision is a values call** between coordination-free releases (A) and
  pure squash-linear history (B). The analysis favors A on engineering grounds;
  the repo's history (zero merge commits across the project) favors B. The
  reviewer picks; both step rewrites are below.

## Why the current design does not converge

The release is defined by a tag on a commit whose *tree is the release content*
and whose *`CHANGELOG.md` is complete for that tree*. The current skill ties
that commit to `main`'s tip:

- **Step 1** cuts the release branch from `main`'s tip and records that
  `cut-from` SHA. The changelog is drafted to cover `(last-tag, cut-from]`.
- **Step 11** opens a release PR and squash-merges it to `main`. `main` can
  advance during the PR's life.
- **Step 12** checks `git log <cut-from>..origin/main` for exactly one line (the
  release squash commit), folds any extra line into the changelog via a
  follow-up squash PR, re-runs the check, then tags `main`'s tip.

Two facts make step 12 a race rather than a check.

**A GitHub squash re-parents the release diff onto `main`'s current tip.** A
squash merge creates a new commit whose parent is `main`'s tip *at merge time*
and whose tree is that tip merged with the PR's diff. So any commit that landed
between `cut-from` and the merge is in the squash commit's tree, while the
changelog (frozen at `cut-from`) does not mention it. The released binary
contains code the release notes omit.

**The drift window is the whole release prep, not just the PR's CI.** Because
the changelog boundary is fixed at step 1 while `main` keeps moving through every
later step, a commit landing at any point from the cut to the merge drifts in.
Release prep includes the data-loss surface review, changelog drafting, the
mandatory verification subagent, and contributor-credit research, so the window
is hours, not the ~25 minutes of the release PR's own CI.

The 0.63.0 release shows both. Its release commit `50553be3 Release v0.63.0
(#3300)` has parent `a5231f57 (#3299)`: the squash re-parented the release onto
#3299, which had landed during the PR's window, so #3299's code shipped in the
tree while the changelog omitted it. Follow-up #3304 documented #3299. During
#3304's CI, #3303 landed; #3307 documented it. During #3307's CI, #3301, #3305,
#3306 landed. The tag is still `v0.62.0`: it is chasing a tip that moves faster
than a doc-PR cycle.

The feedback loop is the problem. The drift check feeds back into *more* work (a
follow-up doc PR), and that work's own CI window is a *new* opportunity for
drift. When the merge rate exceeds the doc-PR cycle time, the loop diverges.
There is no commit in this history where the changelog matches the tree: doc
commits always lag code commits, and new code interleaves.

## Two constraints that bound the fix

The repo's configuration narrows the solution space to a small, provable set.

**Squash-only merges.** `max-sixty/worktrunk` allows squash merges only
(`allow_merge_commit: false`, `allow_rebase_merge: false`), and `main` carries
zero merge commits across the project. A squash always creates a new commit
re-parented onto `main`'s tip, so the original cut commit can never become an
ancestor of `main`. Under squash, the only new ancestor a PR contributes to
`main` is the squash commit, whose tree includes whatever drifted in before the
merge.

**`git describe`-based version strings.** `build.rs` runs
`vergen_gitcl ... .describe(true, true, None)`, and `wt --version`
(`src/cli/mod.rs:180`) reports `VERGEN_GIT_DESCRIBE` (for example
`v0.62.0-3-gabcdef`), falling back to the Cargo version only for non-git builds.
So a release tag must be reachable from `main`, or a dev build off `main` would
`git describe` to the *previous* tag and report the wrong version. (The describe
call has no `--first-parent`, which matters for option A below.)

These two combine into a constraint that drives everything:

> Under squash-only merges and a describe-reachable tag, the tagged commit is
> necessarily the squash commit on `main`, whose tree includes any commit that
> landed before the merge.

So with the settings unchanged, the tagged tree matches the changelog only when
nothing drifts in before the merge. Convergence therefore requires either
preventing pre-merge drift (a freeze, option B) or relaxing one of the two
constraints (a merge commit, option A; or describe-free versioning, option C).
There is no fourth option.

## What mature Rust release tooling does

Four tools, with the decision-relevant behavior of each. Sources cited inline.

**`cargo-release`** prepares a release commit (version bump, lockfile, changelog
rewrite) and tags *that commit*, then pushes. The tag op is a bare `git tag
<name>` against HEAD, the commit it just made. There is no PR, so there is no
PR-lifetime drift window: the tagged commit is the tip it pushes. A push race
fails closed (atomic, non-forced push; refuses to start if behind remote). It
ships no changelog logic of its own; you drive `CHANGELOG.md` via regex
replacements or a `git-cliff` hook.
([`src/steps/release.rs`](https://github.com/crate-ci/cargo-release/blob/master/src/steps/release.rs))

**`release-plz`** maintains a "release PR" that bumps versions and regenerates
the changelog (via `git-cliff` as a library) from the commit range since the
last tag, force-pushing the PR back into sync on every push to `main`. What it
tags depends on the merge strategy, and it documents the squash hazard
explicitly:

- Default **merge-commit** strategy: it checks out and tags *the PR's own last
  commit*, whose ancestry excludes anything that raced in. Commits landing after
  the last regen roll into the next release. Tree and changelog stay consistent.
- **Squash**: "creates a new commit, so release-plz won't find the commit of the
  PR and will release the latest commit of the main branch," folding raced-in
  commits absent from the changelog into the release. The documented mitigation
  is to "merge release PRs with the default merge strategy."
  ([What commit is released](https://release-plz.dev/docs/usage/release#what-commit-is-released))

**`git-cliff`** tags nothing. It renders a changelog as a pure function of a
commit range and conventional-commit parsing: `git cliff v0.1.0..HEAD`, or
`--unreleased --tag X.Y.Z` to attribute unreleased commits to a forthcoming tag.
"Complete" is mechanical: every commit in the range is grouped or filtered, with
no third state. It has no notion of a branch tip or PR.
([args](https://git-cliff.org/docs/usage/args/))

**`dist`** (the tag-triggered `release.yaml` here) builds the *tagged commit's
tree* (its `actions/checkout` pins no ref, so it defaults to the tag) and sources
the GitHub Release notes by parsing `CHANGELOG.md` *at that commit* with
[`parse-changelog`](https://github.com/taiki-e/parse-changelog), matching the
section whose heading parses to the tag's SemVer version. No match degrades
gracefully to a release titled with the bare tag and no notes. It never checks
that the tag is reachable from `main`; it keys entirely off the tag's commit.
([axoproject/src/changelog.rs](https://github.com/axodotdev/cargo-dist/blob/main/axoproject/src/changelog.rs))

Three patterns recur:

1. **The tagged commit is the unit of truth.** Every tool that publishes decides
   build and notes from the tagged commit alone, never from "main's tip" as a
   proxy. `dist` reads both the binary's tree and the notes from the tagged
   commit, so they are self-consistent with each other but blind to how that
   commit reached `main`.
2. **The drift hazard lives in the PR-merge step, and the defense is to pin the
   tag to a commit whose ancestry excludes raced-in work.** The no-PR tool has no
   window. The PR tool closes the window only by tagging the PR's own commit, and
   names squash/rebase re-parenting as the one case that reopens it. The squash
   problem is real and is the single thing the ecosystem explicitly warns about.
3. **Changelog completeness is a commit-range computation, decoupled from
   tagging.** Pick the range endpoints; the content follows. Choose the wrong tip
   (a squashed merge) and the range silently excludes shipped code.

Worktrunk's changelog is hand-curated by design: it combines related PRs into one
bullet, credits external contributors and issue reporters, and orders by user
impact. That rules out the `release-plz` model of cheaply regenerating the
changelog on every push to `main`; regeneration here means re-running the curation
and the verification subagent. So the applicable pattern is `cargo-release`'s:
prepare a commit, scope the changelog to a fixed range chosen up front, and tag
that prepared commit rather than letting the tag chase `main`.

## Candidate designs

Each option relaxes exactly one term of the constraint above. The current
"racing" behavior is the baseline.

| | Converges under concurrent merges | Tag reachable from `main` | Changelog matches tagged tree | Cost |
|---|---|---|---|---|
| **Baseline (race the drift check)** | No | Yes | Only if no drift | Unbounded follow-up doc PRs |
| **A. Merge-commit + tag prepared commit** | Yes, by construction | Yes (second parent; plain `describe` finds it) | Yes | Enable `allow_merge_commit`; one merge commit per release |
| **B. Short freeze + tag-at-merge** | Yes, if the freeze holds | Yes (squash commit) | Yes | A ~25-min merge freeze; one changelog reconcile per release |
| **C. Drop describe-reachability** | Yes, by construction | No (dangling tag) | Yes | Changes `build.rs`; dev `--version` loses tag accuracy |

### Option A: merge the release PR with a merge commit, tag the prepared commit

Relax squash-only for the release PR. Cut the prepared release commit `C` from
`main`'s tip; its tree is `cut-from` plus the version bump and changelog, and the
changelog covers `(last-tag, cut-from]` exactly. Merge the release PR as a merge
commit `M` (parents `[main-tip, C]`). Tag `C`, not `M`.

- **Converges by construction.** Anything that lands on `main` during prep or CI
  is a sibling of `C`, not an ancestor, so it is simply the next release. No
  freeze, no reconcile, no follow-up doc PR. The owner can keep merging
  throughout.
- **Reachable from `main`.** `C` is `M`'s second parent, hence an ancestor of
  `main`. Plain `git describe` (which `build.rs` uses) walks all parents and
  finds it. This holds only as long as `build.rs` does not add `--first-parent`,
  which would skip a second-parent tag; that is a constraint A imposes on
  `build.rs`.
- **Tree matches changelog.** `C`'s tree is the cut content; the changelog covers
  that range. `dist` builds `C` and reads `C`'s `CHANGELOG.md`. Consistent.

This is `release-plz`'s default strategy. Costs: `allow_merge_commit` is a
repo-wide GitHub setting (no per-PR scoping), so enabling it lets any PR be
merged as a merge commit; the release skill confines the merge-commit merge to
the release PR, and squash stays the default the UI offers. And `main` gains one
merge commit per release, so it is no longer pure squash-linear (`git log
--first-parent main` shows `M`; `C` sits off the first-parent line, as it does
under `release-plz`).

### Option B: a short merge freeze, then tag at merge

Keep squash-only. Remove pre-merge drift instead of tolerating it, and tag the
moment the release PR merges rather than looping afterward.

The current design's two leaks both come from timing: the changelog boundary is
fixed hours early (step 1), and the tag is applied after a drift-check loop (step
12). Close both:

- **Reconcile the changelog to `main` once, right before opening the PR.** Do the
  expensive curation against a recent `main`. Just before opening the release PR,
  `git fetch` and check whether `main` advanced; if it did, extend the changelog
  to cover the new commits and reset the `cut-from` boundary to `main`'s tip. The
  squash re-parents onto that tip, so the changelog covering it is the only
  requirement.
- **Freeze the short window.** Hold other user-facing merges from when the release
  PR opens until the tag is pushed. With no drift, the squash commit's tree
  equals the cut content.
- **Tag at merge.** The instant the release PR squash-merges, tag that squash
  commit and push, then lift the freeze. The step-12 drift check stays only as an
  assertion: under the freeze it finds exactly the release commit, and if it
  finds drift the freeze was violated, which is a loud stop rather than a silent
  divergence.

Converges as long as the freeze holds. Costs: a coordination window of roughly
the release PR's CI duration (~25 min), and one synchronous changelog reconcile
per release. It preserves pure squash-linear history and changes no settings or
code. The residual risk is that the freeze is social and can be violated, which
is what happened in 0.63.0; the assertion in step 12 turns a violation into a
visible failure instead of an undocumented release.

### Option C: drop describe-reachability (not recommended)

Change `build.rs` to derive the version from the Cargo version rather than `git
describe`. Then a tag need not be reachable from `main`, so the prepared commit
`C` can be tagged directly (no merge commit, no freeze), and the changelog and
version land on `main` via an ordinary squash PR. This converges and matches, but
it changes a user-facing behavior: dev builds off `main` lose the
`-<N>-g<sha>` distance suffix that pins a build to a commit, which is useful in
bug reports. Trading a deliberate version feature to avoid a merge commit is the
wrong lever to pull. Listed for completeness.

## Recommendation

Adopt the fixed-range model: the release is `(last-tag, cut-from]`, the changelog
covers exactly that range, and the tag points at a prepared commit. This is the
answer to the question this document opened with, and it is what both
`cargo-release` and `release-plz` do.

For the mechanism, **option A** is the stronger engineering choice and the
prior-art default: it converges with zero release-time coordination, it is
`release-plz`'s documented strategy for exactly this problem, and squash-merging
a release PR is the one practice the ecosystem explicitly warns against, which is
what the current workflow does. **Option B** is the right choice if preserving
pure squash-linear history outweighs a short per-release freeze; the repo's
zero-merge-commit history is a strong signal that it might.

The reviewer decides one thing: accept release merge commits (enable
`allow_merge_commit`), or accept a short release-merge freeze. The step rewrites
for both follow, so whichever is chosen can land directly.

## Proposed skill changes

These rewrite steps 1, 11, and 12 of `.claude/skills/release/SKILL.md`. Steps
2-10 (tests, version checks, changelog drafting and verification) are unchanged.

### Option A rewrites

**Step 1** keeps the fast-forward sync; only the trailing note changes, since
`cut-from` now defines a fixed range rather than a tip the tag must catch:

> Note the resulting commit SHA as `cut-from`. The changelog covers exactly
> `(last-tag, cut-from]`; commits that land on `main` after this point are the
> next release, not this one.

**Step 11** merges with a merge commit instead of a squash:

> **Merge to main:** open the release PR and wait for CI, then merge it as a
> merge commit (`gh pr merge --merge`, which requires `allow_merge_commit`):
> ```bash
> gh pr merge --merge
> ```
> `main` may advance during the CI wait; that is fine, because step 12 tags the
> prepared release commit rather than `main`'s tip.

**Step 12** tags the prepared commit (the merge's second parent) and drops the
drift check entirely:

> **Tag and push.** The release commit is the merge commit's second parent (the
> PR's own commit), whose tree is the `cut-from` content plus the changelog. Tag
> it, not `main`'s tip:
> ```bash
> git fetch origin
> MERGE_SHA=$(gh pr view --json mergeCommit --jq '.mergeCommit.oid')
> RELEASE_SHA=$(git rev-parse "${MERGE_SHA}^2")
> git tag vX.Y.Z "$RELEASE_SHA" && git push origin vX.Y.Z
> ```

A short note belongs near step 12 recording that `build.rs` must keep plain `git
describe` (no `--first-parent`) for the second-parent tag to resolve.

### Option B rewrites

**Step 1** changes its trailing note the same way as option A (`cut-from` is the
range boundary), and adds that the boundary is reconciled to `main` at step 11.

**Step 11** freezes, reconciles the changelog boundary to `main`, then
squash-merges:

> **Merge to main.** Announce a merge freeze: hold other user-facing merges from
> here until step 12 pushes the tag. Then check whether `main` moved during prep:
> ```bash
> git fetch origin
> git log --oneline <cut-from>..origin/main
> ```
> Any commits there landed during prep. Extend the changelog to cover them
> (re-run the verification subagent on the additions) and reset `cut-from` to
> `origin/main`'s tip. A rebase is not needed: the squash re-parents the release
> diff onto `main`'s tip anyway, so the only requirement is that the changelog
> covers that tip. Then `/gpk` to open the PR, wait for CI, and squash-merge. The
> freeze keeps `main` still during CI, so the squash commit's tree matches the
> changelog.

**Step 12** becomes a tag-immediately step with the drift check as an assertion:

> **Tag and push immediately.** The freeze means the squash commit's tree matches
> the changelog. Confirm, then tag without delay:
> ```bash
> git fetch origin
> git log --oneline <cut-from>..origin/main   # expect only the release squash commit
> MERGE_SHA=$(gh pr view --json mergeCommit --jq '.mergeCommit.oid')
> git tag vX.Y.Z "$MERGE_SHA" && git push origin vX.Y.Z
> ```
> An extra line means the freeze was violated: its code already shipped in the
> squash tree, so fold it into the changelog via a follow-up squash PR before
> tagging. Lift the freeze once the tag is pushed.

## What this does not change

- `dist` / `release.yaml` are untouched. Both options tag a commit carrying the
  version bump and the `## X.Y.Z` changelog section, which is all `dist` parses;
  it builds the tagged tree and reads that commit's `CHANGELOG.md` regardless of
  how the commit reached `main`.
- The changelog's content, ordering, credit rules, and the mandatory verification
  subagent are unchanged. Only the *range* it covers becomes fixed up front
  rather than implicitly redefined by `main`'s tip.
- Feature PRs still squash-merge. Under option A only the release PR uses a merge
  commit.
