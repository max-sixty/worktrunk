//! Tests for git parsing functions
//!
//! These tests target edge cases and error conditions in git output parsing
//! that are likely to reveal bugs in real-world usage.

use super::{DefaultBranchName, LineDiff, Worktree};
use insta::assert_debug_snapshot;
use rstest::rstest;

/// Helper to parse a single worktree from porcelain output
fn parse_single(input: &str) -> Worktree {
    let list = Worktree::parse_porcelain_list(input).expect("parse ok");
    assert_eq!(list.len(), 1, "expected single worktree");
    list.into_iter().next().unwrap()
}

#[test]
fn test_parse_worktree_list_no_trailing_blank_line() {
    // Bug hypothesis: If output doesn't end with blank line,
    // the last worktree might not be added
    // Looking at the code (lines 1128-1130), this should be handled correctly
    let output = "worktree /path/to/repo1\nHEAD abc123\nbranch refs/heads/main\n\nworktree /path/to/repo2\nHEAD def456\nbranch refs/heads/dev";
    let result = Worktree::parse_porcelain_list(output);

    assert!(result.is_ok());
    let worktrees = result.unwrap();

    // Should have 2 worktrees - code handles this with "if let Some(wt) = current" at end
    assert_eq!(
        worktrees.len(),
        2,
        "Should parse both worktrees even without trailing blank line"
    );
}

#[test]
fn test_parse_worktree_list_multiple_worktrees() {
    let output = "worktree /path/to/main\nHEAD abc123\nbranch refs/heads/main\n\nworktree /path/to/feature\nHEAD def456\nbranch refs/heads/feature\ndetached\n\n";
    let result = Worktree::parse_porcelain_list(output);

    assert!(result.is_ok());
    let worktrees = result.unwrap();
    assert_eq!(worktrees.len(), 2);

    assert_eq!(worktrees[0].branch, Some("main".to_string()));
    assert!(!worktrees[0].detached);

    assert_eq!(worktrees[1].branch, Some("feature".to_string()));
    assert!(worktrees[1].detached);
}

