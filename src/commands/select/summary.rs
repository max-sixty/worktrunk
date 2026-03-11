//! LLM summary generation for the interactive selector.
//!
//! Thin adapter over `crate::summary` that adds TUI-specific rendering
//! and integrates with the selector's preview cache.

use dashmap::DashMap;
use worktrunk::git::Repository;

use super::super::list::model::ListItem;
use super::items::PreviewCacheKey;
use super::preview::PreviewMode;
use crate::summary::LLM_SEMAPHORE;

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

/// Generate a summary for one item and insert it into the preview cache.
/// Acquires the LLM semaphore to limit concurrent calls across rayon tasks.
pub(super) fn generate_and_cache_summary(
    item: &ListItem,
    llm_command: &str,
    preview_cache: &DashMap<PreviewCacheKey, String>,
    repo: &Repository,
) {
    let _permit = LLM_SEMAPHORE.acquire();
    let branch = item.branch_name();
    let worktree_path = item.worktree_data().map(|d| d.path.as_path());
    let summary =
        crate::summary::generate_summary(branch, item.head(), worktree_path, llm_command, repo);
    preview_cache.insert((branch.to_string(), PreviewMode::Summary), summary);
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use super::*;
    use crate::commands::list::model::{ItemKind, WorktreeData};
    use std::fs;

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
        use crate::summary::{CachedSummary, read_cache, write_cache};
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
        use crate::summary::{CachedSummary, read_cache, write_cache};
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
        use crate::summary::{CachedSummary, cache_dir, read_cache, write_cache};
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
        use crate::summary::{CachedSummary, read_cache, write_cache};
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
        use crate::summary::cache_file;
        let (_dir, repo) = temp_repo();
        let path = cache_file(&repo, "feature/my-branch");
        let filename = path.file_name().unwrap().to_str().unwrap();
        assert!(filename.starts_with("feature-my-branch-"));
        assert!(filename.ends_with(".json"));
    }

    #[test]
    fn test_cache_dir_under_git() {
        use crate::summary::cache_dir;
        let (_dir, repo) = temp_repo();
        let dir = cache_dir(&repo);
        assert!(dir.to_str().unwrap().contains("wt-cache"));
        assert!(dir.to_str().unwrap().contains("summaries"));
    }

    #[test]
    fn test_hash_diff_deterministic() {
        use crate::summary::hash_diff;
        let hash1 = hash_diff("some diff content");
        let hash2 = hash_diff("some diff content");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_diff_different_inputs() {
        use crate::summary::hash_diff;
        let hash1 = hash_diff("diff A");
        let hash2 = hash_diff("diff B");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_render_prompt() {
        use crate::summary::render_prompt;

        // With diff content and stat
        let prompt = render_prompt("diff content", "1 file changed").unwrap();
        assert_snapshot!(prompt, @r#"
        Write a summary of this branch's changes as a commit message.

        <format>
        - Subject line under 50 chars, imperative mood ("Add feature" not "Adds feature")
        - Blank line, then a body paragraph or bullet list explaining the key changes
        - Output only the message — no quotes, code blocks, or labels
        </format>

        <diffstat>
        1 file changed
        </diffstat>

        <diff>
        diff content
        </diff>
        "#);

        // Empty inputs still include format instructions
        let empty_prompt = render_prompt("", "").unwrap();
        assert_snapshot!(empty_prompt, @r#"
        Write a summary of this branch's changes as a commit message.

        <format>
        - Subject line under 50 chars, imperative mood ("Add feature" not "Adds feature")
        - Blank line, then a body paragraph or bullet list explaining the key changes
        - Output only the message — no quotes, code blocks, or labels
        </format>

        <diffstat>

        </diffstat>

        <diff>

        </diff>
        "#);
    }

    #[test]
    fn test_render_summary() {
        // Multi-line: subject promoted to bold H4, body preserved
        assert_snapshot!(
            render_summary("Add new feature\n\nSome body text here.", 80),
            @"
        [1mAdd new feature[0m

        Some body text here.
        "
        );

        // Single line: also promoted to bold H4
        assert_snapshot!(render_summary("Add new feature", 80), @"[1mAdd new feature[0m");

        // Bullet list body preserved
        assert_snapshot!(
            render_summary("Subject\n\n- First bullet\n- Second bullet", 80),
            @"
        [1mSubject[0m

        - First bullet
        - Second bullet
        "
        );

        // Pre-styled text (ANSI escapes) skips H4 promotion
        assert_snapshot!(
            render_summary("\x1b[2mNo changes to summarize.\x1b[0m", 80),
            @"[2mNo changes to summarize.[0m"
        );
    }

    #[test]
    fn test_render_summary_wraps_body() {
        let text = format!("Subject\n\n{}", "word ".repeat(30));
        let rendered = render_summary(&text, 40);
        assert!(rendered.lines().count() > 3);
    }

    #[test]
    fn test_compute_combined_diff_with_branch_changes() {
        use crate::summary::compute_combined_diff;
        let (dir, repo, head) = temp_repo_with_feature();

        let result = compute_combined_diff("feature", &head, Some(dir.path()), &repo);
        assert!(result.is_some());
        let combined = result.unwrap();
        assert!(combined.diff.contains("new.txt"));
        assert!(combined.stat.contains("new.txt"));
    }

    #[test]
    fn test_compute_combined_diff_default_branch_no_changes() {
        use crate::summary::compute_combined_diff;
        let (dir, repo, head) = temp_repo_configured();

        let result = compute_combined_diff("main", &head, Some(dir.path()), &repo);
        assert!(result.is_none());
    }

    #[test]
    fn test_compute_combined_diff_with_uncommitted_changes() {
        use crate::summary::compute_combined_diff;
        let (dir, repo, head) = temp_repo_with_feature();
        // Add uncommitted changes
        fs::write(dir.path().join("uncommitted.txt"), "wip\n").unwrap();
        git(dir.path(), &["add", "uncommitted.txt"]);

        let result = compute_combined_diff("feature", &head, Some(dir.path()), &repo);
        assert!(result.is_some());
        let combined = result.unwrap();
        // Should contain both the branch diff and the working tree diff
        assert!(combined.diff.contains("new.txt"));
        assert!(combined.diff.contains("uncommitted.txt"));
    }

    #[test]
    fn test_compute_combined_diff_branch_only_no_worktree() {
        use crate::summary::compute_combined_diff;
        let (_dir, repo, head) = temp_repo_with_feature();
        // Branch-only item (no worktree data) — only branch diff included
        let result = compute_combined_diff("feature", &head, None, &repo);
        assert!(result.is_some());
        let combined = result.unwrap();
        assert!(combined.diff.contains("new.txt"));
    }

    #[test]
    fn test_compute_combined_diff_no_default_branch_with_worktree_changes() {
        use crate::summary::compute_combined_diff;
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

        let result = compute_combined_diff("feature", &head, Some(dir.path()), &repo);
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

        let summary = crate::summary::generate_summary(
            "feature",
            &head,
            Some(dir.path()),
            "cat >/dev/null && echo 'Add new file'",
            &repo,
        );
        assert_eq!(summary, "Add new file");
    }

    #[test]
    fn test_generate_summary_caches_result() {
        let (dir, repo, head) = temp_repo_with_feature();

        let summary1 = crate::summary::generate_summary(
            "feature",
            &head,
            Some(dir.path()),
            "cat >/dev/null && echo 'Add new file'",
            &repo,
        );
        assert_eq!(summary1, "Add new file");

        // Second call with different command should return cached value
        let summary2 = crate::summary::generate_summary(
            "feature",
            &head,
            Some(dir.path()),
            "cat >/dev/null && echo 'Different output'",
            &repo,
        );
        assert_eq!(summary2, "Add new file");
    }

    #[test]
    fn test_generate_summary_no_changes() {
        let (dir, repo, head) = temp_repo_configured();

        let summary = crate::summary::generate_summary(
            "main",
            &head,
            Some(dir.path()),
            "echo 'should not run'",
            &repo,
        );
        assert_snapshot!(summary, @"[2mNo changes to summarize on main.[22m");
    }

    #[test]
    fn test_generate_summary_llm_error() {
        let (dir, repo, head) = temp_repo_with_feature();

        let summary = crate::summary::generate_summary(
            "feature",
            &head,
            Some(dir.path()),
            "cat >/dev/null && echo 'fail' >&2 && exit 1",
            &repo,
        );
        assert!(summary.starts_with("Error:"));
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
