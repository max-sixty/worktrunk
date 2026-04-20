//! Integration tests for `wt step <alias>`

use crate::common::{
    TestRepo, configure_directive_files, directive_files, make_snapshot_cmd,
    make_snapshot_cmd_with_global_flags, repo, setup_snapshot_settings, wt_bin,
};
use insta_cmd::assert_cmd_snapshot;
use rstest::rstest;
use std::io::Write;
use std::process::Stdio;

/// Alias from project config runs with template expansion (-y bypasses approval)
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

    assert_cmd_snapshot!(make_snapshot_cmd_with_global_flags(
        &repo,
        "step",
        &["hello"],
        Some(&feature_path),
        &["-y"],
    ));
}

/// `wt config alias dry-run <name>` shows the expanded command without running it
/// (no approval needed — preview never executes project commands).
#[rstest]
fn test_config_alias_dry_run(mut repo: TestRepo) {
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
        "config",
        &["alias", "dry-run", "hello"],
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

/// `--KEY=VALUE` binds to `{{ KEY }}` when the template references it.
#[rstest]
fn test_step_alias_binds_referenced_var(mut repo: TestRepo) {
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
        "config",
        &["alias", "dry-run", "greet", "--", "--name=World"],
        Some(&feature_path),
    ));
}

/// `--KEY VALUE` (space-separated) binds the same way `--KEY=VALUE` does
/// when KEY is referenced and VALUE doesn't look like a flag.
#[rstest]
fn test_step_alias_binds_space_separated_var(mut repo: TestRepo) {
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
        "config",
        &["alias", "dry-run", "greet", "--", "--name", "World"],
        Some(&feature_path),
    ));
}

/// `--KEY=VALUE` for a key the template doesn't reference forwards to
/// `{{ args }}` instead of binding silently.
#[rstest]
fn test_step_alias_unreferenced_key_forwards_to_args(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
run = "echo got {{ args }}"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd_with_global_flags(
        &repo,
        "run",
        &["--env=staging", "foo"],
        Some(&feature_path),
        &["-y"],
    ));
}

/// `--` is a literal-forward escape: every later token goes to `{{ args }}`,
/// so flag-shaped values that would normally bind are passed through verbatim.
#[rstest]
fn test_step_alias_double_dash_escape(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
run = "echo got {{ args }}"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd_with_global_flags(
        &repo,
        "run",
        &["--", "--env=staging", "literal"],
        Some(&feature_path),
        &["-y"],
    ));
}

/// `--KEY=VALUE` overrides built-in template variables. The user-supplied
/// value wins because `extra_refs` is applied after built-ins are seeded.
#[rstest]
fn test_step_alias_user_var_overshadows_builtin(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
show = "echo branch={{ branch }}"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd_with_global_flags(
        &repo,
        "show",
        &["--branch=override"],
        Some(&feature_path),
        &["-y"],
    ));
}

/// Multi-step pipeline: each step's `{{ KEY }}` references contribute to
/// the binding-eligible set, so a single invocation can bind both `env`
/// (referenced in step 1) and `region` (referenced in step 2).
#[rstest]
fn test_step_alias_multi_step_binds_across_pipeline(mut repo: TestRepo) {
    repo.write_test_config(
        r#"
[aliases]
deploy = [
    "echo step1 env={{ env }}",
    { publish = "echo step2 region={{ region }}" },
]
"#,
    );
    repo.commit("initial");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    let mut cmd = make_snapshot_cmd(
        &repo,
        "step",
        &["deploy", "--env=prod", "--region=us-east"],
        Some(&feature_path),
    );
    cmd.env("WORKTRUNK_TEST_SERIAL_CONCURRENT", "1");
    assert_cmd_snapshot!(cmd);
}

