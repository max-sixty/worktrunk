//! Tests for git parsing functions
//!
//! These tests target edge cases and error conditions in git output parsing
//! that are likely to reveal bugs in real-world usage.

use super::{DefaultBranchName, GitError, LineDiff, Worktree};

#[test]
fn test_parse_worktree_list_empty_output() {
    // Bug hypothesis: Empty output might not be handled correctly
    let result = Worktree::parse_porcelain_list("");
    assert!(result.is_ok(), "Empty output should parse successfully");
    assert_eq!(result.unwrap().len(), 0, "Should return empty vector");
}

#[test]
fn test_parse_worktree_list_missing_head() {
    // Bug hypothesis: Worktree without HEAD field might have empty string for head
    // This could cause issues when the head field is used later
    let output = "worktree /path/to/repo\nbranch refs/heads/main\n\n";
    let result = Worktree::parse_porcelain_list(output);

    assert!(result.is_ok(), "Should parse even without HEAD field");
    let worktrees = result.unwrap();
    assert_eq!(worktrees.len(), 1);

    // This is the key assertion - what happens when HEAD is missing?
    // Current code initializes head as String::new() and only updates if HEAD line found
    assert_eq!(
        worktrees[0].head, "",
        "Missing HEAD should result in empty string (current behavior)"
    );
}

#[test]
fn test_parse_worktree_list_locked_with_empty_reason() {
    // Bug hypothesis: "locked" with no reason (just the key) might be handled incorrectly
    // Current code: `locked = Some(value.unwrap_or_default().to_string())`
    // This means locked with no value becomes Some("")
    let output = "worktree /path/to/repo\nHEAD abc123\nbranch refs/heads/main\nlocked\n\n";
    let result = Worktree::parse_porcelain_list(output);

    assert!(result.is_ok());
    let worktrees = result.unwrap();
    assert_eq!(worktrees.len(), 1);

    // What does locked become? Some("") or None?
    match &worktrees[0].locked {
        Some(reason) if reason.is_empty() => {
            // This is what the current code does
            println!("Locked is Some(\"\") - empty reason");
        }
        Some(reason) => {
            panic!("Expected empty reason, got: {}", reason);
        }
        None => {
            panic!("Expected Some(\"\"), got None");
        }
    }
}

#[test]
fn test_parse_worktree_list_locked_with_reason() {
    // Verify normal locked behavior works
    let output =
        "worktree /path/to/repo\nHEAD abc123\nbranch refs/heads/main\nlocked working on it\n\n";
    let result = Worktree::parse_porcelain_list(output);

    assert!(result.is_ok());
    let worktrees = result.unwrap();
    assert_eq!(worktrees.len(), 1);
    assert_eq!(worktrees[0].locked, Some("working on it".to_string()));
}

#[test]
fn test_parse_worktree_list_prunable_empty() {
    // Same issue as locked - prunable with no value
    let output = "worktree /path/to/repo\nHEAD abc123\nbranch refs/heads/main\nprunable\n\n";
    let result = Worktree::parse_porcelain_list(output);

    assert!(result.is_ok());
    let worktrees = result.unwrap();
    assert_eq!(worktrees.len(), 1);

    // Should be Some("") based on current code
    assert!(worktrees[0].prunable.is_some());
    assert_eq!(worktrees[0].prunable.as_ref().unwrap(), "");
}

