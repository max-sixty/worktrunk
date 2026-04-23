//! Shared LLM summary generation for branches.
//!
//! Generates branch summaries using the configured LLM command, with caching
//! in `.git/wt/cache/summaries/{sanitized_branch}/{diff_hash}.json`. The
//! combined diff (branch diff + working tree diff) is hashed with SHA-256
//! and the hex-truncated digest becomes the filename — so a cache hit is
//! just "does the file exist?" rather than parsing JSON to compare a field.
//!
//! Used by both `wt list --full` (Summary column) and `wt switch` (preview tab).
//!
//! # Layout and invalidation
//!
//! Content-addressed filename: a successful write to `{hash}.json` means that
//! file holds the summary for that exact diff. No TTL, no separate staleness
//! check — see [`worktrunk::cache`] for the shared torn-write semantics.
//!
//! # Prune on write
//!
//! After a successful write, sibling entries in the branch directory that
//! don't match the new hash are deleted. For a given branch there is only
//! one "current" diff — old hashes are dead weight. This keeps the cache
//! bounded at ~1 entry per branch without an LRU sweep.
//!
//! Prune is best-effort: two concurrent writers for the same branch can
//! briefly leave two entries, and a sweep racing a just-written sibling
//! can delete it. Worst case is one extra LLM call on the next invocation.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anstyle::Reset;
use color_print::cformat;
use minijinja::Environment;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use worktrunk::cache;
use worktrunk::git::Repository;
use worktrunk::path::sanitize_for_filename;
use worktrunk::styling::INFO_SYMBOL;
use worktrunk::sync::Semaphore;
use worktrunk::utils::epoch_now;

use crate::llm::{execute_llm_command, prepare_diff};

/// Limits concurrent LLM calls to avoid overwhelming the network / LLM
/// provider. 8 permits balances parallelism with resource usage — LLM calls
/// are I/O-bound (1-5s network waits), so more permits than the CPU-bound
/// `HEAVY_OPS_SEMAPHORE` (4) but still bounded.
static LLM_SEMAPHORE: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(8));

/// Cached summary stored in `.git/wt/cache/summaries/{branch}/{hash}.json`.
///
/// The diff hash lives in the filename; the branch lives in the directory
/// name. The `branch` field holds the original (un-sanitized) branch name
/// so `list_all` can surface it for display without needing to reverse
/// `sanitize_for_filename`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CachedSummary {
    pub summary: String,
    /// Original branch name (for display in `wt config state get`).
    pub branch: String,
    /// Unix timestamp when the summary was generated. Used for the "Age"
    /// column in `wt config state get`. Mirrors `CachedCiStatus::checked_at`.
    #[serde(default)]
    pub generated_at: u64,
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
const SUMMARY_TEMPLATE: &str = r#"<task>Write a summary of this branch's changes as a commit message.</task>

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

impl CachedSummary {
    /// Root of all summary cache entries: `.git/wt/cache/summaries/`.
    pub(crate) fn cache_root(repo: &Repository) -> PathBuf {
        cache::cache_dir(repo, "summaries")
    }

    /// Per-branch directory holding one file per cached diff hash.
    fn branch_dir(repo: &Repository, branch: &str) -> PathBuf {
        Self::cache_root(repo).join(sanitize_for_filename(branch))
    }

    /// Full path for a specific (branch, hash) pair.
    pub(crate) fn cache_file(repo: &Repository, branch: &str, hash: &str) -> PathBuf {
        Self::branch_dir(repo, branch).join(format!("{hash}.json"))
    }

    /// Read the cached summary for a branch at a specific diff hash.
    /// See [`worktrunk::cache::read_json`] for the miss semantics.
    pub(crate) fn read(repo: &Repository, branch: &str, hash: &str) -> Option<Self> {
        cache::read_json(&Self::cache_file(repo, branch, hash))
    }

    /// Write the summary at `{branch}/{hash}.json` and prune sibling hashes
    /// for the same branch. `hash` is the SHA-256 digest of the combined
    /// diff that produced this summary.
    pub(crate) fn write(&self, repo: &Repository, hash: &str) {
        let path = Self::cache_file(repo, &self.branch, hash);
        cache::write_json(&path, self);
        prune_siblings(&Self::branch_dir(repo, &self.branch), &path);
    }

    /// List one cached summary per branch — the freshest by `generated_at`
    /// when multiple entries coexist transiently from concurrent writers.
    ///
    /// Used by `wt config state get` to render the SUMMARIES CACHE table.
    /// Missing cache root returns an empty list, matching `CachedCiStatus::list_all`.
    pub(crate) fn list_all(repo: &Repository) -> Vec<Self> {
        let root = Self::cache_root(repo);
        let Ok(branch_dirs) = fs::read_dir(&root) else {
            return Vec::new();
        };

        branch_dirs
            .filter_map(|e| e.ok())
            .filter_map(|entry| {
                if !entry.file_type().ok()?.is_dir() {
                    return None;
                }
                freshest_entry(&entry.path())
            })
            .collect()
    }

