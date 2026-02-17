//! AI summary generation for the interactive selector.
//!
//! Generates branch summaries using the configured LLM command, with caching
//! in `.git/wt-cache/summaries/`. Summaries are invalidated when the combined
//! diff (branch diff + working tree diff) changes.

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::LazyLock;

use color_print::cformat;
use dashmap::DashMap;
use minijinja::Environment;
use serde::{Deserialize, Serialize};
use worktrunk::git::Repository;
use worktrunk::path::sanitize_for_filename;
use worktrunk::sync::Semaphore;

use super::super::list::model::ListItem;
use super::items::PreviewCacheKey;
use super::preview::PreviewMode;
use crate::llm::{execute_llm_command, prepare_diff};

/// Limits concurrent LLM calls to avoid overwhelming the network / LLM
/// provider. 8 permits balances parallelism with resource usage — LLM calls
/// are I/O-bound (1-5s network waits), so more permits than the CPU-bound
/// `HEAVY_OPS_SEMAPHORE` (4) but still bounded.
static LLM_SEMAPHORE: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(8));

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
/// Returns None if there's nothing to summarize (default branch with no changes,
/// or no default branch known and no working tree diff available).
fn compute_combined_diff(item: &ListItem, repo: &Repository) -> Option<CombinedDiff> {
    let branch = item.branch_name();
    let default_branch = repo.default_branch();

    let mut diff = String::new();
    let mut stat = String::new();

    // Branch diff: what's ahead of default branch (skipped if default branch unknown)
    if let Some(ref default_branch) = default_branch {
        let is_default_branch = branch == *default_branch;
        if !is_default_branch {
            let merge_base = format!("{}...{}", default_branch, item.head());
            if let Ok(branch_stat) = repo.run_command(&["diff", &merge_base, "--stat"]) {
                stat.push_str(&branch_stat);
            }
            if let Ok(branch_diff) = repo.run_command(&["diff", &merge_base]) {
                diff.push_str(&branch_diff);
            }
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

/// Generate a summary for one item and insert it into the preview cache.
/// Acquires the LLM semaphore to limit concurrent calls across rayon tasks.
pub(super) fn generate_and_cache_summary(
    item: &ListItem,
    llm_command: &str,
    preview_cache: &DashMap<PreviewCacheKey, String>,
    repo: &Repository,
) {
    let _permit = LLM_SEMAPHORE.acquire();
    let branch = item.branch_name().to_string();
    let summary = generate_summary(item, llm_command, repo);
    preview_cache.insert((branch, PreviewMode::Summary), summary);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::list::model::{ItemKind, WorktreeData};

    fn git_command(dir: &std::path::Path) -> std::process::Command {
        let mut cmd = std::process::Command::new("git");
        cmd.current_dir(dir)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com");
        cmd
    }

    fn git(dir: &std::path::Path, args: &[&str]) {
        let output = git_command(dir).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_output(dir: &std::path::Path, args: &[&str]) -> String {
        let output = git_command(dir).args(args).output().unwrap();
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    /// Create a minimal temp git repo (for cache-only tests that don't need branches).
    fn temp_repo() -> (tempfile::TempDir, Repository) {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--initial-branch=main"]);
        git(dir.path(), &["commit", "--allow-empty", "-m", "init"]);
        let repo = Repository::at(dir.path()).unwrap();
        (dir, repo)
    }

    /// Create a temp repo with main branch, default-branch config, and a real commit.
    fn temp_repo_configured() -> (tempfile::TempDir, Repository, String) {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--initial-branch=main"]);
        git(dir.path(), &["config", "worktrunk.default-branch", "main"]);
        fs::write(dir.path().join("README.md"), "# Project\n").unwrap();
        git(dir.path(), &["add", "README.md"]);
        git(dir.path(), &["commit", "-m", "initial commit"]);
        let head = git_output(dir.path(), &["rev-parse", "HEAD"]);
        let repo = Repository::at(dir.path()).unwrap();
        (dir, repo, head)
    }

    /// Create a temp repo with main + feature branch that has real changes.
    fn temp_repo_with_feature() -> (tempfile::TempDir, Repository, String) {
        let (dir, _, _) = temp_repo_configured();

        git(dir.path(), &["checkout", "-b", "feature"]);
        fs::write(dir.path().join("new.txt"), "new content\n").unwrap();
        git(dir.path(), &["add", "new.txt"]);
        git(dir.path(), &["commit", "-m", "add new file"]);

        let head = git_output(dir.path(), &["rev-parse", "HEAD"]);
        let repo = Repository::at(dir.path()).unwrap();
        (dir, repo, head)
    }

    fn feature_item(head: &str, path: &std::path::Path) -> ListItem {
        let mut item = ListItem::new_branch(head.to_string(), "feature".to_string());
        item.kind = ItemKind::Worktree(Box::new(WorktreeData {
            path: path.to_path_buf(),
            ..Default::default()
        }));
        item
    }

    #[test]
    fn test_cache_roundtrip() {
        let (_dir, repo) = temp_repo();
        let branch = "feature/test-branch";
        let cached = CachedSummary {
            summary: "Add tests\n\nThis adds unit tests for cache.".to_string(),
            diff_hash: 12345,
            branch: branch.to_string(),
        };

        assert!(read_cache(&repo, branch).is_none());

        write_cache(&repo, branch, &cached);
        let loaded = read_cache(&repo, branch).unwrap();
        assert_eq!(loaded.summary, cached.summary);
        assert_eq!(loaded.diff_hash, cached.diff_hash);
        assert_eq!(loaded.branch, cached.branch);
    }

    #[test]
    fn test_write_cache_handles_unwritable_path() {
        let (_dir, repo) = temp_repo();
        // Block cache directory creation by placing a file where the directory should be
        let cache_parent = repo.git_common_dir().join("wt-cache");
        fs::write(&cache_parent, "blocker").unwrap();

        let cached = CachedSummary {
            summary: "test".to_string(),
            diff_hash: 1,
            branch: "main".to_string(),
        };
        // Should not panic — just logs and returns
        write_cache(&repo, "main", &cached);
        assert!(read_cache(&repo, "main").is_none());

        // Cleanup: remove the blocker file so TempDir cleanup works
        fs::remove_file(&cache_parent).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn test_write_cache_handles_write_failure() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, repo) = temp_repo();
        let cache_path = cache_dir(&repo);
        fs::create_dir_all(&cache_path).unwrap();
        // Make directory read-only so file writes fail
        fs::set_permissions(&cache_path, fs::Permissions::from_mode(0o444)).unwrap();

        let cached = CachedSummary {
            summary: "test".to_string(),
            diff_hash: 1,
            branch: "main".to_string(),
        };
        // Should not panic — just logs and returns
        write_cache(&repo, "main", &cached);
        assert!(read_cache(&repo, "main").is_none());

        // Restore permissions so TempDir cleanup works
        fs::set_permissions(&cache_path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn test_cache_invalidation_by_hash() {
        let (_dir, repo) = temp_repo();
        let branch = "main";
        let cached = CachedSummary {
            summary: "Old summary".to_string(),
            diff_hash: 111,
            branch: branch.to_string(),
        };
        write_cache(&repo, branch, &cached);

        let loaded = read_cache(&repo, branch).unwrap();
        assert_ne!(loaded.diff_hash, 222);
    }

    #[test]
    fn test_cache_file_uses_sanitized_branch() {
        let (_dir, repo) = temp_repo();
        let path = cache_file(&repo, "feature/my-branch");
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(filename.starts_with("feature-my-branch-"));
        assert!(filename.ends_with(".json"));
    }

    #[test]
    fn test_cache_dir_under_git() {
        let (_dir, repo) = temp_repo();
        let dir = cache_dir(&repo);
        assert!(dir.to_str().unwrap().contains("wt-cache"));
        assert!(dir.to_str().unwrap().contains("summaries"));
    }

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
        assert!(rendered.contains("\x1b[1m"));
        assert!(rendered.contains("Add new feature"));
    }

    #[test]
    fn test_render_summary_single_line() {
        let text = "Add new feature";
        let rendered = render_summary(text, 80);
        assert!(rendered.contains("\x1b[1m"));
        assert!(rendered.contains("Add new feature"));
    }

    #[test]
    fn test_render_summary_wraps_body() {
        let text = format!("Subject\n\n{}", "word ".repeat(30));
        let rendered = render_summary(&text, 40);
        assert!(rendered.lines().count() > 3);
    }

    #[test]
    fn test_render_summary_body_preserved() {
        let text = "Subject\n\n- First bullet\n- Second bullet";
        let rendered = render_summary(text, 80);
        assert!(rendered.contains("First bullet"));
        assert!(rendered.contains("Second bullet"));
    }

    #[test]
    fn test_render_summary_prestyled_skips_h4() {
        let text = "\x1b[2mNo changes to summarize.\x1b[0m";
        let rendered = render_summary(text, 80);
        assert!(!rendered.contains("####"));
        assert!(rendered.contains("No changes to summarize."));
    }

    #[test]
    fn test_compute_combined_diff_with_branch_changes() {
        let (dir, repo, head) = temp_repo_with_feature();
        let item = feature_item(&head, dir.path());

        let result = compute_combined_diff(&item, &repo);
        assert!(result.is_some());
        let combined = result.unwrap();
        assert!(combined.diff.contains("new.txt"));
        assert!(combined.stat.contains("new.txt"));
    }

    #[test]
    fn test_compute_combined_diff_default_branch_no_changes() {
        let (dir, repo, head) = temp_repo_configured();

        let mut item = ListItem::new_branch(head, "main".to_string());
        item.kind = ItemKind::Worktree(Box::new(WorktreeData {
            path: dir.path().to_path_buf(),
            ..Default::default()
        }));

        let result = compute_combined_diff(&item, &repo);
        assert!(result.is_none());
    }

    #[test]
    fn test_compute_combined_diff_with_uncommitted_changes() {
        let (dir, repo, head) = temp_repo_with_feature();
        // Add uncommitted changes
        fs::write(dir.path().join("uncommitted.txt"), "wip\n").unwrap();
        git(dir.path(), &["add", "uncommitted.txt"]);

        let item = feature_item(&head, dir.path());
        let result = compute_combined_diff(&item, &repo);
        assert!(result.is_some());
        let combined = result.unwrap();
        // Should contain both the branch diff and the working tree diff
        assert!(combined.diff.contains("new.txt"));
        assert!(combined.diff.contains("uncommitted.txt"));
    }

    #[test]
    fn test_compute_combined_diff_branch_only_no_worktree() {
        let (_dir, repo, head) = temp_repo_with_feature();
        // Branch-only item (no worktree data) — only branch diff included
        let item = ListItem::new_branch(head, "feature".to_string());

        let result = compute_combined_diff(&item, &repo);
        assert!(result.is_some());
        let combined = result.unwrap();
        assert!(combined.diff.contains("new.txt"));
    }

    #[test]
    fn test_compute_combined_diff_no_default_branch_with_worktree_changes() {
        // Repo without default-branch config and exotic branch names that
        // infer_default_branch_locally() won't detect (it checks "main",
        // "master", "develop", "trunk"). This ensures default_branch() returns
        // None, exercising the code path where branch diff is skipped.
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--initial-branch=init-branch"]);
        fs::write(dir.path().join("README.md"), "# Project\n").unwrap();
        git(dir.path(), &["add", "README.md"]);
        git(dir.path(), &["commit", "-m", "initial commit"]);
        git(dir.path(), &["checkout", "-b", "feature"]);
        git(
            dir.path(),
            &["commit", "--allow-empty", "-m", "feature commit"],
        );

        // Add uncommitted changes
        fs::write(dir.path().join("wip.txt"), "work in progress\n").unwrap();
        git(dir.path(), &["add", "wip.txt"]);

        let head = git_output(dir.path(), &["rev-parse", "HEAD"]);
        let repo = Repository::at(dir.path()).unwrap();

        // Verify default_branch() actually returns None with these branch names
        assert!(
            repo.default_branch().is_none(),
            "expected no default branch with exotic branch names"
        );

        let item = feature_item(&head, dir.path());
        let result = compute_combined_diff(&item, &repo);
        assert!(
            result.is_some(),
            "should include working tree diff even without default branch"
        );
        let combined = result.unwrap();
        assert!(combined.diff.contains("wip.txt"));
    }

    #[test]
    fn test_generate_summary_calls_llm() {
        let (dir, repo, head) = temp_repo_with_feature();
        let item = feature_item(&head, dir.path());

        let summary = generate_summary(&item, "cat >/dev/null && echo 'Add new file'", &repo);
        assert_eq!(summary, "Add new file");
    }

    #[test]
    fn test_generate_summary_caches_result() {
        let (dir, repo, head) = temp_repo_with_feature();
        let item = feature_item(&head, dir.path());

        let summary1 = generate_summary(&item, "cat >/dev/null && echo 'Add new file'", &repo);
        assert_eq!(summary1, "Add new file");

        // Second call with different command should return cached value
        let summary2 = generate_summary(&item, "cat >/dev/null && echo 'Different output'", &repo);
        assert_eq!(summary2, "Add new file");
    }

    #[test]
    fn test_generate_summary_no_changes() {
        let (dir, repo, head) = temp_repo_configured();

        let mut item = ListItem::new_branch(head, "main".to_string());
        item.kind = ItemKind::Worktree(Box::new(WorktreeData {
            path: dir.path().to_path_buf(),
            ..Default::default()
        }));

        let summary = generate_summary(&item, "echo 'should not run'", &repo);
        assert!(summary.contains("No changes to summarize"));
    }

    #[test]
    fn test_generate_summary_llm_error() {
        let (dir, repo, head) = temp_repo_with_feature();
        let item = feature_item(&head, dir.path());

        let summary = generate_summary(&item, "cat >/dev/null && echo 'fail' >&2 && exit 1", &repo);
        assert!(summary.starts_with("LLM error:"));
    }

    #[test]
    fn test_generate_and_cache_summary_populates_cache() {
        let (dir, repo, head) = temp_repo_with_feature();
        let item = feature_item(&head, dir.path());
        let cache: DashMap<PreviewCacheKey, String> = DashMap::new();

        generate_and_cache_summary(
            &item,
            "cat >/dev/null && echo 'Add new file'",
            &cache,
            &repo,
        );

        let key = ("feature".to_string(), PreviewMode::Summary);
        assert!(cache.contains_key(&key));
        assert_eq!(cache.get(&key).unwrap().value(), "Add new file");
    }
}