/// Alias command failure propagates exit code (-y bypasses approval)
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

    assert_cmd_snapshot!(make_snapshot_cmd_with_global_flags(
        &repo,
        "step",
        &["fail"],
        Some(&feature_path),
        &["-y"],
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

/// Top-level alias dispatch: `wt <name>` runs an alias when `<name>` is not
/// a built-in subcommand, with the same template-expansion and approval flow
/// as `wt step <name>`.
#[rstest]
fn test_top_level_alias_dispatch(mut repo: TestRepo) {
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

    assert_cmd_snapshot!(make_snapshot_cmd_with_global_flags(
        &repo,
        "hello",
        &[],
        Some(&feature_path),
        &["-y"],
    ));
}

/// An alias whose name matches a `wt step` built-in is unreachable via
/// `wt step <name>` (the built-in always wins) but runs from the top level
/// via `wt <name>` — there's no top-level `commit` built-in.
#[rstest]
fn test_top_level_alias_with_step_builtin_name(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
commit = "echo custom-commit"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd_with_global_flags(
        &repo,
        "commit",
        &[],
        Some(&feature_path),
        &["-y"],
    ));
}

/// Top-level typo on an alias name suggests the alias in the `tip:` line,
/// matching `wt step <typo>`.
#[rstest]
fn test_top_level_alias_did_you_mean(mut repo: TestRepo) {
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

    assert_cmd_snapshot!(make_snapshot_cmd(&repo, "deplyo", &[], Some(&feature_path),));
}

/// Aliases shadowed by `wt step` built-ins are filtered from the typo
/// suggestion list — `wt step commit` does not suggest a (shadowed) alias
/// named `commit`, only the real built-in.
#[rstest]
fn test_step_alias_shadows_builtin(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
commit = "echo custom-commit"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["comit"],
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
            "config",
            &["alias", "dry-run", "user-cmd"],
            Some(&feature_path),
        )
    );

    // Project alias available — dry-run never needs approval (it doesn't execute).
    assert_cmd_snapshot!(
        "project_alias",
        make_snapshot_cmd(
            &repo,
            "config",
            &["alias", "dry-run", "project-cmd"],
            Some(&feature_path),
        )
    );

    // Both definitions visible on collision: user first, then project (matches runtime order).
    assert_cmd_snapshot!(
        "user_and_project_append",
        make_snapshot_cmd(
            &repo,
            "config",
            &["alias", "dry-run", "shared"],
            Some(&feature_path),
        )
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

    // Both commands execute: user first, then project (-y approves project alias)
    assert_cmd_snapshot!(make_snapshot_cmd_with_global_flags(
        &repo,
        "step",
        &["greet"],
        Some(&feature_path),
        &["-y"],
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

    // Both run with -y: user first, then project (project needs approval)
    assert_cmd_snapshot!(make_snapshot_cmd_with_global_flags(
        &repo,
        "step",
        &["deploy"],
        Some(&feature_path),
        &["-y"],
    ));
}

/// -y bypasses approval for project-config alias without saving
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

    // First run with -y succeeds
    assert_cmd_snapshot!(
        "alias_approval_yes_first_run",
        make_snapshot_cmd_with_global_flags(
            &repo,
            "step",
            &["deploy"],
            Some(&feature_path),
            &["-y"],
        )
    );

    // Second run without -y should still prompt (-y doesn't save approval)
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

/// `dry-run` for a pipeline where a later step references `{{ vars.X }}` set by
/// an earlier step succeeds, mirroring the lazy execution path. The unresolved
/// `vars.*` reference is shown as the raw template since its value isn't
/// knowable until the earlier step actually runs.
#[rstest]
fn test_config_alias_dry_run_vars_across_steps(mut repo: TestRepo) {
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
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "config",
        &["alias", "dry-run", "deploy"],
        Some(&feature_path),
    ));
}

/// `dry-run` still catches template syntax errors (e.g., `{{ vars..foo }}`) even
/// on the lazy path where `vars.*` rendering is skipped.
#[rstest]
fn test_config_alias_dry_run_catches_syntax_error(mut repo: TestRepo) {
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
        .args(["config", "alias", "dry-run", "broken"])
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

/// Retired `wt <alias> --dry-run` flag produces an actionable error pointing at
/// the new subcommand. Snapshots the top-level path; the shared parser covers
/// both `wt <alias>` and `wt step <alias>` dispatch routes (unit test in
/// `commands::alias::tests::test_parse_errors` verifies the message verbatim).
#[rstest]
fn test_retired_dry_run_flag(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
deploy = "echo hi"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "deploy",
        &["--dry-run"],
        Some(&feature_path),
    ));
}

