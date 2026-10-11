use std::path::PathBuf;

use super::super::{DefaultBranchName, WorktreeInfo, finalize_worktrees};
use crate::git::Repository;
use crate::testing::TestRepo;

#[cfg(unix)]
#[test]
fn alternate_object_directory_uses_git_c_style_quoting() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let path = OsStr::from_bytes(b"a:b\n\r\t\x08\x0c\\\"\x01\xff");
    let quoted = super::quote_alternate_object_directory(path);
    assert_eq!(
        quoted.into_vec(),
        b"\"a:b\\n\\r\\t\\b\\f\\\\\\\"\\001\\377\""
    );
}

#[cfg(not(unix))]
#[test]
fn alternate_object_directory_uses_git_c_style_quoting() {
    let quoted = super::quote_alternate_object_directory(std::ffi::OsStr::new("a;b\n\r\t\\\""));
    assert_eq!(quoted, "\"a;b\\n\\r\\t\\\\\\\"\"");
}

#[test]
fn test_parse_worktree_list() {
    let output = "worktree /path/to/main
HEAD abcd1234
branch refs/heads/main

worktree /path/to/feature
HEAD efgh5678
branch refs/heads/feature

";

    let worktrees = WorktreeInfo::parse_porcelain_list(output).unwrap();
    let [main_wt, feature_wt]: [WorktreeInfo; 2] = worktrees.try_into().unwrap();

    assert_eq!(main_wt.path, PathBuf::from("/path/to/main"));
    assert_eq!(main_wt.head, "abcd1234");
    assert_eq!(main_wt.branch, Some("main".to_string()));
    assert!(!main_wt.bare);
    assert!(!main_wt.detached);

    assert_eq!(feature_wt.path, PathBuf::from("/path/to/feature"));
    assert_eq!(feature_wt.head, "efgh5678");
    assert_eq!(feature_wt.branch, Some("feature".to_string()));
}

#[test]
fn test_parse_detached_worktree() {
    let output = "worktree /path/to/detached
HEAD abcd1234
detached

";

    let worktrees = WorktreeInfo::parse_porcelain_list(output).unwrap();
    let [wt]: [WorktreeInfo; 1] = worktrees.try_into().unwrap();
    assert!(wt.detached);
    assert_eq!(wt.branch, None);
}

#[test]
fn test_finalize_worktree_with_branch() {
    // Worktree with a branch should not be modified
    let wt = WorktreeInfo {
        path: PathBuf::from("/path/to/worktree"),
        head: "abcd1234".to_string(),
        branch: Some("feature".to_string()),
        bare: false,
        detached: false,
        locked: None,
        prunable: None,
    };

    let test = TestRepo::with_initial_commit();
    let repo = Repository::at(test.root_path()).unwrap();
    let mut worktrees = [wt];
    finalize_worktrees(&repo, &mut worktrees);
    let [finalized] = worktrees;
    assert_eq!(finalized.branch, Some("feature".to_string()));
}

#[test]
fn test_finalize_worktree_detached_with_branch() {
    // Detached worktree with a branch (unusual but possible) should keep the branch
    let wt = WorktreeInfo {
        path: PathBuf::from("/path/to/worktree"),
        head: "abcd1234".to_string(),
        branch: Some("feature".to_string()),
        bare: false,
        detached: true,
        locked: None,
        prunable: None,
    };

    let test = TestRepo::with_initial_commit();
    let repo = Repository::at(test.root_path()).unwrap();
    let mut worktrees = [wt];
    finalize_worktrees(&repo, &mut worktrees);
    let [finalized] = worktrees;
    assert_eq!(finalized.branch, Some("feature".to_string()));
}

#[test]
fn test_finalize_worktree_detached_no_branch() {
    // A detached worktree at a nonexistent path fails the git-dir lookup,
    // so the branch stays empty.
    let wt = WorktreeInfo {
        path: PathBuf::from("/nonexistent/path"),
        head: "abcd1234".to_string(),
        branch: None,
        bare: false,
        detached: true,
        locked: None,
        prunable: None,
    };

    let test = TestRepo::with_initial_commit();
    let repo = Repository::at(test.root_path()).unwrap();
    let mut worktrees = [wt];
    finalize_worktrees(&repo, &mut worktrees);
    let [finalized] = worktrees;
    assert_eq!(finalized.branch, None);
}

#[test]
fn test_finalize_worktree_linked_mid_rebase() {
    // A linked worktree stopped mid-rebase is detached, so `git worktree list`
    // reports no branch; the branch comes from `rebase-merge/head-name` under
    // its git dir.
    let test = TestRepo::with_initial_commit();
    let linked = test.root_path().parent().unwrap().join("linked-rebase");
    test.run_git(&["worktree", "add", "-b", "feature", linked.to_str().unwrap()]);
    std::fs::write(linked.join("feature.txt"), "feature\n").unwrap();
    test.run_git_in(&linked, &["add", "feature.txt"]);
    test.run_git_in(&linked, &["commit", "-m", "Feature"]);
    // `--exec` stops the rebase after replaying the commit, leaving it open.
    let _ = test
        .git_command()
        .current_dir(&linked)
        .args(["rebase", "--exec", "false", "HEAD~1"])
        .run();

    let repo = Repository::at(test.root_path()).unwrap();
    let linked = dunce::canonicalize(&linked).unwrap();
    let wt = repo
        .list_worktrees()
        .unwrap()
        .iter()
        .find(|wt| dunce::canonicalize(&wt.path).unwrap() == linked)
        .unwrap()
        .clone();
    assert!(wt.detached, "precondition: the rebase detaches HEAD");
    assert_eq!(wt.branch.as_deref(), Some("feature"));
}

#[test]
fn test_finalize_worktree_stale_reads_its_own_rebase() {
    // A stale entry's rebase state is in its registration. A lookup from its
    // path would walk up past the missing `.git`: `nested` sits inside the main
    // worktree, which is itself rebasing `main-work`, so that lookup would
    // name `nested` after a rebase that is not its own — and `wt step prune`
    // would then judge the entry by `main-work`, not by its own detached HEAD.
    let test = TestRepo::with_initial_commit();
    let root = test.root_path().to_path_buf();
    let rebase_stopped = |dir: &std::path::Path, branch: &str| {
        test.run_git_in(dir, &["switch", "-c", branch]);
        std::fs::write(dir.join(format!("{branch}.txt")), "work\n").unwrap();
        test.run_git_in(dir, &["add", "."]);
        test.run_git_in(dir, &["commit", "-m", branch]);
        // `--exec` stops the rebase after replaying the commit, leaving it open.
        let _ = test
            .git_command()
            .current_dir(dir)
            .args(["rebase", "--exec", "false", "HEAD~1"])
            .run();
    };

    let nested = root.join(".worktrees").join("nested");
    test.run_git(&["worktree", "add", "--detach", nested.to_str().unwrap()]);
    let rebasing = root.parent().unwrap().join("stale-rebase");
    test.run_git(&["worktree", "add", "--detach", rebasing.to_str().unwrap()]);
    rebase_stopped(&rebasing, "feature");
    rebase_stopped(&root, "main-work");
    std::fs::remove_file(nested.join(".git")).unwrap();
    std::fs::remove_file(rebasing.join(".git")).unwrap();

    let repo = Repository::at(&root).unwrap();
    let worktrees = repo.list_worktrees().unwrap();
    let find = |path: &std::path::Path| {
        let path = dunce::canonicalize(path).unwrap();
        worktrees
            .iter()
            .find(|wt| dunce::canonicalize(&wt.path).unwrap() == path)
            .unwrap()
    };
    let (main, nested, rebasing) = (find(&root), find(&nested), find(&rebasing));
    assert_eq!(main.branch.as_deref(), Some("main-work"));
    assert!(nested.is_prunable() && rebasing.is_prunable());
    assert!(nested.detached && rebasing.detached);
    assert_eq!(nested.branch, None);
    assert_eq!(rebasing.branch.as_deref(), Some("feature"));
}

#[test]
fn test_parse_locked_worktree() {
    let output = "worktree /path/to/locked
HEAD abcd1234
branch refs/heads/main
locked reason for lock

";

    let worktrees = WorktreeInfo::parse_porcelain_list(output).unwrap();
    let [wt]: [WorktreeInfo; 1] = worktrees.try_into().unwrap();
    assert_eq!(wt.locked, Some("reason for lock".to_string()));
}