#[rstest]
#[case::missing_path("worktree\nHEAD abc123\n\n", "missing path")]
#[case::head_missing_sha(
    "worktree /path/to/repo\nHEAD\nbranch refs/heads/main\n\n",
    "missing SHA"
)]
#[case::branch_missing_ref("worktree /path/to/repo\nHEAD abc123\nbranch\n\n", "missing ref")]
fn test_parse_worktree_list_error_cases(#[case] input: &str, #[case] expected_message: &str) {
    let result = Worktree::parse_porcelain_list(input);

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

// Tests for LineDiff::from_numstat

#[rstest]
#[case::normal("10\t5\tfile1.rs\n3\t2\tfile2.rs\n", 13, 7)]
#[case::empty("", 0, 0)]
#[case::binary_files("10\t5\tfile1.rs\n-\t-\timage.png\n3\t2\tfile2.rs\n", 13, 7)]
#[case::mixed_binary("10\t-\tfile1.rs\n-\t5\tfile2.rs\n", 0, 0)]
#[case::empty_lines("10\t5\tfile1.rs\n\n3\t2\tfile2.rs\n\n", 13, 7)]
#[case::missing_deleted("10\tfile.rs\n", 0, 0)]
#[case::no_tabs("file.rs\n", 0, 0)]
#[case::non_numeric_added("abc\t5\tfile.rs\n", 0, 0)]
#[case::non_numeric_deleted("5\txyz\tfile.rs\n", 0, 0)]
#[case::zero_values("0\t0\tfile.rs\n", 0, 0)]
fn test_line_diff_from_numstat(
    #[case] input: &str,
    #[case] expected_added: usize,
    #[case] expected_deleted: usize,
) {
    let result = LineDiff::from_numstat(input);

    assert!(result.is_ok());
    let (added, deleted) = result.unwrap().into_tuple();
    assert_eq!(added, expected_added);
    assert_eq!(deleted, expected_deleted);
}

#[test]
fn snapshot_parse_worktree_list_empty_output() {
    let result = Worktree::parse_porcelain_list("").expect("parse ok");
    assert_debug_snapshot!(result, @"[]");
}

#[test]
fn snapshot_parse_worktree_list_missing_head() {
    let wt = parse_single("worktree /path/to/repo\nbranch refs/heads/main\n\n");
    assert_debug_snapshot!(wt, @r#"
    Worktree {
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
    Worktree {
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
fn snapshot_parse_worktree_list_locked_with_reason() {
    let wt = parse_single(
        "worktree /path/to/repo\nHEAD abc123\nbranch refs/heads/main\nlocked working on it\n\n",
    );
    assert_debug_snapshot!(wt, @r#"
    Worktree {
        path: "/path/to/repo",
        head: "abc123",
        branch: Some(
            "main",
        ),
        bare: false,
        detached: false,
        locked: Some(
            "working on it",
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
    Worktree {
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
    Worktree {
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
fn snapshot_parse_worktree_list_bare_repository() {
    let wt = parse_single("worktree /path/to/repo\nbare\n\n");
    assert_debug_snapshot!(wt, @r#"
    Worktree {
        path: "/path/to/repo",
        head: "",
        branch: None,
        bare: true,
        detached: false,
        locked: None,
        prunable: None,
    }
    "#);
}

#[test]
fn snapshot_parse_worktree_list_detached_head() {
    let wt = parse_single("worktree /path/to/repo\nHEAD abc123\ndetached\n\n");
    assert_debug_snapshot!(wt, @r#"
    Worktree {
        path: "/path/to/repo",
        head: "abc123",
        branch: None,
        bare: false,
        detached: true,
        locked: None,
        prunable: None,
    }
    "#);
}

#[test]
fn snapshot_parse_worktree_list_branch_with_refs_prefix() {
    let wt = parse_single(
        "worktree /path/to/repo\nHEAD abc123\nbranch refs/heads/feature/nested/branch\n\n",
    );
    assert_debug_snapshot!(wt, @r#"
    Worktree {
        path: "/path/to/repo",
        head: "abc123",
        branch: Some(
            "feature/nested/branch",
        ),
        bare: false,
        detached: false,
        locked: None,
        prunable: None,
    }
    "#);
}

#[test]
fn snapshot_parse_worktree_list_branch_without_refs_prefix() {
    let wt = parse_single("worktree /path/to/repo\nHEAD abc123\nbranch main\n\n");
    assert_debug_snapshot!(wt, @r#"
    Worktree {
        path: "/path/to/repo",
        head: "abc123",
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
fn snapshot_parse_worktree_list_unknown_attributes() {
    let wt = parse_single("worktree /path/to/repo\nHEAD abc123\nfutureattr somevalue\n\n");
    assert_debug_snapshot!(wt, @r#"
    Worktree {
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

// Tests for escape_branch_for_config and unescape_branch_from_config

use super::{escape_branch_for_config, unescape_branch_from_config};

#[test]
fn test_escape_branch_simple() {
    // Alphanumeric and dots should pass through unchanged
    assert_eq!(escape_branch_for_config("main"), "main");
    assert_eq!(escape_branch_for_config("feature.123"), "feature.123");
    assert_eq!(escape_branch_for_config("AaBbZz0189"), "AaBbZz0189");
}

#[test]
fn test_escape_branch_with_slash() {
    // Slashes should be encoded as -2F
    assert_eq!(
        escape_branch_for_config("feature/branch"),
        "feature-2Fbranch"
    );
    assert_eq!(escape_branch_for_config("a/b/c"), "a-2Fb-2Fc");
}

#[test]
fn test_escape_branch_with_underscore() {
    // Underscores should be encoded as -5F
    assert_eq!(escape_branch_for_config("my_branch"), "my-5Fbranch");
    assert_eq!(escape_branch_for_config("a_b_c"), "a-5Fb-5Fc");
}

#[test]
fn test_escape_branch_with_hyphen() {
    // Hyphens should be encoded as -2D
    assert_eq!(escape_branch_for_config("my-branch"), "my-2Dbranch");
    assert_eq!(escape_branch_for_config("a-b-c"), "a-2Db-2Dc");
}

#[test]
fn test_escape_branch_mixed_special_chars() {
    // Mix of special characters
    assert_eq!(
        escape_branch_for_config("feature/my-branch_test"),
        "feature-2Fmy-2Dbranch-5Ftest"
    );
}

#[test]
fn test_unescape_branch_simple() {
    // Simple strings should pass through unchanged
    assert_eq!(unescape_branch_from_config("main"), "main");
    assert_eq!(unescape_branch_from_config("feature.123"), "feature.123");
}

#[test]
fn test_unescape_branch_with_encoded_slash() {
    // -2F should become /
    assert_eq!(
        unescape_branch_from_config("feature-2Fbranch"),
        "feature/branch"
    );
}

#[test]
fn test_unescape_branch_with_encoded_underscore() {
    // -5F should become _
    assert_eq!(unescape_branch_from_config("my-5Fbranch"), "my_branch");
}

#[test]
fn test_unescape_branch_with_encoded_hyphen() {
    // -2D should become -
    assert_eq!(unescape_branch_from_config("my-2Dbranch"), "my-branch");
}

#[test]
fn test_escape_unescape_roundtrip() {
    // Test that escape followed by unescape returns original
    let test_cases = vec![
        "main",
        "feature/branch",
        "my-branch",
        "my_branch",
        "feature/my-branch_test",
        "a/b/c-d_e.f",
        "",
    ];
    for branch in test_cases {
        let escaped = escape_branch_for_config(branch);
        let unescaped = unescape_branch_from_config(&escaped);
        assert_eq!(
            unescaped, branch,
            "Roundtrip failed for '{branch}': escaped='{}', unescaped='{}'",
            escaped, unescaped
        );
    }
}

#[test]
fn test_unescape_branch_invalid_escape() {
    // Invalid escape sequences should be kept as-is
    // -XX where XX is not valid hex
    assert_eq!(unescape_branch_from_config("test-ZZ"), "test-ZZ");
    // - at end without following chars
    assert_eq!(unescape_branch_from_config("test-"), "test-");
    // - followed by only one char
    assert_eq!(unescape_branch_from_config("test-A"), "test-A");
}

#[test]
fn test_escape_branch_empty() {
    assert_eq!(escape_branch_for_config(""), "");
}

#[test]
fn test_unescape_branch_empty() {
    assert_eq!(unescape_branch_from_config(""), "");
}
