use std::path::PathBuf;

use super::super::{DefaultBranchName, Worktree, finalize_worktree};

#[test]
fn test_parse_worktree_list() {
    let output = "worktree /path/to/main
HEAD abcd1234
branch refs/heads/main

worktree /path/to/feature
HEAD efgh5678
branch refs/heads/feature

";

    let worktrees = Worktree::parse_porcelain_list(output).unwrap();
    assert_eq!(worktrees.len(), 2);

    assert_eq!(worktrees[0].path, PathBuf::from("/path/to/main"));
    assert_eq!(worktrees[0].head, "abcd1234");
    assert_eq!(worktrees[0].branch, Some("main".to_string()));
    assert!(!worktrees[0].bare);
    assert!(!worktrees[0].detached);

    assert_eq!(worktrees[1].path, PathBuf::from("/path/to/feature"));
    assert_eq!(worktrees[1].head, "efgh5678");
    assert_eq!(worktrees[1].branch, Some("feature".to_string()));
}

#[test]
fn test_parse_detached_worktree() {
    let output = "worktree /path/to/detached
HEAD abcd1234
detached

";

    let worktrees = Worktree::parse_porcelain_list(output).unwrap();
    assert_eq!(worktrees.len(), 1);
    assert!(worktrees[0].detached);
    assert_eq!(worktrees[0].branch, None);
}

#[test]
fn test_finalize_worktree_with_branch() {
    // Worktree with a branch should not be modified
    let wt = Worktree {
        path: PathBuf::from("/path/to/worktree"),
        head: "abcd1234".to_string(),
        branch: Some("feature".to_string()),
        bare: false,
        detached: false,
        locked: None,
        prunable: None,
    };

    let finalized = finalize_worktree(wt.clone());
    assert_eq!(finalized.branch, Some("feature".to_string()));
}

#[test]
fn test_finalize_worktree_detached_with_branch() {
    // Detached worktree with a branch (unusual but possible) should keep the branch
    let wt = Worktree {
        path: PathBuf::from("/path/to/worktree"),
        head: "abcd1234".to_string(),
        branch: Some("feature".to_string()),
        bare: false,
        detached: true,
        locked: None,
        prunable: None,
    };

    let finalized = finalize_worktree(wt.clone());
    assert_eq!(finalized.branch, Some("feature".to_string()));
}

#[test]
fn test_finalize_worktree_detached_no_branch() {
    // Detached worktree with no branch should attempt rebase detection
    // Note: This test validates the logic flow but doesn't test actual file reading
    // since that would require setting up git rebase state files.
    // Actual rebase detection has been manually verified.
    let wt = Worktree {
        path: PathBuf::from("/nonexistent/path"),
        head: "abcd1234".to_string(),
        branch: None,
        bare: false,
        detached: true,
        locked: None,
        prunable: None,
    };

    let finalized = finalize_worktree(wt);
    // With a nonexistent path, rebase detection should fail gracefully
    // and branch should remain None
    assert_eq!(finalized.branch, None);
}

#[test]
fn test_parse_locked_worktree() {
    let output = "worktree /path/to/locked
HEAD abcd1234
branch refs/heads/main
locked reason for lock

";

    let worktrees = Worktree::parse_porcelain_list(output).unwrap();
    assert_eq!(worktrees.len(), 1);
    assert_eq!(worktrees[0].locked, Some("reason for lock".to_string()));
}

#[test]
fn test_parse_bare_worktree() {
    let output = "worktree /path/to/bare
HEAD abcd1234
bare

";

    let worktrees = Worktree::parse_porcelain_list(output).unwrap();
    assert_eq!(worktrees.len(), 1);
    assert!(worktrees[0].bare);
}

#[test]
fn test_parse_local_default_branch_with_prefix() {
    let output = "origin/main\n";
    let branch = DefaultBranchName::from_local("origin", output)
        .map(DefaultBranchName::into_string)
        .unwrap();
    assert_eq!(branch, "main");
}

#[test]
fn test_parse_local_default_branch_without_prefix() {
    let output = "main\n";
    let branch = DefaultBranchName::from_local("origin", output)
        .map(DefaultBranchName::into_string)
        .unwrap();
    assert_eq!(branch, "main");
}

#[test]
fn test_parse_local_default_branch_master() {
    let output = "origin/master\n";
    let branch = DefaultBranchName::from_local("origin", output)
        .map(DefaultBranchName::into_string)
        .unwrap();
    assert_eq!(branch, "master");
}

#[test]
fn test_parse_local_default_branch_custom_name() {
    let output = "origin/develop\n";
    let branch = DefaultBranchName::from_local("origin", output)
        .map(DefaultBranchName::into_string)
        .unwrap();
    assert_eq!(branch, "develop");
}

