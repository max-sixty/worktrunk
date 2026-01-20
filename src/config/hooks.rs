use serde::{Deserialize, Serialize};

use crate::git::HookType;

use super::commands::CommandConfig;

/// Shared hook configuration for user and project configs.
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq)]
pub struct HooksConfig {
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
