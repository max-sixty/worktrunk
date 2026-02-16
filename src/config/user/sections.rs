//! Configuration section structs.
//!
//! These structs represent individual configuration sections that can be set
//! globally or per-project. Each implements the `Merge` trait for layering.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::merge::Merge;
use crate::config::HooksConfig;

/// What to stage before committing
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum StageMode {
    /// Stage everything: untracked files + unstaged tracked changes
    #[default]
    All,
    /// Stage tracked changes only (like `git add -u`)
    Tracked,
    /// Stage nothing, commit only what's already in the index
    None,
}

/// Configuration for commit message generation
///
/// The command is a shell string executed via `sh -c`. Environment variables
/// can be set inline (e.g., `MAX_THINKING_TOKENS=0 claude -p ...`).
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, JsonSchema)]
pub struct CommitGenerationConfig {
    /// Shell command to invoke for generating commit messages
    ///
    /// Examples:
    /// - `"llm -m claude-haiku-4.5"`
    /// - `"MAX_THINKING_TOKENS=0 claude -p --model=haiku"`
    ///
    /// The command receives the prompt via stdin and should output the commit message.
    #[serde(default)]
    pub command: Option<String>,

    /// Inline template for commit message prompt
    /// Available variables: {{ git_diff }}, {{ branch }}, {{ recent_commits }}, {{ repo }}
    #[serde(default)]
    pub template: Option<String>,

    /// Path to template file (mutually exclusive with template)
    /// Supports tilde expansion (e.g., "~/.config/worktrunk/commit-template.txt")
    #[serde(default, rename = "template-file")]
    pub template_file: Option<String>,

    /// Inline template for squash commit message prompt
    /// Available variables: {{ commits }}, {{ target_branch }}, {{ branch }}, {{ repo }}
    #[serde(default, rename = "squash-template")]
    pub squash_template: Option<String>,

    /// Path to squash template file (mutually exclusive with squash-template)
    /// Supports tilde expansion (e.g., "~/.config/worktrunk/squash-template.txt")
    #[serde(default, rename = "squash-template-file")]
    pub squash_template_file: Option<String>,
}

impl CommitGenerationConfig {
    /// Returns true if an LLM command is configured
    pub fn is_configured(&self) -> bool {
        self.command
            .as_ref()
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false)
    }
}

impl Merge for CommitGenerationConfig {
    fn merge_with(&self, other: &Self) -> Self {
        // For template/template_file pairs: if project sets one, it clears the other
        // This prevents violating mutual exclusivity when global has one and project has the other
        let (template, template_file) = if other.template.is_some() {
            (other.template.clone(), None)
        } else if other.template_file.is_some() {
            (None, other.template_file.clone())
        } else {
            (self.template.clone(), self.template_file.clone())
        };

        let (squash_template, squash_template_file) = if other.squash_template.is_some() {
            (other.squash_template.clone(), None)
        } else if other.squash_template_file.is_some() {
            (None, other.squash_template_file.clone())
        } else {
            (
                self.squash_template.clone(),
                self.squash_template_file.clone(),
            )
        };

        Self {
            command: other.command.clone().or_else(|| self.command.clone()),
            template,
            template_file,
            squash_template,
            squash_template_file,
        }
    }
}

/// Configuration for the `wt list` command
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default, JsonSchema)]
pub struct ListConfig {
    /// Show CI and `main` diffstat by default
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full: Option<bool>,

    /// Include branches without worktrees by default
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branches: Option<bool>,

    /// Include remote branches by default
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remotes: Option<bool>,

    /// Show AI-generated branch summaries in the interactive picker (tab 5).
    /// Requires `[commit.generation] command` to be configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<bool>,

    /// (Experimental) Per-task timeout in milliseconds.
    /// When set to a positive value, git operations that exceed this timeout are terminated.
    /// Timed-out tasks show defaults in the table. Set to 0 to explicitly disable timeout
    /// (useful to override a global setting). Disabled when --full is used.
    #[serde(rename = "timeout-ms", skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

impl ListConfig {
    /// Show CI and `main` diffstat by default (default: false)
    pub fn full(&self) -> bool {
        self.full.unwrap_or(false)
    }

    /// Include branches without worktrees by default (default: false)
    pub fn branches(&self) -> bool {
        self.branches.unwrap_or(false)
    }

