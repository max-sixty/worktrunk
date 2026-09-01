use std::path::PathBuf;

use super::super::{DefaultBranchName, WorktreeInfo, finalize_worktree};

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

    let finalized = finalize_worktree(wt.clone());
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

    let finalized = finalize_worktree(wt.clone());
    assert_eq!(finalized.branch, Some("feature".to_string()));
}

#[test]
fn test_finalize_worktree_detached_no_branch() {
    // Detached worktree with no branch should attempt rebase detection
    // Note: This test validates the logic flow but doesn't test actual file reading
    // since that would require setting up git rebase state files.
    // Actual rebase detection has been manually verified.
    let wt = WorktreeInfo {
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
        worktree_registry_lock: Arc::new(std::sync::RwLock::new(())),
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
    let git_dir = tmp.path().join(".git");
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
        discovery_path: tmp.path().to_path_buf(),
        git_common_dir: git_dir.clone(),
        cache: Arc::new(cache),
        worktree_registry_lock: Arc::new(std::sync::RwLock::new(())),
        temporary_object_store: None,
    };

    // Should fall through to parent(git_common_dir), ignoring the bulk value.
    assert_eq!(repo.repo_path().unwrap(), tmp.path());
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
fn parse_config_list_z_entry_without_newline_tolerates_key_only() {
    // `git config --list -z` always emits `key\nvalue\0`, but the parser
    // tolerates bare `key\0` by mapping it to `key -> ""` rather than
    // dropping the entry. Lets a future git oddity be diagnosed at the
    // use-site instead of silently missing.
    let input = b"core.bare\0other.key\nfalse\0";
    let map = super::parse_config_list_z(input);
    assert_eq!(map["core.bare"], vec![""]);
    assert_eq!(map["other.key"], vec!["false"]);
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

    let err: anyhow::Error = StreamCommandError {
        output: "fatal: ref exists".into(),
        command: "git worktree add /path".into(),
        exit_info: "exit code 128".into(),
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
            worktree_registry_lock: Arc::new(std::sync::RwLock::new(())),
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
            super::integration::EMPTY_TREE_SHA,
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
fn repository_instances_share_worktree_registry_coordination() {
    use crate::git::Repository;
    use crate::testing::TestRepo;

    let mut test = TestRepo::with_initial_commit();
    let linked = test.add_worktree("registry-lock-linked");
    let first = Repository::at(test.root_path()).unwrap();
    let second = Repository::at(linked).unwrap();

    let _write = first.worktree_registry_write();
    assert!(
        second.worktree_registry_lock.try_read().is_err(),
        "fresh repository handles for one common directory must share the registry lock"
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
        err.to_string().contains("Worktree directory missing"),
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