/// Retired `--dry-run` bail also fires through `wt step <alias>` — the parser
/// is shared, but this pins the `step_alias` dispatch route specifically.
#[rstest]
fn test_retired_dry_run_flag_via_step(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
deploy = "echo hi"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "step",
        &["deploy", "--dry-run"],
        Some(&feature_path),
    ));
}

/// `wt <alias> --help` prints guidance rather than forwarding `--help` into
/// `{{ args }}`. Aliases have no clap-style help page; the canonical
/// inspection path is `wt config alias show / dry-run`.
#[rstest]
fn test_alias_help_flag_prints_hint(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
deploy = "echo hi {{ args }}"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "deploy",
        &["--help"],
        Some(&feature_path),
    ));
}

/// `wt <alias> -- --help` bypasses the intercept and forwards `--help` into
/// the alias body — the documented escape.
#[rstest]
fn test_alias_help_flag_after_double_dash_forwards(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
deploy = "echo {{ args }}"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "deploy",
        &["--", "--help"],
        Some(&feature_path),
    ));
}

/// `wt config alias show <name>` prints the configured template text, source-labeled.
#[rstest]
fn test_config_alias_show_single(mut repo: TestRepo) {
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
        "config",
        &["alias", "show", "deploy"],
        Some(&feature_path),
    ));
}

/// Unknown alias name triggers a did-you-mean suggestion. Format mirrors
/// `wt <typo>` and `wt step <typo>`: clap-native `InvalidSubcommand` with a
/// `tip:` line — same shape at every alias-typo surface.
#[rstest]
fn test_config_alias_show_unknown_suggests(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
deploy = "make deploy"
hello = "echo hi"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "config",
        &["alias", "show", "deplyo"],
        Some(&feature_path),
    ));
}

/// Aliases or typos that literally contain `subcommand` must be echoed
/// verbatim — the word-substitution pass that rewrites clap's
/// "unrecognized subcommand" to "unrecognized alias" must only touch
/// clap's fixed phrases, not the user's input.
#[rstest]
fn test_config_alias_show_unknown_preserves_user_input(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
"my-subcommand" = "echo hi"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    // Typo `my-subcommond` is close enough to `my-subcommand` to suggest it —
    // both the echoed typo and the suggestion must keep `subcommand` intact.
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "config",
        &["alias", "show", "my-subcommond"],
        Some(&feature_path),
    ));
}

/// Single-match case: tip phrasing switches to the singular form
/// ("a similar alias exists") — mirrors clap's own singular rendering.
#[rstest]
fn test_config_alias_show_unknown_singular_suggestion(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
deploy = "make deploy"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "config",
        &["alias", "show", "deplyo"],
        Some(&feature_path),
    ));
}

/// `wt config alias dry-run <typo>` produces the same clap-native typo error
/// as `show` — consistent format across both introspection subcommands.
#[rstest]
fn test_config_alias_dry_run_unknown_suggests(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
deploy = "make deploy"
hello = "echo hi"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "config",
        &["alias", "dry-run", "deplyo"],
        Some(&feature_path),
    ));
}

/// Multi-step pipeline renders with per-step structure in the header.
#[rstest]
fn test_config_alias_show_pipeline(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[[aliases.release]]
install = "npm install"

[[aliases.release]]
build = "npm run build"
lint = "npm run lint"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "config",
        &["alias", "show", "release"],
        Some(&feature_path),
    ));
}

/// Positional args via `wt config alias dry-run <name> -- foo bar` flow through as `{{ args }}`.
#[rstest]
fn test_config_alias_dry_run_positional_args(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
s = "wt switch {{ args }}"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "config",
        &["alias", "dry-run", "s", "--", "target-branch"],
        Some(&feature_path),
    ));
}

/// `wt config alias show <name>` with the same alias defined in both user and
/// project config prints both entries in runtime order (user first).
#[rstest]
fn test_config_alias_show_user_and_project(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
deploy = "echo from project"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");
    repo.write_test_config(
        r#"
[aliases]
deploy = "echo from user"
"#,
    );

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "config",
        &["alias", "show", "deploy"],
        Some(&feature_path),
    ));
}