#[test]
fn test_parse_worktree_list_fields_before_worktree() {
    // Bug hypothesis: If we get HEAD/branch lines before a worktree line,
    // they'll be silently ignored because current is None
    let output = "HEAD abc123\nbranch refs/heads/main\nworktree /path/to/repo\nHEAD def456\n\n";
    let result = Worktree::parse_porcelain_list(output);

    assert!(result.is_ok());
    let worktrees = result.unwrap();
    assert_eq!(worktrees.len(), 1);

    // The HEAD should be def456 (second one), not abc123 (first one that was ignored)
    assert_eq!(worktrees[0].head, "def456");
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
fn test_parse_worktree_list_bare_repository() {
    let output = "worktree /path/to/repo\nbare\n\n";
    let result = Worktree::parse_porcelain_list(output);

    assert!(result.is_ok());
    let worktrees = result.unwrap();
    assert_eq!(worktrees.len(), 1);
    assert!(worktrees[0].bare, "Should parse bare flag");
}

#[test]
fn test_parse_worktree_list_detached_head() {
    let output = "worktree /path/to/repo\nHEAD abc123\ndetached\n\n";
    let result = Worktree::parse_porcelain_list(output);

    assert!(result.is_ok());
    let worktrees = result.unwrap();
    assert_eq!(worktrees.len(), 1);
    assert!(worktrees[0].detached, "Should parse detached flag");
}

#[test]
fn test_parse_worktree_list_branch_with_refs_prefix() {
    let output = "worktree /path/to/repo\nHEAD abc123\nbranch refs/heads/feature/nested/branch\n\n";
    let result = Worktree::parse_porcelain_list(output);

    assert!(result.is_ok());
    let worktrees = result.unwrap();
    assert_eq!(worktrees.len(), 1);

    // Should strip refs/heads/ prefix
    assert_eq!(
        worktrees[0].branch,
        Some("feature/nested/branch".to_string()),
        "Should strip refs/heads/ prefix and preserve slashes in branch name"
    );
}

#[test]
fn test_parse_worktree_list_branch_without_refs_prefix() {
    // Bug hypothesis: What if git returns branch without refs/heads/ prefix?
    let output = "worktree /path/to/repo\nHEAD abc123\nbranch main\n\n";
    let result = Worktree::parse_porcelain_list(output);

    assert!(result.is_ok());
    let worktrees = result.unwrap();
    assert_eq!(worktrees.len(), 1);
    assert_eq!(worktrees[0].branch, Some("main".to_string()));
}

#[test]
fn test_parse_worktree_list_unknown_attributes() {
    // Forward compatibility - unknown attributes should be ignored
    let output = "worktree /path/to/repo\nHEAD abc123\nfutureattr somevalue\n\n";
    let result = Worktree::parse_porcelain_list(output);

    assert!(
        result.is_ok(),
        "Unknown attributes should be ignored for forward compatibility"
    );
    let worktrees = result.unwrap();
    assert_eq!(worktrees.len(), 1);
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

#[test]
fn test_parse_worktree_list_worktree_missing_path() {
    // Bug hypothesis: "worktree" line with no path should error
    let output = "worktree\nHEAD abc123\n\n";
    let result = Worktree::parse_porcelain_list(output);

    // Current code: value.ok_or_else(|| GitError::ParseError("worktree line missing path"))
    assert!(result.is_err(), "Worktree without path should error");
    match result {
        Err(GitError::ParseError(msg)) => {
            assert!(
                msg.contains("missing path"),
                "Error should mention missing path"
            );
        }
        _ => panic!("Expected ParseError about missing path"),
    }
}

#[test]
fn test_parse_worktree_list_head_missing_sha() {
    // Bug hypothesis: "HEAD" line with no SHA should error
    let output = "worktree /path/to/repo\nHEAD\nbranch refs/heads/main\n\n";
    let result = Worktree::parse_porcelain_list(output);

    // Current code: value.ok_or_else(|| GitError::ParseError("HEAD line missing SHA"))
    assert!(result.is_err(), "HEAD without SHA should error");
    match result {
        Err(GitError::ParseError(msg)) => {
            assert!(
                msg.contains("missing SHA"),
                "Error should mention missing SHA"
            );
        }
        _ => panic!("Expected ParseError about missing SHA"),
    }
}

#[test]
fn test_parse_worktree_list_branch_missing_ref() {
    // Bug hypothesis: "branch" line with no ref should error
    let output = "worktree /path/to/repo\nHEAD abc123\nbranch\n\n";
    let result = Worktree::parse_porcelain_list(output);

    // Current code: value.ok_or_else(|| GitError::ParseError("branch line missing ref"))
    assert!(result.is_err(), "Branch without ref should error");
    match result {
        Err(GitError::ParseError(msg)) => {
            assert!(
                msg.contains("missing ref"),
                "Error should mention missing ref"
            );
        }
        _ => panic!("Expected ParseError about missing ref"),
    }
}

// Tests for parse_remote_default_branch

#[test]
fn test_parse_remote_default_branch_normal() {
    // Normal git ls-remote output for HEAD
    let output = "ref: refs/heads/main\tHEAD\n";
    let result = DefaultBranchName::from_remote(output).map(DefaultBranchName::into_string);

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "main");
}

#[test]
fn test_parse_remote_default_branch_with_feature_branch() {
    // Remote default is a feature branch with slashes
    let output = "ref: refs/heads/feature/nested/branch\tHEAD\n";
    let result = DefaultBranchName::from_remote(output).map(DefaultBranchName::into_string);

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "feature/nested/branch");
}

