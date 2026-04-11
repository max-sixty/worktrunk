//! Integration tests for `wt step <alias>`

use crate::common::{
    TestRepo, configure_directive_file, directive_file, make_snapshot_cmd, repo,
    setup_snapshot_settings, wt_bin,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::io::Write;
use std::process::Stdio;

/// Alias from project config runs with template expansion (--yes bypasses approval)
#[rstest]
fn test_step_alias_from_project_config(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
hello = "echo Hello from {{ branch }}"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["hello", "--yes"],
        Some(&feature_path),
    ));
}

/// --dry-run shows the expanded command without running it (no approval needed)
#[rstest]
fn test_step_alias_dry_run(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
hello = "echo Hello from {{ branch }}"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    // No --yes needed: --dry-run skips approval
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["hello", "--dry-run"],
        Some(&feature_path),
    ));
}

/// Unknown alias shows error with available aliases
#[rstest]
fn test_step_alias_unknown_with_available(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
hello = "echo Hello"
deploy = "make deploy"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["nonexistent"],
        Some(&feature_path),
    ));
}

/// Typo in alias name suggests the closest match
#[rstest]
fn test_step_alias_did_you_mean(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
deploy = "make deploy"
hello = "echo Hello"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["deplyo"],
        Some(&feature_path),
    ));
}

/// Unknown step command with no aliases configured
#[rstest]
fn test_step_alias_unknown_no_aliases(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["deploy"],
        Some(&feature_path),
    ));
}

/// --var flag adds extra template variables (--yes bypasses approval)
#[rstest]
fn test_step_alias_with_var(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
greet = "echo Hello {{ name }} from {{ branch }}"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["greet", "--dry-run", "--var", "name=World", "--yes"],
        Some(&feature_path),
    ));
}

/// Alias command failure propagates exit code (--yes bypasses approval)
#[rstest]
fn test_step_alias_exit_code(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
fail = "exit 42"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["fail", "--yes"],
        Some(&feature_path),
    ));
}

/// Alias from user config works
#[rstest]
fn test_step_alias_from_user_config(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");
    repo.write_test_config(
        r#"
[aliases]
greet = "echo Greetings from {{ branch }}"
"#,
    );

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["greet"],
        Some(&feature_path),
    ));
}

/// Shadowed aliases are filtered from the "available" list in error messages
#[rstest]
fn test_step_alias_shadows_builtin(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
commit = "echo custom-commit"
hello = "echo hello"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    // "commit" is shadowed by the built-in and should not appear in available list
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["nonexistent"],
        Some(&feature_path),
    ));
}

/// Multiple shadowed aliases use plural grammar
#[rstest]
fn test_step_alias_shadows_builtin_plural(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
commit = "echo custom-commit"
rebase = "echo custom-rebase"
hello = "echo hello"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["nonexistent"],
        Some(&feature_path),
    ));
}

/// User config aliases merge with project config aliases
#[rstest]
fn test_step_alias_merge_user_and_project(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
project-cmd = "echo from-project"
shared = "echo project-version"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");
    repo.write_test_config(
        r#"
[aliases]
user-cmd = "echo from-user"
shared = "echo user-version"
"#,
    );

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    // User alias available
    assert_cmd_snapshot!(
        "user_alias",
        make_snapshot_cmd(
            &repo,
            "step",
            &["user-cmd", "--dry-run"],
            Some(&feature_path),
        )
    );

    // Project alias available (--yes bypasses approval for project-config aliases)
    assert_cmd_snapshot!(
        "project_alias",
        make_snapshot_cmd(
            &repo,
            "step",
            &["project-cmd", "--dry-run", "--yes"],
            Some(&feature_path),
        )
    );

    // Both run on collision: user first, then project (append semantics)
    assert_cmd_snapshot!(
        "user_and_project_append",
        make_snapshot_cmd(&repo, "step", &["shared", "--dry-run"], Some(&feature_path),)
    );
}

/// Both global and per-project user aliases execute in order on name collision.
///
/// Uses project config (`.config/wt.toml`) + user config to verify the
/// project-vs-user append, since `test_aliases_accessor_appends_on_collision`
/// already covers the user-config internal append via unit test.
#[rstest]
fn test_alias_append_executes_both(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
greet = "echo PROJECT"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");
    repo.write_test_config(
        r#"
[aliases]
greet = "echo USER"
"#,
    );

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    // Both commands execute: user first, then project (--yes approves project alias)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["greet", "--yes"],
        Some(&feature_path),
    ));
}

// ============================================================================
// Approval tests
// ============================================================================

/// Helper for alias approval snapshot tests
fn snapshot_alias_approval(
    test_name: &str,
    repo: &TestRepo,
    alias_args: &[&str],
    approve: bool,
    cwd: Option<&std::path::Path>,
) {
    let mut cmd = make_snapshot_cmd(repo, "step", alias_args, cwd);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().unwrap();

    {
        let stdin = child.stdin.as_mut().unwrap();
        let response = if approve { b"y\n" } else { b"n\n" };
        stdin.write_all(response).unwrap();
    }

    let output = child.wait_with_output().unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!(
        "exit_code: {}\n----- stdout -----\n{}\n----- stderr -----\n{}",
        output.status.code().unwrap_or(-1),
        stdout,
        stderr
    );

    insta::assert_snapshot!(test_name, combined);
}