    /// Clear all cached summaries, returning the count of `.json` entries removed.
    ///
    /// Summaries are two levels deep (`summaries/{branch}/{hash}.json`), so
    /// iterate branch subdirs and delegate per-directory cleanup to
    /// [`cache::clear_json_files`]. Non-directory entries at the root are
    /// skipped. Empty branch dirs get best-effort rmdir so the tree stays
    /// tidy.
    pub(crate) fn clear_all(repo: &Repository) -> anyhow::Result<usize> {
        let root = Self::cache_root(repo);
        let branch_dirs = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => {
                return Err(
                    anyhow::Error::new(e).context(format!("failed to read {}", root.display()))
                );
            }
        };

        let mut cleared = 0;
        for entry in branch_dirs.flatten() {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let dir = entry.path();
            cleared += cache::clear_json_files(&dir)?;
            let _ = fs::remove_dir(&dir);
        }
        Ok(cleared)
    }
}

/// Load every `.json` cache entry in a branch directory and return the one
/// with the newest `generated_at`. Corrupt entries are skipped.
fn freshest_entry(dir: &Path) -> Option<CachedSummary> {
    fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_str().is_some_and(|s| s.ends_with(".json")))
        .filter_map(|e| cache::read_json::<CachedSummary>(&e.path()))
        .max_by_key(|s| s.generated_at)
}

/// Delete sibling `.json` entries in `dir` that aren't `keep`. Errors are
/// swallowed — prune is best-effort and the cache stays correct even if
/// stale entries linger.
fn prune_siblings(dir: &Path, keep: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == keep {
            continue;
        }
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let _ = fs::remove_file(&path);
    }
}

/// Hash a string with SHA-256 and return the first 16 hex chars (64 bits).
///
/// 64 bits is more than enough entropy to make collisions astronomically
/// unlikely at realistic cache sizes. We truncate for shorter, friendlier
/// filenames — the full 256-bit digest would produce 64-char filenames
/// with no practical benefit.
///
/// `DefaultHasher` (previously used) is explicitly undocumented as
/// stable across Rust versions, which is unsafe for a value persisted
/// to disk. SHA-256 is deterministic across toolchains and platforms.
pub(crate) fn hash_diff(diff: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut hasher = Sha256::new();
    hasher.update(diff.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(16);
    for &b in digest.iter().take(8) {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
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

    // Cache hit is "does the file exist?" — no JSON parse needed for the
    // staleness check because the hash lives in the filename.
    if let Some(cached) = CachedSummary::read(repo, branch, &diff_hash) {
        return Ok(Some(cached.summary));
    }

    // Prepare diff (filter large diffs)
    let prepared = prepare_diff(combined.diff, combined.stat);
    let prompt = render_prompt(&prepared.diff, &prepared.stat)?;

    // Acquire the LLM permit only around the actual LLM call. The no-changes
    // and cache-hit fast paths above return without contending — otherwise a
    // clean `main` branch sits behind up to 8 slow summary calls and misses
    // the picker's collect deadline, surfacing as a `·` in the Summary column.
    let _permit = LLM_SEMAPHORE.acquire();
    let summary = execute_llm_command(llm_command, &prompt)?;

    let cached = CachedSummary {
        summary: summary.clone(),
        branch: branch.to_string(),
        generated_at: epoch_now(),
    };
    cached.write(repo, &diff_hash);

    Ok(Some(summary))
}

/// Generate a summary for a single branch, using cache when available.
///
/// This is the TUI-friendly wrapper that returns a formatted string for all cases,
/// including errors and "no changes" — suitable for `wt switch` preview pane.
#[cfg_attr(windows, allow(dead_code))] // Called from picker module (unix-only)
pub(crate) fn generate_summary(
    branch: &str,
    head: &str,
    worktree_path: Option<&Path>,
    llm_command: &str,
    repo: &Repository,
) -> String {
    match generate_summary_core(branch, head, worktree_path, llm_command, repo) {
        Ok(Some(summary)) => summary,
        Ok(None) => {
            let reset = Reset;
            cformat!("{INFO_SYMBOL}{reset} <bold>{branch}</>{reset} has no changes to summarize\n")
        }
        Err(e) => format!("Error: {e:#}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_prompt_includes_diff_and_stat() {
        let result = render_prompt("diff content here", "stat content here").unwrap();
        insta::assert_snapshot!(result, @r#"
        <task>Write a summary of this branch's changes as a commit message.</task>

        <format>
        - Subject line under 50 chars, imperative mood ("Add feature" not "Adds feature")
        - Blank line, then a body paragraph or bullet list explaining the key changes
        - Output only the message — no quotes, code blocks, or labels
        </format>

        <diffstat>
        stat content here
        </diffstat>

        <diff>
        diff content here
        </diff>
        "#);
    }

    #[test]
    fn test_hash_diff_is_sha256_prefix() {
        // SHA-256("hello world") hex prefix is "b94d27b9934d3e08".
        assert_eq!(hash_diff("hello world"), "b94d27b9934d3e08");
        // Deterministic.
        assert_eq!(hash_diff("hello world"), hash_diff("hello world"));
        // Different inputs → different hashes.
        assert_ne!(hash_diff("hello"), hash_diff("world"));
    }
}