#[test]
fn test_parse_remote_default_branch_empty_output() {
    // Bug hypothesis: Empty output should error, not panic
    let output = "";
    let result = DefaultBranchName::from_remote(output).map(DefaultBranchName::into_string);

    assert!(result.is_err(), "Empty output should return an error");
    match result {
        Err(GitError::ParseError(msg)) => {
            assert!(
                msg.contains("symbolic ref"),
                "Error should mention symbolic ref"
            );
        }
        _ => panic!("Expected ParseError"),
    }
}

#[test]
fn test_parse_remote_default_branch_no_ref_prefix() {
    // Bug hypothesis: Line without "ref: " prefix should be ignored
    let output = "refs/heads/main\tHEAD\n";
    let result = DefaultBranchName::from_remote(output).map(DefaultBranchName::into_string);

    // Should error because no line matches the pattern
    assert!(result.is_err());
}

#[test]
fn test_parse_remote_default_branch_missing_tab() {
    // Bug hypothesis: What if there's no tab separator?
    // Looking at the code: line.split_once('\t') returns None, so line is ignored
    let output = "ref: refs/heads/main";
    let result = DefaultBranchName::from_remote(output).map(DefaultBranchName::into_string);

    // Should error because split_once returns None
    assert!(result.is_err(), "Missing tab should cause error");
}

#[test]
fn test_parse_remote_default_branch_multiple_lines() {
    // Bug hypothesis: Multiple matching lines - should use first match
    let output = "ref: refs/heads/main\tHEAD\nref: refs/heads/develop\tHEAD\n";
    let result = DefaultBranchName::from_remote(output).map(DefaultBranchName::into_string);

    assert!(result.is_ok());
    // find_map returns first match
    assert_eq!(result.unwrap(), "main");
}