    /// Include remote branches by default (default: false)
    pub fn remotes(&self) -> bool {
        self.remotes.unwrap_or(false)
    }

    /// Show AI-generated branch summaries in picker (default: false)
    pub fn summary(&self) -> bool {
        self.summary.unwrap_or(false)
    }

    /// Per-task timeout in milliseconds (default: None)
    pub fn timeout_ms(&self) -> Option<u64> {
        self.timeout_ms
    }
}

impl Merge for ListConfig {
    fn merge_with(&self, other: &Self) -> Self {
        Self {
            full: other.full.or(self.full),
            branches: other.branches.or(self.branches),
            remotes: other.remotes.or(self.remotes),
            summary: other.summary.or(self.summary),
            timeout_ms: other.timeout_ms.or(self.timeout_ms),
        }
    }
}

/// Configuration for the `wt step commit` command
///
/// Also used by `wt merge` for shared settings like `stage`.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default, JsonSchema)]
pub struct CommitConfig {
    /// What to stage before committing (default: all)
    /// Values: "all", "tracked", "none"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage: Option<StageMode>,

    /// LLM commit message generation settings
    ///
    /// Nested under `[commit.generation]` in TOML.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<CommitGenerationConfig>,
}

impl CommitConfig {
    /// What to stage before committing (default: All)
    pub fn stage(&self) -> StageMode {
        self.stage.unwrap_or_default()
    }
}

impl Merge for CommitConfig {
    fn merge_with(&self, other: &Self) -> Self {
        Self {
            stage: other.stage.or(self.stage),
            generation: match (&self.generation, &other.generation) {
                (None, None) => None,
                (Some(s), None) => Some(s.clone()),
                (None, Some(o)) => Some(o.clone()),
                (Some(s), Some(o)) => Some(s.merge_with(o)),
            },
        }
    }
}

/// Configuration for the `wt merge` command
///
/// Note: `stage` defaults from `[commit]` section, not here.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default, JsonSchema)]
pub struct MergeConfig {
    /// Squash commits when merging (default: true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub squash: Option<bool>,

    /// Commit, squash, and rebase during merge (default: true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<bool>,

    /// Rebase onto target branch before merging (default: true)
    ///
    /// When false, merge fails if branch is not already rebased.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rebase: Option<bool>,

    /// Remove worktree after merge (default: true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remove: Option<bool>,

    /// Run project hooks (default: true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify: Option<bool>,
}

impl MergeConfig {
    /// Squash commits when merging (default: true)
    pub fn squash(&self) -> bool {
        self.squash.unwrap_or(true)
    }

    /// Commit, squash, and rebase during merge (default: true)
    pub fn commit(&self) -> bool {
        self.commit.unwrap_or(true)
    }

    /// Rebase onto target branch before merging (default: true)
    pub fn rebase(&self) -> bool {
        self.rebase.unwrap_or(true)
    }

    /// Remove worktree after merge (default: true)
    pub fn remove(&self) -> bool {
        self.remove.unwrap_or(true)
    }

    /// Run project hooks (default: true)
    pub fn verify(&self) -> bool {
        self.verify.unwrap_or(true)
    }
}

impl Merge for MergeConfig {
    fn merge_with(&self, other: &Self) -> Self {
        Self {
            squash: other.squash.or(self.squash),
            commit: other.commit.or(self.commit),
            rebase: other.rebase.or(self.rebase),
            remove: other.remove.or(self.remove),
            verify: other.verify.or(self.verify),
        }
    }
}

/// Configuration for the `wt switch` interactive picker.
// TODO(#890): Rename to SwitchPickerConfig and [switch.picker] once migration is confirmed
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default, JsonSchema)]
pub struct SelectConfig {
    /// Pager command with flags for diff preview
    ///
    /// Overrides git's core.pager for the interactive picker's preview panel.
    /// Use this to specify pager flags needed for non-TTY contexts.
    ///
    /// Example: `pager = "delta --paging=never"`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pager: Option<String>,
}

impl SelectConfig {
    /// Pager command with flags for diff preview (default: None, uses git default)
    pub fn pager(&self) -> Option<&str> {
        self.pager.as_deref()
    }
}

