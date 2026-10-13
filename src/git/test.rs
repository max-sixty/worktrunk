//! Tests for git parsing functions
//!
//! These tests target edge cases and error conditions in git output parsing
//! that are likely to reveal bugs in real-world usage.

use super::{DefaultBranchName, LineDiff, WorktreeInfo};
use insta::assert_debug_snapshot;
use rstest::rstest;

#[test]
fn git_path_decoders_preserve_content_and_distinguish_terminators() {
    use super::{git_path_from_file, git_path_from_output};
    use std::path::Path;

    assert_eq!(
        git_path_from_output(b"store\nwith spaces \t\n").unwrap(),
        Path::new("store\nwith spaces \t")
    );
    assert_eq!(
        git_path_from_file(b"store\nwith spaces \t\r\n").unwrap(),
        Path::new("store\nwith spaces \t")
    );
    assert_eq!(
        git_path_from_output(b"unterminated").unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
}

/// Helper to parse a single worktree from porcelain output
fn parse_single(input: &str) -> WorktreeInfo {
    let list = WorktreeInfo::parse_porcelain_list_z(input.replace('\n', "\0").as_bytes())
        .expect("parse ok");
    assert_eq!(list.len(), 1);
    list.into_iter().next().unwrap()
}

#[rstest]
#[case::missing_path("worktree\nHEAD abc123\n\n", "missing path")]
#[case::head_missing_sha(
    "worktree /path/to/repo\nHEAD\nbranch refs/heads/main\n\n",
    "missing SHA"
)]
#[case::branch_missing_ref("worktree /path/to/repo\nHEAD abc123\nbranch\n\n", "missing ref")]
fn test_parse_worktree_list_error_cases(#[case] input: &str, #[case] expected_message: &str) {
    let result = WorktreeInfo::parse_porcelain_list_z(input.replace('\n', "\0").as_bytes());

    assert!(result.is_err(), "Parsing should fail");
    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains(expected_message),
        "Error should mention {expected_message}, got: {msg}"
    );
}

// Tests for parse_remote_default_branch

#[rstest]
#[case::normal("ref: refs/heads/main\tHEAD\n", Ok("main"))]
#[case::feature_branch(
    "ref: refs/heads/feature/nested/branch\tHEAD\n",
    Ok("feature/nested/branch")
)]
#[case::empty_output("", Err(Some("symbolic ref")))]
#[case::missing_prefix("refs/heads/main\tHEAD\n", Err(None))]
#[case::missing_tab("ref: refs/heads/main", Err(None))]
#[case::multiple_matches(
    "ref: refs/heads/main\tHEAD\nref: refs/heads/develop\tHEAD\n",
    Ok("main")
)]
#[case::missing_refs_heads_prefix("ref: main\tHEAD\n", Err(None))]
fn test_parse_remote_default_branch(
    #[case] input: &str,
    #[case] expected: Result<&str, Option<&str>>,
) {
    let result = DefaultBranchName::from_remote(input).map(DefaultBranchName::into_string);

    match expected {
        Ok(expected_branch) => {
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), expected_branch);
        }
        Err(expected_substr) => {
            assert!(result.is_err());
            if let Some(substr) = expected_substr {
                let msg = result.unwrap_err().to_string();
                assert!(
                    msg.contains(substr),
                    "Error should mention {substr}, got: {msg}"
                );
            }
        }
    }
}

// Tests for parse_local_default_branch

#[test]
fn test_parse_local_default_branch_normal() {
    let result =
        DefaultBranchName::from_local("origin", "origin/main").map(DefaultBranchName::into_string);

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "main");
}

#[test]
fn test_parse_local_default_branch_without_remote_prefix() {
    // Bug hypothesis: If output doesn't have remote prefix, just return it as-is
    let result =
        DefaultBranchName::from_local("origin", "main").map(DefaultBranchName::into_string);

    assert!(result.is_ok());
    // strip_prefix fails, so unwrap_or returns original
    assert_eq!(result.unwrap(), "main");
}

#[test]
fn test_parse_local_default_branch_with_nested_slashes() {
    // Bug hypothesis: Branch name like "feature/sub/branch" might break if we have
    // multiple slashes. Let's verify it works correctly.
    let result = DefaultBranchName::from_local("origin", "origin/feature/sub/branch")
        .map(DefaultBranchName::into_string);

    assert!(result.is_ok());
    // Should strip only "origin/" prefix, leaving "feature/sub/branch"
    assert_eq!(result.unwrap(), "feature/sub/branch");
}