#[test]
fn test_parse_remote_default_branch_missing_refs_heads_prefix() {
    // Bug hypothesis: What if the ref doesn't have refs/heads/ prefix?
    // Looking at the code: strip_prefix returns None, so line is ignored
    let output = "ref: main\tHEAD\n";
    let result = DefaultBranchName::from_remote(output).map(DefaultBranchName::into_string);

    // Should error because strip_prefix fails
    assert!(result.is_err());
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
    match result {
        Err(GitError::ParseError(msg)) => {
            assert!(
                msg.contains("Empty branch"),
                "Error should mention empty branch"
            );
        }
        _ => panic!("Expected ParseError about empty branch"),
    }
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

#[test]
fn test_line_diff_from_numstat_normal() {
    let output = "10\t5\tfile1.rs\n3\t2\tfile2.rs\n";
    let result = LineDiff::from_numstat(output);

    assert!(result.is_ok());
    let (added, deleted) = result.unwrap().into_tuple();
    assert_eq!(added, 13, "Should sum added lines");
    assert_eq!(deleted, 7, "Should sum deleted lines");
}

#[test]
fn test_line_diff_from_numstat_empty_output() {
    // Bug hypothesis: Empty output should return (0, 0), not error
    let result = LineDiff::from_numstat("");

    assert!(result.is_ok());
    let (added, deleted) = result.unwrap().into_tuple();
    assert_eq!(added, 0);
    assert_eq!(deleted, 0);
}

#[test]
fn test_line_diff_from_numstat_binary_files() {
    // Binary files show "-" for added/deleted
    let output = "10\t5\tfile1.rs\n-\t-\timage.png\n3\t2\tfile2.rs\n";
    let result = LineDiff::from_numstat(output);

    assert!(result.is_ok());
    let (added, deleted) = result.unwrap().into_tuple();
    // Should skip the binary file line
    assert_eq!(added, 13);
    assert_eq!(deleted, 7);
}

#[test]
fn test_line_diff_from_numstat_mixed_binary() {
    // Bug hypothesis: What if only one side is "-"?
    // Current code checks "if added_str == '-' || deleted_str == '-'"
    // So it skips the line if EITHER is "-"
    let output = "10\t-\tfile1.rs\n-\t5\tfile2.rs\n";
    let result = LineDiff::from_numstat(output);

    assert!(result.is_ok());
    let (added, deleted) = result.unwrap().into_tuple();
    // Both lines should be skipped
    assert_eq!(added, 0);
    assert_eq!(deleted, 0);
}

#[test]
fn test_line_diff_from_numstat_empty_lines() {
    let output = "10\t5\tfile1.rs\n\n3\t2\tfile2.rs\n\n";
    let result = LineDiff::from_numstat(output);

    assert!(result.is_ok());
    let (added, deleted) = result.unwrap().into_tuple();
    // Empty lines should be skipped
    assert_eq!(added, 13);
    assert_eq!(deleted, 7);
}

#[test]
fn test_line_diff_from_numstat_malformed_missing_second_value() {
    // BUG FIXED: Line with only one tab used to try to parse filename as number
    // Input: "10\tfile.rs\n"
    // parts.next() -> Some("10") ✓
    // parts.next() -> Some("file.rs") ✓ (not None!)
    // Old behavior: tried to parse "file.rs" as usize -> ERROR
    // New behavior: skip malformed lines gracefully
    //
    // The real git numstat format is: <added>\t<deleted>\t<filename>
    let output = "10\tfile.rs\n";
    let result = LineDiff::from_numstat(output);

    // Fixed: Malformed lines are now skipped instead of causing errors
    assert!(result.is_ok(), "Malformed lines should be skipped");
    let (added, deleted) = result.unwrap().into_tuple();
    assert_eq!(added, 0, "Malformed line should be skipped");
    assert_eq!(deleted, 0, "Malformed line should be skipped");
}

#[test]
fn test_line_diff_from_numstat_malformed_missing_first_value() {
    // Bug hypothesis: Line with no tabs
    // parts.next() returns the whole line, parts.next() again returns None
    let output = "file.rs\n";
    let result = LineDiff::from_numstat(output);

    assert!(result.is_ok());
    // Line should be skipped
    let (added, deleted) = result.unwrap().into_tuple();
    assert_eq!(added, 0);
    assert_eq!(deleted, 0);
}

#[test]
fn test_line_diff_from_numstat_non_numeric_values() {
    // Non-numeric values should be skipped (after the fix)
    let output = "abc\t5\tfile.rs\n";
    let result = LineDiff::from_numstat(output);

    assert!(result.is_ok(), "Malformed lines should be skipped");
    let (added, deleted) = result.unwrap().into_tuple();
    assert_eq!(added, 0, "Non-numeric line should be skipped");
    assert_eq!(deleted, 0, "Non-numeric line should be skipped");
}

#[test]
fn test_line_diff_from_numstat_non_numeric_deleted() {
    // Non-numeric deleted value should be skipped
    let output = "5\txyz\tfile.rs\n";
    let result = LineDiff::from_numstat(output);

    assert!(result.is_ok(), "Malformed lines should be skipped");
    let (added, deleted) = result.unwrap().into_tuple();
    assert_eq!(added, 0, "Non-numeric line should be skipped");
    assert_eq!(deleted, 0, "Non-numeric line should be skipped");
}

#[test]
fn test_line_diff_from_numstat_zero_values() {
    let output = "0\t0\tfile.rs\n";
    let result = LineDiff::from_numstat(output);

    assert!(result.is_ok());
    let (added, deleted) = result.unwrap().into_tuple();
    assert_eq!(added, 0);
    assert_eq!(deleted, 0);
}
