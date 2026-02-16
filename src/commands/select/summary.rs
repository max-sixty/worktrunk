//! AI summary generation for the interactive selector.
//!
//! Generates branch summaries using the configured LLM command, with caching
//! in `.git/wt-cache/summaries/`. Summaries are invalidated when the combined
//! diff (branch diff + working tree diff) changes.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

use color_print::cformat;
use dashmap::DashMap;
use minijinja::Environment;
use serde::{Deserialize, Serialize};
use worktrunk::git::Repository;
use worktrunk::path::sanitize_for_filename;

use super::super::list::model::ListItem;
use super::items::PreviewCacheKey;
use super::preview::PreviewMode;
use crate::llm::{execute_llm_command, prepare_diff};

/// Cached summary stored in `.git/wt-cache/summaries/<branch>.json`
#[derive(Serialize, Deserialize)]
struct CachedSummary {
    summary: String,
    diff_hash: u64,
    /// Original branch name (useful for humans inspecting cache files)
    branch: String,
}

/// Combined diff output for a branch (branch diff + working tree diff)
struct CombinedDiff {
    diff: String,
    stat: String,
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

/// Render LLM summary for terminal display using the project's markdown theme.
///
/// Promotes the first line to an H4 header (renders bold) so the commit-message
/// subject line stands out, then renders everything through the standard
/// markdown renderer used by `--help` pages.
///
/// Pre-styled text (containing ANSI escapes) is passed through with word
/// wrapping only — no H4 promotion.
pub(super) fn render_summary(text: &str, width: usize) -> String {
    // Already styled (e.g. dim "no changes" message) — just wrap
    if text.contains('\x1b') {
        return crate::md_help::render_markdown_in_help_with_width(text, Some(width));
    }

    // Promote subject line to H4 (bold) for visual hierarchy
    let markdown = if let Some((subject, body)) = text.split_once('\n') {
        format!("#### {subject}\n{body}")
    } else {
        format!("#### {text}")
    };

    crate::md_help::render_markdown_in_help_with_width(&markdown, Some(width))
}

/// Get the cache directory for summaries
fn cache_dir(repo: &Repository) -> PathBuf {
    repo.git_common_dir().join("wt-cache").join("summaries")
}

/// Get the cache file path for a branch
fn cache_file(repo: &Repository, branch: &str) -> PathBuf {
    let safe_branch = sanitize_for_filename(branch);
    cache_dir(repo).join(format!("{safe_branch}.json"))
}

/// Read cached summary from file
fn read_cache(repo: &Repository, branch: &str) -> Option<CachedSummary> {
    let path = cache_file(repo, branch);
    let json = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&json).ok()
}

/// Write summary to cache file (atomic write via temp file + rename)
fn write_cache(repo: &Repository, branch: &str, cached: &CachedSummary) {
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
fn hash_diff(diff: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    diff.hash(&mut hasher);
    hasher.finish()
}

/// Compute the combined diff for a branch (branch diff + working tree diff).
///
/// Returns None if there's nothing to summarize (default branch with no changes).
fn compute_combined_diff(item: &ListItem, repo: &Repository) -> Option<CombinedDiff> {
    let branch = item.branch_name();
    let default_branch = repo.default_branch()?;

    let mut diff = String::new();
    let mut stat = String::new();

    // Branch diff: what's ahead of default branch
    let is_default_branch = branch == default_branch;
    if !is_default_branch {
        let merge_base = format!("{}...{}", default_branch, item.head());
        if let Ok(branch_stat) = repo.run_command(&["diff", &merge_base, "--stat"]) {
            stat.push_str(&branch_stat);
        }
        if let Ok(branch_diff) = repo.run_command(&["diff", &merge_base]) {
            diff.push_str(&branch_diff);
        }
    }

    // Working tree diff: uncommitted changes
    if let Some(wt_data) = item.worktree_data() {
        let path = wt_data.path.display().to_string();
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
fn render_prompt(diff: &str, stat: &str) -> anyhow::Result<String> {
    let env = Environment::new();
    let tmpl = env.template_from_str(SUMMARY_TEMPLATE)?;
    let rendered = tmpl.render(minijinja::context! {
        git_diff => diff,
        git_diff_stat => stat,
    })?;
    Ok(rendered)
}

/// Generate a summary for a single item, using cache when available.
fn generate_summary(item: &ListItem, llm_command: &str, repo: &Repository) -> String {
    let branch = item.branch_name();

    // Compute combined diff
    let Some(combined) = compute_combined_diff(item, repo) else {
        return cformat!("<dim>No changes to summarize on {branch}.</>");
    };

    let diff_hash = hash_diff(&combined.diff);

    // Check cache
    if let Some(cached) = read_cache(repo, branch)
        && cached.diff_hash == diff_hash
    {
        return cached.summary;
    }

    // Prepare diff (filter large diffs)
    let prepared = prepare_diff(combined.diff, combined.stat);

    // Render template
    let prompt = match render_prompt(&prepared.diff, &prepared.stat) {
        Ok(p) => p,
        Err(e) => return format!("Template error: {e}"),
    };

    // Call LLM
    let summary = match execute_llm_command(llm_command, &prompt) {
        Ok(s) => s,
        Err(e) => return format!("LLM error: {e}"),
    };

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

    summary
}

/// Generate summaries for all items in parallel, inserting results into the preview cache.
///
/// Uses `std::thread::scope` for maximum I/O concurrency — LLM calls are I/O-bound
/// (1-5s network waits), so one thread per item outperforms a CPU-sized thread pool.
pub(super) fn generate_all_summaries(
    items: &[Arc<ListItem>],
    llm_command: &str,
    preview_cache: &Arc<DashMap<PreviewCacheKey, String>>,
    repo: &Repository,
) {
    std::thread::scope(|s| {
        for item in items {
            let item = Arc::clone(item);
            let cache = Arc::clone(preview_cache);
            s.spawn(move || {
                let branch = item.branch_name().to_string();
                let summary = generate_summary(&item, llm_command, repo);
                cache.insert((branch, PreviewMode::Summary), summary);
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_diff_deterministic() {
        let hash1 = hash_diff("some diff content");
        let hash2 = hash_diff("some diff content");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_diff_different_inputs() {
        let hash1 = hash_diff("diff A");
        let hash2 = hash_diff("diff B");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_render_prompt() {
        let result = render_prompt("diff content", "1 file changed");
        assert!(result.is_ok());
        let prompt = result.unwrap();
        assert!(prompt.contains("diff content"));
        assert!(prompt.contains("1 file changed"));
    }

    #[test]
    fn test_render_prompt_commit_message_format() {
        let result = render_prompt("", "").unwrap();
        assert!(result.contains("commit message"));
        assert!(result.contains("imperative mood"));
    }

    #[test]
    fn test_render_summary_subject_bold() {
        let text = "Add new feature\n\nSome body text here.";
        let rendered = render_summary(text, 80);
        // Subject line rendered as H4 (bold)
        assert!(rendered.contains("\x1b[1m"));
        assert!(rendered.contains("Add new feature"));
    }

    #[test]
    fn test_render_summary_wraps_body() {
        let text = format!("Subject\n\n{}", "word ".repeat(30));
        let rendered = render_summary(&text, 40);
        // Body should wrap (subject + blank + multiple wrapped lines)
        assert!(rendered.lines().count() > 3);
    }

    #[test]
    fn test_render_summary_body_preserved() {
        let text = "Subject\n\n- First bullet\n- Second bullet";
        let rendered = render_summary(text, 80);
        assert!(rendered.contains("First bullet"));
        assert!(rendered.contains("Second bullet"));
    }
}