#[test]
fn test_parse_bare_worktree() {
    let output = "worktree /path/to/bare
HEAD abcd1234
bare

";

    let worktrees = WorktreeInfo::parse_porcelain_list(output).unwrap();
    let [wt]: [WorktreeInfo; 1] = worktrees.try_into().unwrap();
    assert!(wt.bare);
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

use super::ResolvedWorktree;

#[test]
fn test_resolved_worktree_clone() {
    let wt = ResolvedWorktree::worktree(
        PathBuf::from("/path/to/worktree"),
        Some("feature".to_string()),
    );
    let cloned = wt.clone();
    if let ResolvedWorktree::Worktree { path, branch, .. } = cloned {
        assert_eq!(path, PathBuf::from("/path/to/worktree"));
        assert_eq!(branch, Some("feature".to_string()));
    } else {
        panic!("Expected Worktree variant");
    }
}

#[test]
fn test_resolved_worktree_none_branch() {
    // Worktree with detached HEAD (no branch)
    let wt = ResolvedWorktree::worktree(PathBuf::from("/path/to/worktree"), None);
    if let ResolvedWorktree::Worktree { path, branch, .. } = wt {
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

    let worktrees = WorktreeInfo::parse_porcelain_list(output).unwrap();
    let [wt]: [WorktreeInfo; 1] = worktrees.try_into().unwrap();
    // Empty lock reason should still be recorded
    assert_eq!(wt.locked, Some(String::new()));
}

#[test]
fn test_worktree_prunable() {
    let output = "worktree /path/to/prunable
HEAD abcd1234
detached
prunable gitdir file points to non-existent location

";

    let worktrees = WorktreeInfo::parse_porcelain_list(output).unwrap();
    let [wt]: [WorktreeInfo; 1] = worktrees.try_into().unwrap();
    assert!(wt.prunable.is_some());
    assert!(wt.prunable.as_ref().unwrap().contains("non-existent"));
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

    let worktrees = WorktreeInfo::parse_porcelain_list(output).unwrap();
    let [main_wt, feature_a, feature_b, detached_wt]: [WorktreeInfo; 4] =
        worktrees.try_into().unwrap();
    assert_eq!(main_wt.branch, Some("main".to_string()));
    assert_eq!(feature_a.branch, Some("feature-a".to_string()));
    assert_eq!(feature_b.branch, Some("feature-b".to_string()));
    assert!(detached_wt.detached);
    assert_eq!(detached_wt.branch, None);
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
fn repo_path_error_when_is_bare_fails() {
    use super::RepoCache;
    use std::sync::Arc;

    // Create a Repository with a non-existent git_common_dir.
    // is_bare() fails because the bulk config read can't run in a missing dir.
    let repo = super::Repository {
        discovery_path: PathBuf::from("/nonexistent/repo"),
        git_common_dir: PathBuf::from("/nonexistent/.git"),
        cache: Arc::new(RepoCache::default()),
        locks: Arc::new(super::RepositoryLocks::default()),
        temporary_object_store: None,
    };

    let err = repo.repo_path().unwrap_err();
    let msg = format!("{err:#}");
    // The OS error text is platform-specific (e.g., "No such file or directory" on Unix,
    // "The directory name is invalid." on Windows), so only assert the stable prefix.
    assert!(
        msg.starts_with("failed to read git config: "),
        "unexpected error message: {msg}"
    );
}

#[test]
fn repo_path_ignores_non_local_core_worktree() {
    use super::RepoCache;
    use indexmap::IndexMap;
    use std::sync::{Arc, RwLock};

    // Regression test: `git config --list -z` merges system + global + local
    // scope, but git only honors `core.worktree` from local config. A user
    // with a stray global `core.worktree` in a normal repo should still see
    // `parent(.git)` as the repo root — `rev-parse --show-toplevel` fails
    // from inside `.git` in that case and we fall through.
    let tmp = tempfile::tempdir().unwrap();
    // Match Repository::at's canonical common-directory invariant.
    let root = dunce::canonicalize(tmp.path()).unwrap();
    let git_dir = root.join(".git");
    std::fs::create_dir(&git_dir).unwrap();

    let cache = RepoCache::default();
    let mut map: IndexMap<String, Vec<String>> = IndexMap::new();
    map.insert("core.bare".to_string(), vec!["false".to_string()]);
    // Simulate a `core.worktree` picked up from global config. The path
    // doesn't have to exist — it only has to be present in the bulk map.
    map.insert(
        "core.worktree".to_string(),
        vec!["/nonexistent/global/worktree".to_string()],
    );
    cache.all_config.set(RwLock::new(map)).unwrap();

    let repo = super::Repository {
        discovery_path: root.clone(),
        git_common_dir: git_dir.clone(),
        cache: Arc::new(cache),
        locks: Arc::new(super::RepositoryLocks::default()),
        temporary_object_store: None,
    };

    // The verified conventional parent wins over the non-local bulk value.
    assert_eq!(repo.repo_path().unwrap(), root);
}

#[test]
fn parse_config_list_z_basic() {
    let input = b"core.bare\nfalse\0remote.origin.url\nhttps://example.com/a.git\0";
    let map = super::parse_config_list_z(input);
    assert_eq!(map["core.bare"], vec!["false"]);
    assert_eq!(map["remote.origin.url"], vec!["https://example.com/a.git"]);
}

#[test]
fn parse_config_list_z_multivar() {
    let input =
        b"remote.origin.fetch\n+refs/heads/*:refs/remotes/origin/*\0remote.origin.fetch\n+refs/tags/*:refs/tags/*\0";
    let map = super::parse_config_list_z(input);
    assert_eq!(
        map["remote.origin.fetch"],
        vec![
            "+refs/heads/*:refs/remotes/origin/*",
            "+refs/tags/*:refs/tags/*"
        ]
    );
}

#[test]
fn parse_config_list_z_newline_in_value() {
    // A value with embedded newlines is preserved verbatim because -z uses
    // NUL as the record separator. The split_once('\n') only splits on the
    // first newline (which separates key from value).
    let input = b"commit.template\nline1\nline2\0core.bare\nfalse\0";
    let map = super::parse_config_list_z(input);
    assert_eq!(map["commit.template"], vec!["line1\nline2"]);
    assert_eq!(map["core.bare"], vec!["false"]);
}

#[test]
fn parse_config_list_z_equals_in_value() {
    // Values containing `=` are preserved — no splitting on `=` because
    // the key/value separator under `-z` is `\n`.
    let input = b"user.email\nme=you@example.com\0";
    let map = super::parse_config_list_z(input);
    assert_eq!(map["user.email"], vec!["me=you@example.com"]);
}

#[test]
fn parse_config_list_z_empty() {
    let map = super::parse_config_list_z(b"");
    assert!(map.is_empty());
}

#[test]
fn parse_config_list_z_key_without_value_is_true() {
    // A key written with no `= value` is git's implicit boolean true, and
    // `--list -z` emits it as `key\0` with no newline. An explicitly empty
    // value (`key = ""`) keeps its newline, and git reads that as false.
    let input = b"core.bare\0core.fsmonitor\n\0other.key\nfalse\0";
    let map = super::parse_config_list_z(input);
    assert_eq!(map["core.bare"], vec!["true"]);
    assert_eq!(map["core.fsmonitor"], vec![""]);
    assert_eq!(map["other.key"], vec!["false"]);
}

#[test]
fn config_bool_reads_key_without_value_as_true() {
    let test = TestRepo::with_initial_commit();
    let config_path = test.root_path().join(".git/config");
    let mut config = std::fs::read_to_string(&config_path).unwrap();
    config.push_str("[core]\n\tfsmonitor\n");
    std::fs::write(&config_path, config).unwrap();

    let repo = Repository::at(test.root_path()).unwrap();
    assert!(repo.is_builtin_fsmonitor_enabled());
}

#[test]
fn canonical_config_key_cases() {
    // section + variable: both lowercased
    assert_eq!(
        super::canonical_config_key("init.defaultBranch"),
        "init.defaultbranch"
    );
    assert_eq!(
        super::canonical_config_key("checkout.defaultRemote"),
        "checkout.defaultremote"
    );
    assert_eq!(super::canonical_config_key("core.Bare"), "core.bare");
    // 3+ parts: section + variable lowercased, subsection preserved
    assert_eq!(
        super::canonical_config_key("remote.MyFork.url"),
        "remote.MyFork.url"
    );
    assert_eq!(
        super::canonical_config_key("branch.MyBranch.pushRemote"),
        "branch.MyBranch.pushremote"
    );
    // 4+ parts: subsection is the middle (spanning dots); only first and last lowercase
    assert_eq!(
        super::canonical_config_key("worktrunk.state.MyBranch.marker"),
        "worktrunk.state.MyBranch.marker"
    );
}

#[test]
fn parse_git_bool_variants() {
    for truthy in ["true", "TRUE", "True", "1", "yes", "YES", "on", "ON"] {
        assert!(super::parse_git_bool(truthy), "{truthy} should be true");
    }
    for falsy in ["false", "0", "no", "off", "", "anything-else"] {
        assert!(!super::parse_git_bool(falsy), "{falsy} should be false");
    }
}

#[test]
fn worktree_config_enabled_detects_extension() {
    // The detector keys on the lowercased canonical form that git emits in
    // `--list -z` output. Both git-bool truthy spellings and the absent/
    // explicitly-false cases must classify correctly. A false truthy costs
    // one extra prewarm fork (the common-dir post-pass caches the same map);
    // a false falsy caches the incomplete linked-worktree map and
    // reintroduces the #2779 `is_bare=false` bug.
    use indexmap::IndexMap;

    let mut empty: IndexMap<String, Vec<String>> = IndexMap::new();
    assert!(!super::worktree_config_enabled(&empty));

    empty.insert(
        "extensions.worktreeconfig".to_string(),
        vec!["false".to_string()],
    );
    assert!(!super::worktree_config_enabled(&empty));

    for truthy in ["true", "1", "yes", "on"] {
        let mut map: IndexMap<String, Vec<String>> = IndexMap::new();
        map.insert(
            "extensions.worktreeconfig".to_string(),
            vec![truthy.to_string()],
        );
        assert!(
            super::worktree_config_enabled(&map),
            "{truthy} should be truthy"
        );
    }

    // Multivar: the *last* value wins, matching git's own semantics.
    let mut map: IndexMap<String, Vec<String>> = IndexMap::new();
    map.insert(
        "extensions.worktreeconfig".to_string(),
        vec!["true".to_string(), "false".to_string()],
    );
    assert!(!super::worktree_config_enabled(&map));
    let mut map: IndexMap<String, Vec<String>> = IndexMap::new();
    map.insert(
        "extensions.worktreeconfig".to_string(),
        vec!["false".to_string(), "true".to_string()],
    );
    assert!(super::worktree_config_enabled(&map));
}

#[test]
fn extract_failed_command_from_stream_error() {
    use crate::shell_exec::StreamCommandError;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt;

    let err: anyhow::Error = StreamCommandError {
        output: "fatal: ref exists".into(),
        command: "git worktree add /path".into(),
        status: std::process::ExitStatus::from_raw(if cfg!(unix) { 128 << 8 } else { 128 }),
    }
    .into();

    let (output, cmd) = super::Repository::extract_failed_command(&err);
    assert_eq!(output, "fatal: ref exists");
    let cmd = cmd.unwrap();
    assert_eq!(cmd.command, "git worktree add /path");
    assert_eq!(cmd.exit_info, "exit code 128");
}

#[test]
fn extract_failed_command_from_other_error() {
    let err = anyhow::anyhow!("some other error");

    let (output, cmd) = super::Repository::extract_failed_command(&err);
    assert_eq!(output, "some other error");
    assert!(cmd.is_none());
}

#[test]
fn extract_failed_command_from_command_error() {
    // A buffered `git` failure (Repository::run_command,
    // WorkingTree::run_command) surfaces as a typed CommandError, possibly
    // wrapped by `.context(...)`. extract_failed_command must walk the
    // chain so callers like switch::worktree_creation_error keep working.
    use crate::git::CommandError;
    use anyhow::Context;

    let inner = CommandError {
        program: "git".into(),
        args: vec!["worktree".into(), "add".into(), "/path".into()],
        stderr: "fatal: invalid reference: foo".into(),
        stdout: String::new(),
        exit_code: Some(128),
        signal: None,
    };
    let err: anyhow::Error = Err::<(), _>(inner)
        .context("creating worktree")
        .unwrap_err();

    let (output, cmd) = super::Repository::extract_failed_command(&err);
    assert_eq!(output, "fatal: invalid reference: foo");
    let cmd = cmd.unwrap();
    assert_eq!(cmd.command, "git worktree add /path");
    assert_eq!(cmd.exit_info, "exit code 128");
}

#[test]
fn is_builtin_fsmonitor_enabled_variants() {
    use super::RepoCache;
    use indexmap::IndexMap;
    use std::sync::{Arc, RwLock};

    fn repo_with_fsmonitor(value: Option<&str>) -> super::Repository {
        let cache = RepoCache::default();
        let mut map: IndexMap<String, Vec<String>> = IndexMap::new();
        if let Some(v) = value {
            map.insert("core.fsmonitor".to_string(), vec![v.to_string()]);
        }
        cache.all_config.set(RwLock::new(map)).unwrap();
        super::Repository {
            discovery_path: PathBuf::from("/nonexistent/repo"),
            git_common_dir: PathBuf::from("/nonexistent/.git"),
            cache: Arc::new(cache),
            locks: Arc::new(super::RepositoryLocks::default()),
            temporary_object_store: None,
        }
    }

    // Builtin daemon: any git-bool truthy value enables it.
    for truthy in ["true", "1", "yes", "on", "TRUE"] {
        assert!(
            repo_with_fsmonitor(Some(truthy)).is_builtin_fsmonitor_enabled(),
            "{truthy} should enable builtin fsmonitor"
        );
    }

    // Watchman hook path is NOT the builtin daemon — must return false even
    // though the value is non-empty and truthy in the colloquial sense.
    assert!(
        !repo_with_fsmonitor(Some("/usr/local/bin/git-fsmonitor-watchman.sh"))
            .is_builtin_fsmonitor_enabled()
    );

    // Explicitly disabled and unset both report false.
    for falsy in ["false", "0", "no", "off"] {
        assert!(
            !repo_with_fsmonitor(Some(falsy)).is_builtin_fsmonitor_enabled(),
            "{falsy} should disable builtin fsmonitor"
        );
    }
    assert!(!repo_with_fsmonitor(None).is_builtin_fsmonitor_enabled());
}

/// A daemon answering on the worktree's IPC socket means no `git
/// fsmonitor--daemon start` fork; with no socket to reach, the fork runs.
/// The listener stands in for a running daemon, so no real daemon starts.
#[cfg(all(unix, feature = "cli"))]
#[test]
fn start_fsmonitor_daemon_skips_fork_when_daemon_answers() {
    use std::os::unix::net::UnixListener;
    use std::sync::{Arc, Mutex};

    use tracing::field::{Field, Visit};
    use tracing_subscriber::Registry;
    use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

    use crate::git::Repository;
    use crate::testing::{TestRepo, test_tempdir};
    use crate::trace::WT_TRACE_TARGET;

    /// Collects the `cmd` of every traced subprocess.
    struct Commands(Arc<Mutex<Vec<String>>>);
    struct CmdField(Option<String>);
    impl Visit for CmdField {
        fn record_str(&mut self, field: &Field, value: &str) {
            if field.name() == "cmd" {
                self.0 = Some(value.to_string());
            }
        }
        fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
    }
    impl<S: tracing::Subscriber> Layer<S> for Commands {
        fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
            if event.metadata().target() != WT_TRACE_TARGET {
                return;
            }
            let mut cmd = CmdField(None);
            event.record(&mut cmd);
            self.0.lock().unwrap().extend(cmd.0);
        }
    }
    // While exactly one dispatcher is registered, tracing-core resolves a
    // callsite first hit on *another* thread against that thread's default
    // (no subscriber here) and caches `Interest::never` process-wide. A
    // parallel test's git command reaching the shared `cmd_completed`
    // callsite first would then silence the fork event below. A second live
    // dispatcher makes tracing-core resolve against every registered
    // dispatcher instead.
    let _pin = tracing::Dispatch::new(Registry::default());
    let traced_starts = |start: &dyn Fn()| {
        let commands = Arc::new(Mutex::new(Vec::new()));
        let subscriber = Registry::default().with(Commands(commands.clone()));
        tracing::subscriber::with_default(subscriber, start);
        let commands = commands.lock().unwrap();
        commands
            .iter()
            .filter(|cmd| *cmd == "git fsmonitor--daemon start")
            .count()
    };

    let test = TestRepo::with_initial_commit();
    let repo = Repository::at(test.root_path()).unwrap();
    let socket = repo
        .worktree_at(test.root_path())
        .git_dir()
        .unwrap()
        .join(super::super::fsmonitor::IPC_SOCKET_NAME);
    let listener = UnixListener::bind(&socket).unwrap_or_else(|e| {
        panic!(
            "bind {} ({} bytes; sun_path holds 103 on macOS): {e}",
            socket.display(),
            socket.as_os_str().len()
        )
    });
    listener.set_nonblocking(true).unwrap();

    let starts = traced_starts(&|| repo.start_fsmonitor_daemon_at(test.root_path()));
    assert_eq!(starts, 0, "a daemon answered, so nothing should fork");
    listener
        .accept()
        .expect("the probe should have connected to the daemon socket");

    // A directory outside any repository has no git dir to probe, so the
    // start forks (and git exits "not a git repository").
    let outside = test_tempdir();
    let starts = traced_starts(&|| repo.start_fsmonitor_daemon_at(outside.path()));
    assert_eq!(starts, 1, "nothing answered, so the start should fork");
}

#[test]
fn commit_details_many_returns_subject_with_spaces() {
    use crate::testing::TestRepo;

    let test = TestRepo::new();
    // Commit with a multi-word subject — regression against parsers that split
    // on the first space and treat "word1" as the timestamp.
    test.commit_with_message("first commit with spaces");
    let sha1 = test.repo.run_command(&["rev-parse", "HEAD"]).unwrap();
    let sha1 = sha1.trim().to_string();

    test.commit_with_message("second commit");
    let sha2 = test.repo.run_command(&["rev-parse", "HEAD"]).unwrap();
    let sha2 = sha2.trim().to_string();

    let result = test
        .repo
        .commit_details_many(&[sha1.as_str(), sha2.as_str()])
        .unwrap();

    assert_eq!(result.len(), 2);
    let (short1, ts1, subject1) = &result[&sha1];
    let (short2, ts2, subject2) = &result[&sha2];
    assert_eq!(subject1, "first commit with spaces");
    assert_eq!(subject2, "second commit");
    assert!(*ts1 > 0);
    assert!(*ts2 > 0);
    // Short SHAs are a prefix of the full SHA; default `core.abbrev` is 7,
    // and a fresh test repo never needs more than that.
    assert!(sha1.starts_with(short1.as_str()));
    assert!(sha2.starts_with(short2.as_str()));
}

#[test]
fn abbrev_len_follows_core_abbrev_and_covers_absent_objects() {
    use crate::git::Repository;
    use crate::testing::TestRepo;

    let test = TestRepo::with_initial_commit();

    // The width git actually uses here — the same one `%h` and `short_sha`
    // produce, which is the whole point of asking git rather than picking a
    // number.
    let head_short = test.repo.short_sha("HEAD").unwrap();
    assert_eq!(test.repo.abbrev_len(), head_short.chars().count());

    // `core.abbrev` reaches it. A fresh `Repository` because the width is
    // resolved once per repo handle and cached for the process.
    test.repo
        .run_command(&["config", "core.abbrev", "12"])
        .unwrap();
    let reread = Repository::at(test.path()).unwrap();
    assert_eq!(reread.abbrev_len(), 12);

    // The case `short_sha` can't serve per-SHA: an object that isn't in the
    // store, as every commit the `--prs` log tab gets from a forge API is. git
    // has nothing to disambiguate against, so this width is its whole answer.
    let absent = "0".repeat(head_short.chars().count().max(40));
    assert_eq!(
        reread.short_sha(&absent).unwrap().chars().count(),
        reread.abbrev_len(),
    );
}

#[test]
fn commit_details_many_empty_input_is_noop() {
    use crate::testing::TestRepo;

    let test = TestRepo::with_initial_commit();
    let result = test.repo.commit_details_many(&[]).unwrap();
    assert!(result.is_empty());
}

#[test]
fn commit_details_many_primes_commit_tree_cache() {
    use crate::testing::TestRepo;

    // The batch reads `%T` so the `wt list` per-row tree lookups
    // (CommittedTreesMatch / WouldMergeAdd) hit memory instead of forking
    // `git rev-parse <sha>^{tree}` each.
    let test = TestRepo::new();
    test.commit_with_message("first");
    let sha = test.repo.run_command(&["rev-parse", "HEAD"]).unwrap();
    let sha = sha.trim().to_string();
    let tree = test.git_output(&["rev-parse", "HEAD^{tree}"]);

    // Cold before the batch.
    assert!(!test.repo.cache.commit_tree.contains_key(&sha));

    test.repo.commit_details_many(&[sha.as_str()]).unwrap();

    let cached = test
        .repo
        .cache
        .commit_tree
        .get(&sha)
        .map(|e| e.value().clone());
    assert_eq!(cached, Some(tree));
}

#[test]
fn prime_worktree_path_caches_matches_subprocess() {
    use crate::git::Repository;
    use crate::testing::TestRepo;
    use dunce::canonicalize;

    let test = TestRepo::with_initial_commit();
    // A linked worktree as a sibling of the main worktree.
    let linked = test.root_path().parent().unwrap().join("linked-feature");
    test.run_git(&["worktree", "add", "-b", "feature", linked.to_str().unwrap()]);

    let repo = Repository::at(test.root_path()).unwrap();
    let worktrees: Vec<_> = repo.list_worktrees().unwrap().to_vec();
    assert!(worktrees.len() >= 2, "main + linked worktree");

    repo.prime_worktree_path_caches(&worktrees);

    for wt in &worktrees {
        let key = canonicalize(&wt.path).unwrap();
        // Priming actually populated both maps (no silent skip).
        let primed_root = super::WORKTREE_ROOTS
            .get(&key)
            .map(|e| e.value().clone())
            .expect("root primed");
        let primed_git_dir = super::GIT_DIRS
            .get(&key)
            .map(|e| e.value().clone())
            .expect("git_dir primed");

        // Oracle: drop the primed entries, then let the production `root()` /
        // `git_dir()` recompute via `git rev-parse`. The primed values must
        // match what the subprocess path produces.
        super::WORKTREE_ROOTS.remove(&key);
        super::GIT_DIRS.remove(&key);
        let wtree = repo.worktree_at(&wt.path);
        assert_eq!(primed_root, wtree.root().unwrap(), "root for {key:?}");
        assert_eq!(
            primed_git_dir,
            wtree.git_dir().unwrap(),
            "git_dir for {key:?}"
        );
    }
}

/// Git's own gitfile identity must survive whitespace and native bytes;
/// resolving a trimmed or replacement-character neighbor is a different repo.
#[cfg(unix)]
#[test]
fn git_dir_at_preserves_native_gitfile_target() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let test = TestRepo::with_initial_commit();
    let names = [
        b"store\twith\nwhitespace  ".as_slice(),
        #[cfg(target_os = "linux")]
        b"store-\xff  ".as_slice(),
    ];
    for (index, name) in names.into_iter().enumerate() {
        let target = test
            .root_path()
            .parent()
            .unwrap()
            .join(std::ffi::OsString::from_vec(name.to_vec()));
        let neighbor = PathBuf::from(target.to_string_lossy().trim_end());
        let child = test
            .root_path()
            .parent()
            .unwrap()
            .join(format!("gitfile-{index}"));
        let output = test
            .git_command()
            .arg("clone")
            .arg(test.root_path().to_str().unwrap())
            .arg(child.to_str().unwrap())
            .run()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        // Move native paths through the filesystem rather than Cmd's UTF-8 arguments.
        std::fs::rename(child.join(".git"), &target).unwrap();
        std::fs::create_dir(&neighbor).unwrap();
        let output = test
            .git_command()
            .current_dir(&neighbor)
            .args(["init", "--bare"])
            .run()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_ne!(
            dunce::canonicalize(&target).unwrap(),
            dunce::canonicalize(&neighbor).unwrap()
        );
        // CRLF is a Git terminator; the trailing spaces belong to the path.
        let mut content = b"gitdir: ".to_vec();
        content.extend_from_slice(target.as_os_str().as_bytes());
        content.extend_from_slice(b"\r\n");
        std::fs::write(child.join(".git"), content).unwrap();
        assert_eq!(
            Repository::git_dir_at(&child).unwrap(),
            Some(dunce::canonicalize(&target).unwrap())
        );
        let output = test
            .git_command()
            .current_dir(&child)
            .args(["rev-parse", "--absolute-git-dir"])
            .run()
            .unwrap();
        assert!(
            output.status.success(),
            "Git must accept the real gitfile: {output:?}"
        );
        let mut expected = dunce::canonicalize(target)
            .unwrap()
            .into_os_string()
            .into_vec();
        expected.push(b'\n');
        assert_eq!(output.stdout, expected);
    }
}

#[cfg(unix)]
#[test]
fn git_dir_at_propagates_own_entry_io_failure() {
    let directory = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(".git", directory.path().join(".git")).unwrap();
    assert!(Repository::git_dir_at(directory.path()).is_err());
}

/// Unsupported own entries must not inherit a parent repository's identity.
#[test]
fn git_dir_at_refuses_malformed_gitfiles() {
    let mut test = TestRepo::with_initial_commit();
    let outer = test.add_worktree("malformed");
    let repo = Repository::at(test.root_path()).unwrap();
    let registration = repo.worktree_at(&outer).git_dir().unwrap();
    let index = std::fs::read(registration.join("index")).unwrap();
    let marker = outer.join(".git");
    let original = std::fs::read(&marker).unwrap();
    for contents in [b"not a gitfile".as_slice(), b"gitdir: \r\n".as_slice()] {
        std::fs::write(&marker, contents).unwrap();
        assert_eq!(Repository::git_dir_at(&outer).unwrap(), None);
        let error = repo
            .worktree_at(&outer)
            .ensure_holds_this_worktree()
            .unwrap_err();
        assert!(
            matches!(
                error.downcast_ref::<crate::git::GitError>(),
                Some(crate::git::GitError::WorktreePathNotOurs { .. })
            ),
            "{error:#}"
        );
        assert_eq!(std::fs::read(registration.join("index")).unwrap(), index);
    }
    std::fs::write(marker, original).unwrap();
    assert_eq!(
        repo.worktree_at(&outer)
            .ensure_holds_this_worktree()
            .unwrap(),
        registration
    );
}

#[cfg(unix)]
#[test]
fn git_dir_at_refuses_socket_and_unresolvable_target() {
    let mut test = TestRepo::with_initial_commit();
    let outer = test.add_worktree("own-entry-errors");
    let repo = Repository::at(test.root_path()).unwrap();
    let registration = repo.worktree_at(&outer).git_dir().unwrap();
    let index = std::fs::read(registration.join("index")).unwrap();
    let marker = outer.join(".git");
    let original = std::fs::read(&marker).unwrap();
    std::fs::remove_file(&marker).unwrap();
    let socket = std::os::unix::net::UnixListener::bind(&marker).unwrap();
    assert_eq!(Repository::git_dir_at(&outer).unwrap(), None);
    assert!(
        repo.worktree_at(&outer)
            .ensure_holds_this_worktree()
            .is_err()
    );
    drop(socket);
    std::fs::remove_file(&marker).unwrap();
    std::os::unix::fs::symlink("loop", outer.join("loop")).unwrap();
    std::fs::write(&marker, "gitdir: loop\n").unwrap();
    assert!(Repository::git_dir_at(&outer).is_err());
    let error = repo
        .worktree_at(&outer)
        .ensure_holds_this_worktree()
        .unwrap_err();
    assert!(
        error.to_string().contains("Failed to read Git entry"),
        "{error:#}"
    );
    assert_eq!(std::fs::read(registration.join("index")).unwrap(), index);
    std::fs::write(marker, original).unwrap();
}

#[test]
fn commit_details_many_fails_loudly_on_unknown_sha() {
    // `git log --no-walk` refuses the whole batch on a single bad ref. The
    // error surfaces as an `Err`, which `collect()` turns into a user-facing
    // warning. Pin this behavior so we don't silently fall back to
    // empty-map defaults.
    use crate::testing::TestRepo;

    let test = TestRepo::with_initial_commit();
    let bogus = "0000000000000000000000000000000000000001";
    let err = test.repo.commit_details_many(&[bogus]).unwrap_err();
    let msg = format!("{err:#}");
    assert!(
        msg.contains("git") || msg.contains("unknown") || msg.contains("bad"),
        "error message should surface git's complaint about the bogus SHA, got: {msg}"
    );
}

#[test]
fn commit_details_many_preserves_multibyte_utf8_subject() {
    // Pin that multibyte UTF-8 round-trips through the NUL-delimited parser —
    // `splitn(5, '\0')` works on byte positions, so char boundaries inside the
    // subject must be preserved intact.
    use crate::testing::TestRepo;

    let test = TestRepo::new();
    let subject = "Add support for 日本語 and émoji 🎉";
    test.commit_with_message(subject);
    let sha = test
        .repo
        .run_command(&["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();

    let result = test.repo.commit_details_many(&[sha.as_str()]).unwrap();

    assert_eq!(result[&sha].2, subject);
}

#[test]
fn commit_details_many_deduplicates_repeated_sha() {
    // `git log --no-walk SHA SHA` emits each commit once; pin that the batch
    // returns a single entry for a duplicated input SHA.
    use crate::testing::TestRepo;

    let test = TestRepo::new();
    test.commit_with_message("only commit");
    let sha = test
        .repo
        .run_command(&["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();

    let result = test
        .repo
        .commit_details_many(&[sha.as_str(), sha.as_str(), sha.as_str()])
        .unwrap();

    assert_eq!(result.len(), 1);
    assert_eq!(result[&sha].2, "only commit");
}

#[test]
fn commit_message_details_returns_subjects_and_bodies_in_git_order() {
    use super::CommitMessageDetail;
    use crate::testing::TestRepo;

    let test = TestRepo::new();
    test.commit_with_message("base");

    test.commit_in_worktree(
        test.root_path(),
        "first.txt",
        "first content",
        "first subject\n\nline 1\nline 2\n\nTrailer: value",
    );
    test.commit_in_worktree(
        test.root_path(),
        "second.txt",
        "second content",
        "second subject",
    );

    let range = "HEAD~2..HEAD";
    let result = test.repo.commit_message_details(range).unwrap();

    assert_eq!(
        result,
        vec![
            CommitMessageDetail {
                subject: "second subject".to_string(),
                body: String::new(),
            },
            CommitMessageDetail {
                subject: "first subject".to_string(),
                body: "line 1\nline 2\n\nTrailer: value\n".to_string(),
            },
        ]
    );
}

#[test]
#[cfg(unix)]
fn worktree_at_path_resolves_symlinked_path() {
    // Reproduction for issue #2460: `wt switch` fails to resolve existing
    // worktrees through symlink paths because `worktree_at_path` compares
    // paths lexically and misses symlink-equivalent spellings.
    use crate::testing::TestRepo;

    let mut test = TestRepo::with_initial_commit();
    let worktree_path = test.add_worktree("feature");

    let parent = worktree_path.parent().unwrap();
    let symlink_path = parent.join("feature-link");
    std::os::unix::fs::symlink(&worktree_path, &symlink_path).unwrap();

    let resolved = test.repo.worktree_at_path(&symlink_path).unwrap();
    assert!(
        resolved.is_some(),
        "worktree_at_path should resolve a symlinked path to its target worktree, got None for {symlink_path:?} -> {worktree_path:?}"
    );
    let (_, branch) = resolved.unwrap();
    assert_eq!(branch.as_deref(), Some("feature"));
}

/// `signs_commits` answers as `git commit` would in the worktree it runs from:
/// a valueless `gpgsign` is true to git, and a `--worktree` setting applies to
/// its own worktree only.
#[test]
fn signs_commits_follows_gits_reading_per_worktree() {
    use crate::git::Repository;
    use crate::testing::TestRepo;

    let mut test = TestRepo::with_initial_commit();
    let feature_path = test.add_worktree("feature");
    let signs = |path: &std::path::Path| Repository::at(path).unwrap().signs_commits().unwrap();

    assert!(!signs(test.root_path()), "unset means unsigned");

    let config_path = test.root_path().join(".git/config");
    let mut config = std::fs::read_to_string(&config_path).unwrap();
    config.push_str("[commit]\n\tgpgsign\n");
    std::fs::write(&config_path, config).unwrap();
    assert!(signs(test.root_path()), "a valueless key is true");

    test.run_git(&["config", "extensions.worktreeConfig", "true"]);
    test.run_git_in(
        &feature_path,
        &["config", "--worktree", "commit.gpgSign", "false"],
    );
    assert!(!signs(&feature_path), "the feature worktree opted out");
    assert!(signs(test.root_path()), "the opt-out stays in its worktree");

    // A value git can't read as a boolean is an error, not "unsigned" —
    // `git commit` would refuse the same way.
    test.run_git(&["config", "commit.gpgSign", "maybe"]);
    assert!(
        Repository::at(test.root_path())
            .unwrap()
            .signs_commits()
            .is_err(),
        "a value that isn't a boolean must surface as an error"
    );
}

#[test]
fn current_worktree_anchors_to_repository_discovery_path() {
    // `Repository::at(p).current_worktree()` must resolve relative to `p`,
    // not to the test process's CWD. PR #2625 fixed one symptom of the
    // opposite behavior (`infer_default_branch_locally` calling
    // `current_worktree().is_linked()` and reaching for process CWD); the
    // structural fix anchored `current_worktree` to `self.discovery_path`,
    // which this test pins down so the bug class can't regress.
    use super::Repository;
    use crate::testing::TestRepo;
    use dunce::canonicalize;

    let test = TestRepo::with_initial_commit();
    let repo = Repository::at(test.repo.discovery_path().to_path_buf()).unwrap();

    let wt_path = repo.current_worktree().path().to_path_buf();
    let expected = canonicalize(test.repo.discovery_path()).unwrap();
    assert_eq!(
        wt_path, expected,
        "current_worktree() should resolve to the Repository's discovery path, not the process CWD",
    );
    // The bug we're guarding against: `current_worktree()` previously used
    // the process CWD (`base_path()`), which for unit tests is the cargo
    // working directory — different from any test repo's tempdir.
    let process_cwd = canonicalize(std::env::current_dir().unwrap()).unwrap();
    assert_ne!(
        wt_path, process_cwd,
        "current_worktree() must not resolve to the process CWD when Repository::at(p) was given a different path",
    );
}

/// Build the bare-style `myproject/.git` layout with
/// `extensions.worktreeConfig = true` that triggers issue #2779.
///
/// Returns `(temp_dir, project_root, main_worktree_path, linked_worktree_path)`.
/// The bare git data lives in `<project>/.git` (no separate `.bare/` subdir);
/// `extensions.worktreeConfig` is set on shared config, and `core.bare = true`
/// is set on the main worktree's per-worktree config. The main and linked
/// worktrees sit beside `.git/` as sibling subdirectories.
#[cfg(test)]
fn build_worktree_config_bare_layout() -> (tempfile::TempDir, std::path::PathBuf, PathBuf, PathBuf)
{
    use super::canonicalize;
    use crate::shell_exec::Cmd;

    let tmp = tempfile::tempdir().unwrap();
    let project_root = canonicalize(tmp.path()).unwrap().join("myproject");
    std::fs::create_dir_all(&project_root).unwrap();
    let git_dir = project_root.join(".git");

    let git = || crate::testing::configure_git_env(Cmd::new("git"));

    let path_str = |p: &std::path::Path| p.to_str().unwrap().to_owned();

    // Bare repo at `myproject/.git` (no sibling `.bare/` — minimal layout
    // from issue #2779).
    let out = git()
        .args(["init", "--bare", "-b", "main", &path_str(&git_dir)])
        .run()
        .unwrap();
    assert!(out.status.success(), "git init --bare failed");

    // Flip the trio: bare = false in shared config, worktreeConfig enabled,
    // bare = true in the *main worktree's* per-worktree config (which lives
    // at `.git/config.worktree`).
    let shared_config = path_str(&git_dir.join("config"));
    git()
        .args(["config", "--file", &shared_config, "core.bare", "false"])
        .run()
        .unwrap();
    git()
        .args([
            "config",
            "--file",
            &shared_config,
            "extensions.worktreeConfig",
            "true",
        ])
        .run()
        .unwrap();
    std::fs::write(git_dir.join("config.worktree"), "[core]\n\tbare = true\n").unwrap();

    // Make HEAD resolvable and add a commit so worktree add can check out.
    let commit_sha = git()
        .current_dir(&git_dir)
        .args([
            "commit-tree",
            "-m",
            "init",
            super::integration::EMPTY_TREE_SHA1,
        ])
        .run()
        .unwrap();
    assert!(commit_sha.status.success(), "commit-tree failed");
    let sha = String::from_utf8(commit_sha.stdout)
        .unwrap()
        .trim()
        .to_string();
    git()
        .current_dir(&git_dir)
        .args(["update-ref", "refs/heads/main", &sha])
        .run()
        .unwrap();

    let main_worktree = project_root.join("main");
    let out = git()
        .current_dir(&git_dir)
        .args(["worktree", "add", &path_str(&main_worktree), "main"])
        .run()
        .unwrap();
    assert!(
        out.status.success(),
        "worktree add main: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let linked = project_root.join("feature");
    let out = git()
        .current_dir(&git_dir)
        .args(["worktree", "add", "-b", "feature", &path_str(&linked)])
        .run()
        .unwrap();
    assert!(
        out.status.success(),
        "worktree add feature: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    (
        tmp,
        project_root,
        canonicalize(&main_worktree).unwrap(),
        canonicalize(&linked).unwrap(),
    )
}

#[test]
fn prewarm_from_linked_worktree_under_worktree_config_preserves_is_bare() {
    // Regression for issue #2779. The bare-style `myproject/.git + sibling
    // worktrees` layout with `extensions.worktreeConfig = true` parks
    // `core.bare = true` in the *main worktree's* `config.worktree`. Git's
    // `config --list` run from a linked worktree merges only the linked
    // worktree's own per-worktree config, so it sees `core.bare = false`
    // and misses the `true`. Before the fix, `prewarm_git_config` cached
    // that incomplete map; `Repository::at` then short-circuited
    // `all_config()` with the stale value, and `is_bare()` returned
    // `false` from the linked worktree.
    //
    // The prewarm detects `extensions.worktreeconfig=true`, declines the
    // discovery-path map, and preloads the `git_common_dir` read instead —
    // the merged set `all_config()` would fork for on demand.
    use super::Repository;

    let (_tmp, _project, _main, linked) = build_worktree_config_bare_layout();

    Repository::prewarm_at(&linked);
    let repo = Repository::at(&linked).unwrap();

    assert!(
        repo.is_bare().unwrap(),
        "is_bare must read core.bare=true from the main worktree's config.worktree, \
         even when prewarm ran from a linked worktree"
    );
}

#[test]
fn repo_path_from_linked_worktree_under_worktree_config_is_git_common_dir() {
    // Companion to the is_bare regression above: once `is_bare()` reads
    // correctly, `repo_path()` returns `git_common_dir` (the bare branch
    // of the match). Before the fix it fell through to
    // `parent(git_common_dir)`, walking one level too high and producing
    // the "wt switch creates worktree in the parent dir" symptom from
    // issue #2779. Pin both paths.
    use super::Repository;
    use super::canonicalize;

    let (_tmp, _project, _main, linked) = build_worktree_config_bare_layout();

    Repository::prewarm_at(&linked);
    let repo = Repository::at(&linked).unwrap();
    let expected = canonicalize(repo.git_common_dir()).unwrap();
    let actual = canonicalize(repo.repo_path().unwrap()).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn prewarm_partial_warm_runs_only_the_cold_threads() {
    // The gates are per-cache: with the git-config preload already present
    // (and the process-wide user-config preload latched) but no resolved
    // common dir, prewarm still runs the rev-parse thread, and neither the
    // config thread nor the post-pass touches the existing preload.
    use super::canonicalize;
    use crate::shell_exec::Cmd;

    let tmp = tempfile::tempdir().unwrap();
    let init_repo = |name: &str| {
        let root = canonicalize(tmp.path()).unwrap().join(name);
        std::fs::create_dir_all(&root).unwrap();
        let out = crate::testing::configure_git_env(Cmd::new("git"))
            .args(["init", "-b", "main", root.to_str().unwrap()])
            .run()
            .unwrap();
        assert!(out.status.success(), "git init failed");
        root
    };

    // A full prewarm on one repo latches the process-wide user-config
    // preload (OnceLock), whichever test runs first.
    super::Repository::prewarm_at(&init_repo("first"));
    assert!(super::WORKTRUNK_USER_CONFIG_PRELOAD.get().is_some());

    // Second repo: hand-populate the config preload, then prewarm. Only
    // the rev-parse cache is cold.
    let second = init_repo("second");
    let sentinel: indexmap::IndexMap<String, Vec<String>> =
        [("wt.sentinel".to_string(), vec!["1".to_string()])]
            .into_iter()
            .collect();
    super::GIT_CONFIG_PRELOAD.insert(second.clone(), sentinel);

    super::Repository::prewarm_at(&second);

    assert!(
        super::GIT_COMMON_DIR_CACHE.contains_key(&second),
        "rev-parse thread must run when only its cache is cold"
    );
    let sentinel_value = |path: &std::path::Path| {
        super::GIT_CONFIG_PRELOAD
            .get(path)
            .and_then(|e| e.value().get("wt.sentinel").and_then(|v| v.last()).cloned())
    };
    assert_eq!(
        sentinel_value(&second),
        Some("1".to_string()),
        "an existing preload must not be re-read or overwritten"
    );

    // Fully warm: every gate is satisfied, so a repeat call early-returns
    // without spawning anything or touching the preload.
    super::Repository::prewarm_at(&second);
    assert_eq!(sentinel_value(&second), Some("1".to_string()));
}

#[test]
fn prewarm_post_pass_swallows_common_dir_read_failures() {
    // Best-effort contract: when the declined preload's common-dir re-read
    // fails — the directory is gone, or its config is corrupt — the post-pass
    // leaves the preload empty and the on-demand `all_config()` surfaces the
    // error. The rev-parse thread is skipped (its cache is hand-populated),
    // so the bogus common-dir entries survive to the post-pass.
    let (_tmp, _project, _main, linked) = build_worktree_config_bare_layout();

    // Spawn failure: the cached common dir does not exist.
    let missing = linked.join("no-such-dir");
    super::GIT_COMMON_DIR_CACHE.insert(linked.clone(), missing);
    super::Repository::prewarm_at(&linked);
    assert!(
        super::GIT_CONFIG_PRELOAD.get(&linked).is_none(),
        "a failed spawn must not populate the preload"
    );

    // Non-zero exit: the cached common dir is a git dir with corrupt config.
    let (_tmp2, project2, _main2, linked2) = build_worktree_config_bare_layout();
    let corrupt = project2.join("corrupt.git");
    let out = crate::testing::configure_git_env(crate::shell_exec::Cmd::new("git"))
        .args(["init", "--bare", corrupt.to_str().unwrap()])
        .run()
        .unwrap();
    assert!(out.status.success(), "git init --bare failed");
    std::fs::write(corrupt.join("config"), "[core\ngarbage").unwrap();
    super::GIT_COMMON_DIR_CACHE.insert(linked2.clone(), corrupt);
    super::Repository::prewarm_at(&linked2);
    assert!(
        super::GIT_CONFIG_PRELOAD.get(&linked2).is_none(),
        "a non-zero config read must not populate the preload"
    );
}

#[test]
fn all_config_without_prewarm_reads_common_dir_under_worktree_config() {
    // The on-demand branch: a `Repository::at` with no prewarm (a non-base
    // discovery path in production) gets no preload, so `all_config()` forks
    // — and it must fork from `git_common_dir`, not the discovery path, or
    // the linked worktree's partial config map reintroduces the #2779
    // `is_bare=false` bug. The prewarm post-pass caches the same read, so
    // only this prewarm-less construction still exercises the fork.
    use super::Repository;

    let (_tmp, _project, _main, linked) = build_worktree_config_bare_layout();

    let repo = Repository::at(&linked).unwrap();
    assert!(
        super::GIT_CONFIG_PRELOAD.get(&linked).is_none(),
        "test setup: no preload may exist for this path"
    );
    assert!(
        repo.is_bare().unwrap(),
        "on-demand all_config must read core.bare=true from the common dir"
    );
}

#[test]
fn prewarm_preload_under_worktree_config_holds_common_dir_map() {
    // With the extension on, the discovery-path read is declined (it misses
    // the main worktree's `config.worktree`) and the post-pass re-reads from
    // `git_common_dir`. The preload must land, and it must be the merged
    // common-dir map — `core.bare = true` from `.git/config.worktree` is the
    // #2779 poison pin: the declined linked-worktree read would say `false`.
    let (_tmp, _project, _main, linked) = build_worktree_config_bare_layout();

    super::Repository::prewarm_at(&linked);
    let entry = super::GIT_CONFIG_PRELOAD
        .get(&linked)
        .expect("prewarm must preload the common-dir map under extensions.worktreeConfig");
    assert_eq!(
        entry.value().get("core.bare").and_then(|v| v.last()),
        Some(&"true".to_string()),
        "the preload must hold the merged common-dir map, not the linked worktree's partial one"
    );
}

#[test]
fn prewarm_still_caches_preload_when_worktree_config_disabled() {
    // The #2779 decline is targeted: normal repos (no worktreeConfig
    // extension) get the preload from the discovery-path read alone.
    use super::canonicalize;
    use crate::shell_exec::Cmd;

    let tmp = tempfile::tempdir().unwrap();
    let root = canonicalize(tmp.path()).unwrap().join("normal");
    std::fs::create_dir_all(&root).unwrap();

    let out = crate::testing::configure_git_env(Cmd::new("git"))
        .args(["init", "-b", "main", root.to_str().unwrap()])
        .run()
        .unwrap();
    assert!(out.status.success(), "git init failed");

    super::Repository::prewarm_at(&root);
    assert!(
        super::GIT_CONFIG_PRELOAD.get(&root).is_some(),
        "prewarm should preload normal repos (no extensions.worktreeConfig)"
    );
}

#[test]
fn prewarm_after_early_repository_still_preloads_config() {
    // A `Repository` constructed before prewarm — the `-vv` log-file sinks
    // in `log_files::init` do this during `logging::init` — populates
    // `GIT_COMMON_DIR_CACHE` on its own. Prewarm used to fast-path on that
    // one key and silently skip the config preloads, so every `-vv` run
    // lost them and its trace overstated the `git config --list -z` forks
    // of a normal run. Each prewarm thread now gates on the cache it
    // populates: the config preload must land even when the common dir is
    // already resolved.
    use super::canonicalize;
    use crate::shell_exec::Cmd;

    let tmp = tempfile::tempdir().unwrap();
    let root = canonicalize(tmp.path()).unwrap().join("early");
    std::fs::create_dir_all(&root).unwrap();

    let out = crate::testing::configure_git_env(Cmd::new("git"))
        .args(["init", "-b", "main", root.to_str().unwrap()])
        .run()
        .unwrap();
    assert!(out.status.success(), "git init failed");

    super::Repository::at(&root).unwrap();
    assert!(
        super::GIT_COMMON_DIR_CACHE.contains_key(&root),
        "Repository::at should have resolved the common dir"
    );

    super::Repository::prewarm_at(&root);
    assert!(
        super::GIT_CONFIG_PRELOAD.get(&root).is_some(),
        "prewarm must still preload git config when GIT_COMMON_DIR_CACHE was populated first"
    );
}

#[test]
fn repository_instances_share_mutation_coordination() {
    use crate::git::Repository;
    use crate::testing::TestRepo;

    let mut test = TestRepo::with_initial_commit();
    let linked = test.add_worktree("registry-lock-linked");
    let first = Repository::at(test.root_path()).unwrap();
    let second = Repository::at(linked).unwrap();
    test.run_git(&["branch", "safe-deletion"]);
    let expected = test.git_output(&["rev-parse", "safe-deletion"]);
    assert!(std::ptr::eq(
        first.branch_deletions(),
        second.branch_deletions()
    ));

    {
        let _write = first.worktree_registry_write();
        assert!(
            second.locks.worktree_registry.try_read().is_err(),
            "fresh repository handles for one common directory must share the registry lock"
        );
        assert!(
            second
                .branch_deletions()
                .delete(&second, "refs/heads/safe-deletion", &expected)
                .unwrap(),
            "registry coordination must not block ref mutation"
        );
    }
    assert!(
        second.locks.worktree_registry.try_read().is_ok(),
        "registry remains readable after branch deletion"
    );
}

/// A worktree answers to its branch and to its own path, and the branch wins.
///
/// Both spellings reaching the same worktree is the point of routing every
/// worktree argument through one canonicalizer; branch-first is what keeps a
/// directory from shadowing a branch that shares its name.
#[test]
fn resolve_worktree_accepts_branch_and_path() {
    use crate::git::ResolvedWorktree;
    use crate::testing::TestRepo;
    use dunce::canonicalize;

    let mut test = TestRepo::with_initial_commit();
    let worktree_path = test.add_worktree("feature");

    // A relative path resolves against `-C`, which only a spawned `wt` has —
    // `switch::switch_by_relative_worktree_path` covers that spelling.
    let mut ids = Vec::new();
    for selector in ["feature", worktree_path.to_str().unwrap()] {
        let resolved = test.repo.resolve_worktree(selector).unwrap();
        ids.push(
            resolved
                .id()
                .expect("resolved Git item has an identity")
                .clone(),
        );
        let ResolvedWorktree::Worktree { path, branch, .. } = resolved else {
            panic!("{selector} should resolve to a worktree");
        };
        assert_eq!(canonicalize(&path).unwrap(), worktree_path);
        assert_eq!(branch.as_deref(), Some("feature"));
    }
    assert_eq!(ids[0], ids[1]);
}

/// A directory whose name matches a branch does not shadow it: `wt switch docs`
/// means the branch even when `docs/` is also a worktree.
#[test]
fn resolve_worktree_prefers_branch_over_same_named_directory() {
    use crate::git::ResolvedWorktree;
    use crate::testing::TestRepo;
    use dunce::canonicalize;

    let mut test = TestRepo::with_initial_commit();
    let branch_worktree = test.add_worktree("docs");
    // A second worktree literally at `<repo>/docs`, on a different branch.
    let nested = test.root_path().join("docs");
    test.add_worktree_at_path("docs-nested", &nested);

    let ResolvedWorktree::Worktree { path, branch, .. } =
        test.repo.resolve_worktree("docs").unwrap()
    else {
        panic!("docs should resolve to a worktree");
    };
    assert_eq!(branch.as_deref(), Some("docs"));
    assert_eq!(canonicalize(&path).unwrap(), branch_worktree);
}

/// A detached worktree has no branch to be named by, so its path is the only
/// selector that reaches it.
#[test]
fn resolve_worktree_reaches_detached_worktree_by_path() {
    use crate::git::ResolvedWorktree;
    use crate::testing::TestRepo;
    use dunce::canonicalize;

    let mut test = TestRepo::with_initial_commit();
    let worktree_path = test.add_worktree("feature");
    test.detach_head_in_worktree("feature");

    let ResolvedWorktree::Worktree { path, branch, .. } = test
        .repo
        .resolve_worktree(worktree_path.to_str().unwrap())
        .unwrap()
    else {
        panic!("a detached worktree should resolve by path");
    };
    assert_eq!(canonicalize(&path).unwrap(), worktree_path);
    assert_eq!(branch, None);

    // And it is the one case `require_selected_branch` refuses.
    let err = test
        .repo
        .require_selected_branch(worktree_path.to_str().unwrap(), "promote")
        .unwrap_err();
    assert!(
        err.to_string().contains("detached"),
        "expected a detached-HEAD error, got: {err}"
    );
}

/// `-` and `^` expand to a branch, so the literal token never reaches the path
/// lookup — a directory named `-` cannot hijack `wt switch -`.
#[test]
fn resolve_worktree_does_not_treat_shortcuts_as_paths() {
    use crate::git::ResolvedWorktree;
    use crate::testing::TestRepo;

    let mut test = TestRepo::with_initial_commit();
    let default_branch = test.repo.default_branch().unwrap();
    // A worktree literally at `<repo>/^`, which `^` must not resolve to.
    let decoy = test.root_path().join("^");
    test.add_worktree_at_path("decoy", &decoy);

    let resolved = test.repo.resolve_worktree("^").unwrap();
    let branch = match resolved {
        ResolvedWorktree::Worktree { branch, .. } => branch,
        ResolvedWorktree::BranchOnly { branch, .. } => Some(branch),
        ResolvedWorktree::NoWorktreeAtPath { path } => {
            panic!("`^` resolved to a directory: {}", path.display())
        }
    };
    assert_eq!(branch.as_deref(), Some(default_branch.as_str()));
}

/// A name matching neither a branch nor a worktree path is branch-only, which
/// is what lets `wt remove` still delete a worktree-less branch.
#[test]
fn resolve_worktree_falls_through_to_branch_only() {
    use crate::git::ResolvedWorktree;
    use crate::testing::TestRepo;

    let test = TestRepo::with_initial_commit();

    let resolved = test.repo.resolve_worktree("nowhere").unwrap();
    let ResolvedWorktree::BranchOnly { branch, .. } = resolved else {
        panic!("an unmatched selector should resolve to branch-only");
    };
    assert_eq!(branch, "nowhere");

    // A selector matching nothing names neither, so the error claims neither —
    // suggesting `wt switch nowhere` to create a worktree would only fail.
    let err = test.repo.require_worktree("nowhere").unwrap_err();
    assert!(
        err.to_string().contains("No branch or worktree named"),
        "expected an unmatched-selector error, got: {err}"
    );
}

/// A directory holding no worktree is reported as the directory it is, rather
/// than as a missing branch — the `wt list --branches` the branch error suggests
/// would never list it.
///
/// Both halves of the test are exercised, since either alone over-claims: a
/// selector git could accept as a branch name keeps the branch error even with a
/// directory beside it, and a selector it could not keeps the branch error when
/// nothing is on disk.
#[test]
fn directory_holding_no_worktree_is_reported_as_a_directory() {
    use crate::git::{ErrorExt, ResolvedWorktree};
    use crate::testing::TestRepo;

    let test = TestRepo::with_initial_commit();
    let ghost = test.root_path().join("ghost");
    std::fs::create_dir(&ghost).unwrap();

    // Resolution reaches the verdict itself, so no reporting site has to
    // re-derive what the selector was reaching for.
    let selector = ghost.to_str().unwrap();
    let resolved = test.repo.resolve_worktree(selector).unwrap();
    assert!(
        matches!(resolved, ResolvedWorktree::NoWorktreeAtPath { .. }),
        "a directory holding no worktree is its own verdict, got: {resolved:?}"
    );

    let rendered = test
        .repo
        .require_worktree(selector)
        .unwrap_err()
        .render_diagnostic()
        .unwrap();
    assert!(
        rendered.contains("No worktree @") && rendered.contains("is not a worktree"),
        "expected a directory-holds-no-worktree error, got: {rendered}"
    );

    // A branch is what a state key names, so a directory is refused rather than
    // silently stored as one.
    let err = test
        .repo
        .require_selected_branch(selector, "set marker")
        .unwrap_err();
    assert!(
        err.to_string().contains("No worktree @"),
        "expected a directory selector to be refused as a branch, got: {err}"
    );

    // Nothing on disk: wt cannot tell a mistyped path from a mistyped branch,
    // so the error claims neither.
    let err = test
        .repo
        .require_worktree(ghost.join("deeper").to_str().unwrap())
        .unwrap_err();
    assert!(
        err.to_string().contains("No branch or worktree named"),
        "expected an unmatched-selector error, got: {err}"
    );

    // A name git would accept as a branch is a branch, whatever sits beside it:
    // `wt remove docs` means the branch even when `docs/` is right there.
    std::fs::create_dir(test.root_path().join("docs")).unwrap();
    let err = test.repo.require_worktree("docs").unwrap_err();
    assert!(
        err.to_string().contains("No branch or worktree named"),
        "a directory must not shadow a branch name, got: {err}"
    );

    // Somebody's checkout, which `list_worktrees` cannot see because it belongs
    // to another repository. worktrunk gathers every repo and worktree into one
    // parent, so a sibling is one `../` from any command, and calling a live
    // one "not a worktree" reads as an invitation to delete it.
    let parent = test.root_path().parent().unwrap();
    let sibling = parent.join("sibling");
    std::fs::create_dir_all(sibling.join(".git")).unwrap();

    // And a bare repository, which is the git directory rather than holding
    // one — worktrunk's bare layout sits it among the worktrees it serves, so
    // it is as reachable as they are, and it holds every object.
    let bare = parent.join("project.bare");
    std::fs::create_dir_all(bare.join("objects")).unwrap();
    std::fs::create_dir_all(bare.join("refs")).unwrap();
    std::fs::write(bare.join("HEAD"), "ref: refs/heads/main\n").unwrap();

    for checkout in [&sibling, &bare] {
        assert!(
            test.repo
                .path_selector_directory(checkout.to_str().unwrap())
                .is_none(),
            "a directory holding git data must not be called a leftover: {}",
            checkout.display()
        );
    }

    // A lone `HEAD` is not git data — the skeleton keeps its report.
    let decoy = test.root_path().join("decoy");
    std::fs::create_dir(&decoy).unwrap();
    std::fs::write(decoy.join("HEAD"), "").unwrap();
    assert!(
        test.repo
            .path_selector_directory(decoy.to_str().unwrap())
            .is_some(),
        "a directory that merely contains a HEAD file is still a leftover"
    );
}

/// A `.git` nobody can follow still marks somebody's checkout.
///
/// `Path::exists` answers `false` for every error alike and resolves symlinks,
/// so both shapes here would read as "no git data" — and both are ways the
/// bystander of a repo-wide prune presents: a `.git` symlink is how a git
/// directory is put on another filesystem, which is the volume that gets
/// unmounted.
#[test]
fn an_unresolvable_git_entry_still_counts_as_git_data() {
    use crate::testing::TestRepo;

    let test = TestRepo::with_initial_commit();
    let parent = test.root_path().parent().unwrap();

    let dangling_file = parent.join("bystander-file");
    std::fs::create_dir_all(&dangling_file).unwrap();
    std::fs::write(dangling_file.join(".git"), "gitdir: /gone/worktrees/x\n").unwrap();

    let checkouts = [dangling_file];

    #[cfg(unix)]
    let checkouts = {
        let dangling_link = parent.join("bystander-link");
        std::fs::create_dir_all(&dangling_link).unwrap();
        std::os::unix::fs::symlink("/gone/worktrees/x", dangling_link.join(".git")).unwrap();
        [checkouts[0].clone(), dangling_link]
    };

    for checkout in checkouts {
        assert!(
            test.repo
                .path_selector_directory(checkout.to_str().unwrap())
                .is_none(),
            "an unresolvable .git must still withhold the claim: {}",
            checkout.display()
        );
    }
}

/// A detached worktree is reachable by its path and by nothing else, so a merge
/// target naming one is reported as detached rather than as absent — the claim
/// `wt list` would contradict.
#[test]
fn detached_worktree_target_is_reported_as_detached() {
    use crate::git::ErrorExt;
    use crate::testing::TestRepo;

    let mut test = TestRepo::with_initial_commit();
    let worktree_path = test.add_worktree("feature");
    test.detach_head_in_worktree("feature");
    let selector = worktree_path.to_str().unwrap();

    let err = test.repo.require_target_branch(Some(selector)).unwrap_err();
    let rendered = err.to_string();
    assert!(
        rendered.contains("detached") && !rendered.contains("No worktree @"),
        "expected a detached-target error, got: {rendered}"
    );

    // The detached worktree is the one the argument named, so the hint has to
    // send the user there: `git switch` run where they are standing would
    // switch the wrong tree.
    let hint = err.render_diagnostic().expect("typed diagnostic");
    let quoted = crate::path::format_path_for_display(&worktree_path);
    assert!(
        hint.contains(&format!("git -C {quoted} switch")),
        "expected the hint to name the target worktree, got: {hint}"
    );

    // A commit-ish target is spelled in a wider vocabulary than a worktree
    // selector, so it claims nothing about paths either way.
    if let Err(err) = test.repo.require_target_ref(Some(selector)) {
        assert!(
            !err.to_string().contains("No worktree @"),
            "a registered worktree must never be reported as absent, got: {err}"
        );
    }
}

/// The directory claim checks for itself that nothing is registered at the path,
/// rather than trusting the caller to have looked.
///
/// A shortcut is the case that breaks the trust: `resolve_worktree` skips its
/// path lookup whenever one rewrote the argument, so `-` expanding to a
/// worktree's own path arrives having matched nothing at all.
#[test]
fn path_selector_error_checks_for_a_registered_worktree() {
    use crate::testing::TestRepo;

    let mut test = TestRepo::with_initial_commit();
    let worktree_path = test.add_worktree("feature");
    let selector = worktree_path.to_str().unwrap();

    assert!(
        test.repo.path_selector_directory(selector).is_none(),
        "a registered worktree's own path must never be reported as holding none"
    );

    // Reached through the shortcut, which is the route with no prior lookup.
    test.repo.set_switch_previous(Some(selector)).unwrap();
    let err = test.repo.require_worktree("-").unwrap_err();
    assert!(
        !err.to_string().contains("No worktree @"),
        "an expanded shortcut naming a live worktree must not report it as absent, got: {err}"
    );
}

/// A merge target naming a directory that holds no worktree gets the same
/// answer as every other worktree argument, rather than an offer to create a
/// branch git would reject.
#[test]
fn directory_merge_target_is_reported_as_a_directory() {
    use crate::git::ErrorExt;
    use crate::testing::TestRepo;

    let test = TestRepo::with_initial_commit();
    let ghost = test.root_path().join("ghost");
    std::fs::create_dir(&ghost).unwrap();

    let rendered = test
        .repo
        .require_target_branch(Some(ghost.to_str().unwrap()))
        .unwrap_err()
        .render_diagnostic()
        .unwrap();
    assert!(
        rendered.contains("No worktree @") && !rendered.contains("--create"),
        "expected a directory-holds-no-worktree error, got: {rendered}"
    );

    // Revision syntax is no more a branch name than a path is, and naming no
    // directory is what keeps it out of the path claim.
    for revspec in ["HEAD@{1}", "main^", "v1.0..v2.0", "pr:5"] {
        let err = test
            .repo
            .require_target_branch(Some(revspec))
            .expect_err(revspec);
        assert!(
            !err.to_string().contains("No worktree @"),
            "{revspec} must not be reported as a path, got: {err}"
        );
    }
}

/// A mistyped path names no directory, so it keeps the branch error — but the
/// offer to create a branch goes, because git would reject the name whether or
/// not anything sits at the path.
#[test]
fn a_path_spelling_is_never_offered_as_a_branch_to_create() {
    use crate::git::ErrorExt;
    use crate::testing::TestRepo;

    let test = TestRepo::with_initial_commit();

    for selector in ["../repo.mistyped", "./nowhere", "/abs/nowhere"] {
        let rendered = test
            .repo
            .require_target_branch(Some(selector))
            .unwrap_err()
            .render_diagnostic()
            .unwrap();
        assert!(
            !rendered.contains("--create"),
            "{selector} must not be offered to --create, got: {rendered}"
        );
    }

    // A name git would take keeps the offer.
    let rendered = test
        .repo
        .require_target_branch(Some("new-branch"))
        .unwrap_err()
        .render_diagnostic()
        .unwrap();
    assert!(
        rendered.contains("--create"),
        "a usable branch name keeps the create hint, got: {rendered}"
    );
}

/// The branch-name rules are git's, so git decides them.
#[test]
fn branch_name_matches_git_check_ref_format() {
    use super::is_valid_branch_name;
    use crate::shell_exec::Cmd;

    let names = [
        // Ordinary branch names, including the ones that look like paths.
        "main",
        "feature/auth",
        "release/1.2/rc",
        "HEAD",
        "-x",
        "@",
        "nowhere",
        // The `.lock` rule is case-sensitive and non-ASCII is fine — both are
        // easy to over-reject when re-deriving the rules from prose.
        "a.LOCK",
        "café",
        // Path spellings.
        "/abs/path/ghost",
        "./ghost",
        "../ghost",
        "~/code/myproject",
        "ghost/",
        ".claude/worktrees/ghost",
        "C:\\code\\myproject",
        // The remaining ref-format rules, one name each.
        "",
        "a//b",
        "a..b",
        "a.lock",
        "a.lock/b",
        "a.",
        "a@{b",
        "a b",
        "a\tb",
        "a\u{7f}b",
        "a~b",
        "a^b",
        "a:b",
        "a?b",
        "a*b",
        "a[b",
    ];

    for name in names {
        let out = crate::testing::configure_git_env(Cmd::new("git"))
            .args(["check-ref-format", &format!("refs/heads/{name}")])
            .run()
            .unwrap();
        assert_eq!(
            is_valid_branch_name(name),
            out.status.success(),
            "disagreed with git check-ref-format on {name:?}"
        );
    }
}

/// A branch that exists without a checkout is the one case where offering to
/// create a worktree is right, so it keeps the distinct message and hint.
#[test]
fn require_worktree_distinguishes_a_branch_with_no_checkout() {
    use crate::testing::TestRepo;

    let test = TestRepo::with_initial_commit();
    test.run_git(&["branch", "worktreeless"]);

    let err = test.repo.require_worktree("worktreeless").unwrap_err();
    let rendered = format!("{err}");
    assert!(
        rendered.contains("has no worktree"),
        "expected the branch-without-worktree error, got: {rendered}"
    );
}

#[test]
fn test_worktree_paths_for_branch_detects_duplicates() {
    use super::worktrees::{duplicated_branches, worktree_paths_for_branch};

    // Two worktrees on `feature` — the state `git worktree add --force`
    // produces. Porcelain retains every entry; only resolution collapses it.
    // The detached worktree has no branch to duplicate.
    let output = "worktree /path/to/main
HEAD abcd1234
branch refs/heads/main

worktree /path/to/feature
HEAD efgh5678
branch refs/heads/feature

worktree /path/to/feature-dup
HEAD efgh5678
branch refs/heads/feature

worktree /path/to/detached
HEAD efgh5678
detached

";
    let worktrees = WorktreeInfo::parse_porcelain_list(output).unwrap();

    // The duplicated branch yields both paths, in git's listing order — the
    // first is what resolution uses, the rest are what the warning surfaces.
    assert_eq!(
        worktree_paths_for_branch(&worktrees, "feature"),
        vec![
            PathBuf::from("/path/to/feature"),
            PathBuf::from("/path/to/feature-dup"),
        ]
    );
    // A branch with a single worktree is unambiguous (len 1, no warning).
    assert_eq!(
        worktree_paths_for_branch(&worktrees, "main"),
        vec![PathBuf::from("/path/to/main")]
    );
    // A branch with no worktree yields nothing.
    assert!(worktree_paths_for_branch(&worktrees, "absent").is_empty());

    // The set form `wt list` uses to flag rows names only the branch that
    // repeats — a single-worktree branch and a detached head can't duplicate.
    assert_eq!(
        duplicated_branches(&worktrees),
        std::collections::HashSet::from(["feature"])
    );
}

#[test]
fn test_worktree_for_branch_dedups_duplicate_warning() {
    // `git worktree add --force` puts one branch in two worktrees. Resolving it
    // twice in a single process must warn only once — this drives both sides of
    // the once-per-branch dedup guard (emit on the first, skip on the second)
    // while confirming resolution is stable.
    use super::Repository;
    use super::canonicalize;
    use crate::shell_exec::Cmd;
    use std::path::Path;

    let tmp = tempfile::tempdir().unwrap();
    let root = canonicalize(tmp.path()).unwrap().join("repo");
    std::fs::create_dir_all(&root).unwrap();
    let git = |args: &[&str], dir: &Path| {
        let out = crate::testing::configure_git_env(Cmd::new("git"))
            .args(args.iter().copied())
            .current_dir(dir)
            .run()
            .unwrap();
        assert!(out.status.success(), "git {args:?} failed");
    };

    git(&["init", "-b", "main", "."], &root);
    std::fs::write(root.join("f"), "x").unwrap();
    git(&["add", "."], &root);
    git(&["commit", "-m", "init"], &root);
    git(&["branch", "dup-feature"], &root);

    let wt1 = tmp.path().join("wt1");
    let wt2 = tmp.path().join("wt2");
    git(
        &["worktree", "add", wt1.to_str().unwrap(), "dup-feature"],
        &root,
    );
    git(
        &[
            "worktree",
            "add",
            "--force",
            wt2.to_str().unwrap(),
            "dup-feature",
        ],
        &root,
    );

    let repo = Repository::at(&root).unwrap();
    let first = repo.worktree_for_branch("dup-feature").unwrap();
    let second = repo.worktree_for_branch("dup-feature").unwrap();
    assert!(first.is_some(), "an ambiguous branch still resolves");
    assert_eq!(first, second, "resolution is stable across the dedup guard");
}

/// A trailing path separator is not part of any branch name git will accept, so
/// stripping it lets `docs/` — what shell completion produces beside a `docs`
/// directory — find the branch. A selector made only of separators is left
/// alone: `/` is the root directory and the empty string names nothing.
#[test]
fn normalize_selector_strips_only_trailing_separators() {
    use crate::git::normalize_selector;

    assert_eq!(normalize_selector("docs/"), "docs");
    assert_eq!(normalize_selector("../repo.feature/"), "../repo.feature");
    assert_eq!(normalize_selector("docs"), "docs");
    assert_eq!(normalize_selector("feat/sub"), "feat/sub");
    assert_eq!(normalize_selector("/"), "/");
    assert_eq!(normalize_selector(""), "");
}

/// A worktree git reports as prunable is one nothing can run in, so the lookup
/// every command shares refuses it rather than handing back a path that fails
/// at `git rev-parse` a few calls later.
///
/// The directory is recreated rather than left absent, because that is the
/// state the `Path::exists()` probes this replaced could not see.
#[test]
fn usable_worktree_for_branch_refuses_a_prunable_registration() {
    use crate::git::Repository;
    use crate::testing::TestRepo;

    let mut test = TestRepo::with_initial_commit();
    let worktree_path = test.add_worktree("feature");

    assert_eq!(
        test.repo.usable_worktree_for_branch("feature").unwrap(),
        Some(worktree_path.clone()),
        "a healthy worktree resolves"
    );

    std::fs::remove_dir_all(&worktree_path).unwrap();
    std::fs::create_dir_all(&worktree_path).unwrap();
    let repo = Repository::at(test.root_path()).unwrap();

    let err = repo.usable_worktree_for_branch("feature").unwrap_err();
    assert!(
        err.to_string().contains("is stale"),
        "a prunable registration must be refused, got: {err}"
    );
    assert_eq!(
        repo.usable_worktree_for_branch("no-such-branch").unwrap(),
        None,
        "a branch with no worktree is a normal answer, not an error"
    );
}

/// A selector reports whether anything rewrote it, rather than that being
/// inferred by comparing an expansion's output against its input.
///
/// The two answers differ, which is the whole reason to carry the fact: an
/// expansion can legitimately return the token it was given — `-` pointing at
/// the branch you are already on — and string equality then reads a rewrite as
/// a literal, turning the path arm back on for a token nobody typed.
/// Normalization breaks it in the other direction: `docs/` and `docs` are one
/// selector, and comparing them would read a rewrite that never happened.
#[test]
fn selector_reports_rewriting_rather_than_inferring_it() {
    use crate::testing::TestRepo;

    let test = TestRepo::with_initial_commit();
    let repo = &test.repo;
    let current = repo.current_worktree().branch().unwrap().unwrap();

    let literal = repo.expand_selector("some-branch").unwrap();
    assert_eq!(literal.token(), "some-branch");
    assert!(literal.names_a_path(), "an untouched token may be a path");

    // Normalization is not rewriting: `docs/` still gets its path arm, which
    // is what keeps `../repo.feature/` resolving as a worktree path.
    let normalized = repo.expand_selector("docs/").unwrap();
    assert_eq!(normalized.token(), "docs");
    assert!(
        normalized.names_a_path(),
        "stripping a separator must not read as a rewrite"
    );

    // `^` expands to the default branch, which here IS the current branch — so
    // the expansion's output equals neither its input nor nothing useful. The
    // flag, not a comparison, is what records that a shortcut fired.
    let shortcut = repo.expand_selector("^").unwrap();
    assert_eq!(shortcut.token(), current);
    assert!(
        !shortcut.names_a_path(),
        "a shortcut's expansion is nobody's path"
    );

    // The degenerate case string equality gets wrong: history pointing at the
    // branch already checked out. Output == input, yet a shortcut fired.
    repo.set_switch_previous(Some(&current)).unwrap();
    let previous = repo.expand_selector("-").unwrap();
    assert_eq!(previous.token(), current);
    assert!(
        !previous.names_a_path(),
        "a shortcut that expands to its own input is still a rewrite"
    );
}

/// "Nothing can run here" is the union of two independent failures, because
/// neither implies the other.
///
/// git withholds the `prunable` attribute from a **locked** worktree even when
/// its directory is gone — prunability is git's pruning *policy*, and a lock
/// means "don't prune this". So a `prunable`-only test reads a locked worktree
/// on an unmounted volume as healthy, which is the state
/// `prepare_worktree_removal`'s lock guard exists for. The recreated directory
/// is the converse: present, so an existence probe passes, and holding nothing.
#[test]
fn worktree_is_unusable_covers_locked_absent_and_recreated() {
    use crate::git::Repository;
    use crate::testing::TestRepo;

    let mut test = TestRepo::with_initial_commit();
    let healthy = test.add_worktree("healthy");
    let absent = test.add_worktree("absent");
    let locked = test.add_worktree("locked-absent");
    let recreated = test.add_worktree("recreated");

    test.lock_worktree("locked-absent", Some("removable media"));
    std::fs::remove_dir_all(&absent).unwrap();
    std::fs::remove_dir_all(&locked).unwrap();
    std::fs::remove_dir_all(&recreated).unwrap();
    std::fs::create_dir_all(&recreated).unwrap();

    let repo = Repository::at(test.root_path()).unwrap();

    assert!(!repo.worktree_is_unusable(&healthy).unwrap());
    assert!(
        repo.worktree_is_unusable(&absent).unwrap(),
        "an absent directory is unusable"
    );
    assert!(
        repo.worktree_is_unusable(&locked).unwrap(),
        "a locked worktree whose directory is gone carries no `prunable` line, \
         so only the existence half catches it"
    );
    assert!(
        repo.worktree_is_unusable(&recreated).unwrap(),
        "a recreated directory exists, so only the `prunable` half catches it"
    );
}

/// Unregistering a stale worktree repeats git's own prune test at deletion
/// time, since the caller's `prunable` came from an earlier listing: an entry
/// locked or reconnected since is refused and keeps its registration. A stale
/// entry goes whatever is left at its path — nothing, a directory, or a file —
/// and a directory that remains keeps its files.
#[test]
fn prune_worktree_entry_repeats_git_prune_test() {
    use crate::git::Repository;
    use crate::testing::TestRepo;

    let mut test = TestRepo::with_initial_commit();
    let live = test.add_worktree("live");
    let locked = test.add_worktree("locked-absent");
    let absent = test.add_worktree("absent");
    let dotgit_gone = test.add_worktree("dotgit-gone");
    let now_a_file = test.add_worktree("now-a-file");

    test.lock_worktree("locked-absent", Some("removable media"));
    std::fs::remove_dir_all(&locked).unwrap();
    std::fs::remove_dir_all(&absent).unwrap();
    std::fs::remove_file(dotgit_gone.join(".git")).unwrap();
    std::fs::write(dotgit_gone.join("leftover.txt"), "kept").unwrap();
    // `now-a-file/.git` fails with `NotADirectory`, which git also counts as
    // nothing there.
    std::fs::remove_dir_all(&now_a_file).unwrap();
    std::fs::write(&now_a_file, "not a directory").unwrap();

    let repo = Repository::at(test.root_path()).unwrap();
    let registered = || {
        test.git_output(&["worktree", "list", "--porcelain"])
            .lines()
            .filter_map(|line| line.strip_prefix("worktree "))
            .filter_map(|path| PathBuf::from(path).file_name().map(|n| n.to_owned()))
            .collect::<Vec<_>>()
    };

    for mode in [
        crate::git::WorktreePruneMode::Stale,
        crate::git::WorktreePruneMode::RemovedLive,
        crate::git::WorktreePruneMode::Force,
        crate::git::WorktreePruneMode::RemovedLiveForce,
    ] {
        let err = repo.prune_worktree_entry(&live, mode).unwrap_err();
        assert!(err.to_string().contains("no longer stale"), "got: {err}");
        let err = repo.prune_worktree_entry(&locked, mode).unwrap_err();
        assert!(err.to_string().contains("is locked"), "got: {err}");
    }

    repo.prune_worktree_entry(&absent, crate::git::WorktreePruneMode::stale(false))
        .unwrap();
    repo.prune_worktree_entry(&dotgit_gone, crate::git::WorktreePruneMode::stale(false))
        .unwrap();
    repo.prune_worktree_entry(&now_a_file, crate::git::WorktreePruneMode::stale(false))
        .unwrap();
    let names = registered();
    for (path, kept) in [
        (&live, true),
        (&locked, true),
        (&absent, false),
        (&dotgit_gone, false),
        (&now_a_file, false),
    ] {
        assert_eq!(
            names.iter().any(|name| name == path.file_name().unwrap()),
            kept,
            "{} registration; worktrees: {names:?}",
            path.display()
        );
    }
    assert!(dotgit_gone.join("leftover.txt").is_file());

    let err = repo
        .prune_worktree_entry(&absent, crate::git::WorktreePruneMode::stale(false))
        .unwrap_err();
    assert!(
        err.to_string().contains("No worktree registered"),
        "got: {err}"
    );
}

/// A `.git` that can't be statted is not taken for an absent one: the entry
/// keeps its registration, and the error names what couldn't be checked.
#[cfg(unix)]
#[test]
fn prune_worktree_entry_keeps_an_entry_it_cannot_check() {
    use std::os::unix::fs::PermissionsExt;

    use crate::git::Repository;
    use crate::testing::TestRepo;

    let test = TestRepo::with_initial_commit();
    let guarded = test.root_path().parent().unwrap().join("guarded");
    std::fs::create_dir(&guarded).unwrap();
    let worktree_path = guarded.join("wt");
    test.run_git(&[
        "worktree",
        "add",
        "--detach",
        worktree_path.to_str().unwrap(),
    ]);
    let repo = Repository::at(test.root_path()).unwrap();

    let set_mode =
        |mode| std::fs::set_permissions(&guarded, std::fs::Permissions::from_mode(mode)).unwrap();
    set_mode(0o000);
    // Skip if running as root: euid 0 ignores DAC mode bits, so the stat
    // would succeed. Probe with the stat the mode should refuse.
    if std::fs::symlink_metadata(worktree_path.join(".git")).is_ok() {
        set_mode(0o755);
        crate::styling::eprintln!("Skipping - running with elevated privileges");
        return;
    }
    let result =
        repo.prune_worktree_entry(&worktree_path, crate::git::WorktreePruneMode::stale(false));
    set_mode(0o755);

    let err = result.unwrap_err();
    assert!(
        format!("{err:#}").contains("Failed to check"),
        "got: {err:#}"
    );
    assert!(
        test.git_output(&["worktree", "list", "--porcelain"])
            .contains("guarded/wt"),
        "the entry should stay registered"
    );
}

/// A stale registration is asked what unregistering it would destroy: its
/// index against `HEAD`, and git's in-progress state files. A registration
/// with no index has nothing staged, and on an unborn branch the index is
/// read against the empty tree, so a staged file counts and an emptied index
/// doesn't.
#[test]
fn stale_worktree_work_reads_the_registration() {
    use crate::git::{InProgressOperation, Repository, StaleWorktreeWork};
    use crate::testing::TestRepo;

    let mut test = TestRepo::with_initial_commit();
    let clean = test.add_worktree("clean");
    let staged = test.add_worktree("staged");
    std::fs::write(staged.join("new.txt"), "work").unwrap();
    test.run_git_in(&staged, &["add", "new.txt"]);
    let bisecting = test.add_worktree("bisecting");
    test.run_git_in(&bisecting, &["bisect", "start"]);
    let parent = test.root_path().parent().unwrap().to_path_buf();
    let no_checkout = parent.join("repo.no-checkout");
    test.run_git(&[
        "worktree",
        "add",
        "--no-checkout",
        "--detach",
        no_checkout.to_str().unwrap(),
    ]);
    let unborn = parent.join("repo.unborn");
    test.run_git(&[
        "worktree",
        "add",
        "--orphan",
        "-b",
        "fresh",
        unborn.to_str().unwrap(),
    ]);
    std::fs::write(unborn.join("first.txt"), "work").unwrap();
    test.run_git_in(&unborn, &["add", "first.txt"]);
    // An unborn branch whose index exists but holds nothing.
    let unborn_empty = parent.join("repo.unborn-empty");
    test.run_git(&[
        "worktree",
        "add",
        "--orphan",
        "-b",
        "fresh-empty",
        unborn_empty.to_str().unwrap(),
    ]);
    std::fs::write(unborn_empty.join("first.txt"), "work").unwrap();
    test.run_git_in(&unborn_empty, &["add", "first.txt"]);
    test.run_git_in(&unborn_empty, &["rm", "--cached", "-q", "first.txt"]);
    for path in [
        &clean,
        &staged,
        &bisecting,
        &no_checkout,
        &unborn,
        &unborn_empty,
    ] {
        std::fs::remove_dir_all(path).unwrap();
    }

    let repo = Repository::at(test.root_path()).unwrap();
    assert_eq!(repo.stale_worktree_work(&clean).unwrap(), None);
    assert_eq!(
        repo.stale_worktree_work(&staged).unwrap(),
        Some(StaleWorktreeWork::StagedChanges)
    );
    assert_eq!(
        repo.stale_worktree_work(&bisecting).unwrap(),
        Some(StaleWorktreeWork::Operation(InProgressOperation::Bisect))
    );
    assert_eq!(repo.stale_worktree_work(&no_checkout).unwrap(), None);
    assert_eq!(
        repo.stale_worktree_work(&unborn).unwrap(),
        Some(StaleWorktreeWork::StagedChanges)
    );
    assert_eq!(repo.stale_worktree_work(&unborn_empty).unwrap(), None);
}

/// The deletion waits for in-process registry readers: `git worktree list`
/// reads every entry's files, so one overlapping the deletion could read the
/// entry half-deleted and fail. A held read guard keeps the entry intact; its
/// release lets a clean prune through, but retains work added while waiting.
#[test]
fn prune_worktree_entry_waits_for_registry_readers() {
    use std::time::{Duration, Instant};

    use crate::git::Repository;
    use crate::testing::TestRepo;

    for (new_work, detached) in [(false, false), (true, false), (true, true)] {
        let mut test = TestRepo::with_initial_commit();
        let worktree_path = test.add_worktree("feature");
        if detached {
            test.detach_head_in_worktree("feature");
        }
        std::fs::remove_dir_all(&worktree_path).unwrap();
        let registration = test
            .root_path()
            .join(".git/worktrees")
            .join(worktree_path.file_name().unwrap());
        assert!(registration.is_dir());
        let repo = Repository::at(test.root_path()).unwrap();
        let worker_repo = repo.clone();
        let worker_path = worktree_path.clone();

        let reader = repo.worktree_registry_read();
        let worker = std::thread::spawn(move || {
            worker_repo
                .prune_worktree_entry(&worker_path, crate::git::WorktreePruneMode::stale(false))
        });
        let deadline = Instant::now() + Duration::from_millis(200);
        while registration.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let survived_reader = registration.exists();
        if new_work {
            // The writer has not passed the registry lock yet. New operation state
            // must therefore be checked after the lock is acquired, not before it.
            std::fs::write(
                registration.join("MERGE_HEAD"),
                test.git_output(&["rev-parse", "HEAD"]).trim(),
            )
            .unwrap();
        }
        drop(reader);
        let result = worker.join().expect("prune thread should not panic");

        assert!(
            survived_reader,
            "the prune ran under a held registry read guard"
        );
        if new_work {
            let error = result.unwrap_err();
            let Some(crate::git::GitError::StaleWorktreeHoldsWork { branch, .. }) =
                error.downcast_ref::<crate::git::GitError>()
            else {
                panic!("unexpected error: {error:#}");
            };
            assert_eq!(branch.as_deref(), (!detached).then_some("feature"));
            assert!(registration.join("MERGE_HEAD").is_file());
            repo.prune_worktree_entry(&worktree_path, crate::git::WorktreePruneMode::stale(true))
                .unwrap();
        } else {
            result.unwrap();
        }
        assert!(!registration.exists());
    }
}

/// The ownership gate accepts a worktree that holds its own registration, in
/// both shapes: a linked worktree, whose `.git` file names a registration that
/// names it back, and the main worktree, which has no registration at all
/// because its git dir *is* the common dir.
///
/// The main-worktree arm is a backstop rather than a path a command reaches —
/// `wt remove` rejects the main worktree well before this gate, and a bare
/// repository's worktrees are all linked — so nothing through the CLI would
/// notice it inverting.
#[test]
fn ensure_holds_this_worktree_accepts_both_worktree_shapes() {
    use crate::git::Repository;
    use crate::testing::TestRepo;

    let mut test = TestRepo::with_initial_commit();
    let linked = test.add_worktree("linked");
    let repo = Repository::at(test.root_path()).unwrap();

    repo.worktree_at(&linked)
        .ensure_holds_this_worktree()
        .expect("a linked worktree holding its own registration is accepted");
    repo.worktree_at(test.root_path())
        .ensure_holds_this_worktree()
        .expect("the main worktree is accepted, having no registration to point back at");
}

/// A directory that has lost its `.git` entry holds no worktree of ours, even
/// where the tree above it answers for this repository.
///
/// The gate reads `<dir>/.git` and stops there, as git's own validation does.
/// `git rev-parse --git-dir` walks up instead, so from a worktree nested inside
/// another it resolves to the enclosing repository's git dir — which *is* the
/// common dir, so a gate resolving that way accepts the directory as the main
/// worktree.
///
/// `wt remove` refuses this state at the upstream `prunable` check, which is why
/// the gate is the only place it can be asked about directly.
#[test]
fn ensure_holds_this_worktree_refuses_a_directory_that_lost_its_git_entry() {
    use crate::git::Repository;
    use crate::testing::TestRepo;

    let mut test = TestRepo::with_initial_commit();
    let nested_path = test.root_path().join("nested");
    let nested = test.add_worktree_at_path("nested", &nested_path);
    std::fs::remove_file(nested.join(".git")).unwrap();

    let repo = Repository::at(test.root_path()).unwrap();
    assert_eq!(
        repo.worktree_at(&nested).git_dir().unwrap(),
        repo.git_common_dir(),
        "premise: walking up from the emptied directory reaches this repository"
    );
    repo.worktree_at(&nested)
        .ensure_holds_this_worktree()
        .expect_err("a directory with no `.git` entry holds no worktree of ours");
}

/// The refusal names where the occupant belongs in a form the user can act on,
/// including when the registration records it relatively.
///
/// A relative `gitdir` entry is resolved against the registration directory, so
/// the recorded path arrives with a `..` chain through `.git/worktrees/<id>`
/// still in it, and the directory it names no longer exists — that is why the
/// gate is refusing. Plain canonicalization can't normalize a path that isn't
/// there, so the hint would otherwise print the traversal verbatim.
///
/// Asserted against `Diagnostic::render`, since the path is in the hint and
/// `Display` carries only the title.
#[test]
fn worktree_path_not_ours_names_a_normalized_path() {
    use crate::git::{Diagnostic, GitError, Repository};
    use crate::testing::TestRepo;

    let mut test = TestRepo::with_initial_commit();
    let occupied = test.add_worktree("occupied");
    let occupant = test.add_worktree("occupant");

    // Record the occupant's own worktree relatively, as git does under
    // `worktree.useRelativePaths`, then move it onto the other's path.
    let registration = PathBuf::from(
        std::fs::read_to_string(occupant.join(".git"))
            .unwrap()
            .trim()
            .strip_prefix("gitdir: ")
            .unwrap(),
    );
    let relative = PathBuf::from("../../../..")
        .join(occupant.file_name().unwrap())
        .join(".git");
    std::fs::write(
        registration.join("gitdir"),
        relative.to_string_lossy().as_ref(),
    )
    .unwrap();
    std::fs::remove_dir_all(&occupied).unwrap();
    std::fs::rename(&occupant, &occupied).unwrap();

    let repo = Repository::at(test.root_path()).unwrap();
    let refusal = repo
        .worktree_at(&occupied)
        .ensure_holds_this_worktree()
        .expect_err("the occupant's registration records a path it has left")
        .downcast_ref::<GitError>()
        .expect("the gate refuses with a GitError")
        .render();

    assert!(
        !refusal.contains(".."),
        "the refusal must name a normalized path:\n{refusal}"
    );
    assert!(
        refusal.contains(&occupant.file_name().unwrap().to_string_lossy().to_string()),
        "the refusal must name where the occupant belongs:\n{refusal}"
    );
}

/// Fresh separate-directory repositories identify their main from discovery,
/// without requiring repair or accepting a linked checkout as the main.
#[test]
fn repo_path_discovers_fresh_separate_git_dir_main() {
    let seed = TestRepo::with_initial_commit();
    for initialization in ["init", "clone"] {
        let directory = crate::testing::test_tempdir();
        let store = directory.path().join("store");
        let main = directory.path().join("main");
        let outer = directory.path().join("outer");
        let args = if initialization == "init" {
            vec![
                "init",
                "-b",
                "main",
                "--separate-git-dir",
                store.to_str().unwrap(),
                main.to_str().unwrap(),
            ]
        } else {
            vec![
                "clone",
                "--separate-git-dir",
                store.to_str().unwrap(),
                seed.root_path().to_str().unwrap(),
                main.to_str().unwrap(),
            ]
        };
        seed.run_git(&args);
        std::fs::write(main.join("main.txt"), "main checkout contents").unwrap();
        seed.run_git_in(&main, &["add", "main.txt"]);
        seed.run_git_in(&main, &["commit", "-m", "main checkout"]);
        seed.run_git_in(
            &main,
            &["worktree", "add", "-b", "outer", outer.to_str().unwrap()],
        );
        let discovery = main.join("subdirectory");
        std::fs::create_dir(&discovery).unwrap();
        assert!(
            !store.join("gitdir").exists(),
            "native init/clone must not have recorded a backlink"
        );
        let repo = Repository::at(&discovery).unwrap();
        assert_eq!(
            repo.repo_path().unwrap(),
            dunce::canonicalize(&main).unwrap()
        );
        assert_eq!(
            repo.list_worktrees().unwrap()[0].path,
            dunce::canonicalize(&main).unwrap()
        );
        assert_eq!(
            repo.primary_worktree().unwrap(),
            Some(dunce::canonicalize(&main).unwrap())
        );
        assert!(!repo.worktree_at(&main).is_linked().unwrap());
        assert_eq!(repo.nested_worktree(&outer).unwrap(), None);
        let linked = Repository::at(&outer).unwrap();
        assert!(
            linked
                .nested_worktree(&outer)
                .unwrap_err()
                .to_string()
                .contains("Cannot determine the main worktree location")
        );
        let staged =
            crate::git::remove::stage_worktree_removal(&repo, &outer, Some("outer"), false)
                .unwrap()
                .expect("clean linked checkout must stage normally");
        assert!(!outer.exists());
        assert!(!repo.git_common_dir().join("worktrees/outer").exists());
        assert_eq!(
            std::fs::read_to_string(staged.join("main.txt")).unwrap(),
            "main checkout contents"
        );
        assert_eq!(
            std::fs::read_to_string(main.join("main.txt")).unwrap(),
            "main checkout contents"
        );
        assert!(main.join(".git").is_file());
        assert!(
            !store.join("gitdir").exists(),
            "discovery must not write a repair backlink"
        );
    }
}

/// Git's one output newline must not consume newline or whitespace path bytes.
#[cfg(unix)]
#[test]
fn repo_path_preserves_fresh_main_path_bytes() {
    use std::os::unix::ffi::OsStringExt;

    let seed = TestRepo::with_initial_commit();
    let names = [
        b"main with space\n".as_slice(),
        b"main with trailing whitespace\t  ".as_slice(),
        #[cfg(target_os = "linux")]
        b"main-\xff  ".as_slice(),
    ];
    for name in names {
        let directory = crate::testing::test_tempdir();
        let initial = directory.path().join("initial");
        let main = directory
            .path()
            .join(std::ffi::OsString::from_vec(name.to_vec()));
        let store = directory.path().join("store");
        seed.run_git(&[
            "clone",
            "--separate-git-dir",
            store.to_str().unwrap(),
            seed.root_path().to_str().unwrap(),
            initial.to_str().unwrap(),
        ]);
        std::fs::rename(&initial, &main).unwrap();
        assert!(!store.join("gitdir").exists());
        let subdirectory = main.join("subdirectory");
        std::fs::create_dir(&subdirectory).unwrap();
        for discovery in [&main, &subdirectory] {
            Repository::prewarm_at(discovery);
            let repo = Repository::at(discovery).unwrap();
            let info = repo.current_worktree().prewarm_info().unwrap();
            assert!(info.is_inside);
            let expected = dunce::canonicalize(&main).unwrap();
            assert_eq!(info.root.as_deref(), Some(expected.as_path()));
            assert_eq!(repo.current_worktree().root().unwrap(), expected);
            assert_eq!(repo.repo_path().unwrap(), expected);
        }
    }
}

/// A partial bare batch must not cache a newline-truncated neighboring store.
/// Exact common-directory discovery also preserves trailing/native path bytes.
#[cfg(unix)]
#[test]
fn prewarm_preserves_native_common_directory_paths() {
    use std::os::unix::ffi::OsStringExt;

    let seed = TestRepo::with_initial_commit();
    let names = [
        b"bare\nstore".as_slice(),
        b"bare\ntrue\nstore\nHEAD".as_slice(),
        b"bare  ".as_slice(),
        #[cfg(target_os = "linux")]
        b"bare-\xff  ".as_slice(),
    ];
    for name in names {
        let directory = crate::testing::test_tempdir();
        let prefix = directory.path().join("bare");
        let initial = directory.path().join("initial");
        for target in [&prefix, &initial] {
            seed.run_git(&[
                "clone",
                "--bare",
                seed.root_path().to_str().unwrap(),
                target.to_str().unwrap(),
            ]);
        }
        let store = directory
            .path()
            .join(std::ffi::OsString::from_vec(name.to_vec()));
        std::fs::rename(&initial, &store).unwrap();
        let child = store.join("subdirectory");
        std::fs::create_dir(&child).unwrap();
        let expected = dunce::canonicalize(&store).unwrap();
        assert_ne!(expected, dunce::canonicalize(&prefix).unwrap());
        for discovery in [&store, &child] {
            Repository::prewarm_at(discovery);
            let repo = Repository::at(discovery).unwrap();
            assert_eq!(repo.git_common_dir(), expected);
            assert!(repo.is_bare().unwrap());
            assert_eq!(repo.repo_path().unwrap(), expected);
            assert_eq!(
                repo.current_worktree().root().unwrap(),
                dunce::canonicalize(discovery).unwrap()
            );
        }
    }
}

/// Locally configured main roots (also used by submodules) obey the same
/// native show-toplevel protocol: trailing whitespace is part of the path.
#[cfg(unix)]
#[test]
fn repo_path_preserves_local_core_worktree_path_bytes() {
    use std::os::unix::ffi::OsStrExt;

    let seed = TestRepo::with_initial_commit();
    let directory = crate::testing::test_tempdir();
    let main = directory.path().join("configured main\n\t  ");
    let store = directory.path().join("store");
    seed.run_git(&[
        "clone",
        "--separate-git-dir",
        store.to_str().unwrap(),
        seed.root_path().to_str().unwrap(),
        main.to_str().unwrap(),
    ]);
    seed.run_git_in(&store, &["config", "core.worktree", main.to_str().unwrap()]);
    let oracle = seed
        .git_command()
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(&store)
        .run()
        .unwrap();
    assert!(oracle.status.success());
    let expected = dunce::canonicalize(&main).unwrap();
    assert_eq!(
        oracle.stdout.strip_suffix(b"\n").unwrap(),
        expected.as_os_str().as_bytes()
    );
    let repo = Repository::at(&main).unwrap();
    assert_eq!(repo.repo_path().unwrap(), expected);
}

/// Discovery of an unrepaired main still protects it when it lives inside the
/// selected linked checkout, including explicit force removal.
#[test]
fn nested_worktree_protects_unrepaired_separate_git_dir_main() {
    let seed = TestRepo::with_initial_commit();
    let directory = crate::testing::test_tempdir();
    let store = directory.path().join("store");
    let main = directory.path().join("main");
    let outer = directory.path().join("outer");
    seed.run_git(&[
        "clone",
        "--separate-git-dir",
        store.to_str().unwrap(),
        seed.root_path().to_str().unwrap(),
        main.to_str().unwrap(),
    ]);
    std::fs::write(main.join(".gitignore"), "main/\n").unwrap();
    seed.run_git_in(&main, &["add", ".gitignore"]);
    seed.run_git_in(&main, &["commit", "-m", "ignore nested main"]);
    seed.run_git_in(
        &main,
        &["worktree", "add", "-b", "outer", outer.to_str().unwrap()],
    );
    // A removal plan can cache the main location before a hook moves it.
    let cached_repo = Repository::at(&main).unwrap();
    assert_eq!(
        cached_repo.repo_path().unwrap(),
        dunce::canonicalize(&main).unwrap()
    );
    let nested_main = outer.join("main");
    std::fs::rename(&main, &nested_main).unwrap();
    assert!(!store.join("gitdir").exists());
    let error = cached_repo.nested_worktree(&outer).unwrap_err();
    assert!(
        error.to_string().contains("git worktree repair"),
        "{error:#}"
    );
    let repo = Repository::at(&nested_main).unwrap();
    assert_eq!(
        repo.nested_worktree(&outer).unwrap(),
        Some(dunce::canonicalize(&nested_main).unwrap())
    );
    let registration = repo.worktree_at(&outer).git_dir().unwrap();
    let outer_index = std::fs::read(registration.join("index")).unwrap();
    let main_index = std::fs::read(store.join("index")).unwrap();
    for force in [false, true] {
        let error =
            crate::git::remove::stage_worktree_removal(&cached_repo, &outer, Some("outer"), force)
                .unwrap_err();
        assert!(
            error.to_string().contains("git worktree repair"),
            "{error:#}"
        );
        assert_eq!(
            std::fs::read(registration.join("index")).unwrap(),
            outer_index
        );
        assert_eq!(std::fs::read(store.join("index")).unwrap(), main_index);
        let error = crate::git::remove::stage_worktree_removal(&repo, &outer, Some("outer"), force)
            .unwrap_err();
        assert!(
            matches!(
                error.downcast_ref::<crate::git::GitError>(),
                Some(crate::git::GitError::NestedWorktree { .. })
            ),
            "{error:#}"
        );
        assert_eq!(
            std::fs::read(registration.join("index")).unwrap(),
            outer_index
        );
        assert_eq!(std::fs::read(store.join("index")).unwrap(), main_index);
        assert!(nested_main.join(".git").is_file());
        assert!(outer.join(".git").is_file());
    }
}

/// An unrecorded main can share its Git directory with an apparent conventional
/// parent. Its own gitfile still protects it inside an ignored removal tree.
#[test]
fn nested_worktree_protects_main_with_dot_git_separate_store() {
    let seed = TestRepo::with_initial_commit();
    let directory = crate::testing::test_tempdir();
    let store = directory.path().join("x/.git");
    let main = directory.path().join("work");
    let outer = directory.path().join("outer");
    std::fs::create_dir_all(store.parent().unwrap()).unwrap();
    seed.run_git(&[
        "clone",
        "--separate-git-dir",
        store.to_str().unwrap(),
        seed.root_path().to_str().unwrap(),
        main.to_str().unwrap(),
    ]);
    std::fs::write(main.join(".gitignore"), "nest/\n").unwrap();
    seed.run_git_in(&main, &["add", ".gitignore"]);
    seed.run_git_in(&main, &["commit", "-m", "ignore nested main"]);
    seed.run_git_in(
        &main,
        &["worktree", "add", "-b", "outer", outer.to_str().unwrap()],
    );
    let discovered = Repository::at(&main).unwrap();
    let canonical_main = dunce::canonicalize(&main).unwrap();
    assert_eq!(discovered.repo_path().unwrap(), canonical_main);
    assert_eq!(
        discovered.primary_worktree().unwrap(),
        Some(canonical_main.clone())
    );
    assert_eq!(discovered.list_worktrees().unwrap()[0].path, canonical_main);
    std::fs::create_dir(main.join("subdirectory")).unwrap();
    assert_eq!(
        Repository::at(main.join("subdirectory"))
            .unwrap()
            .repo_path()
            .unwrap(),
        canonical_main
    );
    std::fs::write(main.join("notes.txt"), "sole main notes").unwrap();
    let nested = outer.join("nest/work");
    std::fs::create_dir(nested.parent().unwrap()).unwrap();
    std::fs::rename(&main, &nested).unwrap();
    assert!(!store.join("gitdir").exists());
    // Linked discovery has no record of the actual main; inspect what removal
    // would delete instead of trusting Git's conventional-parent listing.
    let repo = Repository::at(&outer).unwrap();
    let registration = repo.worktree_at(&outer).git_dir().unwrap();
    let main_index = std::fs::read(store.join("index")).unwrap();
    let outer_index = std::fs::read(registration.join("index")).unwrap();
    assert_eq!(
        repo.nested_worktree(&outer).unwrap(),
        Some(dunce::canonicalize(&nested).unwrap())
    );
    for force in [false, true] {
        let error = crate::git::remove::stage_worktree_removal(&repo, &outer, Some("outer"), force)
            .unwrap_err();
        assert!(
            matches!(
                error.downcast_ref::<crate::git::GitError>(),
                Some(crate::git::GitError::NestedWorktree { .. })
            ),
            "{error:#}"
        );
        assert_eq!(std::fs::read(store.join("index")).unwrap(), main_index);
        assert_eq!(
            std::fs::read(registration.join("index")).unwrap(),
            outer_index
        );
        assert_eq!(
            std::fs::read_to_string(nested.join("notes.txt")).unwrap(),
            "sole main notes"
        );
    }
}

/// Removal does not follow directory aliases, but unreadable own Git entries
/// inside the actual removal tree must not become evidence of no checkout.
#[cfg(unix)]
#[test]
fn nested_worktree_scan_uses_removal_tree_and_propagates_io_errors() {
    let mut seed = TestRepo::with_initial_commit();
    let outer = seed.add_worktree("outer");
    let repo = Repository::at(seed.root_path()).unwrap();
    std::fs::write(repo.git_common_dir().join("info/exclude"), ".ignored/\n").unwrap();
    let ignored = outer.join(".ignored");
    std::fs::create_dir(&ignored).unwrap();
    std::os::unix::fs::symlink(seed.root_path(), ignored.join("main-alias")).unwrap();
    assert_eq!(repo.nested_worktree(&outer).unwrap(), None);
    let malformed = ignored.join("malformed");
    std::fs::create_dir(&malformed).unwrap();
    std::os::unix::fs::symlink(".git", malformed.join(".git")).unwrap();
    let registration = repo.worktree_at(&outer).git_dir().unwrap();
    let index = std::fs::read(registration.join("index")).unwrap();
    let error = repo.nested_worktree(&outer).unwrap_err();
    assert!(
        error.to_string().contains("Failed to inspect Git entry @"),
        "{error:#}"
    );
    assert!(
        error
            .to_string()
            .contains(&crate::path::format_path_for_display(
                &malformed.join(".git")
            )),
        "{error:#}"
    );
    for force in [false, true] {
        let error = crate::git::remove::stage_worktree_removal(&repo, &outer, Some("outer"), force)
            .unwrap_err();
        assert!(
            error.downcast_ref::<std::io::Error>().is_some(),
            "{error:#}"
        );
        assert!(error.to_string().contains("Failed to inspect Git entry @"));
        assert!(
            error
                .to_string()
                .contains(&crate::path::format_path_for_display(
                    &malformed.join(".git")
                ))
        );
        assert_eq!(std::fs::read(registration.join("index")).unwrap(), index);
        assert!(outer.join(".git").is_file());
    }
}

/// Reconnecting a bare common store inside the selected linked checkout must
/// not let force removal delete the only objects and registration index.
#[test]
fn nested_worktree_refuses_contained_common_git_storage() {
    let seed = TestRepo::with_initial_commit();
    std::fs::write(seed.root_path().join(".gitignore"), ".store/\n").unwrap();
    seed.run_git(&["add", ".gitignore"]);
    seed.run_git(&["commit", "-m", "ignore moved store"]);
    let directory = crate::testing::test_tempdir();
    let original = directory.path().join("bare.git");
    let outer = directory.path().join("outer");
    seed.run_git(&[
        "clone",
        "--bare",
        seed.root_path().to_str().unwrap(),
        original.to_str().unwrap(),
    ]);
    seed.run_git_in(
        &original,
        &["worktree", "add", "-b", "outer", outer.to_str().unwrap()],
    );
    std::fs::write(outer.join("only-history.txt"), "sole unpushed history").unwrap();
    seed.run_git_in(&outer, &["add", "only-history.txt"]);
    seed.run_git_in(&outer, &["commit", "-m", "sole history"]);
    let head = String::from_utf8(
        seed.git_command()
            .args(["rev-parse", "HEAD"])
            .current_dir(&outer)
            .run()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let store = outer.join(".store");
    std::fs::rename(&original, &store).unwrap();
    std::fs::write(
        outer.join(".git"),
        format!("gitdir: {}\n", store.join("worktrees/outer").display()),
    )
    .unwrap();
    let repo = Repository::at(&outer).unwrap();
    assert!(repo.is_bare().unwrap());
    let registration = repo.worktree_at(&outer).git_dir().unwrap();
    let index = std::fs::read(registration.join("index")).unwrap();
    let error = repo.nested_worktree(&outer).unwrap_err();
    assert!(
        error.to_string().contains("contains a Git directory"),
        "{error:#}"
    );
    for force in [false, true] {
        let error = crate::git::remove::stage_worktree_removal(&repo, &outer, Some("outer"), force)
            .unwrap_err();
        assert!(
            error.to_string().contains("contains a Git directory"),
            "{error:#}"
        );
        assert_eq!(std::fs::read(registration.join("index")).unwrap(), index);
        assert_eq!(
            std::fs::read_to_string(outer.join("only-history.txt")).unwrap(),
            "sole unpushed history"
        );
        seed.run_git_in(&outer, &["cat-file", "-e", head.trim()]);
    }
}

/// A stale backlink must not hide the checkout's actual own registration
/// after a manual move into the ignored removal tree.
#[test]
fn nested_worktree_protects_moved_linked_checkout() {
    let mut seed = TestRepo::with_initial_commit();
    std::fs::write(seed.root_path().join(".gitignore"), "nest/\n").unwrap();
    seed.run_git(&["add", ".gitignore"]);
    seed.run_git(&["commit", "-m", "ignore nested checkout"]);
    let outer = seed.add_worktree("outer");
    let inner = seed.add_worktree("inner");
    std::fs::write(inner.join("staged.txt"), "staged content").unwrap();
    seed.run_git_in(&inner, &["add", "staged.txt"]);
    std::fs::write(inner.join("staged.txt"), "unstaged content").unwrap();
    std::fs::write(inner.join("notes.txt"), "untracked notes").unwrap();
    let repo = Repository::at(seed.root_path()).unwrap();
    let inner_registration = repo.worktree_at(&inner).git_dir().unwrap();
    let outer_registration = repo.worktree_at(&outer).git_dir().unwrap();
    let inner_index = std::fs::read(inner_registration.join("index")).unwrap();
    let outer_index = std::fs::read(outer_registration.join("index")).unwrap();
    let nested = outer.join("nest/inner");
    std::fs::create_dir(nested.parent().unwrap()).unwrap();
    std::fs::rename(&inner, &nested).unwrap();
    assert!(!inner.exists());
    assert_eq!(
        repo.nested_worktree(&outer).unwrap(),
        Some(dunce::canonicalize(&nested).unwrap())
    );
    #[cfg(unix)]
    {
        let marker = seed.home_path().join("moved-checkout-gitfile");
        std::fs::rename(nested.join(".git"), &marker).unwrap();
        std::os::unix::fs::symlink(marker, nested.join(".git")).unwrap();
        assert_eq!(
            repo.nested_worktree(&outer).unwrap(),
            Some(dunce::canonicalize(&nested).unwrap())
        );
    }
    #[cfg(any(target_os = "macos", windows))]
    {
        std::fs::rename(nested.join(".git"), nested.join(".GIT")).unwrap();
        let expected = nested
            .join(".git")
            .is_file()
            .then(|| dunce::canonicalize(&nested).unwrap());
        assert_eq!(repo.nested_worktree(&outer).unwrap(), expected);
        std::fs::rename(nested.join(".GIT"), nested.join(".git")).unwrap();
    }
    for force in [false, true] {
        let error = crate::git::remove::stage_worktree_removal(&repo, &outer, Some("outer"), force)
            .unwrap_err();
        assert!(
            matches!(
                error.downcast_ref::<crate::git::GitError>(),
                Some(crate::git::GitError::NestedWorktree { .. })
            ),
            "{error:#}"
        );
        assert_eq!(
            std::fs::read(inner_registration.join("index")).unwrap(),
            inner_index
        );
        assert_eq!(
            std::fs::read(outer_registration.join("index")).unwrap(),
            outer_index
        );
        assert_eq!(
            std::fs::read_to_string(nested.join("staged.txt")).unwrap(),
            "unstaged content"
        );
        assert_eq!(
            std::fs::read_to_string(nested.join("notes.txt")).unwrap(),
            "untracked notes"
        );
    }
}

/// A registration alias may point anywhere, but outer removal must preserve
/// sibling metadata when the resolved store lies inside the removal tree.
#[cfg(unix)]
#[test]
fn nested_worktree_preserves_contained_registration_storage() {
    for contained in [false, true] {
        let mut seed = TestRepo::with_initial_commit();
        std::fs::write(seed.root_path().join(".gitignore"), ".storage/\n").unwrap();
        seed.run_git(&["add", ".gitignore"]);
        seed.run_git(&["commit", "-m", "ignore registration storage"]);
        let outer = seed.add_worktree("outer");
        let inner = seed.add_worktree("inner");
        std::fs::write(inner.join("staged.txt"), "staged contents").unwrap();
        seed.run_git_in(&inner, &["add", "staged.txt"]);
        std::fs::write(inner.join("staged.txt"), "unstaged contents").unwrap();
        std::fs::write(inner.join("notes.txt"), "untracked notes").unwrap();
        let repo = Repository::at(seed.root_path()).unwrap();
        let registration = repo.worktree_at(&inner).git_dir().unwrap();
        let outer_registration = repo.worktree_at(&outer).git_dir().unwrap();
        let storage = if contained {
            outer.join(".storage/inner-registration")
        } else {
            seed.root_path().join("external-registration")
        };
        std::fs::create_dir_all(storage.parent().unwrap()).unwrap();
        std::fs::rename(&registration, &storage).unwrap();
        std::fs::write(
            storage.join("commondir"),
            format!("{}\n", repo.git_common_dir().display()),
        )
        .unwrap();
        std::os::unix::fs::symlink(&storage, &registration).unwrap();
        let status = |path: &std::path::Path| {
            let output = seed
                .git_command()
                .args(["--no-optional-locks", "status", "--porcelain"])
                .current_dir(path)
                .run()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            output.stdout
        };
        assert!(status(&outer).is_empty());
        assert_eq!(status(&inner), b"AM staged.txt\n?? notes.txt\n");
        let staged = seed
            .git_command()
            .args(["show", ":staged.txt"])
            .current_dir(&inner)
            .run()
            .unwrap();
        assert!(staged.status.success());
        assert_eq!(staged.stdout, b"staged contents");
        let metadata: Vec<_> = ["HEAD", "index", "commondir", "gitdir"]
            .into_iter()
            .map(|name| (name, std::fs::read(storage.join(name)).unwrap()))
            .collect();
        let outer_index = std::fs::read(outer_registration.join("index")).unwrap();
        let force_modes: &[bool] = if contained { &[false, true] } else { &[false] };
        for &force in force_modes {
            let removal =
                crate::git::remove::stage_worktree_removal(&repo, &outer, Some("outer"), force);
            if contained {
                let error = removal.unwrap_err();
                assert!(
                    error.to_string().contains("contains a Git directory"),
                    "{error:#}"
                );
                assert!(outer.join(".git").is_file());
                assert_eq!(
                    std::fs::read(outer_registration.join("index")).unwrap(),
                    outer_index
                );
            } else {
                assert!(removal.unwrap().is_some());
                assert!(!outer.exists());
            }
            for (name, bytes) in &metadata {
                assert_eq!(std::fs::read(storage.join(name)).unwrap(), *bytes, "{name}");
            }
            assert!(registration.is_symlink());
            assert_eq!(
                std::fs::read_to_string(inner.join("staged.txt")).unwrap(),
                "unstaged contents"
            );
            assert_eq!(
                std::fs::read_to_string(inner.join("notes.txt")).unwrap(),
                "untracked notes"
            );
            let staged = seed
                .git_command()
                .args(["show", ":staged.txt"])
                .current_dir(&inner)
                .run()
                .unwrap();
            assert!(staged.status.success());
            assert_eq!(staged.stdout, b"staged contents");
            assert_eq!(status(&inner), b"AM staged.txt\n?? notes.txt\n");
            assert_eq!(
                Repository::at(&inner).unwrap().git_common_dir(),
                repo.git_common_dir()
            );
        }
    }
}

/// Build a `git init --separate-git-dir` layout: the work tree's `.git` is a
/// *file* pointing at a store directory outside it. Returns the tempdir, the
/// store (which is the repository's git common dir) and the work tree.
///
/// `git worktree repair` runs last so the store carries git's `gitdir`
/// backlink. Neither `git init --separate-git-dir` nor `git clone
/// --separate-git-dir` writes one, so an unrepaired repository of this shape
/// has no recorded backlink; discovery from its main still identifies it.
///
/// `relative_backlink` sets `worktree.useRelativePaths` before the repair, so
/// git writes the backlink as a path relative to the store rather than an
/// absolute one. Git honors the setting from 2.48; below that it keeps writing
/// the absolute form, which the tests accept the same way
/// `test_switch_with_relative_worktree_paths` does.
fn build_separate_git_dir_layout() -> (tempfile::TempDir, PathBuf, PathBuf) {
    build_separate_git_dir_layout_with(false)
}

fn build_separate_git_dir_layout_with(
    relative_backlink: bool,
) -> (tempfile::TempDir, PathBuf, PathBuf) {
    use super::canonicalize;
    use crate::shell_exec::Cmd;

    let tmp = tempfile::tempdir().unwrap();
    let root = canonicalize(tmp.path()).unwrap();
    let store = root.join("store").join("repo.gitdir");
    let work_tree = root.join("work");
    std::fs::create_dir_all(root.join("store")).unwrap();

    let git = || crate::testing::configure_git_env(Cmd::new("git"));
    let path_str = |p: &std::path::Path| p.to_str().unwrap().to_owned();

    let out = git()
        .args([
            "init",
            "-b",
            "main",
            "--separate-git-dir",
            &path_str(&store),
            &path_str(&work_tree),
        ])
        .run()
        .unwrap();
    assert!(out.status.success(), "git init --separate-git-dir failed");

    let out = git()
        .current_dir(&work_tree)
        .args(["commit", "--allow-empty", "-m", "init"])
        .run()
        .unwrap();
    assert!(out.status.success(), "git commit failed");

    if relative_backlink {
        let out = git()
            .current_dir(&work_tree)
            .args(["config", "worktree.useRelativePaths", "true"])
            .run()
            .unwrap();
        assert!(out.status.success(), "git config failed");
    }

    let out = git()
        .current_dir(&work_tree)
        .args(["worktree", "repair"])
        .run()
        .unwrap();
    assert!(out.status.success(), "git worktree repair failed");

    (tmp, store, work_tree)
}

#[test]
fn repo_path_follows_the_separate_git_dir_backlink() {
    // Regression test for #4235. With the git dir outside the work tree, the
    // `parent(git_common_dir)` fallback names the store's *parent* — so
    // `{{ repo_path }}.{{ branch }}` placed new worktrees beside the store
    // rather than beside the work tree.
    use super::{Repository, canonicalize};

    let (_tmp, store, work_tree) = build_separate_git_dir_layout();

    let repo = Repository::at(&work_tree).unwrap();
    assert_eq!(
        canonicalize(repo.git_common_dir()).unwrap(),
        canonicalize(&store).unwrap(),
        "the store is the git common dir in this layout"
    );
    assert_eq!(
        canonicalize(repo.repo_path().unwrap()).unwrap(),
        work_tree,
        "repo_path must name the work tree, not the store's parent"
    );
}

#[test]
fn list_worktrees_names_the_work_tree_of_a_separate_git_dir_repo() {
    // Companion to the above: `git worktree list` reports the git common dir
    // as the main worktree entry here, the same shape `list_worktrees`
    // already corrects for submodules. Pin the corrected path, which is what
    // `wt list` renders.
    use super::{Repository, canonicalize};

    let (_tmp, _store, work_tree) = build_separate_git_dir_layout();

    let repo = Repository::at(&work_tree).unwrap();
    let worktrees = repo.list_worktrees().unwrap();
    assert_eq!(
        canonicalize(&worktrees[0].path).unwrap(),
        work_tree,
        "the main worktree entry must be the work tree"
    );
}

#[test]
fn repo_path_declines_a_stale_separate_git_dir_backlink() {
    // The backlink is one-way, so `repo_path` confirms it: a work tree that
    // has moved away leaves a `gitdir` file naming a path that no longer
    // points back. Store-only discovery cannot recover the moved main, so
    // the canonical resolver must report that it cannot identify it.
    use super::{Repository, canonicalize};

    let (_tmp, store, work_tree) = build_separate_git_dir_layout();
    let moved = work_tree.with_file_name("moved");
    std::fs::rename(&work_tree, &moved).unwrap();
    // Plant an unrelated repository where the work tree used to be, so the
    // backlink still resolves to a directory and the `.git` entry there is
    // what rejects it — not the path simply being gone.
    let out = crate::testing::configure_git_env(crate::shell_exec::Cmd::new("git"))
        .args(["init", "-b", "main", work_tree.to_str().unwrap()])
        .run()
        .unwrap();
    assert!(out.status.success(), "git init failed");

    let repo = Repository::at(&store).unwrap();
    assert!(
        repo.repo_path()
            .unwrap_err()
            .to_string()
            .contains("Cannot determine the main worktree location")
    );
    let discovered = Repository::at(&moved).unwrap();
    assert_eq!(
        discovered.repo_path().unwrap(),
        canonicalize(&moved).unwrap()
    );
}

#[test]
fn repo_path_follows_a_relative_separate_git_dir_backlink() {
    // Under `worktree.useRelativePaths` git writes the backlink relative to
    // the store — `../../work/.git` — the same form it uses for a linked
    // worktree's registration, and it resolves it against the directory the
    // file sits in. Declining that form, or resolving it against the process
    // cwd, would leave #4235 unfixed for anyone with the setting on.
    use std::path::Path;

    use super::{Repository, canonicalize};

    let (_tmp, store, work_tree) = build_separate_git_dir_layout_with(true);

    let backlink = std::fs::read_to_string(store.join("gitdir")).unwrap();
    assert!(
        backlink.starts_with("../") || Path::new(backlink.trim()).is_absolute(),
        "expected a relative or (pre-2.48) absolute backlink: {backlink}"
    );

    let repo = Repository::at(&work_tree).unwrap();
    assert_eq!(
        canonicalize(repo.repo_path().unwrap()).unwrap(),
        work_tree,
        "a relative backlink must resolve against the store, not the cwd"
    );
}

#[test]
fn nested_worktree_reads_fresh_topology() {
    let mut test = TestRepo::with_initial_commit();
    let source = test.add_worktree("source");
    let repo = Repository::at(test.root_path()).unwrap();
    // Warm the cache before adding another checkout: the guard must read
    // topology after planning, independently of that cache.
    repo.list_worktrees().unwrap();
    let late = test.add_worktree_at_path("late", &source.join("late"));
    assert_eq!(repo.nested_worktree(&source).unwrap(), Some(late));
}

#[test]
fn nested_worktree_accepts_missing_registry() {
    let test = TestRepo::with_initial_commit();
    let repo = Repository::at(test.root_path()).unwrap();
    assert!(!repo.git_common_dir().join("worktrees").exists());
    let index = std::fs::read(repo.git_common_dir().join("index")).unwrap();
    let empty = test.root_path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    assert_eq!(repo.nested_worktree(&empty).unwrap(), None);
    assert_eq!(
        std::fs::read(repo.git_common_dir().join("index")).unwrap(),
        index
    );
}

#[cfg(unix)]
#[test]
fn nested_worktree_resolves_symlinked_parent() {
    let mut test = TestRepo::with_initial_commit();
    let source = test.add_worktree("source");
    let nested = test.add_worktree_at_path("nested", &source.join("nested"));
    let alias = test.root_path().parent().unwrap().join("source-alias");
    std::os::unix::fs::symlink(&source, &alias).unwrap();
    let repo = Repository::at(test.root_path()).unwrap();
    assert_eq!(repo.nested_worktree(&alias).unwrap(), Some(nested));
}

#[test]
fn nested_worktree_skips_missing_checkout() {
    let mut test = TestRepo::with_initial_commit();
    let source = test.add_worktree("source");
    let nested = test.add_worktree_at_path("nested", &source.join("nested"));
    std::fs::rename(&nested, test.root_path().parent().unwrap().join("saved")).unwrap();
    let repo = Repository::at(test.root_path()).unwrap();
    assert_eq!(repo.nested_worktree(&source).unwrap(), None);
}

#[cfg(unix)]
#[test]
fn nested_worktree_refuses_unresolvable_checkout() {
    let mut test = TestRepo::with_initial_commit();
    let source = test.add_worktree("source");
    let nested = test.add_worktree_at_path("nested", &source.join("nested"));
    let saved = test.root_path().parent().unwrap().join("saved");
    std::fs::rename(&nested, &saved).unwrap();
    // A symlink loop is an actual resolution error, not evidence that the
    // registered checkout is absent. Refuse cleanup rather than skip it.
    std::os::unix::fs::symlink(&nested, &nested).unwrap();
    let repo = Repository::at(test.root_path()).unwrap();
    let err = repo.nested_worktree(&source).unwrap_err();
    assert!(
        err.to_string().contains("Failed to resolve worktree @"),
        "{err:?}"
    );
    assert!(saved.join(".git").exists());
}

#[test]
fn nested_worktree_refuses_incomplete_registration() {
    let cases: [Option<&[u8]>; 4] = [
        None,
        Some(b""),
        Some(b"not-a-gitdir-backlink"),
        Some(b"\xff"),
    ];
    for contents in cases {
        let mut test = TestRepo::with_initial_commit();
        let outer = test.add_worktree("outer");
        let inner = test.add_worktree_at_path("inner", &outer.join("inner"));
        let repo = Repository::at(test.root_path()).unwrap();
        let registration = repo.worktree_at(&inner).git_dir().unwrap();
        let backlink = registration.join("gitdir");
        let saved = std::fs::read(&backlink).unwrap();
        std::fs::remove_file(&backlink).unwrap();
        if let Some(contents) = contents {
            std::fs::write(&backlink, contents).unwrap();
        }
        let error = repo.nested_worktree(&outer).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Failed to read worktree registration"),
            "{error:?}"
        );
        std::fs::write(&backlink, saved).unwrap();
        assert_eq!(repo.nested_worktree(&outer).unwrap(), Some(inner));
    }
}

#[test]
fn nested_worktree_refuses_unreadable_registration() {
    let mut test = TestRepo::with_initial_commit();
    let outer = test.add_worktree("outer");
    let inner = test.add_worktree_at_path("inner", &outer.join("inner"));
    let repo = Repository::at(test.root_path()).unwrap();
    let backlink = repo.worktree_at(&inner).git_dir().unwrap().join("gitdir");
    let saved = std::fs::read(&backlink).unwrap();
    std::fs::remove_file(&backlink).unwrap();
    std::fs::create_dir(&backlink).unwrap();
    let error = repo.nested_worktree(&outer).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Failed to read worktree registration"),
        "{error:?}"
    );
    std::fs::remove_dir(&backlink).unwrap();
    std::fs::write(backlink, saved).unwrap();
}

#[test]
fn nested_worktree_uses_git_generated_relative_backlinks() {
    let mut test = TestRepo::with_initial_commit();
    test.run_git(&["config", "worktree.useRelativePaths", "true"]);
    let outer = test.add_worktree("outer");
    let inner = test.add_worktree_at_path("inner", &outer.join("inner"));
    let repo = Repository::at(test.root_path()).unwrap();
    let backlink =
        std::fs::read_to_string(repo.worktree_at(&inner).git_dir().unwrap().join("gitdir"))
            .unwrap();
    assert_eq!(
        std::path::Path::new(backlink.trim()).is_relative(),
        repo.config_bool("extensions.relativeWorktrees").unwrap()
    );
    assert_eq!(repo.nested_worktree(&outer).unwrap(), Some(inner));
}

#[test]
fn nested_worktree_in_bare_repository() {
    let seed = TestRepo::with_initial_commit();
    let mut test = TestRepo::bare();
    test.run_git(&[
        "fetch",
        seed.root_path().to_str().unwrap(),
        "main:refs/heads/main",
    ]);
    let outer = test.add_worktree("outer");
    let inner = test.add_worktree_at_path("inner", &outer.join("inner"));
    let repo = Repository::at(&outer).unwrap();
    assert_eq!(repo.nested_worktree(&outer).unwrap(), Some(inner));
}

#[cfg(unix)]
#[test]
fn nested_worktree_refuses_broken_registration_symlink() {
    let mut test = TestRepo::with_initial_commit();
    let outer = test.add_worktree("outer");
    let registry = test.root_path().join(".git/worktrees");
    std::os::unix::fs::symlink(registry.join("missing"), registry.join("broken")).unwrap();
    let repo = Repository::at(test.root_path()).unwrap();
    let error = repo.nested_worktree(&outer).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Failed to read worktree registration"),
        "{error:?}"
    );
}

#[test]
fn nested_worktree_finds_main_with_separate_git_dir() {
    let (_tmp, _store, main) = build_separate_git_dir_layout();
    let outer = main.parent().unwrap().join("outer");
    let git = || crate::testing::configure_git_env(crate::shell_exec::Cmd::new("git"));
    let added = git()
        .current_dir(&main)
        .args(["worktree", "add", "-b", "outer", outer.to_str().unwrap()])
        .run()
        .unwrap();
    assert!(added.status.success(), "{added:?}");
    let nested_main = outer.join("main");
    std::fs::rename(main, &nested_main).unwrap();
    let repair = git()
        .current_dir(&nested_main)
        .args(["worktree", "repair"])
        .run()
        .unwrap();
    assert!(repair.status.success(), "{repair:?}");
    let repo = Repository::at(&outer).unwrap();
    assert_eq!(repo.nested_worktree(&outer).unwrap(), Some(nested_main));
}

#[test]
fn nested_worktree_refuses_unknown_main_with_separate_git_dir() {
    let (_tmp, store, main) = build_separate_git_dir_layout();
    let outer = main.parent().unwrap().join("outer");
    let added = crate::testing::configure_git_env(crate::shell_exec::Cmd::new("git"))
        .current_dir(&main)
        .args(["worktree", "add", "-b", "outer", outer.to_str().unwrap()])
        .run()
        .unwrap();
    assert!(added.status.success(), "{added:?}");
    std::fs::remove_file(store.join("gitdir")).unwrap();
    let repo = Repository::at(&outer).unwrap();
    assert!(
        repo.nested_worktree(&outer)
            .unwrap_err()
            .to_string()
            .contains("Cannot determine the main worktree location")
    );
}

#[test]
fn nested_worktree_in_submodule_repository() {
    let source = TestRepo::with_initial_commit();
    let host = TestRepo::with_initial_commit();
    host.run_git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        source.root_path().to_str().unwrap(),
        "sub",
    ]);
    let main = host.root_path().join("sub");
    let outer = main.join(".worktrees/outer");
    let inner = outer.join("inner");
    for (branch, path) in [("outer", &outer), ("inner", &inner)] {
        let output = host
            .git_command()
            .current_dir(&main)
            .args(["worktree", "add", "-b", branch, path.to_str().unwrap()])
            .run()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }
    let repo = Repository::at(&outer).unwrap();
    assert_eq!(repo.nested_worktree(&outer).unwrap(), Some(inner));
}

/// Atomic unregister may overlap fresh topology reads. Neither partial metadata
/// failures nor the registry teardown lock may obstruct an unrelated guard.
#[test]
fn nested_worktree_reads_during_atomic_unregister() {
    let mut test = TestRepo::with_initial_commit();
    let outer = test.add_worktree("outer");
    let mut stale = Vec::new();
    for index in 0..8 {
        let path = test.add_worktree(&format!("stale-{index}"));
        std::fs::remove_dir_all(&path).unwrap();
        stale.push(path);
    }
    let repo = Repository::at(test.root_path()).unwrap();
    let worker_repo = repo.clone();
    let worker = std::thread::spawn(move || {
        for path in stale {
            worker_repo
                .prune_worktree_entry(&path, crate::git::WorktreePruneMode::Stale)
                .unwrap();
        }
    });
    for _ in 0..32 {
        assert_eq!(repo.nested_worktree(&outer).unwrap(), None);
    }
    worker.join().unwrap();
    assert_eq!(repo.nested_worktree(&outer).unwrap(), None);
}

#[cfg(unix)]
#[test]
fn nested_worktree_refuses_broken_registry_symlink() {
    let mut test = TestRepo::with_initial_commit();
    let outer = test.add_worktree("outer");
    let registry = test.root_path().join(".git/worktrees");
    let saved = test.root_path().join(".git/saved-worktrees");
    std::fs::rename(&registry, &saved).unwrap();
    std::os::unix::fs::symlink(test.root_path().join(".git/missing"), &registry).unwrap();
    let repo = Repository::at(test.root_path()).unwrap();
    let error = repo.nested_worktree(&outer).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Failed to read worktree registry"),
        "{error:?}"
    );
    std::fs::remove_file(&registry).unwrap();
    std::fs::rename(saved, registry).unwrap();
}

#[test]
fn nested_worktree_skips_regular_registry_files() {
    let mut test = TestRepo::with_initial_commit();
    let outer = test.add_worktree("outer");
    let repo = Repository::at(test.root_path()).unwrap();
    let stray = repo.git_common_dir().join("worktrees/leftover-note");
    std::fs::write(&stray, "unrelated file").unwrap();
    assert_eq!(repo.nested_worktree(&outer).unwrap(), None);
    assert_eq!(std::fs::read_to_string(stray).unwrap(), "unrelated file");
}

/// Final unregister rechecks module storage after planning, even for a live
/// removal whose checkout has already moved. Inspection failures keep metadata.
#[test]
fn prune_worktree_entry_preserves_submodule_storage() {
    use crate::git::{StaleWorktreeWork, WorktreePruneMode};
    for mode in [WorktreePruneMode::Stale, WorktreePruneMode::RemovedLive] {
        let mut test = TestRepo::with_initial_commit();
        let outer = test.add_worktree("outer");
        let repo = Repository::at(test.root_path()).unwrap();
        let registration = repo.worktree_at(&outer).git_dir().unwrap();
        let modules = registration.join("modules");
        std::fs::create_dir(&modules).unwrap();
        // An empty container has no repository to preserve.
        assert!(repo.require_no_submodule_storage(&outer).is_ok());
        test.run_git(&["init", "--bare", modules.join("sub").to_str().unwrap()]);
        std::fs::remove_file(outer.join(".git")).unwrap();
        assert_eq!(
            repo.stale_worktree_work(&outer).unwrap(),
            Some(StaleWorktreeWork::SubmoduleRepositories)
        );
        let index = std::fs::read(registration.join("index")).unwrap();
        let error = repo.prune_worktree_entry(&outer, mode).unwrap_err();
        assert!(
            error.to_string().contains("submodule repositories"),
            "{error:#}"
        );
        assert_eq!(std::fs::read(registration.join("index")).unwrap(), index);
        assert!(modules.join("sub/objects").is_dir());
        repo.prune_worktree_entry(&outer, WorktreePruneMode::Force)
            .unwrap();
        assert!(!registration.exists());
    }
}

/// An unreadable indexed repository must never become force-discard consent.
#[cfg(unix)]
#[test]
fn submodule_inventory_preserves_data_on_inspection_errors() {
    for broken in ["index", "marker"] {
        let mut test = TestRepo::with_initial_commit();
        let outer = test.add_worktree("outer");
        let child = outer.join("child");
        test.run_git(&[
            "clone",
            test.root_path().to_str().unwrap(),
            child.to_str().unwrap(),
        ]);
        test.run_git_in(
            &child,
            &["commit", "--allow-empty", "-m", "sole child history"],
        );
        test.run_git_in(&outer, &["add", "child"]);
        let repo = Repository::at(test.root_path()).unwrap();
        let registration = repo.worktree_at(&outer).git_dir().unwrap();
        let outer_index = std::fs::read(registration.join("index")).unwrap();
        let child_index = std::fs::read(child.join(".git/index")).unwrap();
        let history = std::fs::read(child.join(".git/refs/heads/main")).unwrap();
        let (saved, expected) = if broken == "index" {
            let saved = child.join(".git/saved-index");
            std::fs::rename(child.join(".git/index"), &saved).unwrap();
            std::os::unix::fs::symlink("index", child.join(".git/index")).unwrap();
            (saved, "Failed to inspect submodule index")
        } else {
            let saved = outer.join("saved-child");
            std::fs::rename(&child, &saved).unwrap();
            std::os::unix::fs::symlink("child", &child).unwrap();
            (saved, "Failed to inspect")
        };
        let error = repo.require_no_submodule_storage(&outer).unwrap_err();
        assert!(
            format!("{error:#}").contains(expected),
            "{broken}: {error:#}"
        );
        assert!(
            error.downcast_ref::<crate::git::GitError>().is_none(),
            "An I/O error must not be a force-waivable refusal: {error:#}"
        );
        assert_eq!(
            std::fs::read(registration.join("index")).unwrap(),
            outer_index
        );
        if broken == "index" {
            std::fs::remove_file(child.join(".git/index")).unwrap();
            std::fs::rename(saved, child.join(".git/index")).unwrap();
        } else {
            std::fs::remove_file(&child).unwrap();
            std::fs::rename(saved, &child).unwrap();
        }
        assert_eq!(
            std::fs::read(child.join(".git/index")).unwrap(),
            child_index
        );
        assert_eq!(
            std::fs::read(child.join(".git/refs/heads/main")).unwrap(),
            history
        );
    }
}

/// A nested gitlink alias must inventory its own repository only once.
#[cfg(unix)]
#[test]
fn submodule_inventory_terminates_on_checkout_cycle() {
    let mut test = TestRepo::with_initial_commit();
    let outer = test.add_worktree("outer");
    let child = outer.join("embedded");
    test.run_git(&[
        "clone",
        test.root_path().to_str().unwrap(),
        child.to_str().unwrap(),
    ]);
    let head = test.git_output(&["rev-parse", "HEAD"]);
    test.run_git_in(
        &child,
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            "160000",
            &head,
            "self",
        ],
    );
    std::os::unix::fs::symlink(".", child.join("self")).unwrap();
    test.run_git_in(&outer, &["add", "embedded"]);
    let repo = Repository::at(test.root_path()).unwrap();
    let registration = repo.worktree_at(&outer).git_dir().unwrap();
    let index = std::fs::read(registration.join("index")).unwrap();
    let child_git_dir = dunce::canonicalize(child.join(".git")).unwrap();
    let child_index = std::fs::read(child_git_dir.join("index")).unwrap();
    assert_eq!(
        repo.worktree_at(&outer)
            .initialized_submodule_repositories()
            .unwrap(),
        Some(vec![child_git_dir.clone()])
    );
    let error = repo.require_no_submodule_storage(&outer).unwrap_err();
    assert!(
        matches!(
            error.downcast_ref::<crate::git::GitError>(),
            Some(crate::git::GitError::SubmoduleRepositories { repositories, .. })
                if repositories.as_slice() == std::slice::from_ref(&child_git_dir)
        ),
        "{error:#}"
    );
    assert_eq!(std::fs::read(registration.join("index")).unwrap(), index);
    assert_eq!(
        std::fs::read(child_git_dir.join("index")).unwrap(),
        child_index
    );
    test.run_git_in(&child, &["cat-file", "-e", "HEAD^{commit}"]);
}

/// Only stores that deletion reaches belong in its discard inventory.
#[cfg(unix)]
#[test]
fn submodule_inventory_distinguishes_owned_dangling_and_unresolvable_stores() {
    use crate::git::{GitError, WorktreePruneMode};
    for target in ["checkout", "dangling", "loop"] {
        let mut test = TestRepo::with_initial_commit();
        let outer = test.add_worktree("outer");
        let repo = Repository::at(test.root_path()).unwrap();
        let registration = repo.worktree_at(&outer).git_dir().unwrap();
        let index = std::fs::read(registration.join("index")).unwrap();
        let store = outer.join("store");
        if target == "checkout" {
            test.run_git(&[
                "clone",
                "--bare",
                test.root_path().to_str().unwrap(),
                store.to_str().unwrap(),
            ]);
            std::os::unix::fs::symlink(&store, registration.join("modules")).unwrap();
        } else {
            std::os::unix::fs::symlink(
                if target == "loop" {
                    "modules"
                } else {
                    "missing"
                },
                registration.join("modules"),
            )
            .unwrap();
        }
        let error = repo.require_no_submodule_storage(&outer).unwrap_err();
        if target == "loop" {
            assert!(
                error
                    .to_string()
                    .contains("Failed to resolve submodule repositories"),
                "{error:#}"
            );
        } else {
            let Some(GitError::SubmoduleRepositories { repositories, .. }) =
                error.downcast_ref::<GitError>()
            else {
                panic!("{error:#}")
            };
            let expected = if target == "checkout" {
                vec![dunce::canonicalize(&store).unwrap()]
            } else {
                vec![]
            };
            assert_eq!(*repositories, expected);
        }
        std::fs::remove_file(outer.join(".git")).unwrap();
        assert!(
            repo.prune_worktree_entry(&outer, WorktreePruneMode::Stale)
                .is_err()
        );
        assert_eq!(std::fs::read(registration.join("index")).unwrap(), index);
        let forced = repo.prune_worktree_entry(&outer, WorktreePruneMode::Force);
        if target == "loop" {
            assert!(
                forced
                    .unwrap_err()
                    .to_string()
                    .contains("Failed to resolve submodule repositories")
            );
            assert_eq!(std::fs::read(registration.join("index")).unwrap(), index);
        } else {
            forced.unwrap();
            assert!(!registration.exists());
            if target == "checkout" {
                assert!(store.join("objects").is_dir());
                test.run_git_in(&store, &["cat-file", "-e", "HEAD^{commit}"]);
            }
        }
    }
}

/// Unregistering a registration alias unlinks the alias, preserving its target.
#[cfg(unix)]
#[test]
fn submodule_inventory_preserves_aliased_registration_target() {
    use crate::git::{GitError, WorktreePruneMode};
    let mut test = TestRepo::with_initial_commit();
    let outer = test.add_worktree("outer");
    let repo = Repository::at(test.root_path()).unwrap();
    let registration = repo.worktree_at(&outer).git_dir().unwrap();
    let saved = repo.git_common_dir().join("saved-registration");
    std::fs::rename(&registration, &saved).unwrap();
    std::fs::write(
        saved.join("commondir"),
        repo.git_common_dir().to_str().unwrap(),
    )
    .unwrap();
    std::os::unix::fs::symlink(&saved, &registration).unwrap();
    let store = saved.join("modules/sub");
    test.run_git(&[
        "clone",
        "--bare",
        test.root_path().to_str().unwrap(),
        store.to_str().unwrap(),
    ]);
    let index = std::fs::read(saved.join("index")).unwrap();
    let error = repo.require_no_submodule_storage(&outer).unwrap_err();
    assert!(
        matches!(error.downcast_ref::<GitError>(), Some(GitError::SubmoduleRepositories { repositories, .. }) if repositories.is_empty()),
        "{error:#}"
    );
    std::fs::remove_file(outer.join(".git")).unwrap();
    repo.prune_worktree_entry(&outer, WorktreePruneMode::Force)
        .unwrap();
    assert!(!registration.exists());
    assert_eq!(std::fs::read(saved.join("index")).unwrap(), index);
    test.run_git_in(&store, &["cat-file", "-e", "HEAD^{commit}"]);
}
