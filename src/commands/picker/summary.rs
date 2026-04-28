//! LLM summary generation for the interactive selector.
//!
//! Thin adapter over `crate::summary` that adds TUI-specific rendering
//! and integrates with the selector's preview cache.

use dashmap::DashMap;
use worktrunk::git::Repository;

use super::super::list::model::ListItem;
use super::items::PreviewCacheKey;
use super::preview::PreviewMode;

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
///
/// `generate_summary_core` acquires `LLM_SEMAPHORE` internally, so the
/// no-changes and cache-hit fast paths return without contending.
pub(super) fn generate_and_cache_summary(
    item: &ListItem,
    llm_command: &str,
    preview_cache: &DashMap<PreviewCacheKey, String>,
    repo: &Repository,
) {
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
    use worktrunk::testing::TestRepo;

    /// Create a minimal temp git repo (for cache-only tests that don't need branches).
    fn temp_repo() -> (TestRepo, Repository) {
        let t = TestRepo::new();
        t.repo
            .run_command(&["commit", "--allow-empty", "-m", "init"])
            .unwrap();
        let repo = Repository::at(t.path()).unwrap();
        (t, repo)
    }

    /// Create a temp repo with main branch, default-branch config, and a real commit.
    fn temp_repo_configured() -> (TestRepo, Repository, String) {
        let t = TestRepo::new();
        t.repo
            .run_command(&["config", "worktrunk.default-branch", "main"])
            .unwrap();
        fs::write(t.path().join("README.md"), "# Project\n").unwrap();
        t.repo.run_command(&["add", "README.md"]).unwrap();
        t.repo
            .run_command(&["commit", "-m", "initial commit"])
            .unwrap();
        let head = t
            .repo
            .run_command(&["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        let repo = Repository::at(t.path()).unwrap();
        (t, repo, head)
    }

    /// Create a temp repo with main + feature branch that has real changes.
    fn temp_repo_with_feature() -> (TestRepo, Repository, String) {
        let (t, repo, _) = temp_repo_configured();

        repo.run_command(&["checkout", "-b", "feature"]).unwrap();
        fs::write(t.path().join("new.txt"), "new content\n").unwrap();
        repo.run_command(&["add", "new.txt"]).unwrap();
        repo.run_command(&["commit", "-m", "add new file"]).unwrap();

        let head = repo
            .run_command(&["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        let repo = Repository::at(t.path()).unwrap();
        (t, repo, head)
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
    fn test_cache_roundtrip_and_prune_on_write() {
        use crate::summary::CachedSummary;
        let (_t, repo) = temp_repo();
        let branch = "feature/test-branch";

        // Empty cache → read misses.
        assert!(CachedSummary::read(&repo, branch, "deadbeef").is_none());

        let first = CachedSummary {
            summary: "Add tests\n\nThis adds unit tests for cache.".to_string(),
            branch: branch.to_string(),
            generated_at: 100,
        };
        first.write(&repo, "deadbeef");

        let loaded = CachedSummary::read(&repo, branch, "deadbeef").unwrap();
        assert_eq!(loaded.summary, first.summary);
        assert_eq!(loaded.branch, first.branch);
        assert_eq!(loaded.generated_at, first.generated_at);

        // Different hash for the same branch is a miss (content-addressed).
        assert!(CachedSummary::read(&repo, branch, "cafebabe").is_none());

        // sweep_lru picks the eviction victim by file mtime; on filesystems
        // with coarse mtime resolution, back-to-back writes can share a tick
        // and the order becomes nondeterministic. Match the gap used in
        // test_sweep_lru_trims_oldest_entries.
        std::thread::sleep(std::time::Duration::from_millis(10));

        // Writing a second hash for the same branch prunes the old one.
        let second = CachedSummary {
            summary: "Refactor tests".to_string(),
            branch: branch.to_string(),
            generated_at: 200,
        };
        second.write(&repo, "cafebabe");

        assert!(CachedSummary::read(&repo, branch, "deadbeef").is_none());
        assert_eq!(
            CachedSummary::read(&repo, branch, "cafebabe")
                .unwrap()
                .summary,
            "Refactor tests"
        );

        let dir =
            CachedSummary::cache_root(&repo).join(worktrunk::path::sanitize_for_filename(branch));
        let remaining: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(remaining, vec!["cafebabe.json"]);
    }

    #[test]
    fn test_write_handles_unwritable_path() {
        use crate::summary::CachedSummary;
        let (_t, repo) = temp_repo();
        // Block cache directory creation by placing a file where the directory should be
        let wt_dir = repo.wt_dir();
        fs::create_dir_all(&wt_dir).unwrap();
        let cache_parent = wt_dir.join("cache");
        fs::write(&cache_parent, "blocker").unwrap();

        let cached = CachedSummary {
            summary: "test".to_string(),
            branch: "main".to_string(),
            generated_at: 0,
        };
        // Should not panic — just logs and returns
        cached.write(&repo, "deadbeef");
        assert!(CachedSummary::read(&repo, "main", "deadbeef").is_none());

        // Cleanup: remove the blocker file so TempDir cleanup works
        fs::remove_file(&cache_parent).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn test_write_handles_write_failure() {
        use crate::summary::CachedSummary;
        use std::os::unix::fs::PermissionsExt;

        let (_t, repo) = temp_repo();
        // Pre-create the branch dir (which write() would otherwise create)
        // and make it read-only so the json write fails.
        let branch_dir = CachedSummary::cache_root(&repo).join("main");
        fs::create_dir_all(&branch_dir).unwrap();
        fs::set_permissions(&branch_dir, fs::Permissions::from_mode(0o444)).unwrap();

        let cached = CachedSummary {
            summary: "test".to_string(),
            branch: "main".to_string(),
            generated_at: 0,
        };
        // Should not panic — just logs and returns
        cached.write(&repo, "deadbeef");
        assert!(CachedSummary::read(&repo, "main", "deadbeef").is_none());

        // Restore permissions so TempDir cleanup works
        fs::set_permissions(&branch_dir, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn test_cache_file_uses_sanitized_branch() {
        use crate::summary::CachedSummary;
        let (_t, repo) = temp_repo();
        let path = CachedSummary::cache_file(&repo, "feature/my-branch", "abc123");
        // Branch dir is sanitized.
        let parent = path
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        assert!(parent.starts_with("feature-my-branch-"));
        // Filename is the raw hash.
        assert_eq!(path.file_name().unwrap().to_str().unwrap(), "abc123.json");
    }

    #[test]
    fn test_cache_root_under_git() {
        use crate::summary::CachedSummary;
        let (_t, repo) = temp_repo();
        let dir = CachedSummary::cache_root(&repo);
        assert!(dir.to_str().unwrap().contains("wt"));
        assert!(dir.to_str().unwrap().contains("summary"));
    }

    #[test]
    fn test_list_all_returns_freshest_per_branch() {
        use crate::summary::CachedSummary;
        let (_t, repo) = temp_repo();

        CachedSummary {
            summary: "a".to_string(),
            branch: "feature-a".to_string(),
            generated_at: 100,
        }
        .write(&repo, "aaaa");
        CachedSummary {
            summary: "b".to_string(),
            branch: "feature-b".to_string(),
            generated_at: 200,
        }
        .write(&repo, "bbbb");

        let entries = CachedSummary::list_all(&repo);
        assert_eq!(entries.len(), 2);
        let mut branches: Vec<_> = entries.iter().map(|e| e.branch.clone()).collect();
        branches.sort();
        assert_eq!(branches, vec!["feature-a", "feature-b"]);
    }

    #[test]
    fn test_clear_all() {
        use crate::summary::CachedSummary;
        let (_t, repo) = temp_repo();

        // Empty cache: zero cleared, no error.
        assert_eq!(CachedSummary::clear_all(&repo).unwrap(), 0);

        CachedSummary {
            summary: "a".to_string(),
            branch: "feature-a".to_string(),
            generated_at: 0,
        }
        .write(&repo, "aaaa");
        CachedSummary {
            summary: "b".to_string(),
            branch: "feature-b".to_string(),
            generated_at: 0,
        }
        .write(&repo, "bbbb");

        assert_eq!(CachedSummary::clear_all(&repo).unwrap(), 2);
        assert!(CachedSummary::read(&repo, "feature-a", "aaaa").is_none());
        assert!(CachedSummary::read(&repo, "feature-b", "bbbb").is_none());
    }

    #[test]
    fn test_clear_all_propagates_non_not_found_read_dir_error() {
        use crate::summary::CachedSummary;
        let (_t, repo) = temp_repo();

        // Put a regular file where the summaries root is expected so
        // read_dir returns NotADirectory (non-NotFound).
        let root = CachedSummary::cache_root(&repo);
        fs::create_dir_all(root.parent().unwrap()).unwrap();
        fs::write(&root, "not a dir").unwrap();

        let err = CachedSummary::clear_all(&repo).unwrap_err();
        assert!(
            err.to_string().contains("failed to read"),
            "expected read-failure context, got: {err}"
        );
    }

    #[test]
    fn test_clear_all_propagates_per_file_remove_error() {
        use crate::summary::CachedSummary;
        let (_t, repo) = temp_repo();

        // Real entry to make the branch dir exist.
        CachedSummary {
            summary: "a".to_string(),
            branch: "feature".to_string(),
            generated_at: 0,
        }
        .write(&repo, "aaaa");

        // Replace the json file with a directory named `bad.json` so
        // remove_file returns a non-NotFound error (EISDIR / similar).
        let branch_dir = CachedSummary::cache_root(&repo).join("feature");
        for entry in fs::read_dir(&branch_dir).unwrap().flatten() {
            fs::remove_file(entry.path()).unwrap();
        }
        fs::create_dir(branch_dir.join("bad.json")).unwrap();

        let err = CachedSummary::clear_all(&repo).unwrap_err();
        assert!(
            err.to_string().contains("failed to remove"),
            "expected remove-failure context, got: {err}"
        );
    }

    #[test]
    fn test_clear_all_skips_non_json_and_non_dir_entries() {
        use crate::summary::CachedSummary;
        let (_t, repo) = temp_repo();

        let root = CachedSummary::cache_root(&repo);
        fs::create_dir_all(&root).unwrap();
        // Real branch dir with one entry that should be counted.
        let branch_dir = root.join("feature");
        fs::create_dir_all(&branch_dir).unwrap();
        fs::write(branch_dir.join("aaaa.json"), "{}").unwrap();
        // Non-.json siblings inside the branch dir must be skipped.
        fs::write(branch_dir.join("README"), "stray").unwrap();
        // Stray file at the root (not a branch dir) must be skipped.
        fs::write(root.join("stray.txt"), "noise").unwrap();

        let count = CachedSummary::clear_all(&repo).unwrap();
        assert_eq!(count, 1, "only the .json inside a branch dir should count");
        assert!(!branch_dir.join("aaaa.json").exists());
        assert!(branch_dir.join("README").exists());
        assert!(root.join("stray.txt").exists());
    }

    #[test]
    fn test_render_prompt() {
        use crate::summary::render_prompt;

        // With diff content and stat
        let prompt = render_prompt("diff content", "1 file changed").unwrap();
        assert_snapshot!(prompt, @r#"
        <task>Write a summary of this branch's changes as a commit message.</task>

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
        <task>Write a summary of this branch's changes as a commit message.</task>

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
        let (t, repo, head) = temp_repo_with_feature();

        let result = compute_combined_diff("feature", &head, Some(t.path()), &repo);
        assert!(result.is_some());
        let combined = result.unwrap();
        assert!(combined.diff.contains("new.txt"));
        assert!(combined.stat.contains("new.txt"));
    }

    #[test]
    fn test_compute_combined_diff_default_branch_no_changes() {
        use crate::summary::compute_combined_diff;
        let (t, repo, head) = temp_repo_configured();

        let result = compute_combined_diff("main", &head, Some(t.path()), &repo);
        assert!(result.is_none());
    }

    #[test]
    fn test_compute_combined_diff_with_uncommitted_changes() {
        use crate::summary::compute_combined_diff;
        let (t, repo, head) = temp_repo_with_feature();
        // Add uncommitted changes
        fs::write(t.path().join("uncommitted.txt"), "wip\n").unwrap();
        repo.run_command(&["add", "uncommitted.txt"]).unwrap();

        let result = compute_combined_diff("feature", &head, Some(t.path()), &repo);
        assert!(result.is_some());
        let combined = result.unwrap();
        // Should contain both the branch diff and the working tree diff
        assert!(combined.diff.contains("new.txt"));
        assert!(combined.diff.contains("uncommitted.txt"));
    }

    #[test]
    fn test_compute_combined_diff_branch_only_no_worktree() {
        use crate::summary::compute_combined_diff;
        let (_t, repo, head) = temp_repo_with_feature();
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
        let t = TestRepo::new();
        t.commit("initial commit");
        // Rename to exotic branch name so infer_default_branch_locally() returns None
        t.run_git(&["branch", "-m", "main", "init-branch"]);
        t.run_git(&["checkout", "-b", "feature"]);
        t.run_git(&["commit", "--allow-empty", "-m", "feature commit"]);

        // Add uncommitted changes
        fs::write(t.path().join("wip.txt"), "work in progress\n").unwrap();
        t.repo.run_command(&["add", "wip.txt"]).unwrap();

        let head = t
            .repo
            .run_command(&["rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string();
        let repo = Repository::at(t.path()).unwrap();

        // Verify default_branch() actually returns None with these branch names
        assert!(
            repo.default_branch().is_none(),
            "expected no default branch with exotic branch names"
        );

        let result = compute_combined_diff("feature", &head, Some(t.path()), &repo);
        assert!(
            result.is_some(),
            "should include working tree diff even without default branch"
        );
        let combined = result.unwrap();
        assert!(combined.diff.contains("wip.txt"));
    }

    #[test]
    fn test_generate_summary_calls_llm() {
        let (t, repo, head) = temp_repo_with_feature();

        let summary = crate::summary::generate_summary(
            "feature",
            &head,
            Some(t.path()),
            "cat >/dev/null && echo 'Add new file'",
            &repo,
        );
        assert_eq!(summary, "Add new file");
    }

    #[test]
    fn test_generate_summary_caches_result() {
        let (t, repo, head) = temp_repo_with_feature();

        let summary1 = crate::summary::generate_summary(
            "feature",
            &head,
            Some(t.path()),
            "cat >/dev/null && echo 'Add new file'",
            &repo,
        );
        assert_eq!(summary1, "Add new file");

        // Second call with different command should return cached value
        let summary2 = crate::summary::generate_summary(
            "feature",
            &head,
            Some(t.path()),
            "cat >/dev/null && echo 'Different output'",
            &repo,
        );
        assert_eq!(summary2, "Add new file");
    }

    #[test]
    fn test_generate_summary_no_changes() {
        let (t, repo, head) = temp_repo_configured();

        let summary = crate::summary::generate_summary(
            "main",
            &head,
            Some(t.path()),
            "echo 'should not run'",
            &repo,
        );
        assert_snapshot!(summary, @"[2m○[22m[0m [1mmain[22m[0m has no changes to summarize");
    }

    #[test]
    fn test_generate_summary_llm_error() {
        let (t, repo, head) = temp_repo_with_feature();

        let summary = crate::summary::generate_summary(
            "feature",
            &head,
            Some(t.path()),
            "cat >/dev/null && echo 'fail' >&2 && exit 1",
            &repo,
        );
        assert!(summary.starts_with("Error:"));
    }

    #[test]
    fn test_generate_and_cache_summary_populates_cache() {
        let (t, repo, head) = temp_repo_with_feature();
        let item = feature_item(&head, t.path());
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
