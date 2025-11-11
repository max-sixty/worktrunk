use crate::common::{
    TestRepo,
    shell::{execute_shell_script, generate_init_code, path_export_syntax, wt_bin_dir},
};
use rstest::rstest;

#[rstest]
// bash is baseline, fish exercises alternate syntax
#[case("bash")]
#[case("fish")]
#[case("zsh")]
// Oil remains optional until tier-2 feature flag is enabled
#[cfg_attr(feature = "tier-2-integration-tests", case("oil"))]
fn test_e2e_switch_and_remove_roundtrip(#[case] shell: &str) {
    let mut repo = TestRepo::new();
    repo.commit("Initial commit");
    repo.setup_remote("main");

    let init_code = generate_init_code(&repo, shell);
    let bin_path = wt_bin_dir();

    let script = format!(
        r#"
        {}
        {}
        wt switch --create combo-branch
        echo "__PWD_AFTER_SWITCH__ $PWD"
        wt remove
        echo "__PWD_AFTER_REMOVE__ $PWD"
        "#,
        path_export_syntax(shell, &bin_path),
        init_code
    );

    let output = execute_shell_script(&repo, shell, &script);

    // Ensure human output is still visible (not just directives)
    assert!(
        output.contains("combo-branch"),
        "Combined e2e run should mention combo-branch, got:\n{}",
        output
    );

    // Directives must remain hidden from the user.
    assert!(
        !output.contains("__WORKTRUNK"),
        "Directive leakage detected in shell output:\n{}",
        output
    );

    let after_switch =
        extract_pwd_marker(&output, "__PWD_AFTER_SWITCH__").expect("Missing switch marker");
    let after_remove =
        extract_pwd_marker(&output, "__PWD_AFTER_REMOVE__").expect("Missing remove marker");

    assert!(
        after_switch.contains("combo-branch"),
        "Shell should cd into combo-branch worktree, saw: {}",
        after_switch
    );

    let repo_root = repo.root_path().to_string_lossy();
    assert!(
        after_remove.ends_with(repo_root.as_ref()),
        "Shell should cd back to repo root {} after remove, got: {}",
        repo_root,
        after_remove
    );
}

#[test]
fn test_bash_e2e_error_handling() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let init_code = generate_init_code(&repo, "bash");
    let bin_path = wt_bin_dir();

    let script = format!(
        r#"
        {}
        {}
        wt switch --create dup-branch
        if wt switch --create dup-branch 2>&1; then
          echo "__UNEXPECTED_SUCCESS__"
        else
          echo "__DUPLICATE_ERROR__"
        fi
        "#,
        path_export_syntax("bash", &bin_path),
        init_code
    );

    let output = execute_shell_script(&repo, "bash", &script);
    assert!(
        output.contains("__DUPLICATE_ERROR__"),
        "Second switch should have failed, output: {}",
        output
    );
    assert!(
        output.contains("already exists") || output.contains("Branch \"dup-branch\""),
        "User-facing error details missing: {}",
        output
    );
    assert!(
        !output.contains("__WORKTRUNK"),
        "Directive leakage detected in error flow: {}",
        output
    );
}

#[test]
fn test_bash_e2e_switch_existing_worktree() {
    let repo = TestRepo::new();
    repo.commit("Initial commit");

    let init_code = generate_init_code(&repo, "bash");
    let bin_path = wt_bin_dir();
    let repo_root = repo.root_path().display();

    let script = format!(
        r#"
        {}
        {}
        wt switch --create existing-branch
        echo "__AFTER_CREATE__ $PWD"
        REPO_ROOT="{}"
        cd "$REPO_ROOT"
        wt switch existing-branch
        echo "__AFTER_EXISTING__ $PWD"
        "#,
        path_export_syntax("bash", &bin_path),
        init_code,
        repo_root
    );

    let output = execute_shell_script(&repo, "bash", &script);
    let after_create =
        extract_pwd_marker(&output, "__AFTER_CREATE__").expect("missing create marker");
    let after_existing =
        extract_pwd_marker(&output, "__AFTER_EXISTING__").expect("missing existing marker");

    assert!(
        after_create.contains("existing-branch"),
        "First switch should cd into worktree, saw: {}",
        after_create
    );
    assert!(
        after_existing.contains("existing-branch"),
        "Switching to existing worktree should cd there again, saw: {}",
        after_existing
    );
}

fn extract_pwd_marker(output: &str, marker: &str) -> Option<String> {
    output
        .lines()
        .find(|line| line.contains(marker))
        .map(|line| line.split(marker).nth(1).unwrap_or("").trim().to_string())
}
