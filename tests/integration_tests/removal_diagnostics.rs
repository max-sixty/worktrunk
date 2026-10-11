//! Removal must distinguish an ineligible checkout from a failed safety query,
//! preserve filesystem causes, and identify cleanup failures after a real merge.

use std::fs;
use std::path::PathBuf;

use ansi_str::AnsiStr;
use rstest::rstest;

use crate::common::{TestRepo, repo, setup_snapshot_settings};

fn unreadable_registration(repo: &TestRepo) -> (PathBuf, String) {
    let registration = repo.root_path().join(".git/worktrees/broken");
    fs::create_dir_all(registration.join("gitdir")).unwrap();
    let registration = fs::canonicalize(registration).unwrap();
    let cause = fs::read_to_string(registration.join("gitdir"))
        .unwrap_err()
        .to_string();
    (registration, cause)
}

#[rstest]
#[case(false)]
#[case(true)]
fn test_prune_reports_unreadable_registry(mut repo: TestRepo, #[case] dry_run: bool) {
    let merged = repo.add_worktree("merged");
    let before = repo.git_output(&["rev-parse", "merged"]);
    let (registration, cause) = unreadable_registration(&repo);
    let mut command = repo.wt_command();
    command.args(["step", "prune", "--yes", "--foreground", "--min-age=0s"]);
    if dry_run {
        command.arg("--dry-run");
    }
    let output = command.output().unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(merged.exists());
    assert_eq!(repo.git_output(&["rev-parse", "merged"]), before);
    assert!(registration.join("gitdir").is_dir());
    let stderr = String::from_utf8_lossy(&output.stderr)
        .ansi_strip()
        .into_owned();
    assert!(stderr.contains(&cause), "the actual I/O cause must survive");
    let diagnostic = stderr.replace(&cause, "[IO_ERROR]");
    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();
    insta::allow_duplicates! {
        insta::assert_snapshot!(diagnostic, @"
        ✗ checking branch integration
          Failed to read worktree registration @ _REPO_/.git/worktrees/broken
          [IO_ERROR]
        ");
    }
}

#[rstest]
fn test_remove_reports_unreadable_registry(mut repo: TestRepo) {
    let merged = repo.add_worktree("merged");
    let before = repo.git_output(&["rev-parse", "merged"]);
    let (registration, cause) = unreadable_registration(&repo);
    let output = repo
        .wt_command()
        .args(["remove", "--foreground", "--no-hooks", "merged"])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(merged.exists());
    assert_eq!(repo.git_output(&["rev-parse", "merged"]), before);
    assert!(registration.join("gitdir").is_dir());
    let stderr = String::from_utf8_lossy(&output.stderr)
        .ansi_strip()
        .into_owned();
    assert!(stderr.contains(&cause), "the actual I/O cause must survive");
    let diagnostic = stderr.replace(&cause, "[IO_ERROR]");
    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();
    insta::assert_snapshot!(diagnostic, @"
    ✗ Failed to read worktree registration @ _REPO_/.git/worktrees/broken
      [IO_ERROR]
    ");
}

#[rstest]
fn test_merge_identifies_success_before_cleanup_failure(mut repo: TestRepo) {
    let source = repo.add_feature();
    let source_head = repo.git_output(&["rev-parse", "feature"]);
    let target_before = repo.git_output(&["rev-parse", "main"]);
    assert_ne!(source_head, target_before);
    let (registration, cause) = unreadable_registration(&repo);
    let output = repo
        .wt_command()
        .args([
            "merge",
            "--branch",
            "feature",
            "--no-commit",
            "--no-rebase",
            "--no-hooks",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(repo.git_output(&["rev-parse", "main"]), source_head);
    assert_eq!(repo.git_output(&["rev-parse", "feature"]), source_head);
    assert!(source.exists());
    assert_eq!(
        fs::read_to_string(source.join("feature.txt")).unwrap(),
        "feature content"
    );
    assert!(registration.join("gitdir").is_dir());
    let stderr = String::from_utf8_lossy(&output.stderr)
        .ansi_strip()
        .into_owned();
    assert!(stderr.contains(&cause), "the actual I/O cause must survive");
    let diagnostic = stderr.replace(&cause, "[IO_ERROR]");
    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();
    insta::assert_snapshot!(diagnostic, @"
    ◎ Merging 1 commit to main @ 36ee216 (no commit/squash/rebase needed)
      * 36ee216 Add feature file
       feature.txt | 1 +
       1 file changed, 1 insertion(+)
    ✓ Merged to main (1 commit, 1 file, +1)
    ○ Merge succeeded; cleanup failed
    ○ Merge source @ _REPO_.feature
    ✗ Failed to read worktree registration @ _REPO_/.git/worktrees/broken
      [IO_ERROR]
    ");
}

/// A successful superproject merge cannot prove a submodule's only local
/// commit disposable, even after deinitialization hides its checkout.
#[rstest]
#[case::initialized(false)]
#[case::deinitialized(true)]
fn test_merge_preserves_submodule_history_and_runs_post_merge(
    mut repo: TestRepo,
    #[case] deinitialized: bool,
) {
    let origin = repo.root_path().parent().unwrap().join("sub-origin");
    fs::create_dir(&origin).unwrap();
    repo.run_git_in(&origin, &["init", "-b", "main"]);
    repo.run_git_in(
        &origin,
        &["commit", "--allow-empty", "-m", "Submodule base"],
    );
    repo.run_git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        origin.to_str().unwrap(),
        "sub",
    ]);
    repo.write_test_config(
        r#"pre-remove = "exit 91"
post-merge = "printf '%s' '{{ branch }}:{{ target }}' > post-merge-ran.txt"
"#,
    );
    repo.run_git(&["add", "."]);
    repo.run_git(&["commit", "-m", "Add submodule and hooks"]);
    let source = repo.add_worktree("feature");
    repo.run_git_in(
        &source,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "update",
            "--init",
        ],
    );
    repo.run_git_in(
        &source.join("sub"),
        &["commit", "--allow-empty", "-m", "Sole local history"],
    );
    let submodule_head = {
        let output = repo
            .git_command()
            .current_dir(source.join("sub"))
            .args(["rev-parse", "HEAD"])
            .run()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    let remote_object = repo
        .git_command()
        .current_dir(&origin)
        .args(["cat-file", "-t", &submodule_head])
        .run()
        .unwrap();
    assert!(
        !remote_object.status.success(),
        "history must be solely local"
    );
    repo.run_git_in(&source, &["add", "sub"]);
    repo.run_git_in(&source, &["commit", "-m", "Record sole submodule history"]);
    if deinitialized {
        repo.run_git_in(&source, &["submodule", "deinit", "--all"]);
    }
    let source_head = repo.git_output(&["rev-parse", "feature"]);
    let target_before = repo.git_output(&["rev-parse", "main"]);
    assert_ne!(source_head, target_before);
    let registration = {
        let output = repo
            .git_command()
            .current_dir(&source)
            .args(["rev-parse", "--absolute-git-dir"])
            .run()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
    };
    let index = fs::read(registration.join("index")).unwrap();
    let output = repo
        .wt_command()
        .args([
            "merge",
            "--branch",
            "feature",
            "--yes",
            "--no-commit",
            "--no-rebase",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(repo.git_output(&["rev-parse", "main"]), source_head);
    assert_eq!(repo.git_output(&["rev-parse", "feature"]), source_head);
    assert!(source.exists());
    assert_eq!(fs::read(registration.join("index")).unwrap(), index);
    let object = repo
        .git_command()
        .args([
            "--git-dir",
            registration.join("modules/sub").to_str().unwrap(),
            "cat-file",
            "-t",
            &submodule_head,
        ])
        .run()
        .unwrap();
    assert!(
        object.status.success(),
        "sole submodule history must survive: {object:?}"
    );
    assert_eq!(String::from_utf8(object.stdout).unwrap().trim(), "commit");
    let marker = repo.root_path().join("post-merge-ran.txt");
    crate::common::wait_for_file_content(&marker);
    assert_eq!(fs::read_to_string(marker).unwrap(), "feature:main");
    let stderr = String::from_utf8_lossy(&output.stderr)
        .ansi_strip()
        .into_owned();
    let mut settings = setup_snapshot_settings(&repo);
    settings.add_filter(&source_head.trim()[..7], "[COMMIT]");
    let _guard = settings.bind_to_scope();
    insta::allow_duplicates! {
        insta::assert_snapshot!(stderr, @"
        ◎ Merging 1 commit to main @ [COMMIT] (no commit/squash/rebase needed)
          * [COMMIT] Record sole submodule history
           sub | 2 +-
           1 file changed, 1 insertion(+), 1 deletion(-)
        ✓ Merged to main (1 commit, 1 file, +1, -1)
        ○ Worktree preserved (submodule repositories)
        ◎ Running post-merge: user
        ");
    }
}
