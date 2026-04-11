//! Detached cache-warming subcommand for the picker.
//!
//! Spawned by `wt switch` (interactive picker) at startup as a fully detached
//! child process. Runs every expensive task in `wt list` against the current
//! repository with no timeout, then exits. The cache writes happen as side
//! effects of the task computations themselves — `MergeTreeConflictsTask` and
//! `WouldMergeAddTask` write to `probe_cache` inside `compute()`, before
//! sending their results.
//!
//! ## Why this exists
//!
//! The picker uses a 500ms wall-clock budget for its own data collection. Any
//! tasks that don't finish in that window keep running on the picker's worker
//! thread, but the process eventually exits after the user makes a selection
//! and the switch completes. Tasks mid-execution at exit are killed and never
//! write their cache entries.
//!
//! Spawning this command detached at picker *startup* gives a separate process
//! the full skim-interaction window plus arbitrary post-exit time to finish
//! every task and populate the cache for the next invocation. Both processes
//! contend for the same `probe_cache` files; concurrent writes of the same
//! key produce the same value (commit SHAs are content-addressed) so the race
//! is benign.
//!
//! ## Output
//!
//! None. stdout, stderr, and stdin are redirected to `/dev/null` by the
//! parent before spawn so the child can't pollute the user's terminal even
//! if a downstream library writes to a stream directly.

use anyhow::Result;
use worktrunk::git::Repository;

use super::list::collect::{self, ShowConfig};

/// Run every expensive task in `wt list` against the current repository,
/// populating the persistent probe cache. Returns when all work is done.
///
/// Spawned detached from the picker; never invoked directly by users.
pub(crate) fn handle_warm_cache() -> Result<()> {
    let repo = Repository::current()?;

    // Run with no timeout, no rendering, every task enabled (skip_tasks empty),
    // and don't skip stale branches — the whole point is to warm the cache for
    // every (branch, target) pair.
    let _ = collect::collect(
        &repo,
        ShowConfig::Resolved {
            show_branches: true,
            show_remotes: false,
            skip_tasks: std::collections::HashSet::new(),
            command_timeout: None,
            collect_deadline: None,
        },
        false, // show_progress
        false, // render_table
        false, // skip_expensive_for_stale
    )?;

    Ok(())
}
