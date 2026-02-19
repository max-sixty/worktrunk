//! Resolved configuration with all merging applied.
//!
//! `ResolvedConfig` holds the merged configuration for a specific project context.
//! Config types provide accessor methods that apply defaults, so callers use
//! `resolved.list.full()` instead of `resolved.list.full.unwrap_or(false)`.

use super::UserConfig;
use super::sections::{
    CommitConfig, CommitGenerationConfig, ListConfig, MergeConfig, SwitchPickerConfig,
};

/// All resolved configuration for a specific project context.
///
/// Holds merged Config types (global + per-project). Use accessor methods
/// on each config to get values with defaults applied.
///
/// # Example
/// ```ignore
/// let resolved = config.resolved(project);
/// let full = resolved.list.full();                          // bool, default applied
/// let squash = resolved.merge.squash();                     // bool, default applied
/// let stage = resolved.commit.stage();                      // StageMode, default applied
/// let pager = resolved.switch_picker.pager();               // Option<&str>
/// let timeout = resolved.switch_picker.picker_command_timeout(); // Option<Duration>
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedConfig {
    pub list: ListConfig,
    pub merge: MergeConfig,
    pub commit: CommitConfig,
    /// Resolved commit generation config (handles deprecated `[commit-generation]` fallback)
    pub commit_generation: CommitGenerationConfig,
    /// Resolved switch picker config (handles deprecated `[select]` fallback)
    pub switch_picker: SwitchPickerConfig,
}

impl ResolvedConfig {
    /// Resolve all configuration for a project.
    pub fn for_project(config: &UserConfig, project: Option<&str>) -> Self {
        Self {
            list: config.list(project).unwrap_or_default(),
            merge: config.merge(project).unwrap_or_default(),
            commit: config.commit(project).unwrap_or_default(),
            commit_generation: config.commit_generation(project),
            switch_picker: config.switch_picker(project),
        }
    }
}
