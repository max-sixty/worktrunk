//! Bundle type for loading user and project config together.
//!
//! [`LoadedConfigs::load`] returns borrows into the per-`Repository` cache
//! for both fields. By the time anything reaches it, the user-config and
//! `git config --list -z` reads have both completed in
//! [`Repository::prewarm`]'s parallel scope, and `project_identifier()` is
//! a memory-only walk over the preloaded git-config map. Only the small
//! `.config/wt.toml` read (~30 µs) still happens here, sequentially.
//!
//! ## When to use
//!
//! Call [`LoadedConfigs::load`] from command handlers that consume both
//! configs — alias dispatch, `wt config alias show`/`dry-run`,
//! `wt hook show`, hook execution, picker post-switch. Sites that only
//! consume `UserConfig` (e.g. `wt step eval`, `for-each`, `prune`,
//! `relocate`) call [`UserConfig::load`] directly so they don't trigger
//! `.config/wt.toml` reads or project-config deprecation warnings.
//!
//! ## Why not return a merged config?
//!
//! User and project configs serve different roles — user config is trusted,
//! project config requires command approval. Downstream merges
//! (`load_aliases`, hook resolution) keep the source distinction so
//! per-source policy can be applied. A flattened merged struct would erase
//! that. Methods that walk both sources with the right precedence belong on
//! `LoadedConfigs` itself as the bundle grows.
//!
//! ## Warning ordering
//!
//! `UserConfig` warnings (parse failures, env-var rejections) emit from the
//! prewarm thread that runs before alias dispatch. Project-config warnings
//! emit sequentially from `repo.project_config()` here. Both routes funnel
//! to the same stderr in deterministic order — user-config warnings first
//! (prewarm joined before any command runs), then project-config warnings.

use anyhow::Result;

use crate::git::Repository;

use super::{ProjectConfig, UserConfig};

/// User and project configs borrowed together from `repo`'s cache.
///
/// `project` is `None` when the repo has no `.config/wt.toml`. Lifetime
/// `'r` is tied to the `Repository` whose cache the references point into.
pub struct LoadedConfigs<'r> {
    pub user: &'r UserConfig,
    pub project: Option<&'r ProjectConfig>,
}

impl<'r> LoadedConfigs<'r> {
    /// Returns user- and project-config references, both pre-warmed.
    ///
    /// `user_config` is preloaded by [`Repository::prewarm`] in parallel
    /// with the two git forks, so it's a memory-only hit here. The
    /// `project_config` file read is a few-tens-of-µs sequential step.
    pub fn load(repo: &'r Repository) -> Result<Self> {
        Ok(Self {
            user: repo.user_config(),
            project: repo.project_config()?,
        })
    }
}
