//! Shared LLM summary generation for branches.
//!
//! Generates branch summaries using the configured LLM command, with caching
//! in `.git/wt-cache/summaries/`. Summaries are invalidated when the combined
//! diff (branch diff + working tree diff) changes.
//!
//! Used by both `wt list --full` (Summary column) and `wt switch` (preview tab).

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use color_print::cformat;
use minijinja::Environment;
use serde::{Deserialize, Serialize};
use worktrunk::git::Repository;
use worktrunk::path::sanitize_for_filename;
use worktrunk::sync::Semaphore;

use crate::llm::{execute_llm_command, prepare_diff};

/// Limits concurrent LLM calls to avoid overwhelming the network / LLM
/// provider. 8 permits balances parallelism with resource usage — LLM calls
/// are I/O-bound (1-5s network waits), so more permits than the CPU-bound
/// `HEAVY_OPS_SEMAPHORE` (4) but still bounded.
pub(crate) static LLM_SEMAPHORE: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(8));

/// Cached summary stored in `.git/wt-cache/summaries/<branch>.json`
#[derive(Serialize, Deserialize)]
pub(crate) struct CachedSummary {
    pub summary: String,
    pub diff_hash: u64,
    /// Original branch name (useful for humans inspecting cache files)
    pub branch: String,
}

/// Combined diff output for a branch (branch diff + working tree diff)
pub(crate) struct CombinedDiff {
    pub diff: String,
    pub stat: String,
}

/// Template for summary generation.
///
/// Uses commit-message format (subject + body) which naturally produces
/// imperative-mood summaries without "This branch..." preamble.
const SUMMARY_TEMPLATE: &str = r#"Write a summary of this branch's changes as a commit message.

<format>
- Subject line under 50 chars, imperative mood ("Add feature" not "Adds feature")
- Blank line, then a body paragraph or bullet list explaining the key changes
- Output only the message — no quotes, code blocks, or labels
</format>

<diffstat>
{{ git_diff_stat }}
</diffstat>

<diff>
{{ git_diff }}
</diff>
"#;

/// Get the cache directory for summaries
pub(crate) fn cache_dir(repo: &Repository) -> PathBuf {
    repo.git_common_dir().join("wt-cache").join("summaries")
}

/// Get the cache file path for a branch
pub(crate) fn cache_file(repo: &Repository, branch: &str) -> PathBuf {
    let safe_branch = sanitize_for_filename(branch);
    cache_dir(repo).join(format!("{safe_branch}.json"))
}

/// Read cached summary from file
pub(crate) fn read_cache(repo: &Repository, branch: &str) -> Option<CachedSummary> {
    let path = cache_file(repo, branch);
    let json = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&json).ok()
}

/// Write summary to cache file (atomic write via temp file + rename)
pub(crate) fn write_cache(repo: &Repository, branch: &str, cached: &CachedSummary) {
    let path = cache_file(repo, branch);

    if let Some(parent) = path.parent()
        && let Err(e) = fs::create_dir_all(parent)
    {
        log::debug!("Failed to create summary cache dir for {}: {}", branch, e);
        return;
    }

    let Ok(json) = serde_json::to_string(cached) else {
        log::debug!("Failed to serialize summary cache for {}", branch);
        return;
    };

    let temp_path = path.with_extension("json.tmp");
    if let Err(e) = fs::write(&temp_path, &json) {
        log::debug!(
            "Failed to write summary cache temp file for {}: {}",
            branch,
            e
        );
        return;
    }

    #[cfg(windows)]
    let _ = fs::remove_file(&path);

    if let Err(e) = fs::rename(&temp_path, &path) {
        log::debug!("Failed to rename summary cache file for {}: {}", branch, e);
        let _ = fs::remove_file(&temp_path);
    }
}

/// Hash a string to produce a cache invalidation key
pub(crate) fn hash_diff(diff: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    diff.hash(&mut hasher);
    hasher.finish()
}