#[test]
fn test_parse_local_default_branch_custom_remote() {
    let output = "upstream/main\n";
    let branch = DefaultBranchName::from_local("upstream", output)
        .map(DefaultBranchName::into_string)
        .unwrap();
    assert_eq!(branch, "main");
}

#[test]
fn test_parse_local_default_branch_empty() {
    let output = "";
    let result =
        DefaultBranchName::from_local("origin", output).map(DefaultBranchName::into_string);
    assert!(result.is_err());
}

#[test]
fn test_parse_local_default_branch_whitespace_only() {
    let output = "  \n  ";
    let result =
        DefaultBranchName::from_local("origin", output).map(DefaultBranchName::into_string);
    assert!(result.is_err());
}

#[test]
fn test_parse_remote_default_branch_main() {
    let output = "ref: refs/heads/main\tHEAD
85a1ce7c7182540f9c02453441cb3e8bf0ced214\tHEAD
";
    let branch = DefaultBranchName::from_remote(output)
        .map(DefaultBranchName::into_string)
        .unwrap();
    assert_eq!(branch, "main");
}

#[test]
fn test_parse_remote_default_branch_master() {
    let output = "ref: refs/heads/master\tHEAD
abcd1234567890abcd1234567890abcd12345678\tHEAD
";
    let branch = DefaultBranchName::from_remote(output)
        .map(DefaultBranchName::into_string)
        .unwrap();
    assert_eq!(branch, "master");
}

#[test]
fn test_parse_remote_default_branch_custom() {
    let output = "ref: refs/heads/develop\tHEAD
1234567890abcdef1234567890abcdef12345678\tHEAD
";
    let branch = DefaultBranchName::from_remote(output)
        .map(DefaultBranchName::into_string)
        .unwrap();
    assert_eq!(branch, "develop");
}

#[test]
fn test_parse_remote_default_branch_only_symref_line() {
    let output = "ref: refs/heads/main\tHEAD\n";
    let branch = DefaultBranchName::from_remote(output)
        .map(DefaultBranchName::into_string)
        .unwrap();
    assert_eq!(branch, "main");
}

#[test]
fn test_parse_remote_default_branch_missing_symref() {
    let output = "85a1ce7c7182540f9c02453441cb3e8bf0ced214\tHEAD\n";
    let result = DefaultBranchName::from_remote(output).map(DefaultBranchName::into_string);
    assert!(result.is_err());
}

#[test]
fn test_parse_remote_default_branch_empty() {
    let output = "";
    let result = DefaultBranchName::from_remote(output).map(DefaultBranchName::into_string);
    assert!(result.is_err());
}

#[test]
fn test_parse_remote_default_branch_malformed_ref() {
    // Missing refs/heads/ prefix
    let output = "ref: main\tHEAD\n";
    let result = DefaultBranchName::from_remote(output).map(DefaultBranchName::into_string);
    assert!(result.is_err());
}

#[test]
fn test_parse_remote_default_branch_with_spaces() {
    // Space instead of tab - should be rejected as malformed input
    let output = "ref: refs/heads/main HEAD\n";
    let result = DefaultBranchName::from_remote(output).map(DefaultBranchName::into_string);
    // Using split_once correctly rejects malformed input with spaces instead of tabs
    assert!(result.is_err());
}

#[test]
fn test_parse_remote_default_branch_branch_with_slash() {
    let output = "ref: refs/heads/feature/new-ui\tHEAD\n";
    let branch = DefaultBranchName::from_remote(output)
        .map(DefaultBranchName::into_string)
        .unwrap();
    assert_eq!(branch, "feature/new-ui");
}

use super::{Repository, ResolvedWorktree};

#[test]
fn test_resolved_worktree_debug() {
    let wt = ResolvedWorktree::Worktree {
        path: PathBuf::from("/path/to/worktree"),
        branch: Some("feature".to_string()),
    };
    let debug = format!("{:?}", wt);
    assert!(debug.contains("Worktree"));
    assert!(debug.contains("/path/to/worktree"));
    assert!(debug.contains("feature"));
}

#[test]
fn test_resolved_worktree_branch_only_debug() {
    let wt = ResolvedWorktree::BranchOnly {
        branch: "feature".to_string(),
    };
    let debug = format!("{:?}", wt);
    assert!(debug.contains("BranchOnly"));
    assert!(debug.contains("feature"));
}

#[test]
fn test_resolved_worktree_clone() {
    let wt = ResolvedWorktree::Worktree {
        path: PathBuf::from("/path/to/worktree"),
        branch: Some("feature".to_string()),
    };
    let cloned = wt.clone();
    if let ResolvedWorktree::Worktree { path, branch } = cloned {
        assert_eq!(path, PathBuf::from("/path/to/worktree"));
        assert_eq!(branch, Some("feature".to_string()));
    } else {
        panic!("Expected Worktree variant");
    }
}

