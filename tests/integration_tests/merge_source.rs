//! Source selection must apply to the whole merge, without borrowing the
//! invoking worktree's index, hooks, HEAD, or shell directory changes.

use crate::common::{
    SLEEP_FOR_ABSENCE_CHECK, TestRepo, repo, wait_for_file_content, wait_for_worktree_removed,
};
use path_slash::PathExt as _;
use rstest::rstest;
use std::fs;
use std::path::Path;

fn git_in(repo: &TestRepo, path: &Path, args: &[&str]) -> String {
    let output = repo
        .git_command()
        .current_dir(path)
        .args(args.iter().copied())
        .run()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[rstest]
#[case(false, false)]
#[case(true, false)]
#[case(false, true)]
#[case(true, true)]
fn test_merge_source_pipeline_preserves_invoker(
    mut repo: TestRepo,
    #[case] no_squash: bool,
    #[case] no_ff: bool,
) {
    let source = repo.add_feature();
    repo.commit_in_worktree(&source, "second.txt", "second", "Second source commit");
    fs::write(source.join("dirty-source.txt"), "source pending work").unwrap();
    repo.commit_in_worktree(repo.root_path(), "target.txt", "target", "Advance target");
    let target_before = repo.git_output(&["rev-parse", "main"]);
    let caller = repo.add_worktree("caller");
    fs::write(caller.join("staged.txt"), "caller's staged work").unwrap();
    repo.run_git_in(&caller, &["add", "staged.txt"]);
    fs::write(caller.join("untracked.txt"), "caller's untracked work").unwrap();
    let caller_head = git_in(&repo, &caller, &["rev-parse", "HEAD"]);
    let caller_index = git_in(&repo, &caller, &["write-tree"]);
    let caller_status = git_in(&repo, &caller, &["status", "--porcelain"]);
    let directive = caller.parent().unwrap().join("directive");
    fs::write(&directive, "").unwrap();

    let mut command = repo.wt_command();
    command
        .current_dir(&caller)
        .args([
            "merge",
            "--branch",
            "feature",
            "--yes",
            "--no-hooks",
            "--format=json",
        ])
        .env("WORKTRUNK_DIRECTIVE_CD_FILE", &directive);
    if no_squash {
        command.arg("--no-squash");
    }
    if no_ff {
        command.arg("--no-ff");
    }
    let output = command.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["branch"], "feature");
    assert_eq!(result["target"], "main");
    assert_eq!(result["committed"], no_squash);
    assert_eq!(result["squashed"], !no_squash);
    assert_eq!(result["rebased"], true);
    assert_eq!(result["removed"], true);
    wait_for_worktree_removed(&source);
    assert_eq!(
        repo.git_output(&["rev-list", "--count", &format!("{target_before}..main")]),
        ((if no_squash { 3 } else { 1 }) + usize::from(no_ff)).to_string()
    );
    for (name, expected) in [
        ("feature.txt", "feature content"),
        ("second.txt", "second"),
        ("dirty-source.txt", "source pending work"),
        ("target.txt", "target"),
    ] {
        assert_eq!(
            fs::read_to_string(repo.root_path().join(name)).unwrap(),
            expected
        );
    }
    assert_eq!(git_in(&repo, &caller, &["rev-parse", "HEAD"]), caller_head);
    assert_eq!(git_in(&repo, &caller, &["write-tree"]), caller_index);
    assert_eq!(
        git_in(&repo, &caller, &["status", "--porcelain"]),
        caller_status
    );
    assert_eq!(
        fs::read_to_string(caller.join("untracked.txt")).unwrap(),
        "caller's untracked work"
    );
    assert_eq!(fs::read_to_string(directive).unwrap(), "");
}

