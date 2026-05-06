# Eliminating ref-keyed cache staleness

## Background

`Repository::RepoCache` (in `src/git/repository/mod.rs`) holds per-command
caches that survive any number of git operations on the same `Repository`
instance. Several of those caches are keyed on **ref names** (e.g. `"main"`,
`"refs/heads/main"`, `"HEAD"`). Ref names are not stable — their SHAs change
when wt itself updates a ref mid-command (`wt merge`'s `update-ref`,
`commit-tree` + `update-ref` in `worktree/push.rs`, future write paths). After
such a write, ref-keyed entries return the pre-write SHA and any computation
downstream of that SHA is silently wrong.

PR #2507 fixed exactly this for one call site: `Repository::is_ancestor` was
reading stale SHAs out of `commit_shas`, so `integration_reason` misclassified
a just-merged branch as unmerged. The fix added a local helper
(`ref_is_ancestor`) that bypasses the cache. It works, but every other ref-name
read through `rev_parse_commit` after a write has the same latent bug.

We want to eliminate the bug class **structurally**, not by remembering to
invalidate. Invalidation discipline scales linearly with new write call sites;
structural elimination scales with zero.

## Inventory: caches keyed on ref names today

Read `RepoCache` (`src/git/repository/mod.rs:204-316`) and treat anything
keyed by `String` ref name as a staleness vector. Concretely:

