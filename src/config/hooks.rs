use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::git::HookType;

use super::commands::CommandConfig;

/// Shared hook configuration for user and project configs.
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, JsonSchema)]
pub struct HooksConfig {
    /// Commands to execute before switch begins (blocking, fail-fast)
    #[serde(
        default,
        rename = "pre-switch",
        skip_serializing_if = "Option::is_none"
    )]
    pub pre_switch: Option<CommandConfig>,

    /// Commands to execute after worktree creation (blocking)
    #[serde(
        default,
        rename = "post-create",
        skip_serializing_if = "Option::is_none"
    )]
    pub post_create: Option<CommandConfig>,

    /// Commands to execute after worktree creation (background)
    #[serde(
        default,
        rename = "post-start",
        skip_serializing_if = "Option::is_none"
    )]
    pub post_start: Option<CommandConfig>,

    /// Commands to execute after switching to a worktree (background)
    #[serde(
        default,
        rename = "post-switch",
        skip_serializing_if = "Option::is_none"
    )]
    pub post_switch: Option<CommandConfig>,

    /// Commands to execute before committing during merge (blocking, fail-fast)
    #[serde(
        default,
        rename = "pre-commit",
        skip_serializing_if = "Option::is_none"
    )]
    pub pre_commit: Option<CommandConfig>,

    /// Commands to execute before merging (blocking, fail-fast)
    #[serde(default, rename = "pre-merge", skip_serializing_if = "Option::is_none")]
    pub pre_merge: Option<CommandConfig>,

    /// Commands to execute after successful merge (blocking, best-effort)
    #[serde(
        default,
        rename = "post-merge",
        skip_serializing_if = "Option::is_none"
    )]
    pub post_merge: Option<CommandConfig>,

    /// Commands to execute before worktree removal (blocking, fail-fast)
    #[serde(
        default,
        rename = "pre-remove",
        skip_serializing_if = "Option::is_none"
    )]
    pub pre_remove: Option<CommandConfig>,

    /// Commands to execute after worktree removal (background)
    #[serde(
        default,
        rename = "post-remove",
        skip_serializing_if = "Option::is_none"
    )]
    pub post_remove: Option<CommandConfig>,
}

impl HooksConfig {
    pub fn get(&self, hook: HookType) -> Option<&CommandConfig> {
        match hook {
            HookType::PreSwitch => self.pre_switch.as_ref(),
            HookType::PostCreate => self.post_create.as_ref(),
            HookType::PostStart => self.post_start.as_ref(),
            HookType::PostSwitch => self.post_switch.as_ref(),
            HookType::PreCommit => self.pre_commit.as_ref(),
            HookType::PreMerge => self.pre_merge.as_ref(),
            HookType::PostMerge => self.post_merge.as_ref(),
            HookType::PreRemove => self.pre_remove.as_ref(),
            HookType::PostRemove => self.post_remove.as_ref(),
        }
    }
}

use super::user::Merge;

/// Merge two optional command configs by appending (base commands first, then overlay).
fn merge_append_hooks(
    base: &Option<CommandConfig>,
    overlay: &Option<CommandConfig>,
) -> Option<CommandConfig> {
    match (base, overlay) {
        (None, None) => None,
        (Some(b), None) => Some(b.clone()),
        (None, Some(o)) => Some(o.clone()),
        (Some(b), Some(o)) => Some(b.merge_append(o)),
    }
}

impl Merge for HooksConfig {
    /// Merge two hook configs using append semantics.
    ///
    /// Both global and per-project hooks run (global first, then per-project).
    fn merge_with(&self, other: &Self) -> Self {
        Self {
            pre_switch: merge_append_hooks(&self.pre_switch, &other.pre_switch),
            post_create: merge_append_hooks(&self.post_create, &other.post_create),
            post_start: merge_append_hooks(&self.post_start, &other.post_start),
            post_switch: merge_append_hooks(&self.post_switch, &other.post_switch),
            pre_commit: merge_append_hooks(&self.pre_commit, &other.pre_commit),
            pre_merge: merge_append_hooks(&self.pre_merge, &other.pre_merge),
            post_merge: merge_append_hooks(&self.post_merge, &other.post_merge),
            pre_remove: merge_append_hooks(&self.pre_remove, &other.pre_remove),
            post_remove: merge_append_hooks(&self.post_remove, &other.post_remove),
        }
    }
}
