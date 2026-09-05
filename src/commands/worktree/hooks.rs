//! Hook execution for worktree operations.
//!
//! CommandContext implementations for pre-start hooks, and PostRemoveContext
//! for building template variables for post-remove hooks.

use std::path::Path;

use worktrunk::HookType;
use worktrunk::git::Repository;
use worktrunk::path::to_posix_path;

use crate::commands::command_executor::CommandContext;
use crate::commands::command_executor::FailureStrategy;
use crate::commands::hook_plan::{ApprovedHookPlan, execute_planned_hook};

impl<'a> CommandContext<'a> {
    /// Execute pre-start commands sequentially (blocking) from the frozen plan.
    ///
    /// Runs user hooks first, then project hooks. `anchor` is the new
    /// worktree's path — the gate selected `pre-start` under it from the
    /// invoking worktree's config; the executor never re-reads any config.
    /// Shows path in hook announcements when shell integration isn't active
    /// (the user's shell won't cd to the new worktree).
    pub fn execute_pre_create_commands(
        &self,
        extra_vars: &[(&str, &str)],
        plan: &ApprovedHookPlan,
        anchor: &Path,
    ) -> anyhow::Result<()> {
        execute_planned_hook(
            plan,
            anchor,
            self,
            HookType::PreCreate,
            extra_vars,
            FailureStrategy::FailFast,
            crate::output::post_hook_display_path(self.worktree_path),
        )
    }
}

/// Context for post-remove hooks, holding owned strings for template variables.
///
/// Post-remove hooks need template variables that reflect the *removed* worktree
/// (not the destination), since hooks may reference the removed path and branch
/// (e.g., for cleanup scripts that use the path in container names). This struct
/// owns the computed strings so callers can borrow them as extra_vars.
pub(crate) struct PostRemoveContext {
    worktree_path_str: String,
    worktree_name: String,
    commit: String,
    short_commit: String,
    target_path_str: String,
    target_branch: Option<String>,
}

impl PostRemoveContext {
    pub fn new(
        removed_worktree_path: &Path,
        removed_commit: Option<&str>,
        main_path: &Path,
        repo: &Repository,
    ) -> Self {
        let worktree_path_str = to_posix_path(&removed_worktree_path.to_string_lossy());
        let worktree_name = removed_worktree_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let commit = removed_commit.unwrap_or("").to_string();
        // Empty commit (no removal SHA recorded) skips the short form rather
        // than asking git to abbreviate "". For a real SHA, fall back to the
        // full string only if `rev-parse --short` errors (very rare).
        let short_commit = if commit.is_empty() {
            String::new()
        } else {
            repo.short_sha(&commit).unwrap_or_else(|_| commit.clone())
        };

        // Target vars: where the user ends up after removal (primary worktree).
        // A detached primary worktree leaves `target` unset rather than empty,
        // matching `branch`: `format_variables_table` renders an absent var as
        // `(unset)` — "the operation couldn't supply this" — and that label only
        // holds while a branch var nobody can name stays out of the map.
        let target_path_str = to_posix_path(&main_path.to_string_lossy());
        let target_branch = repo.worktree_at(main_path).branch().ok().flatten();

        Self {
            worktree_path_str,
            worktree_name,
            commit,
            short_commit,
            target_path_str,
            target_branch,
        }
    }

    /// Build extra_vars that override the base context with removed-worktree identity.
    ///
    /// `removed_branch` is borrowed from the caller (it outlives the returned Vec).
    /// It is `None` when the removed worktree was detached: `extra_vars` is
    /// applied unconditionally at the end of `build_hook_context`, so emitting a
    /// `"HEAD"` literal here would overwrite the unset `branch` that context
    /// deliberately leaves out (issue #4009). `target` is skipped the same way
    /// when the primary worktree the removal lands in is itself detached.
    pub fn extra_vars<'a>(&'a self, removed_branch: Option<&'a str>) -> Vec<(&'a str, &'a str)> {
        let mut vars = vec![
            ("worktree_path", &*self.worktree_path_str),
            ("worktree", &self.worktree_path_str), // deprecated alias
            ("worktree_name", &self.worktree_name),
            ("commit", &self.commit),
            ("short_commit", &self.short_commit),
            ("target_worktree_path", &self.target_path_str),
        ];
        if let Some(target) = self.target_branch.as_deref() {
            vars.push(("target", target));
        }
        if let Some(branch) = removed_branch {
            vars.push(("branch", branch));
        }
        vars
    }
}
