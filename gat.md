# Proposal: Replace downcast pattern with GAT-based VcsOps

## Problem

Commands that need VCS-specific behavior use runtime downcasting:

```rust
let workspace = open_workspace()?;
if workspace.as_any().downcast_ref::<Repository>().is_none() {
    return handle_merge_jj(opts);
}
```

This produces 5 parallel handler files (~876 lines of jj handlers) with
~17% code duplication against their git counterparts. 16 downcast sites
across 11 files.

## Proposal

Add a GAT `Ops` to the `Workspace` trait that carries VCS-specific
operations. Commands become generic over `W: Workspace` with one
implementation instead of two.

```rust
trait VcsOps {
    fn prepare_commit(&self, path: &Path, mode: StageMode) -> Result<()>;
    fn guarded_push(&self, target: &str, push: &dyn Fn() -> Result<PushResult>) -> Result<PushResult>;
    fn squash(&self, target: &str, message: &str, path: &Path) -> Result<SquashOutcome>;
}

trait Workspace: Send + Sync {
    type Ops<'a>: VcsOps where Self: 'a;

    fn ops(&self) -> Self::Ops<'_>;

    // ... existing 28 methods unchanged
}
```

Git implementation:

```rust
struct GitOps<'a> { repo: &'a Repository }

impl VcsOps for GitOps<'_> {
    fn prepare_commit(&self, path: &Path, mode: StageMode) -> Result<()> {
        stage_files(self.repo, path, mode)?;
        run_pre_commit_hooks(self.repo, path)
    }

    fn guarded_push(&self, target: &str, push: &dyn Fn() -> Result<PushResult>) -> Result<PushResult> {
        let stash = stash_target_if_dirty(self.repo, target)?;
        let result = push();
        if let Some(s) = stash { restore_stash(self.repo, s); }
        result
    }
}

impl Workspace for Repository {
    type Ops<'a> = GitOps<'a>;
    fn ops(&self) -> GitOps<'_> { GitOps { repo: self } }
}
```

Jj implementation:

```rust
struct JjOps<'a> { ws: &'a JjWorkspace }

impl VcsOps for JjOps<'_> {
    fn prepare_commit(&self, _path: &Path, _mode: StageMode) -> Result<()> {
        Ok(()) // jj auto-snapshots
    }

    fn guarded_push(&self, _target: &str, push: &dyn Fn() -> Result<PushResult>) -> Result<PushResult> {
        push() // no stash needed
    }
}
```

Command handlers become generic:

```rust
fn handle_merge<W: Workspace>(ws: &W, opts: MergeOptions) -> Result<()> {
    let target = ws.resolve_integration_target(opts.target)?;

    if ws.is_dirty(&path)? {
        ws.ops().prepare_commit(&path, stage_mode)?;
        ws.commit(message, &path)?;
    }

    ws.rebase_onto(&target, &path)?;

    ws.ops().guarded_push(&target, &|| {
        ws.advance_and_push(&target, &path, display)
    })?;

    // ... shared output, removal, hooks
}
```

Top-level dispatch (once, in main or cli):

```rust
fn dispatch<W: Workspace>(ws: &W, cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::Merge(opts) => handle_merge(ws, opts),
        Cmd::Switch(opts) => handle_switch(ws, opts),
        // ...
    }
}

match detect_vcs(&path) {
    Some(VcsKind::Git) => dispatch(&Repository::current()?, cmd),
    Some(VcsKind::Jj) => dispatch(&JjWorkspace::from_current_dir()?, cmd),
    None => // fallback
}
```

## What VcsOps would contain

~3-5 methods covering the structural differences between git and jj flows:

| Method | Git | Jj |
|--------|-----|----|
| `prepare_commit` | Stage files + pre-commit hooks | No-op |
| `guarded_push` | Stash target, push, restore | Just push |
| `squash` | `reset --soft` + commit | `jj squash --from` |
| `feature_tip` | `"HEAD"` | `@` or `@-` if empty |
| `committable_diff` | `git diff --staged` | `jj diff -r @` |

Some of these are already on the Workspace trait (`feature_head`,
`squash_commits`, `committable_diff_for_prompt`). They could stay there
or move to VcsOps — the distinction is whether the *caller's control
flow* differs (VcsOps) or just the implementation (Workspace).

## What this eliminates

- `as_any()` method on Workspace trait
- All 16 downcast sites
- 5 parallel handler files: `handle_merge_jj.rs`, `handle_switch_jj.rs`,
  `handle_remove_jj.rs`, `handle_step_jj.rs`, `list/collect_jj.rs`
- `CommandEnv::require_repo()`, `CommandContext::repo()` downcast helpers

## What this costs

- GAT syntax in bounds: `W: Workspace` works, but if you need to name
  the ops type explicitly, bounds get verbose
- `open_workspace()` can no longer return `Box<dyn Workspace>` — dyn
  dispatch and GATs don't mix cleanly. The top-level dispatch must be
  an enum match or generic function, not a trait object
- Monomorphization: generic commands are compiled twice (once per VCS).
  Increases binary size slightly, irrelevant for a CLI

## Migration

Incremental, one command at a time:

1. Add `VcsOps` trait and `type Ops<'a>` to Workspace (backward
   compatible — existing code still compiles)
2. Add `VcsOps` impls for `GitOps`/`JjOps` with the 3-5 methods
3. Convert one command (start with `remove` — simplest) to generic
4. Delete `handle_remove_jj.rs`
5. Repeat for step, merge, switch, list
6. Remove `as_any()` from Workspace trait once no downcasts remain
7. Replace `open_workspace() -> Box<dyn Workspace>` with enum dispatch

Steps 1-2 are additive. Steps 3-6 can be done one command per PR.
Step 7 is the final breaking change to the internal API.

## Alternative: Box<dyn VcsOps + '_>

Avoids GATs entirely:

```rust
trait Workspace {
    fn ops(&self) -> Box<dyn VcsOps + '_>;
}
```

Simpler bounds, works with `dyn Workspace`. Costs one heap allocation
per `ops()` call (negligible). Loses monomorphization. Worth considering
if GAT ergonomics prove annoying in practice.
