//! Bundle type for loading user and project config together.
//!
//! [`LoadedConfigs::load`] warms the user-config and project-config caches in
//! parallel on scoped threads, then returns borrows into the per-`Repository`
//! cache. Both fields go through the same caching path, so subsequent
//! `Repository::user_config` / `Repository::project_config` calls are pure
//! cache hits — no second disk read, no asymmetry.
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
//! Both loads emit deprecation/unknown-field warnings to stderr inline.
//! Running them on sibling threads makes warning order nondeterministic
//! when both files have warnings. No existing test fixture exercises both
//! at once. Acceptable trade-off for the parallel savings; revisit with
//! a buffer-and-replay design if the ordering becomes a problem.

use anyhow::Result;

use crate::git::Repository;
use crate::trace::Span;

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
    /// Warm the user- and project-config caches in parallel; warm
    /// `project_identifier` alongside the project load.
    ///
    /// On a cold cache the wall-clock cost is the longest pole rather than
    /// the sum (~6ms vs ~13ms sequential on a typical project). On a warm
    /// cache both threads are no-ops.
    ///
    /// `project_identifier` shares its thread with the project-config load
    /// because both touch the same `Repository` and have no warning
    /// interleave concern; combining them saves a spawn/join for the same
    /// wall pole (the identifier's `git config --list -z` dominates either
    /// way).
    pub fn load(repo: &'r Repository) -> Result<Self> {
        std::thread::scope(|s| {
            s.spawn(|| {
                let _span = Span::new("user_config_load");
                let _ = repo.user_config();
            });
            s.spawn(|| {
                {
                    let _span = Span::new("project_config_load");
                    let _ = repo.project_config();
                }
                {
                    let _span = Span::new("project_identifier_warm");
                    let _ = repo.project_identifier();
                }
            });
        });
        Ok(Self {
            user: repo.user_config(),
            project: repo.project_config()?,
        })
    }
}