/// Compute the combined diff for a branch (branch diff + working tree diff).
///
/// Returns None if there's nothing to summarize (default branch with no changes,
/// or no default branch known and no working tree diff available).
pub(crate) fn compute_combined_diff(
    branch: &str,
    head: &str,
    worktree_path: Option<&Path>,
    repo: &Repository,
) -> Option<CombinedDiff> {
    let default_branch = repo.default_branch();

    let mut diff = String::new();
    let mut stat = String::new();

    // Branch diff: what's ahead of default branch (skipped if default branch unknown)
    if let Some(ref default_branch) = default_branch {
        let is_default_branch = branch == *default_branch;
        if !is_default_branch {
            let merge_base = format!("{}...{}", default_branch, head);
            if let Ok(branch_stat) = repo.run_command(&["diff", &merge_base, "--stat"]) {
                stat.push_str(&branch_stat);
            }
            if let Ok(branch_diff) = repo.run_command(&["diff", &merge_base]) {
                diff.push_str(&branch_diff);
            }
        }
    }

    // Working tree diff: uncommitted changes
    if let Some(wt_path) = worktree_path {
        let path = wt_path.display().to_string();
        if let Ok(wt_stat) = repo.run_command(&["-C", &path, "diff", "HEAD", "--stat"])
            && !wt_stat.trim().is_empty()
        {
            stat.push_str(&wt_stat);
        }
        if let Ok(wt_diff) = repo.run_command(&["-C", &path, "diff", "HEAD"])
            && !wt_diff.trim().is_empty()
        {
            diff.push_str(&wt_diff);
        }
    }

    if diff.trim().is_empty() {
        return None;
    }

    Some(CombinedDiff { diff, stat })
}

/// Render the summary prompt template
pub(crate) fn render_prompt(diff: &str, stat: &str) -> anyhow::Result<String> {
    let env = Environment::new();
    let tmpl = env.template_from_str(SUMMARY_TEMPLATE)?;
    let rendered = tmpl.render(minijinja::context! {
        git_diff => diff,
        git_diff_stat => stat,
    })?;
    Ok(rendered)
}

/// Core summary generation pipeline: diff → cache check → LLM → cache write.
///
/// Returns `Ok(None)` when there are no changes to summarize (e.g., default branch
/// with clean worktree). Returns `Ok(Some(full_summary))` on success. Errors
/// propagate from template rendering or LLM execution.
///
/// Both `generate_summary` (TUI) and `SummaryGenerateTask` (list column) delegate
/// to this function, wrapping its result with their own error formatting.
pub(crate) fn generate_summary_core(
    branch: &str,
    head: &str,
    worktree_path: Option<&Path>,
    llm_command: &str,
    repo: &Repository,
) -> anyhow::Result<Option<String>> {
    let Some(combined) = compute_combined_diff(branch, head, worktree_path, repo) else {
        return Ok(None);
    };

    let diff_hash = hash_diff(&combined.diff);

    // Check cache
    if let Some(cached) = read_cache(repo, branch)
        && cached.diff_hash == diff_hash
    {
        return Ok(Some(cached.summary));
    }

    // Prepare diff (filter large diffs)
    let prepared = prepare_diff(combined.diff, combined.stat);
    let prompt = render_prompt(&prepared.diff, &prepared.stat)?;
    let summary = execute_llm_command(llm_command, &prompt)?;

    // Write cache
    write_cache(
        repo,
        branch,
        &CachedSummary {
            summary: summary.clone(),
            diff_hash,
            branch: branch.to_string(),
        },
    );

    Ok(Some(summary))
}

/// Generate a summary for a single branch, using cache when available.
///
/// This is the TUI-friendly wrapper that returns a formatted string for all cases,
/// including errors and "no changes" — suitable for `wt switch` preview pane.
#[cfg_attr(windows, allow(dead_code))] // Called from select module (unix-only)
pub(crate) fn generate_summary(
    branch: &str,
    head: &str,
    worktree_path: Option<&Path>,
    llm_command: &str,
    repo: &Repository,
) -> String {
    match generate_summary_core(branch, head, worktree_path, llm_command, repo) {
        Ok(Some(summary)) => summary,
        Ok(None) => cformat!("<dim>No changes to summarize on {branch}.</>"),
        Err(e) => format!("Error: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_prompt_includes_diff_and_stat() {
        let result = render_prompt("diff content here", "stat content here").unwrap();
        assert!(result.contains("diff content here"));
        assert!(result.contains("stat content here"));
    }

    #[test]
    fn test_hash_diff_deterministic() {
        let h1 = hash_diff("hello world");
        let h2 = hash_diff("hello world");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_diff_different_inputs() {
        let h1 = hash_diff("hello");
        let h2 = hash_diff("world");
        assert_ne!(h1, h2);
    }
}