/// Project-config alias prompts for approval in non-TTY (fails with hint)
#[rstest]
fn test_alias_approval_project_config_prompts(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
deploy = "echo deploying {{ branch }}"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    // Without --yes, project alias triggers approval prompt (fails in non-TTY)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["deploy"],
        Some(&feature_path),
    ));
}

/// Already-approved project-config alias runs without re-prompting
#[rstest]
fn test_alias_approval_already_approved(mut repo: TestRepo) {
    // Remove origin so worktrunk uses directory name as project identifier
    repo.run_git(&["remote", "remove", "origin"]);

    repo.write_project_config(
        r#"
[aliases]
deploy = "echo deploying {{ branch }}"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    // Pre-approve the alias command
    repo.write_test_approvals(&format!(
        r#"[projects.'{}']
approved-commands = ["echo deploying {{{{ branch }}}}"]
"#,
        repo.project_id()
    ));

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    // Should run without prompting
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["deploy"],
        Some(&feature_path),
    ));
}

/// User-config alias skips approval entirely
#[rstest]
fn test_alias_approval_user_config_skips(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");
    repo.write_test_config(
        r#"
[aliases]
deploy = "echo deploying from user config"
"#,
    );

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    // User alias runs without approval
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["deploy"],
        Some(&feature_path),
    ));
}

/// User override of project alias skips approval (user is trusted)
#[rstest]
fn test_alias_approval_user_and_project_both_need_approval(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
deploy = "echo project deploy"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");
    repo.write_test_config(
        r#"
[aliases]
deploy = "echo user deploy"
"#,
    );

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    // Both run with --yes: user first, then project (project needs approval)
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["deploy", "--yes"],
        Some(&feature_path),
    ));
}

/// --yes bypasses approval for project-config alias without saving
#[rstest]
fn test_alias_approval_yes_bypasses(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
deploy = "echo deploying"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    // First run with --yes succeeds
    assert_cmd_snapshot!(
        "alias_approval_yes_first_run",
        make_snapshot_cmd(&repo, "step", &["deploy", "--yes"], Some(&feature_path),)
    );

    // Second run without --yes should still prompt (--yes doesn't save approval)
    assert_cmd_snapshot!(
        "alias_approval_yes_second_run_prompts",
        make_snapshot_cmd(&repo, "step", &["deploy"], Some(&feature_path),)
    );
}

// ============================================================================
// Directive file passthrough
// ============================================================================

/// `wt step <alias>` passes the parent's `WORKTRUNK_DIRECTIVE_FILE` through to
/// the alias subprocess so inner `wt switch --create` calls can land the user
/// in the new worktree.
///
/// Regression test for #2075: without the passthrough, an alias that wraps
/// `wt switch --create` prints the "shell integration not installed" hint and
/// the parent shell never `cd`s into the new worktree.
#[rstest]
fn test_alias_passes_directive_file_to_subprocess(repo: TestRepo) {
    repo.commit("initial");

    // Escape the wt binary path for embedding in a sh -c command string.
    // Test temp paths never contain single quotes.
    let wt = wt_bin();
    let wt_str = wt.to_string_lossy();
    assert!(
        !wt_str.contains('\''),
        "wt binary path should not contain single quotes: {wt_str}"
    );
    // Double backslashes so the Windows path (e.g. `D:\a\worktrunk\...\wt.exe`)
    // parses as literal characters inside a TOML basic string rather than
    // being interpreted as escape sequences (`\a`, `\w`, ...).
    let wt_toml = wt_str.replace('\\', "\\\\");

    // Alias body invokes the test wt binary directly (PATH lookup in the
    // subprocess shell wouldn't find it).
    repo.write_test_config(&format!(
        r#"
[aliases]
new-branch = "'{wt_toml}' switch --create alias-created"
"#
    ));

    let (directive_path, _guard) = directive_file();

    let mut cmd = repo.wt_command();
    configure_directive_file(&mut cmd, &directive_path);
    cmd.args(["step", "new-branch"]);
    let output = cmd.output().unwrap();

    assert!(
        output.status.success(),
        "wt step new-branch failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let directives = std::fs::read_to_string(&directive_path).unwrap_or_default();
    assert!(
        directives.contains("cd '"),
        "alias wrapping `wt switch --create` should write a cd directive to the \
         parent directive file, got: {directives:?}"
    );
    assert!(
        directives.contains("alias-created"),
        "cd directive should target the new worktree (alias-created), got: {directives:?}"
    );

    // Stderr should NOT contain the "shell integration not installed" hint
    // — that hint is what appears when the inner wt can't find the directive
    // file, which is exactly the bug this test guards against.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("shell integration"),
        "inner wt should not warn about shell integration being uninstalled, got: {stderr}",
    );
}

/// Declining approval prevents alias execution
#[rstest]
fn test_alias_approval_decline(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
deploy = "echo deploying"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    snapshot_alias_approval(
        "alias_approval_decline",
        &repo,
        &["deploy"],
        false,
        Some(&feature_path),
    );
}
