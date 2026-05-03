//! Bundle type for loading user and project config together.
//!
//! [`LoadedConfigs::load`] runs the user-config disk load, the project-config
//! disk load, and a `project_identifier` cache warm-up on three scoped
//! threads. All three share `Repository`'s `Arc<RepoCache>` — clones are
//! cheap, and the `OnceCell`/`DashMap` entries each thread populates are
//! visible to every other clone (and to subsequent cache reads).
//!
//! ## When to use
//!
//! Call [`LoadedConfigs::load`] from command handlers that consume both
//! configs — alias dispatch, `wt config alias show`/`dry-run`,
//! `wt hook show`, hook execution, and any path whose downstream code calls
//! `Repository::load_project_config`.
//!
//! Sites that only consume `UserConfig` should call [`UserConfig::load`]
//! directly. The bundle loader reads `.config/wt.toml` and would emit
//! deprecation/unknown-field warnings from project config in commands that
//! never use it.
//!
//! ## Why not return a merged config?
//!
//! User and project configs serve different roles — user config is trusted,
//! project config requires command approval. Downstream merges
//! (`load_aliases`, hook resolution) keep the source distinction so
//! per-source policy can be applied. A flattened merged struct would erase
//! that.
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

/// User and project configs loaded together.
///
/// `project` is `None` when the repo has no `.config/wt.toml`.
pub struct LoadedConfigs {
    pub user: UserConfig,
    pub project: Option<ProjectConfig>,
}

impl LoadedConfigs {
    /// Load user config and project config in parallel; warm
    /// `project_identifier` alongside.
    ///
    /// On a cold cache the wall-clock cost is the longest pole rather than
    /// the sum (~5ms vs ~13ms sequential on a typical project). On a warm
    /// cache every thread is a no-op.
    pub fn load(repo: &Repository) -> Result<Self> {
        std::thread::scope(|s| {
            let user_handle = s.spawn(|| {
                let _span = Span::new("user_config_load");
                UserConfig::load().map_err(anyhow::Error::from)
            });
            let project_repo = repo.clone();
            let project_handle = s.spawn(move || {
                let _span = Span::new("project_config_load");
                project_repo.load_project_config()
            });
            let id_repo = repo.clone();
            let id_handle = s.spawn(move || {
                let _span = Span::new("project_identifier_warm");
                let _ = id_repo.project_identifier();
            });

            let user = user_handle
                .join()
                .map_err(|_| anyhow::anyhow!("user_config thread panicked"))??;
            let project = project_handle
                .join()
                .map_err(|_| anyhow::anyhow!("project_config thread panicked"))??;
            id_handle
                .join()
                .map_err(|_| anyhow::anyhow!("project_identifier thread panicked"))?;
            Ok(Self { user, project })
        })
    }
}
