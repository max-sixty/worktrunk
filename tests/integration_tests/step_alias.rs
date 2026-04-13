//! Integration tests for `wt step <alias>`

use crate::common::{
    TestRepo, configure_directive_files, directive_files, make_snapshot_cmd, repo,
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

/// --key=value shorthand for --var key=value
#[rstest]
fn test_step_alias_with_shorthand_var(mut repo: TestRepo) {
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
        &["greet", "--dry-run", "--name=World", "--yes"],
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

/// `wt step <alias>` passes the parent's `WORKTRUNK_DIRECTIVE_CD_FILE` through
/// to the alias subprocess so inner `wt switch --create` calls can land the
/// user in the new worktree.
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
    let wt_toml = wt_str.replace('\\', r"\\");

    // Alias body invokes the test wt binary directly (PATH lookup in the
    // subprocess shell wouldn't find it).
    repo.write_test_config(&format!(
        r#"
[aliases]
new-branch = "'{wt_toml}' switch --create alias-created"
"#
    ));

    let (cd_path, exec_path, _guard) = directive_files();

    let mut cmd = repo.wt_command();
    configure_directive_files(&mut cmd, &cd_path, &exec_path);
    cmd.args(["step", "new-branch"]);
    let output = cmd.output().unwrap();

    assert!(
        output.status.success(),
        "wt step new-branch failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let cd_content = std::fs::read_to_string(&cd_path).unwrap_or_default();
    assert!(
        !cd_content.trim().is_empty(),
        "alias wrapping `wt switch --create` should write a path to the \
         CD directive file, got: {cd_content:?}"
    );
    assert!(
        cd_content.contains("alias-created"),
        "cd directive should target the new worktree (alias-created), got: {cd_content:?}"
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

/// Pipeline aliases announce their structure: named serial and concurrent
/// steps appear in the "Running alias" line, joined by `;` and `,`.
///
/// `WORKTRUNK_TEST_SERIAL_CONCURRENT=1` forces the concurrent step to run
/// commands sequentially (in declaration order) so the snapshot captures a
/// deterministic interleaving — analogous to how `RAYON_NUM_THREADS=1` is
/// used in `step_prune` tests.
#[rstest]
fn test_alias_pipeline_announcement(mut repo: TestRepo) {
    repo.write_test_config(
        r#"
[aliases]
deploy = [
    { install = "echo INSTALL" },
    { build = "echo BUILD", lint = "echo LINT" },
]
"#,
    );
    repo.commit("initial");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    let mut cmd = make_snapshot_cmd(&repo, "step", &["deploy"], Some(&feature_path));
    cmd.env("WORKTRUNK_TEST_SERIAL_CONCURRENT", "1");
    assert_cmd_snapshot!(cmd);
}

/// Concurrent alias steps (named table) execute all commands
#[rstest]
fn test_alias_concurrent_steps(mut repo: TestRepo) {
    // Named table form: commands run concurrently within the step
    repo.write_test_config(
        r#"
[aliases.build]
lint = "echo LINT"
test = "echo TEST"
"#,
    );
    repo.commit("initial");
    let feature_path = repo.add_worktree("feature");

    let mut cmd = repo.wt_command();
    cmd.args(["step", "build"]).current_dir(&feature_path);
    let output = cmd.output().unwrap();

    assert!(
        output.status.success(),
        "concurrent alias failed: stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    // Both commands should have run (order may vary due to concurrency)
    assert!(stderr.contains("LINT"), "expected LINT in output: {stderr}");
    assert!(stderr.contains("TEST"), "expected TEST in output: {stderr}");
}

/// Concurrent alias commands have their output streamed with a per-command
/// colored prefix label (`{name} │ …`), so multiple children's lines remain
/// attributable even when they interleave.
#[rstest]
fn test_alias_concurrent_prefixes_output(mut repo: TestRepo) {
    repo.write_test_config(
        r#"
[aliases.build]
lint = "echo HELLO_LINT"
test = "echo HELLO_TEST"
"#,
    );
    repo.commit("initial");
    let feature_path = repo.add_worktree("feature");

    let mut cmd = repo.wt_command();
    cmd.args(["step", "build"])
        .current_dir(&feature_path)
        .env("NO_COLOR", "1");
    let output = cmd.output().unwrap();

    assert!(
        output.status.success(),
        "concurrent alias failed: stderr={}",
        String::from_utf8_lossy(&output.stderr),
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    // Each command must produce a line starting with its prefixed label and
    // separated by the box-drawing `│` that the executor emits. Whitespace
    // between label and `│` varies with padding-to-widest-label.
    for (label, body) in [("lint", "HELLO_LINT"), ("test", "HELLO_TEST")] {
        let has_prefixed_line = stderr
            .lines()
            .any(|l| l.starts_with(label) && l.contains('│') && l.contains(body));
        assert!(
            has_prefixed_line,
            "expected a line starting with '{label}' containing '{body}' and a '│' separator, got:\n{stderr}"
        );
    }
}

/// A failing concurrent step causes the alias to fail. Covers the error
/// propagation path in the `HookStep::Concurrent` join loop, complementing
/// the happy-path `test_alias_concurrent_steps` above.
#[rstest]
fn test_alias_concurrent_step_failure(repo: TestRepo) {
    repo.write_test_config(
        r#"
[aliases.check]
ok = "true"
fail = "exit 1"
"#,
    );
    repo.commit("initial");

    let mut cmd = repo.wt_command();
    cmd.args(["step", "check"]);
    let output = cmd.output().expect("wt step check failed to spawn");
    assert!(
        !output.status.success(),
        "wt step check should fail when a concurrent step exits non-zero"
    );
}

/// SIGINT sent to `wt step <alias>` while a concurrent group is mid-flight
/// must reach every child's process group and tear them all down — otherwise
/// Ctrl-C on a long-running concurrent alias would leave orphans behind.
///
/// We spawn the alias in its own process group, wait until BOTH children have
/// written their "start" marker (proving they're actually running concurrently),
/// send SIGINT to the group, then verify that the subsequent "done" markers
/// never appear — every child was interrupted.
#[rstest]
#[cfg(unix)]
fn test_alias_concurrent_receives_sigint(repo: TestRepo) {
    use crate::common::wait_for_file_content;
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    repo.write_test_config(
        r#"
[aliases.slow]
one = "sh -c 'echo start-one >> slow_one.log; sleep 30; echo done-one >> slow_one.log'"
two = "sh -c 'echo start-two >> slow_two.log; sleep 30; echo done-two >> slow_two.log'"
"#,
    );
    repo.commit("initial");

    let mut cmd = repo.wt_command();
    cmd.args(["step", "slow"]);
    cmd.current_dir(repo.root_path());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    cmd.process_group(0); // wt becomes leader of its own process group
    let mut child = cmd.spawn().expect("failed to spawn wt step slow");

    // Wait until BOTH children write their start marker — proves the group
    // is running concurrently before we send the signal.
    let one_log = repo.root_path().join("slow_one.log");
    let two_log = repo.root_path().join("slow_two.log");
    wait_for_file_content(&one_log);
    wait_for_file_content(&two_log);

    // SIGINT the wt process group (wt == leader). The concurrent executor's
    // signal forwarder must propagate it to every child's process group.
    let wt_pgid = Pid::from_raw(child.id() as i32);
    kill(Pid::from_raw(-wt_pgid.as_raw()), Signal::SIGINT)
        .expect("failed to send SIGINT to wt's process group");

    let status = child.wait().expect("failed to wait for wt");

    use std::os::unix::process::ExitStatusExt;
    assert!(
        status.signal() == Some(2) || status.code() == Some(130),
        "wt should exit from SIGINT (signal 2) or with code 130, got: {status:?}"
    );

    // Grace period — the killed children must NOT reach their "done" write.
    std::thread::sleep(std::time::Duration::from_millis(500));
    for log in [&one_log, &two_log] {
        let contents = std::fs::read_to_string(log).unwrap_or_default();
        assert!(
            !contents.contains("done"),
            "sibling child reached 'done' after SIGINT, log: {contents:?}"
        );
    }
}

/// A second SIGINT (user mashing Ctrl-C) must escalate to SIGKILL on every
/// child immediately — otherwise a child that traps SIGINT keeps the group
/// alive for up to N × 400ms of per-pgid escalation, with subsequent
/// presses silently discarded.
#[rstest]
#[cfg(unix)]
fn test_alias_concurrent_second_sigint_kills(repo: TestRepo) {
    use crate::common::wait_for_file_content;
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;

    // Both children trap SIGINT and sleep; first SIGINT does nothing
    // (graceful escalation to SIGTERM is also trapped), a second SIGINT
    // must SIGKILL the pgids and exit wt promptly.
    repo.write_test_config(
        r#"
[aliases.stubborn]
one = "sh -c 'trap \"\" INT TERM; echo start-one >> stubborn_one.log; sleep 30'"
two = "sh -c 'trap \"\" INT TERM; echo start-two >> stubborn_two.log; sleep 30'"
"#,
    );
    repo.commit("initial");

    let mut cmd = repo.wt_command();
    cmd.args(["step", "stubborn"]);
    cmd.current_dir(repo.root_path());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    cmd.process_group(0);
    let mut child = cmd.spawn().expect("failed to spawn wt step stubborn");

    wait_for_file_content(&repo.root_path().join("stubborn_one.log"));
    wait_for_file_content(&repo.root_path().join("stubborn_two.log"));

    let wt_pgid = Pid::from_raw(child.id() as i32);
    // First SIGINT — trapped by children; graceful path chews through
    // escalation serially.
    kill(Pid::from_raw(-wt_pgid.as_raw()), Signal::SIGINT).expect("failed to send first SIGINT");

    std::thread::sleep(std::time::Duration::from_millis(100));

    // Second SIGINT — impatient path should SIGKILL the whole tree now.
    kill(Pid::from_raw(-wt_pgid.as_raw()), Signal::SIGINT).expect("failed to send second SIGINT");

    let start = std::time::Instant::now();
    let _status = child.wait().expect("failed to wait for wt");
    let elapsed = start.elapsed();

    // With only graceful escalation (200ms × 2 grace windows × 2 pgids),
    // worst case would be ~800ms. The impatient SIGKILL should be faster
    // still. Give 3s headroom for slow CI without being so loose that a
    // regression (no SIGKILL on 2nd press) would slip through — that
    // regression would leave wt waiting ~60s for the sleeps to finish.
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "wt took too long to die after 2nd SIGINT; impatient path may not be firing: {elapsed:?}"
    );
}

/// Non-UTF-8 bytes and CRLF line endings in child output must not stall the
/// executor. Earlier code using `BufRead::lines()` returned `InvalidData` on
/// the first invalid byte and terminated the iterator, leaving the child's
/// pipe un-drained and `child.wait()` hanging forever. Also exercises the
/// `\r\n` strip path so trailing carriage returns don't render visibly.
#[rstest]
fn test_alias_concurrent_handles_non_utf8(repo: TestRepo) {
    // `printf` emits a raw 0xff byte (invalid as a lone UTF-8 sequence), a
    // CRLF-terminated line, then `yes` floods 50_000 more valid UTF-8 lines.
    // If the reader stopped at the bad byte, the pipe would fill and the
    // child would block — we'd time out.
    repo.write_test_config(
        r#"
[aliases.noisy]
mixed = "sh -c 'printf \"BEFORE\\n\\xff\\nCRLF-LINE\\r\\nAFTER\\n\"; yes PAYLOAD | head -n 50000'"
"#,
    );
    repo.commit("initial");

    let mut cmd = repo.wt_command();
    cmd.args(["step", "noisy"]);
    let output = cmd.output().expect("wt step noisy failed to spawn");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "alias should succeed despite non-UTF-8 byte, got: {status:?}\nlast 500 bytes of stderr: {tail}",
        status = output.status,
        tail = &stderr[stderr.len().saturating_sub(500)..],
    );

    // All clean lines plus all 50_000 flood lines must land — proves the
    // reader kept going past the invalid byte.
    assert!(stderr.contains("BEFORE"), "expected BEFORE in stderr");
    assert!(
        stderr.contains("AFTER"),
        "expected AFTER in stderr (reader stopped at the invalid byte)"
    );
    // CRLF line ending: the reader strips the trailing \r, so the visible
    // line is `CRLF-LINE` not `CRLF-LINE\r`. Any `\r` in the output would
    // indicate the strip didn't fire.
    assert!(
        stderr.contains("CRLF-LINE"),
        "expected CRLF-LINE (with the \\r stripped) in stderr"
    );
    assert!(
        !stderr.contains("CRLF-LINE\r"),
        "trailing \\r should have been stripped before printing"
    );
    assert_eq!(
        stderr.matches("PAYLOAD").count(),
        50_000,
        "expected 50000 PAYLOAD lines after the invalid byte, got {}",
        stderr.matches("PAYLOAD").count(),
    );
}

/// A concurrent child that produces a large volume of stdout must not
/// deadlock the executor — the reader thread has to keep draining the pipe so
/// the child can keep writing. We run two commands, each emitting ~400 KB, and
/// assert both streams land in stderr intact.
#[rstest]
fn test_alias_concurrent_large_output(repo: TestRepo) {
    // yes piped through head is a portable way to generate many lines fast.
    // Each command produces ~400 KB = 50_000 * ~8 bytes ("aaaa...\n" etc.).
    // If the reader thread were ever to stall, the child's stdout pipe would
    // fill (default ~64 KB) and the child would block forever — the test would
    // time out rather than produce a misleading pass.
    repo.write_test_config(
        r#"
[aliases.bulk]
first  = "yes 'FIRST-PAYLOAD-AAAAA' | head -n 50000"
second = "yes 'SECOND-PAYLOAD-BBBBB' | head -n 50000"
"#,
    );
    repo.commit("initial");

    let mut cmd = repo.wt_command();
    cmd.args(["step", "bulk"]);
    let output = cmd.output().expect("wt step bulk failed to spawn");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "concurrent alias with large output should exit 0, got: {status:?}\nlast 500 bytes of stderr: {tail}",
        status = output.status,
        tail = &stderr[stderr.len().saturating_sub(500)..],
    );

    // Count occurrences so we know both children's full output was streamed —
    // not truncated by a blocked pipe. 50_000 exact matches per payload.
    let first_count = stderr.matches("FIRST-PAYLOAD-AAAAA").count();
    let second_count = stderr.matches("SECOND-PAYLOAD-BBBBB").count();
    assert_eq!(
        first_count, 50_000,
        "expected 50000 occurrences of first payload in stderr, got {first_count}"
    );
    assert_eq!(
        second_count, 50_000,
        "expected 50000 occurrences of second payload in stderr, got {second_count}"
    );
}

/// Pipeline-form aliases (list of steps) run sequentially. A later step
/// referencing `{{ vars.X }}` must see vars set by an earlier step —
/// `expand_shell_template` reads `vars.*` fresh from git config on each call.
#[rstest]
fn test_alias_pipeline_vars_across_steps(repo: TestRepo) {
    repo.write_test_config(
        r#"
[aliases]
deploy = [
    "git config worktrunk.state.main.vars.target 'staging'",
    { publish = "echo target={{ vars.target }} > alias_lazy.txt" },
]
"#,
    );
    repo.commit("initial");

    let mut cmd = repo.wt_command();
    cmd.args(["step", "deploy"]);
    let output = cmd.output().expect("wt step deploy failed to spawn");
    assert!(
        output.status.success(),
        "wt step deploy failed: stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let marker = repo.root_path().join("alias_lazy.txt");
    let content = std::fs::read_to_string(&marker)
        .unwrap_or_else(|e| panic!("missing marker {marker:?}: {e}"));
    assert_eq!(
        content.trim(),
        "target=staging",
        "lazy step should see var set by prior serial step"
    );
}

/// `--dry-run` for a pipeline where a later step references `{{ vars.X }}`
/// set by an earlier step succeeds, mirroring the lazy execution path. The
/// unresolved `vars.*` reference is shown as the raw template since its value
/// isn't knowable until the earlier step actually runs.
#[rstest]
fn test_step_alias_dry_run_vars_across_steps(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
deploy = [
    "git config worktrunk.state.main.vars.target 'prod'",
    { publish = "echo deploying to {{ vars.target }}" },
]
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    // Must succeed: dry-run must not require vars.* to be resolvable.
    // --yes bypasses approval for project-config aliases.
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["deploy", "--dry-run", "--yes"],
        Some(&feature_path),
    ));
}

/// `--dry-run` still catches template syntax errors (e.g., `{{ vars..foo }}`)
/// even on the lazy path where `vars.*` rendering is skipped.
#[rstest]
fn test_step_alias_dry_run_catches_syntax_error(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
broken = "echo {{ vars..target }}"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let output = repo
        .wt_command()
        .args(["step", "broken", "--dry-run", "--yes"])
        .current_dir(&feature_path)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "dry-run should fail on syntax error; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("syntax error"),
        "expected 'syntax error' in stderr, got:\n{stderr}"
    );
}

/// `wt step` with no subcommand lists built-in steps plus configured aliases.
///
/// Skipped on Windows: clap renders `[experimental]` subcommand tags
/// differently (markdown escaping), same reason `tests/integration_tests/help.rs`
/// is Windows-gated.
#[cfg(not(windows))]
#[rstest]
fn test_step_list_with_aliases(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
deploy = "make deploy BRANCH={{ branch }}"
port = "echo http://localhost:{{ branch | hash_port }}"
squash = "this shadows the built-in"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &[], Some(&feature_path)));
}