/// Unknown alias name with no similar configured aliases shows a plain error
/// without a "did you mean" tail.
#[rstest]
fn test_config_alias_show_unknown_no_suggestions(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
deploy = "echo hi"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    // Query name shares no meaningful prefix with any configured alias — the
    // Jaro-Winkler threshold rejects it, so the error has no suggestion list.
    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "config",
        &["alias", "show", "zzzzzzzz"],
        Some(&feature_path),
    ));
}

/// `wt config alias show <name>` on an alias whose name is also a top-level
/// built-in subcommand warns that the alias is unreachable via `wt <name>`.
/// The alias is still configured, so the show output itself is shown — the
/// warning is an advisory on stderr.
#[rstest]
fn test_config_alias_show_warns_on_shadowed_name(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
list = "echo custom list"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "config",
        &["alias", "show", "list"],
        Some(&feature_path),
    ));
}

/// `wt config alias dry-run` on a shadowed name emits the same advisory as
/// `show` — both are discovery surfaces, so both point out the shadowing.
#[rstest]
fn test_config_alias_dry_run_warns_on_shadowed_name(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
list = "echo custom list"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "config",
        &["alias", "dry-run", "list"],
        Some(&feature_path),
    ));
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

/// Positional args after the alias name forward to `{{ args }}` in the
/// template — space-joined and shell-escaped so args with spaces, quotes,
/// or metacharacters splice safely into a command line.
#[rstest]
fn test_step_alias_forwards_positional_args(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
run = "echo got {{ args }}"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd_with_global_flags(
        &repo,
        "run",
        &["one", "two three", "four"],
        Some(&feature_path),
        &["-y"],
    ));
}

/// Templates can treat `{{ args }}` as a sequence: indexing, iteration,
/// and `length` all work because `ShellArgs` reports as `ObjectRepr::Seq`.
#[rstest]
fn test_step_alias_args_sequence_access(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
show = '''echo first={{ args[0] }}; echo count={{ args | length }}; echo each={% for a in args %} {{ a }}{% endfor %}'''
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd_with_global_flags(
        &repo,
        "show",
        &["alpha", "beta gamma"],
        Some(&feature_path),
        &["-y"],
    ));
}

/// With no positionals, `{{ args }}` renders empty — the rest of the line
/// stays intact and no stray whitespace is introduced.
#[rstest]
fn test_step_alias_empty_args_renders_empty(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
run = "echo [{{ args }}]"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd_with_global_flags(
        &repo,
        "run",
        &[],
        Some(&feature_path),
        &["-y"],
    ));
}

/// `wt s some-branch` with `s = "wt switch {{ args }}"` forwards the
/// positional into the expanded command. Verified via `wt config alias dry-run`
/// so the inner `wt switch` is not actually executed.
#[rstest]
fn test_top_level_alias_positional_expands_in_dry_run(mut repo: TestRepo) {
    repo.write_project_config(
        r#"
[aliases]
s = "wt switch {{ args }}"
"#,
    );
    repo.commit("Add alias config");
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    let _guard = settings.bind_to_scope();

    assert_cmd_snapshot!(make_snapshot_cmd(
        &repo,
        "config",
        &["alias", "dry-run", "s", "--", "target-branch"],
        Some(&feature_path),
    ));
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

/// Under `-v`, aliases print a table of resolved template variables before
/// the announcement — symmetric with the hook-invocation verbose block, but
/// with alias-scoped vars (`args` included, no `hook_*` keys).
#[rstest]
fn test_alias_verbose_prints_variable_table(mut repo: TestRepo) {
    repo.write_test_config(
        r#"
[aliases]
greet = "echo hello {{ args }}"
"#,
    );
    let feature_path = repo.add_worktree("feature");

    let settings = setup_snapshot_settings(&repo);
    settings.bind(|| {
        let mut cmd = make_snapshot_cmd_with_global_flags(
            &repo,
            "greet",
            &["world"],
            Some(&feature_path),
            &["-v"],
        );
        assert_cmd_snapshot!("alias_verbose_variable_table", cmd);
    });
}