#[test]
fn test_resolved_worktree_none_branch() {
    // Worktree with detached HEAD (no branch)
    let wt = ResolvedWorktree::Worktree {
        path: PathBuf::from("/path/to/worktree"),
        branch: None,
    };
    if let ResolvedWorktree::Worktree { path, branch } = wt {
        assert_eq!(path, PathBuf::from("/path/to/worktree"));
        assert!(branch.is_none());
    } else {
        panic!("Expected Worktree variant");
    }
}

#[test]
fn test_worktree_locked_empty_reason() {
    let output = "worktree /path/to/locked
HEAD abcd1234
branch refs/heads/main
locked

";

    let worktrees = Worktree::parse_porcelain_list(output).unwrap();
    assert_eq!(worktrees.len(), 1);
    // Empty lock reason should still be recorded
    assert_eq!(worktrees[0].locked, Some(String::new()));
}

#[test]
fn test_worktree_prunable() {
    let output = "worktree /path/to/prunable
HEAD abcd1234
detached
prunable gitdir file points to non-existent location

";

    let worktrees = Worktree::parse_porcelain_list(output).unwrap();
    assert_eq!(worktrees.len(), 1);
    assert!(worktrees[0].prunable.is_some());
    assert!(
        worktrees[0]
            .prunable
            .as_ref()
            .unwrap()
            .contains("non-existent")
    );
}

#[test]
fn test_parse_multiple_worktrees() {
    let output = "worktree /main
HEAD 1111111111111111111111111111111111111111
branch refs/heads/main

worktree /feature-a
HEAD 2222222222222222222222222222222222222222
branch refs/heads/feature-a

worktree /feature-b
HEAD 3333333333333333333333333333333333333333
branch refs/heads/feature-b

worktree /detached
HEAD 4444444444444444444444444444444444444444
detached

";

    let worktrees = Worktree::parse_porcelain_list(output).unwrap();
    assert_eq!(worktrees.len(), 4);
    assert_eq!(worktrees[0].branch, Some("main".to_string()));
    assert_eq!(worktrees[1].branch, Some("feature-a".to_string()));
    assert_eq!(worktrees[2].branch, Some("feature-b".to_string()));
    assert!(worktrees[3].detached);
    assert_eq!(worktrees[3].branch, None);
}

#[test]
fn test_default_branch_name_display() {
    // Test that DefaultBranchName properly extracts branch names
    let cases = [
        ("origin/main\n", "main"),
        ("upstream/develop\n", "develop"),
        ("origin/master\n", "master"),
    ];

    for (input, expected) in cases {
        let remote = input.split('/').next().unwrap();
        let branch = DefaultBranchName::from_local(remote, input)
            .map(DefaultBranchName::into_string)
            .unwrap();
        assert_eq!(branch, expected);
    }
}

#[test]
fn test_fuzzy_match_exact_match() {
    // Exact match should return the match
    let branches = vec![
        "main".to_string(),
        "online-mods".to_string(),
        "feature".to_string(),
    ];
    let result = Repository::fuzzy_match_branch("online-mods", &branches);
    assert_eq!(result, Some("online-mods"));
}

#[test]
fn test_fuzzy_match_prefix() {
    // Prefix match: "onli" should match "online-mods"
    let branches = vec![
        "main".to_string(),
        "online-mods".to_string(),
        "online-config".to_string(),
        "feature".to_string(),
    ];
    let result = Repository::fuzzy_match_branch("onli", &branches);
    // Should match one of the online branches
    assert!(result == Some("online-mods") || result == Some("online-config"));
}

#[test]
fn test_fuzzy_match_subsequence() {
    // Subsequence match: "f-b" should match "feature-branch"
    let branches = vec![
        "main".to_string(),
        "feature-branch".to_string(),
        "feature-build".to_string(),
    ];
    let result = Repository::fuzzy_match_branch("f-b", &branches);
    assert!(result.is_some());
    let matched = result.unwrap();
    // Should match one of the feature branches
    assert!(matched == "feature-build" || matched == "feature-branch");
}

#[test]
fn test_fuzzy_match_no_match() {
    // No match should return None
    let branches = vec![
        "main".to_string(),
        "online-mods".to_string(),
        "feature".to_string(),
    ];
    let result = Repository::fuzzy_match_branch("xyz", &branches);
    assert_eq!(result, None);
}

#[test]
fn test_fuzzy_match_empty_input() {
    // Empty input should return None
    let branches = vec!["main".to_string(), "online-mods".to_string()];
    let result = Repository::fuzzy_match_branch("", &branches);
    assert_eq!(result, None);
}

#[test]
fn test_fuzzy_match_empty_branches() {
    // Empty branches should return None
    let branches = vec![];
    let result = Repository::fuzzy_match_branch("onli", &branches);
    assert_eq!(result, None);
}