/// `wt step` without configured aliases still works — just prints help.
#[cfg(not(windows))]
#[rstest]
fn test_step_list_no_aliases(mut repo: TestRepo) {
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "step", &[], Some(&feature_path)));
}

/// `wt step --help` includes the same Aliases section as bare `wt step`.
///
/// Without this, users running `--help` in the normal discovery flow would
/// see only built-in commands and miss their configured aliases.
#[cfg(not(windows))]
#[rstest]
fn test_step_help_includes_aliases(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
deploy = "make deploy BRANCH={{ branch }}"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["-h"],
        Some(&feature_path)
    ));
}

/// `wt step --help` must not emit deprecation warnings or write `.new`
/// migration files when the user config contains deprecated patterns —
/// help is a discovery surface, not an execution surface, and the user
/// will see those warnings from `wt config show` and normal commands.
#[cfg(not(windows))]
#[rstest]
fn test_step_help_silent_with_deprecated_user_config(repo: TestRepo) {
    // Deprecated `main_worktree` template variable — migrated to `repo`.
    repo.write_test_config(
        r#"worktree-path = "../{{ main_worktree }}.{{ branch }}"
"#,
    );
    let migration_file = repo.test_config_path().with_extension("toml.new");

    let output = repo.wt_command().args(["step", "--help"]).output().unwrap();

    assert!(
        output.status.success(),
        "step --help should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr, "",
        "step --help must emit no stderr on deprecated user config"
    );
    assert!(
        !migration_file.exists(),
        "step --help must not write .new migration file at {}",
        migration_file.display()
    );
}

/// `wt -C <other> step --help` lists aliases from `<other>`'s project config,
/// not from the process cwd. Without applying global options before the help
/// branch, the Aliases section was rendered from the wrong repo.
#[cfg(not(windows))]
#[rstest]
fn test_step_help_honors_dash_c(repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
xyzzy = "echo nothing happens"
"#,
    );
    repo.commit("Add alias config");
    let repo_path = repo.root_path().to_path_buf();

    // Invoke from a directory that is *not* inside the repo so the alias can
    // only be discovered via -C. Using the system temp dir keeps this
    // independent of the test's working directory.
    let cwd = std::env::temp_dir();
    let mut cmd = repo.wt_command();
    cmd.current_dir(&cwd)
        .args(["-C", repo_path.to_str().unwrap(), "step", "--help"]);
    let output = cmd.output().expect("failed to run wt");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("xyzzy"),
        "expected `xyzzy` alias to appear in `wt -C <repo> step --help` output\n\
         stdout:\n{stdout}\nstderr:\n{stderr}"
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