#[rstest]
fn test_merge_source_uses_source_hooks_without_switching(mut repo: TestRepo) {
    let source = repo.add_feature();
    let caller = repo.add_worktree("caller");
    fs::create_dir_all(caller.join(".config")).unwrap();
    fs::write(caller.join(".config/wt.toml"), "pre-merge = 'exit 91'\n").unwrap();
    let pre = caller.parent().unwrap().join("pre-merge");
    let post = caller.parent().unwrap().join("post-merge");
    let removed = caller.parent().unwrap().join("post-remove");
    let switched = caller.parent().unwrap().join("post-switch");
    fs::create_dir_all(source.join(".config")).unwrap();
    fs::write(
        source.join(".config/wt.toml"),
        format!(
            r#"
pre-merge = "printf '%s' '{{{{ branch }}}}:{{{{ worktree_path }}}}' > '{}'"
post-merge = "printf '%s' '{{{{ branch }}}}:{{{{ worktree_path }}}}:{{{{ target }}}}' > '{}'"
post-remove = "printf '%s' '{{{{ branch }}}}' > '{}'"
post-switch = "echo switched > '{}'"
"#,
            pre.to_slash_lossy(),
            post.to_slash_lossy(),
            removed.to_slash_lossy(),
            switched.to_slash_lossy()
        ),
    )
    .unwrap();
    repo.run_git_in(&source, &["add", ".config/wt.toml"]);
    repo.run_git_in(&source, &["commit", "-m", "Configure source hooks"]);

    let output = repo
        .wt_command()
        .current_dir(&caller)
        .args(["merge", "--branch", "feature", "--yes", "--no-commit"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    wait_for_file_content(&post);
    wait_for_file_content(&removed);
    assert_eq!(
        fs::read_to_string(pre).unwrap(),
        format!(
            "feature:{}",
            worktrunk::path::to_posix_path(&source.to_string_lossy())
        )
    );
    assert_eq!(
        fs::read_to_string(post).unwrap(),
        format!(
            "feature:{}:main",
            worktrunk::path::to_posix_path(&source.to_string_lossy())
        )
    );
    assert_eq!(fs::read_to_string(removed).unwrap(), "feature");
    // Successful background hooks prove the pipeline had a chance to run.
    std::thread::sleep(SLEEP_FOR_ABSENCE_CHECK);
    assert!(!switched.exists());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("post-switch"));
}

#[rstest]
#[case(false)]
#[case(true)]
fn test_merge_source_selected_by_path_or_current_alias(mut repo: TestRepo, #[case] current: bool) {
    let source = repo.add_feature();
    let directive = source.parent().unwrap().join("directive");
    fs::write(&directive, "").unwrap();
    let cwd = if current {
        source.as_path()
    } else {
        repo.root_path()
    };
    let selector = if current {
        "@".to_string()
    } else {
        source.to_string_lossy().into_owned()
    };
    let output = repo
        .wt_command()
        .current_dir(cwd)
        .args([
            "merge",
            "--branch",
            &selector,
            "--no-commit",
            "--no-hooks",
            "--yes",
        ])
        .env("WORKTRUNK_DIRECTIVE_CD_FILE", &directive)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    wait_for_worktree_removed(&source);
    let cd = fs::read_to_string(directive).unwrap();
    if current {
        assert_eq!(cd.trim(), repo.root_path().to_slash_lossy());
    } else {
        assert!(cd.is_empty());
    }
}

#[rstest]
fn test_merge_source_missing_worktree_is_non_mutating(mut repo: TestRepo) {
    repo.add_feature();
    repo.create_branch("no-checkout");
    let before = repo.git_output(&["show-ref"]);
    let output = repo
        .wt_command()
        .args(["merge", "--branch", "no-checkout", "--yes"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(repo.git_output(&["show-ref"]), before);
}

#[rstest]
fn test_merge_source_conflict_stays_in_source(mut repo: TestRepo) {
    let source = repo.add_feature();
    repo.commit_in_worktree(&source, "conflict.txt", "source", "Source conflict");
    repo.commit_in_worktree(
        repo.root_path(),
        "conflict.txt",
        "target",
        "Target conflict",
    );
    let before = repo.git_output(&["rev-parse", "main"]);
    let output = repo
        .wt_command()
        .args([
            "merge",
            "--branch",
            "feature",
            "--no-commit",
            "--no-hooks",
            "--yes",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    use ansi_str::AnsiStr;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let source_line = stderr
        .lines()
        .find(|line| line.contains("Merge source @"))
        .unwrap();
    insta::assert_snapshot!(source_line.ansi_strip(), @"○ Merge source @ _REPO_.feature");
    assert!(git_in(&repo, &source, &["status", "--porcelain"]).contains("AA conflict.txt"));
    assert_eq!(repo.git_output(&["rev-parse", "main"]), before);
    assert_eq!(repo.git_output(&["status", "--porcelain"]), "");
    assert!(source.exists());
}

#[rstest]
fn test_merge_source_respects_dirty_target(mut repo: TestRepo) {
    let source = repo.add_feature();
    fs::write(
        repo.root_path().join("feature.txt"),
        "untracked target work",
    )
    .unwrap();
    let before = repo.git_output(&["rev-parse", "main"]);
    let output = repo
        .wt_command()
        .args([
            "merge",
            "--branch",
            "feature",
            "--no-commit",
            "--no-hooks",
            "--yes",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(repo.git_output(&["rev-parse", "main"]), before);
    assert_eq!(
        fs::read_to_string(repo.root_path().join("feature.txt")).unwrap(),
        "untracked target work"
    );
    assert!(source.exists());
}

#[rstest]
fn test_merge_source_scrubs_inherited_git_context(mut repo: TestRepo) {
    let source = repo.add_feature();
    repo.commit_in_worktree(&source, "second.txt", "second", "Second source commit");
    repo.commit_in_worktree(repo.root_path(), "target.txt", "target", "Advance target");
    let caller = repo.add_worktree("caller");
    let gitdir = git_in(&repo, &caller, &["rev-parse", "--absolute-git-dir"]);
    let caller_head = git_in(&repo, &caller, &["rev-parse", "HEAD"]);
    let output = repo
        .wt_command()
        .current_dir(&caller)
        .args([
            "merge",
            "--branch",
            "feature",
            "--no-hooks",
            "--no-remove",
            "--yes",
        ])
        .env("GIT_DIR", &gitdir)
        .env("GIT_WORK_TREE", &caller)
        .env("GIT_INDEX_FILE", Path::new(&gitdir).join("index"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        repo.git_output(&["rev-parse", "main"]),
        git_in(&repo, &source, &["rev-parse", "HEAD"])
    );
    assert_eq!(git_in(&repo, &caller, &["rev-parse", "HEAD"]), caller_head);
    assert_eq!(git_in(&repo, &caller, &["status", "--porcelain"]), "");
    assert_eq!(repo.git_output(&["status", "--porcelain"]), "");
    assert!(source.exists());
}

#[rstest]
fn test_merge_source_squash_prompt_reads_source_index(mut repo: TestRepo) {
    let source = repo.add_feature();
    repo.commit_in_worktree(&source, "second.txt", "second", "Second source commit");
    fs::write(source.join("pending.txt"), "selected-source-content").unwrap();
    let caller = repo.add_worktree("caller");
    fs::write(caller.join("caller.txt"), "invoking-worktree-content").unwrap();
    repo.run_git_in(&caller, &["add", "caller.txt"]);
    let prompt = caller.parent().unwrap().join("squash-prompt");
    let command = format!(
        "cat > '{}'; echo 'Commit selected source'",
        prompt.to_slash_lossy()
    );
    repo.write_test_config(&format!(
        "[commit.generation]\ncommand = {}\n",
        toml::Value::String(command)
    ));
    let output = repo
        .wt_command()
        .current_dir(&caller)
        .args([
            "merge",
            "--branch",
            "feature",
            "--no-remove",
            "--no-hooks",
            "--yes",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let prompt = fs::read_to_string(prompt).unwrap();
    assert!(prompt.contains("selected-source-content"), "{prompt}");
    assert!(!prompt.contains("invoking-worktree-content"), "{prompt}");
    assert_eq!(
        repo.git_output(&["log", "-1", "--format=%s", "main"]),
        "Commit selected source"
    );
    assert_eq!(
        git_in(&repo, &caller, &["status", "--porcelain"]),
        "A  caller.txt"
    );
}

#[rstest]
#[case(false)]
#[case(true)]
fn test_merge_source_refuses_dirty_or_detached_source(mut repo: TestRepo, #[case] detached: bool) {
    let source = repo.add_feature();
    if detached {
        repo.run_git_in(&source, &["checkout", "--detach"]);
    } else {
        fs::write(source.join("pending.txt"), "uncommitted source work").unwrap();
    }
    let before = repo.git_output(&["show-ref"]);
    let output = repo
        .wt_command()
        .args([
            "merge",
            "--branch",
            source.to_str().unwrap(),
            "--no-commit",
            "--yes",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(repo.git_output(&["show-ref"]), before);
    assert!(source.exists());
    if !detached {
        assert_eq!(
            fs::read_to_string(source.join("pending.txt")).unwrap(),
            "uncommitted source work"
        );
    }
}

#[rstest]
fn test_merge_source_locked_worktree_is_preserved(mut repo: TestRepo) {
    let source = repo.add_feature();
    repo.run_git(&["worktree", "lock", source.to_str().unwrap()]);
    let output = repo
        .wt_command()
        .args([
            "merge",
            "--branch",
            "feature",
            "--no-commit",
            "--no-hooks",
            "--yes",
            "--format=json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["removed"], false);
    assert_eq!(
        repo.git_output(&["rev-parse", "main"]),
        git_in(&repo, &source, &["rev-parse", "HEAD"])
    );
    assert!(source.join("feature.txt").exists());
}

#[rstest]
fn test_merge_source_conflict_check_ignores_inherited_caller_head(mut repo: TestRepo) {
    repo.commit_in_worktree(repo.root_path(), "common.txt", "base", "Common base");
    let source = repo.add_feature();
    let caller = repo.add_worktree("caller");
    repo.commit_in_worktree(&caller, "common.txt", "caller", "Caller-only change");
    fs::write(
        repo.root_path().join("common.txt"),
        "uncommitted target work",
    )
    .unwrap();
    let gitdir = git_in(&repo, &caller, &["rev-parse", "--absolute-git-dir"]);
    let output = repo
        .wt_command()
        .current_dir(&caller)
        .args([
            "merge",
            "--branch",
            "feature",
            "--no-commit",
            "--no-hooks",
            "--no-remove",
            "--yes",
        ])
        .env("GIT_DIR", gitdir)
        .env("GIT_WORK_TREE", &caller)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        repo.git_output(&["rev-parse", "main"]),
        git_in(&repo, &source, &["rev-parse", "HEAD"])
    );
    assert_eq!(
        fs::read_to_string(repo.root_path().join("common.txt")).unwrap(),
        "uncommitted target work"
    );
}

#[rstest]
#[case(true)]
#[case(false)]
fn test_merge_source_preserves_nested_worktree(mut repo: TestRepo, #[case] invoke_nested: bool) {
    repo.commit_in_worktree(
        repo.root_path(),
        ".gitignore",
        ".worktrees/\n",
        "Ignore nested worktrees",
    );
    let source = repo.add_feature();
    let nested = repo.add_worktree_at_path("nested", &source.join(".worktrees/nested"));
    fs::write(nested.join("staged.txt"), "staged nested work").unwrap();
    repo.run_git_in(&nested, &["add", "staged.txt"]);
    fs::write(nested.join("precious.txt"), "untracked nested work").unwrap();
    let before = git_in(&repo, &nested, &["status", "--porcelain"]);
    let nested_head = git_in(&repo, &nested, &["rev-parse", "HEAD"]);
    assert_eq!(git_in(&repo, &source, &["status", "--porcelain"]), "");
    let cwd = if invoke_nested {
        nested.as_path()
    } else {
        repo.root_path()
    };
    let output = repo
        .wt_command()
        .current_dir(cwd)
        .args([
            "merge",
            "--branch",
            "feature",
            "--no-commit",
            "--no-hooks",
            "--yes",
            "--format=json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["removed"], false);
    assert!(String::from_utf8_lossy(&output.stderr).contains("contains worktree"));
    assert_eq!(
        repo.git_output(&["rev-parse", "main"]),
        git_in(&repo, &source, &["rev-parse", "HEAD"])
    );
    assert_eq!(git_in(&repo, &nested, &["status", "--porcelain"]), before);
    assert_eq!(git_in(&repo, &nested, &["rev-parse", "HEAD"]), nested_head);
    assert_eq!(
        fs::read_to_string(nested.join("precious.txt")).unwrap(),
        "untracked nested work"
    );
}

#[rstest]
#[case(Some("feature"))]
#[case(Some("@"))]
#[case(None)]
fn test_merge_source_rechecks_nested_worktree_after_pre_remove(
    mut repo: TestRepo,
    #[case] selector: Option<&str>,
) {
    repo.commit_in_worktree(
        repo.root_path(),
        ".gitignore",
        ".worktrees/\n",
        "Ignore nested worktrees",
    );
    let source = repo.add_feature();
    fs::create_dir_all(source.join(".config")).unwrap();
    fs::write(source.join(".config/wt.toml"), r#"pre-remove = "git worktree add --detach .worktrees/late && echo precious > .worktrees/late/precious.txt"
"#).unwrap();
    repo.run_git_in(&source, &["add", ".config/wt.toml"]);
    repo.run_git_in(&source, &["commit", "-m", "Add pre-remove hook"]);
    let directive = repo.home_path().join("directive");
    fs::write(&directive, "").unwrap();
    let mut command = repo.wt_command();
    command
        .args(["merge", "--no-commit", "--yes"])
        .env("WORKTRUNK_DIRECTIVE_CD_FILE", &directive);
    if selector != Some("feature") {
        command.current_dir(&source);
    }
    if let Some(selector) = selector {
        command.args(["--branch", selector]);
    }
    let output = command.output().unwrap();
    assert_eq!(fs::read_to_string(directive).unwrap(), "");
    assert!(
        !output.status.success(),
        "cleanup must refuse a new nested worktree: {output:?}"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("contains worktree"));
    let nested = source.join(".worktrees/late");
    assert_eq!(
        fs::read_to_string(nested.join("precious.txt"))
            .unwrap()
            .trim(),
        "precious"
    );
    assert_eq!(
        repo.git_output(&["rev-parse", "main"]),
        git_in(&repo, &source, &["rev-parse", "HEAD"])
    );
    assert!(git_in(&repo, &nested, &["status", "--porcelain"]).contains("precious.txt"));
}

#[rstest]
#[case(false)]
#[case(true)]
fn test_merge_source_current_bare_worktree_emits_directory_change(#[case] explicit_source: bool) {
    let mut repo = TestRepo::bare();
    repo.write_test_config("");
    let main = repo.root_path().parent().unwrap().join("repo.main");
    repo.run_git(&[
        "worktree",
        "add",
        "--orphan",
        "-b",
        "main",
        main.to_str().unwrap(),
    ]);
    repo.commit_in_worktree(&main, "initial.txt", "initial", "Initial commit");
    let source = repo.add_feature();
    let directive = main.parent().unwrap().join("directive");
    fs::write(&directive, "").unwrap();
    let mut command = repo.wt_command();
    command
        .current_dir(&source)
        .args(["merge", "main", "--no-commit", "--no-hooks", "--yes"])
        .env("WORKTRUNK_DIRECTIVE_CD_FILE", &directive);
    if explicit_source {
        command.args(["--branch", "@"]);
    }
    let output = command.output().unwrap();
    assert!(output.status.success(), "{output:?}");
    wait_for_worktree_removed(&source);
    assert_eq!(
        fs::read_to_string(directive).unwrap().trim(),
        main.to_slash_lossy()
    );
}

/// A topology read can wait behind another registry teardown. A write made
/// during that read must still be seen by the final dirty-worktree gate.
#[cfg(unix)]
#[rstest]
#[case(false)]
#[case(true)]
fn test_merge_source_checks_dirt_after_final_topology_read(
    mut repo: TestRepo,
    #[case] source_is_current: bool,
) {
    use std::os::unix::fs::PermissionsExt;

    let source = repo.add_feature();
    let phase = repo.home_path().join("pre-remove-ran");
    let reached = repo.home_path().join("topology-read-ran");
    let late_file = source.join("late-untracked.txt");
    fs::create_dir_all(source.join(".config")).unwrap();
    let hook = format!(
        "touch {}",
        shell_escape::unix::escape(phase.to_string_lossy())
    );
    fs::write(
        source.join(".config/wt.toml"),
        format!("pre-remove = {}\n", serde_json::to_string(&hook).unwrap()),
    )
    .unwrap();
    repo.run_git_in(&source, &["add", ".config/wt.toml"]);
    repo.run_git_in(&source, &["commit", "-m", "Add pre-remove marker"]);

    let wrapper = repo.home_path().join("git-wrapper");
    fs::create_dir_all(&wrapper).unwrap();
    let real_git = which::which("git").unwrap();
    let script = format!(
        r#"#!/bin/sh
if [ "$1 $2" = 'worktree list' ] && [ -f {phase} ] && [ ! -f {reached} ]; then
  printf '%s' 'written during topology read' > {late_file}
  touch {reached}
fi
exec {real_git} "$@"
"#,
        phase = shell_escape::unix::escape(phase.to_string_lossy()),
        reached = shell_escape::unix::escape(reached.to_string_lossy()),
        late_file = shell_escape::unix::escape(late_file.to_string_lossy()),
        real_git = shell_escape::unix::escape(real_git.to_string_lossy()),
    );
    let wrapper_git = wrapper.join("git");
    fs::write(&wrapper_git, script).unwrap();
    fs::set_permissions(&wrapper_git, fs::Permissions::from_mode(0o755)).unwrap();
    let mut path = vec![wrapper];
    path.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let mut command = repo.wt_command();
    command
        .env("PATH", std::env::join_paths(path).unwrap())
        .args([
            "merge",
            "--branch",
            if source_is_current { "@" } else { "feature" },
            "--no-commit",
            "--yes",
        ]);
    if source_is_current {
        command.current_dir(&source);
    }
    let output = command.output().unwrap();
    assert!(
        reached.exists(),
        "final topology recheck was not reached: {output:?}"
    );
    assert!(
        !output.status.success(),
        "late write must block cleanup: {output:?}"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("uncommitted changes"));
    assert_eq!(
        fs::read_to_string(&late_file).unwrap(),
        "written during topology read"
    );
    assert_eq!(
        repo.git_output(&["rev-parse", "main"]),
        git_in(&repo, &source, &["rev-parse", "HEAD"])
    );
}