#[test]
fn test_parse_local_default_branch_empty_output() {
    // Bug hypothesis: Empty string after trimming should error
    let result = DefaultBranchName::from_local("origin", "").map(DefaultBranchName::into_string);

    assert!(result.is_err(), "Empty output should error");
    let err = result.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("Empty branch"),
        "Error should mention empty branch, got: {msg}"
    );
}

#[test]
fn test_parse_local_default_branch_whitespace_only() {
    // Bug hypothesis: Whitespace-only input should error after trim
    let result =
        DefaultBranchName::from_local("origin", "  \n  ").map(DefaultBranchName::into_string);

    assert!(result.is_err(), "Whitespace-only should error");
}

#[test]
fn test_parse_local_default_branch_empty_remote() {
    // Bug hypothesis: What if remote name is empty?
    // This creates prefix = "/" which might match branch names starting with /
    let result =
        DefaultBranchName::from_local("", "/weird/branch").map(DefaultBranchName::into_string);

    assert!(result.is_ok());
    // Strips "/" prefix, leaving "weird/branch"
    assert_eq!(result.unwrap(), "weird/branch");
}

// Tests for LineDiff::from_shortstat

#[rstest]
#[case::all_parts(" 23 files changed, 624 insertions(+), 160 deletions(-)", 624, 160)]
#[case::insertions_only(" 1 file changed, 6 insertions(+)", 6, 0)]
#[case::deletions_only(" 2 files changed, 10 deletions(-)", 0, 10)]
#[case::empty("", 0, 0)]
#[case::whitespace("  \n  ", 0, 0)]
#[case::singular(" 1 file changed, 1 insertion(+), 1 deletion(-)", 1, 1)]
fn test_line_diff_from_shortstat(
    #[case] input: &str,
    #[case] expected_added: usize,
    #[case] expected_deleted: usize,
) {
    let diff = LineDiff::from_shortstat(input);
    assert_eq!(diff.added, expected_added);
    assert_eq!(diff.deleted, expected_deleted);
}

#[test]
fn snapshot_parse_worktree_list_missing_head() {
    let wt = parse_single("worktree /path/to/repo\nbranch refs/heads/main\n\n");
    assert_debug_snapshot!(wt, @r#"
    WorktreeInfo {
        path: "/path/to/repo",
        head: "",
        branch: Some(
            "main",
        ),
        bare: false,
        detached: false,
        locked: None,
        prunable: None,
    }
    "#);
}

#[test]
fn snapshot_parse_worktree_list_locked_with_empty_reason() {
    let wt =
        parse_single("worktree /path/to/repo\nHEAD abc123\nbranch refs/heads/main\nlocked\n\n");
    assert_debug_snapshot!(wt, @r#"
    WorktreeInfo {
        path: "/path/to/repo",
        head: "abc123",
        branch: Some(
            "main",
        ),
        bare: false,
        detached: false,
        locked: Some(
            "",
        ),
        prunable: None,
    }
    "#);
}

#[test]
fn snapshot_parse_worktree_list_prunable_empty() {
    let wt =
        parse_single("worktree /path/to/repo\nHEAD abc123\nbranch refs/heads/main\nprunable\n\n");
    assert_debug_snapshot!(wt, @r#"
    WorktreeInfo {
        path: "/path/to/repo",
        head: "abc123",
        branch: Some(
            "main",
        ),
        bare: false,
        detached: false,
        locked: None,
        prunable: Some(
            "",
        ),
    }
    "#);
}

#[test]
fn snapshot_parse_worktree_list_fields_before_worktree() {
    let wt = parse_single(
        "HEAD abc123\nbranch refs/heads/main\nworktree /path/to/repo\nHEAD def456\n\n",
    );
    assert_debug_snapshot!(wt, @r#"
    WorktreeInfo {
        path: "/path/to/repo",
        head: "def456",
        branch: None,
        bare: false,
        detached: false,
        locked: None,
        prunable: None,
    }
    "#);
}

#[test]
fn snapshot_parse_worktree_list_unknown_attributes() {
    let wt = parse_single("worktree /path/to/repo\nHEAD abc123\nfutureattr somevalue\n\n");
    assert_debug_snapshot!(wt, @r#"
    WorktreeInfo {
        path: "/path/to/repo",
        head: "abc123",
        branch: None,
        bare: false,
        detached: false,
        locked: None,
        prunable: None,
    }
    "#);
}