impl Merge for SelectConfig {
    fn merge_with(&self, other: &Self) -> Self {
        Self {
            pager: other.pager.clone().or_else(|| self.pager.clone()),
        }
    }
}

/// Settings that can be set globally or per-project.
///
/// This struct is flattened into both `UserConfig` (global) and `UserProjectOverrides`
/// (per-project), ensuring new settings are automatically available in both
/// contexts without manual synchronization.
///
/// Note: Hooks use append semantics when merging global with per-project:
/// - Global hooks (top-level in TOML) are in `UserConfig.configs.hooks`
/// - Per-project hooks are in `UserProjectOverrides.overrides.hooks`
/// - The `UserConfig::hooks()` method merges both with global running first
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default, JsonSchema)]
pub struct OverridableConfig {
    /// Hooks configuration.
    ///
    /// At top level: global hooks that run for all projects.
    /// In `[projects."..."]`: per-project hooks that append to global hooks.
    #[serde(flatten, default)]
    pub hooks: HooksConfig,

    /// Worktree path template
    #[serde(
        rename = "worktree-path",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub worktree_path: Option<String>,

    /// Configuration for the `wt list` command
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub list: Option<ListConfig>,

    /// Configuration for the `wt step commit` command (also used by merge)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<CommitConfig>,

    /// Configuration for the `wt merge` command
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge: Option<MergeConfig>,

    /// Configuration for the `wt switch` interactive picker
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<SelectConfig>,
}

impl OverridableConfig {
    /// Returns true if all settings are None/default.
    ///
    /// Includes hooks check for per-project configs where hooks are stored here.
    pub fn is_empty(&self) -> bool {
        self.hooks == HooksConfig::default()
            && self.worktree_path.is_none()
            && self.list.is_none()
            && self.commit.is_none()
            && self.merge.is_none()
            && self.select.is_none()
    }
}

impl Merge for OverridableConfig {
    fn merge_with(&self, other: &Self) -> Self {
        use super::merge::merge_optional;

        Self {
            hooks: self.hooks.merge_with(&other.hooks), // Append semantics
            worktree_path: other
                .worktree_path
                .clone()
                .or_else(|| self.worktree_path.clone()),
            list: merge_optional(self.list.as_ref(), other.list.as_ref()),
            commit: merge_optional(self.commit.as_ref(), other.commit.as_ref()),
            merge: merge_optional(self.merge.as_ref(), other.merge.as_ref()),
            select: merge_optional(self.select.as_ref(), other.select.as_ref()),
        }
    }
}

/// Per-project overrides in the user's config file
///
/// Stored under `[projects."project-id"]` in the user's config.
/// These are user preferences (not checked into git) that override
/// the corresponding global settings when set.
///
/// # TOML Format
/// ```toml
/// [projects."github.com/user/repo"]
/// worktree-path = ".worktrees/{{ branch | sanitize }}"
/// approved-commands = ["npm install", "npm test"]
///
/// [projects."github.com/user/repo".commit.generation]
/// command = "llm -m gpt-4"
///
/// [projects."github.com/user/repo".list]
/// full = true
///
/// [projects."github.com/user/repo".merge]
/// squash = false
/// ```
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default, JsonSchema)]
pub struct UserProjectOverrides {
    /// Commands that have been approved for automatic execution in this project
    #[serde(
        default,
        rename = "approved-commands",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub approved_commands: Vec<String>,

    /// **DEPRECATED**: Use `commit.generation` instead.
    ///
    /// Per-project commit generation settings (overrides global `[commit.generation]`)
    #[serde(
        default,
        rename = "commit-generation",
        skip_serializing_if = "Option::is_none"
    )]
    pub commit_generation: Option<CommitGenerationConfig>,

    /// Per-project overrides (worktree-path, list, commit, merge, select)
    #[serde(flatten, default)]
    pub overrides: OverridableConfig,
}

impl UserProjectOverrides {
    /// Returns true if all fields are empty/None (no settings configured).
    ///
    /// Approvals are stored in `approvals.toml`, so `approved_commands` is only
    /// kept here for backward-compatible parsing and migration — not checked.
    pub fn is_empty(&self) -> bool {
        self.commit_generation.is_none() && self.overrides.is_empty()
    }
}