| Cache field                       | Key                          | Used by                                     | Stale risk after a ref update? |
| --------------------------------- | ---------------------------- | ------------------------------------------- | ------------------------------- |
| `commit_shas`                     | ref name                     | `rev_parse_commit` → all SHA-keyed caches   | **Yes** — primary bug source    |
| `tree_shas`                       | tree spec (e.g. `…^{tree}`)  | `rev_parse_tree`                            | **Yes**                         |
| `resolved_refs`                   | short name → full ref        | `resolve_preferring_branch`                 | No (refs/heads/X doesn't move)  |
| `ahead_behind`                    | `(base ref, head ref)`       | `wt list`'s ahead/behind, upstream task     | **Yes** if either ref updates   |
| `effective_integration_targets`   | local target ref name        | `effective_integration_target`              | **Yes** (depends on local tip)  |
| `integration_reasons`             | `(branch, target)` ref names | `integration_reason`                        | **Yes** (composite of above)    |
| `head_shas`                       | worktree path                | `WorkingTree::head_sha`                     | **Yes** (HEAD moves on commit)  |
| `local_branches` inventory        | n/a (one-shot scan)          | every multi-branch op                       | **Yes** (frozen view)           |

Caches keyed entirely by **SHA** (`merge_base`, `diff_stats`, the persistent
on-disk `sha_cache::*`) are structurally safe — a SHA addresses immutable
content. The risk is only at the **ref-name → SHA boundary**.

## Where ref writes happen today

`grep -rn "update-ref\|update_ref" src/`:

- `src/commands/worktree/push.rs:385` — `update-ref` on the merge target.
- `src/commands/merge.rs:286` (calls into push.rs) — `wt merge` flow.
- `src/git/repository/working_tree.rs:563` — `update-ref` for stash/branch ops.
- Any `git commit`, `git rebase`, or hook subprocess can update a local ref
  too. We don't `update-ref` those ourselves but the underlying refs move.

The `wt merge` flow is the canonical example: rebase → `update-ref` target →
`finish_after_merge` → `compute_integration_reason` → reads stale `main` SHA.

## Goals & constraints (recap)

- **No invalidation logic.** "Call `cache.clear_ref(name)` after every write"
  is the failure mode, not the solution.
- **Performance still matters.** `rev_parse_commit` is consulted by
  `is_ancestor`, `merge_base`, `merge_integration_probe`, `branch_diff_stats`,
  `has_added_changes`. In `wt list` with 100 worktrees these run hundreds of
  times. Today, the `local_branches` for-each-ref pre-population dodges most
  rev-parse subprocesses for branch refs.
- **Per-command lifetime.** `Repository` lives for one CLI command, so
  cross-command persistence is irrelevant for in-memory caches.
- **`wt list` is read-only.** No ref writes happen during it; the staleness
  bug class doesn't manifest. Whatever we choose must not regress its
  performance.
- **Mutate-then-read commands are rare** (`wt merge`, `wt remove`, future
  `wt rebase`?). Performance there is bounded by the git operations
  themselves.

## Options

### Option 1 — Cache only by SHA at the boundary

**What it changes structurally.** Drop `commit_shas` and `tree_shas`. Public
API still takes ref names but resolves them locally with no cache. Each
ref-name → SHA conversion spawns one `git rev-parse` per call. SHA-keyed
caches (in-memory `merge_base`, on-disk `sha_cache::*`) keep working as today.

**Where the work lands.** `rev_parse_commit`/`rev_parse_tree` in
`integration.rs:509-547` lose their `DashMap::entry` wrappers. The priming in
`branches.rs:155-166` goes away. Direct callers of those helpers
(`is_ancestor`, `has_added_changes`, `has_merge_conflicts`, `branch_diff_stats`,
`merge_integration_probe`, `has_merge_conflicts_by_tree`, `would_merge_add_to_target`)
keep their signatures but pay one extra subprocess per ref per call.

**Performance impact.** For `wt list` with N worktrees and ~5 SHA-keyed ops
per row: roughly 10×N additional `git rev-parse` subprocesses (2 ref → SHA
conversions × 5 ops). At ~3-5ms each that is **300-500 ms regression on a
100-worktree repo** — same order as the entire current `wt list` runtime
budget. Mutate-then-read commands gain a handful of subprocesses; negligible.

**What it does NOT solve.** `effective_integration_targets`,
`integration_reasons`, `ahead_behind`, `head_shas`. These are keyed on refs
but their *values* are derived through ref-relative operations (upstream
relationships, ahead/behind counts), so they aren't pure functions of SHAs.
You'd still need a separate decision for each one.

**Migration cost.** Low surface area (~60 LOC) but sizable performance loss
on the read-only hot path.

### Option 2 — Two-tier API: `is_ancestor(refs)` uncached, `is_ancestor_by_sha` cached

**What it changes structurally.** Every public op exists in two forms:

```rust
// Public, ref-taking, no SHA cache:
pub fn is_ancestor(&self, base: &str, head: &str) -> Result<bool> { ... }
// Internal, SHA-taking, cached:
fn is_ancestor_by_sha(&self, base: &Sha, head: &Sha) -> Result<bool> { ... }
```

The ref-taking form runs git directly (`merge-base --is-ancestor` accepts ref
names natively, as does `rev-parse a b` for `same_commit`/`trees_match`).
Callers that want cache hits must explicitly hold SHAs.

**Where the work lands.** Same files as Option 1, but with twice as many
public methods. Each task in `commands/list/collect/tasks.rs` switches from
`repo.is_ancestor(branch, target)` to first resolving SHAs through a
snapshot (see Option 5) and then calling `is_ancestor_by_sha`.

**Performance impact.** Same as Option 1 if hot-path callers stay on the
ref-taking form. Same as today if they migrate to the SHA-taking form.
This option *enables* the fast path; it doesn't force it.

**What it does NOT solve.** Same residual risks as Option 1 — composite
caches (`integration_reasons`, `effective_integration_targets`, `ahead_behind`)
need separate treatment.

**Migration cost.** Medium. Each cached op gains a sibling. `wt list` task
code changes to thread SHAs through (~20 call sites).

### Option 3 — Type-level distinction (`Sha` newtype)

**What it changes structurally.** Introduce
`pub struct Sha(String)` (or `Sha([u8; 20])`) constructed only via
`Repository::resolve(&str) -> Sha` (uncached) or via the `for-each-ref` scan
(produces `Sha`s tied to a snapshot). Cached methods take `&Sha` and refuse
`&str`. Compiler enforces "you must resolve before caching."

**Where the work lands.** `Sha` propagates through `BranchRef`, `LocalBranch`,
task contexts in `commands/list/collect/`, every diff/integration helper.
Tests that hardcode SHA strings need conversion.

**Performance impact.** Same as Option 2 — the type system says nothing about
when you spawn rev-parse, only that you've named the conversion.

**What it does NOT solve.** Composite caches still need explicit decisions.
A `Sha` newtype prevents storing `String` ref-name keys in SHA-keyed caches,
but does not make `effective_integration_target` (whose result depends on the
upstream relationship at lookup time, not on the SHA) safe.

**Migration cost.** High. Newtype propagation reaches a lot of call sites
(`BranchRef.commit_sha: String` becomes `Sha`, every test fixture, every
JSON serialization path). The type-level guarantee is real but the surface
area touched is wide.

### Option 4 — Delete the in-memory ref-keyed caches outright

**What it changes structurally.** Drop `commit_shas`, `tree_shas`,
`effective_integration_targets`, `integration_reasons`, `ahead_behind`,
`head_shas`. Stop priming. Every cached op spawns its underlying git
command(s) every call. The persistent on-disk `sha_cache::*` survives because
its keys are SHAs.

**Where the work lands.** Delete fields and entry-block wrappers across
`integration.rs`, `diff.rs`, `working_tree.rs`. Remove the priming loop in
`branches.rs:155-166`.

**Performance impact.** Severe. `wt list` with 100 worktrees today calls
`is_ancestor`, `has_added_changes`, `merge_integration_probe`, etc. relying
on cached SHAs and (for `ahead_behind`) cached counts batched in
`batch_ahead_behind`. Without those caches you're spawning thousands of
subprocesses per command. Likely 5-10× regression on `wt list`.

**What it does NOT solve.** Removes the bug entirely, but at unacceptable
cost. Listing it for completeness — also useful as a baseline reminder that
the cache is load-bearing for the read-only hot path.

**Migration cost.** Low surface area. High user-visible impact.

### Option 5 — Resolve-once-up-front (`RefSnapshot`)

**What it changes structurally.** At command entry, one explicit
`let snapshot = repo.snapshot()?;` call performs the `for-each-ref` scan and
produces an immutable `RefSnapshot`. The snapshot holds ref name → SHA for
every local + remote branch, plus HEAD, plus any other refs of interest.
Cached SHA-keyed ops take `&Sha` (or `&str` plus `&RefSnapshot`). Repository
itself drops ambient ref-keyed caches.

After a ref-mutating operation, the caller produces a new snapshot. Old
snapshots remain valid as historical views — by name, you can see they
represent the pre-write state. This is the same shape `local_branches`
already has (a `OnceCell<LocalBranchInventory>` populated by one
for-each-ref), promoted to first-class status with explicit lifetime.

```rust
let snap = repo.snapshot()?;            // implicit "ref state right now"
let target_sha = snap.resolve(target)?; // never spawns git
let branch_sha = snap.resolve(branch)?;
let integrated = repo.is_ancestor_by_sha(&branch_sha, &target_sha)?;

// ... wt merge writes the target ref ...

let snap2 = repo.snapshot()?;           // explicit "after the write"
let target_sha = snap2.resolve(target)?;
// integration check against the post-write target — naturally fresh
```

**Where the work lands.** New `RefSnapshot` type (~100 LOC). Public ops in
`integration.rs`/`diff.rs` either gain `_by_sha` variants (Option 2 style)
or accept `&RefSnapshot` directly. `commands/list/collect/` builds one
snapshot during pre-skeleton, threads it through tasks instead of (or in
addition to) `Arc<RepoCache>`. `wt merge`'s `finish_after_merge` builds a
fresh snapshot post-`update-ref` and uses it for the integration check —
the post-write call gets the post-write state by construction.

**Performance impact.** Roughly neutral to positive. Today's
`local_branches` scan + `commit_shas` priming already does this work; the
proposal just names it. Subsequent SHA lookups become plain `HashMap::get`
on a frozen view (no `DashMap` shard contention). `wt list` gets one
deterministic scan up-front instead of lazy `OnceCell::get_or_try_init`
race; all parallel tasks read from the immutable snapshot. `wt merge` pays
for one extra `for-each-ref` after the ref update — ~5-15 ms.

**What it does NOT solve.** The discipline of "after a write, take a new
snapshot" is still required. The improvement is that:
1. Snapshots are *named*, so the contract is visible at every call site.
2. Forgetting to re-snapshot reads pre-write state explicitly, not
   "from a cache that is supposed to be transparent."
3. The bug surface compresses to a small number of write boundaries
   instead of every ref-name accessor in the codebase.

`effective_integration_targets`, `integration_reasons`, and `ahead_behind`
become functions of `(snapshot, ref pair)` — keyed by the snapshot's SHAs
they're derived from. Stale-by-construction is no longer possible: a
snapshot is internally consistent.

**Migration cost.** Medium-high. `RefSnapshot` has to be threaded through
hot paths. But the threading replaces existing `Arc<RepoCache>` usage in
`tasks.rs`, so it's a refactor, not net-new plumbing.

### Option 6 — "Take a fresh `Repository::at(...)` after writes"

**What it changes structurally.** Document and rely on the existing pattern:
`Repository::at(...)` builds a brand-new `RepoCache`. Code that reads after
its own writes constructs a fresh Repository.

**Where the work lands.** `finish_after_merge`, `handle_no_ff_merge`, every
future write call site — gain `let repo = Repository::at(...)?;` after the
write.

**Performance impact.** A second Repository pays for a duplicate
`for-each-ref` scan on the next inventory access. ~5-15 ms.

**What it does NOT solve.** This is **invalidation by another name**. It's
exactly the discipline-based approach the goals exclude — easy to forget,
new write call sites are latent bugs. List it only because the codebase
already documents this pattern for `list_worktrees`. Not a recommendation.

**Migration cost.** Trivial in code, infinite in vigilance.

## Recommendation

**Go with Option 5 (`RefSnapshot`), positioned as a refinement of the
existing `local_branches` inventory.**

The argument:

1. **The shape already exists.** `OnceCell<LocalBranchInventory>` plus
   `commit_shas` priming via `for-each-ref` is a snapshot in everything but
   name. Today its frozen-ness is implicit and undocumented; staleness leaks
   through `commit_shas` because callers don't know the cache is a snapshot.
   Promoting it to a first-class `RefSnapshot` type makes the contract
   explicit at every call site.

2. **It's the only option that addresses composite caches structurally.**
   Options 1-3 fix `commit_shas` but leave `effective_integration_targets`,
   `integration_reasons`, and `ahead_behind` exposed. Option 5 makes all of
   them derived properties of an immutable snapshot, so they're internally
   consistent by construction.

3. **It preserves the read-only hot path.** `wt list` builds one snapshot
   pre-skeleton and reads from it for the rest of the command. Performance
   is neutral or marginally better than today's `DashMap` priming.

4. **It localizes the discipline.** Today, every ref-name accessor in the
   codebase is a potential staleness vector. After Option 5, only the
   write boundaries (`finish_after_merge`, `handle_no_ff_merge`,
   `working_tree.rs:563`) need to think about freshness. They take a new
   snapshot. That's the smallest possible surface for the discipline that
   we still need.

5. **The `is_ancestor` workaround in PR #2507 dissolves.** With Option 5,
   `compute_integration_reason_uncached` simply takes the post-write
   snapshot — no ad-hoc `ref_is_ancestor` helper, no comments warning
   future readers about cache bypass.

### Concrete shape

```rust
// New: src/git/repository/snapshot.rs
pub struct RefSnapshot {
    // Frozen at construction. Local + remote branches + HEAD + a few specials.
    refs: HashMap<String, Sha>,
    // Derived in one pass alongside the for-each-ref scan:
    upstream_of: HashMap<String, String>,
    ahead_behind: HashMap<(String, String), (usize, usize)>,
}

impl RefSnapshot {
    pub fn resolve(&self, name: &str) -> anyhow::Result<&Sha> { ... }
    pub fn upstream_of(&self, branch: &str) -> Option<&str> { ... }
    pub fn ahead_behind(&self, base: &str, head: &str) -> Option<(usize, usize)> { ... }
}

impl Repository {
    /// Snapshot the current ref state: one for-each-ref + ahead-behind batch.
    /// Cheap to call; no in-memory cache, by design.
    pub fn snapshot(&self) -> anyhow::Result<RefSnapshot> { ... }
}
```

`Repository`'s ambient ref-keyed fields (`commit_shas`, `tree_shas`,
`effective_integration_targets`, `integration_reasons`, `ahead_behind`,
`head_shas`) all go away. The persistent `sha_cache::*` keeps its SHA keys.
Public ops gain `_by_sha` variants for cached fast paths; ref-taking variants
exist only as thin shims that resolve uncached and call the SHA form (so a
caller that doesn't need a snapshot still works, just spawns one rev-parse).

### What we accept

- One up-front `for-each-ref` per snapshot (~5-15 ms). Already the dominant
  pattern.
- Discipline at write boundaries — but the boundaries are *the writes
  themselves*, not every read.
- A type & lifetime story for `RefSnapshot` to thread through tasks. This
  is the bulk of the migration work.

### What we don't do

- Don't add invalidation hooks.
- Don't keep `commit_shas` "with a TODO".
- Don't bake compatibility with a parallel ambient-cache path. Cut over.

## Open questions for implementation (not answered here)

- Should `RefSnapshot` be `Arc`-wrapped for parallel task fan-out, or is
  cheap clone-by-value enough? (Rough size: ~200 branches × 60 bytes ≈ 12 KB.)
- Does the snapshot need to include tag refs? Audit the call sites.
- For `wt merge`, does `finish_after_merge` build the post-write snapshot
  itself, or does the caller pass it in?
- Does `Sha` become a newtype, or stays a `String` typedef inside
  `RefSnapshot`? The newtype upside (Option 3's static guarantee) compounds
  with Option 5; the migration cost is the question.
