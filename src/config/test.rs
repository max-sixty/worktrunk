//! Tests for template expansion with special characters and edge cases
//!
//! These tests target potential shell injection vulnerabilities and
//! edge cases in template variable substitution.

use super::{expand_template, sanitize_branch_name};
use std::collections::HashMap;

/// Helper to build vars with common fields
fn vars_with_branch(branch: &str) -> HashMap<&str, &str> {
    let mut vars = HashMap::new();
    vars.insert("branch", branch);
    vars.insert("main_worktree", "myrepo");
    vars.insert("repo", "myrepo");
    vars
}

#[test]
fn test_expand_template_normal() {
    let branch = sanitize_branch_name("feature");
    let vars = vars_with_branch(&branch);
    let result = expand_template("echo {{ branch }} {{ main_worktree }}", &vars, true).unwrap();
    assert_eq!(result, "echo feature myrepo");
}

#[test]
fn test_expand_template_branch_with_slashes() {
    // Caller is responsible for sanitizing branch names
    let branch = sanitize_branch_name("feature/nested/branch");
    let vars = vars_with_branch(&branch);
    let result = expand_template("echo {{ branch }}", &vars, true).unwrap();
    assert_eq!(result, "echo feature-nested-branch");
}

// Tests with platform-specific shell escaping (Unix uses single quotes, Windows uses double quotes)
#[test]
#[cfg(unix)]
fn test_expand_template_branch_escaping() {
    let expand = |input| {
        let branch = sanitize_branch_name(input);
        expand_template("echo {{ branch }}", &vars_with_branch(&branch), true).unwrap()
    };

    assert_eq!(expand("feature name"), "echo 'feature name'"); // spaces
    assert_eq!(expand("feature$(whoami)"), "echo 'feature$(whoami)'"); // command sub
    assert_eq!(expand("feature`id`"), "echo 'feature`id`'"); // backticks
}

#[test]
#[cfg(unix)]
fn snapshot_expand_template_branch_with_quotes() {
    let branch = sanitize_branch_name("feature'test");
    let vars = vars_with_branch(&branch);
    let result = expand_template("echo '{{ branch }}'", &vars, true).unwrap();
    insta::assert_snapshot!(result, @"echo ''feature'\\''test''");
}

#[test]
#[cfg(unix)]
fn test_expand_template_extra_vars_path_escaping() {
    let expand = |path| {
        expand_template(
            "cd {{ worktree }}",
            &HashMap::from([("worktree", path)]),
            true,
        )
        .unwrap()
    };

    assert_eq!(
        expand("/path with spaces/to/wt"),
        "cd '/path with spaces/to/wt'"
    ); // spaces
    assert_eq!(expand("/path/$USER/worktree"), "cd '/path/$USER/worktree'"); // dollar sign
}

#[test]
#[cfg(unix)]
fn snapshot_expand_template_extra_vars_with_command_substitution() {
    let mut extras = HashMap::new();
    extras.insert("target", "main; rm -rf /");
    let result = expand_template("git merge {{ target }}", &extras, true).unwrap();

    insta::assert_snapshot!(result, @"git merge 'main; rm -rf /'");
}

#[test]
fn test_expand_template_variable_override() {
    // Variables in the hashmap take precedence
    let mut vars = HashMap::new();
    vars.insert("branch", "overridden");
    let result = expand_template("echo {{ branch }}", &vars, true).unwrap();

    assert_eq!(result, "echo overridden");
}

#[test]
fn test_expand_template_missing_variable() {
    // What happens with undefined variables?
    let vars: HashMap<&str, &str> = HashMap::new();
    let result = expand_template("echo {{ undefined }}", &vars, true).unwrap();

    // minijinja will render undefined variables as empty string
    assert_eq!(result, "echo ");
}

#[test]
#[cfg(unix)]
fn test_expand_template_empty_branch() {
    let mut vars = HashMap::new();
    vars.insert("branch", "");
    let result = expand_template("echo {{ branch }}", &vars, true).unwrap();

    // Empty string is shell-escaped to ''
    assert_eq!(result, "echo ''");
}

#[test]
#[cfg(unix)]
fn test_expand_template_unicode_in_branch() {
    // Unicode characters in branch name are shell-escaped
    let branch = sanitize_branch_name("feature-\u{1F680}");
    let vars = vars_with_branch(&branch);
    let result = expand_template("echo {{ branch }}", &vars, true).unwrap();

    // Unicode is preserved but quoted for shell safety
    assert_eq!(result, "echo 'feature-\u{1F680}'");
}

#[test]
fn test_expand_template_backslash_in_branch() {
    // Windows-style path separators - caller sanitizes
    let branch = sanitize_branch_name("feature\\branch");
    let vars = vars_with_branch(&branch);
    let result = expand_template("echo {{ branch }}", &vars, true).unwrap();

    // Backslashes are replaced with dashes by sanitize_branch_name
    assert_eq!(result, "echo feature-branch");
}

#[test]
fn test_expand_template_multiple_replacements() {
    let branch = sanitize_branch_name("feature");
    let mut vars = vars_with_branch(&branch);
    vars.insert("worktree", "/path/to/wt");
    vars.insert("target", "develop");

    let result = expand_template(
        "cd {{ worktree }} && git merge {{ target }} from {{ branch }}",
        &vars,
        true,
    )
    .unwrap();

    assert_eq!(result, "cd /path/to/wt && git merge develop from feature");
}

#[test]
fn test_expand_template_curly_braces_without_variables() {
    // Just curly braces, not variables
    let vars: HashMap<&str, &str> = HashMap::new();
    let result = expand_template("echo {}", &vars, true).unwrap();

    assert_eq!(result, "echo {}");
}

#[test]
fn test_expand_template_nested_curly_braces() {
    // Nested braces - minijinja doesn't support {{{ syntax, use literal curly braces instead
    let branch = sanitize_branch_name("main");
    let vars = vars_with_branch(&branch);
    let result = expand_template("echo {{ '{' ~ branch ~ '}' }}", &vars, true).unwrap();

    // Renders as {main}
    assert_eq!(result, "echo {main}");
}

// Snapshot tests for shell escaping behavior
// These verify the exact shell-escaped output for security-critical cases
//
// Unix-only: Shell escaping is platform-dependent (Unix uses single quotes,
// Windows uses double quotes). These snapshots verify Unix shell behavior.

#[test]
#[cfg(unix)]
fn snapshot_shell_escaping_special_chars() {
    // Test various shell special characters
    let test_cases = vec![
        ("spaces", "feature name"),
        ("dollar", "feature$USER"),
        ("command_sub", "feature$(whoami)"),
        ("backticks", "feature`id`"),
        ("semicolon", "feature;rm -rf /"),
        ("pipe", "feature|grep foo"),
        ("ampersand", "feature&background"),
        ("redirect", "feature>output.txt"),
        ("wildcard", "feature*glob"),
        ("question", "feature?char"),
        ("brackets", "feature[0-9]"),
    ];

    let mut results = Vec::new();
    for (name, branch_raw) in test_cases {
        let branch = sanitize_branch_name(branch_raw);
        let vars = vars_with_branch(&branch);
        let result = expand_template("echo {{ branch }}", &vars, true).unwrap();
        results.push((name, branch_raw, result));
    }

    insta::assert_yaml_snapshot!(results);
}

#[test]
#[cfg(unix)]
fn snapshot_shell_escaping_quotes() {
    // Test quote handling
    let test_cases = vec![
        ("single_quote", "feature'test"),
        ("double_quote", "feature\"test"),
        ("mixed_quotes", "feature'test\"mixed"),
        ("multiple_single", "don't'panic"),
    ];

    let mut results = Vec::new();
    for (name, branch_raw) in test_cases {
        let branch = sanitize_branch_name(branch_raw);
        let vars = vars_with_branch(&branch);
        let result = expand_template("echo {{ branch }}", &vars, true).unwrap();
        results.push((name, branch_raw, result));
    }

    insta::assert_yaml_snapshot!(results);
}

#[test]
#[cfg(unix)]
fn snapshot_shell_escaping_paths() {
    // Test path escaping with various special characters
    let test_cases = vec![
        ("spaces", "/path with spaces/to/worktree"),
        ("dollar", "/path/$USER/worktree"),
        ("tilde", "~/worktree"),
        ("special_chars", "/path/to/worktree (new)"),
        ("unicode", "/path/to/\u{1F680}/worktree"),
    ];

    let mut results = Vec::new();
    for (name, path) in test_cases {
        let branch = sanitize_branch_name("main");
        let mut vars = vars_with_branch(&branch);
        vars.insert("worktree", path);
        let result =
            expand_template("cd {{ worktree }} && echo {{ branch }}", &vars, true).unwrap();
        results.push((name, path, result));
    }

    insta::assert_yaml_snapshot!(results);
}

#[test]
#[cfg(unix)]
fn snapshot_complex_templates() {
    // Test realistic complex template commands
    let test_cases = vec![
        (
            "cd_and_merge",
            "cd {{ worktree }} && git merge {{ target }}",
            "feature branch",
        ),
        (
            "npm_install",
            "cd {{ main_worktree }}/{{ branch }} && npm install",
            "feature/new-ui",
        ),
        (
            "echo_vars",
            "echo 'Branch: {{ branch }}' 'Worktree: {{ worktree }}'",
            "test$injection",
        ),
    ];

    let mut results = Vec::new();
    for (name, template, branch_raw) in test_cases {
        let branch = sanitize_branch_name(branch_raw);
        let mut vars = HashMap::new();
        vars.insert("branch", branch.as_str());
        vars.insert("main_worktree", "/repo/path");
        vars.insert("worktree", "/path with spaces/wt");
        vars.insert("target", "main; rm -rf /");
        let result = expand_template(template, &vars, true).unwrap();
        results.push((name, template, branch_raw, result));
    }

    insta::assert_yaml_snapshot!(results);
}

// Tests for literal expansion (shell_escape=false)

#[test]
fn test_expand_template_literal_normal() {
    let branch = sanitize_branch_name("feature");
    let mut vars = HashMap::new();
    vars.insert("main_worktree", "myrepo");
    vars.insert("branch", branch.as_str());
    let result = expand_template("{{ main_worktree }}.{{ branch }}", &vars, false).unwrap();
    assert_eq!(result, "myrepo.feature");
}

#[test]
fn test_expand_template_literal_unicode_no_escaping() {
    // Unicode should NOT be shell-escaped in filesystem paths
    let branch = sanitize_branch_name("test-\u{2282}");
    let mut vars = HashMap::new();
    vars.insert("main_worktree", "myrepo");
    vars.insert("branch", branch.as_str());
    let result = expand_template("{{ main_worktree }}.{{ branch }}", &vars, false).unwrap();
    // Path should contain literal unicode, NO quotes
    assert_eq!(result, "myrepo.test-\u{2282}");
    assert!(
        !result.contains('\''),
        "Path should not contain shell quotes"
    );
}

#[test]
fn test_expand_template_literal_spaces_no_escaping() {
    // Spaces should NOT be shell-escaped (filesystem paths can have spaces)
    let branch = sanitize_branch_name("feature name");
    let mut vars = HashMap::new();
    vars.insert("main_worktree", "my repo");
    vars.insert("branch", branch.as_str());
    let result = expand_template("{{ main_worktree }}.{{ branch }}", &vars, false).unwrap();
    // No shell quotes around spaces
    assert_eq!(result, "my repo.feature name");
    assert!(
        !result.contains('\''),
        "Path should not contain shell quotes"
    );
}

#[test]
fn test_expand_template_literal_sanitizes_slashes() {
    // Branch name sanitization is caller's responsibility
    let branch = sanitize_branch_name("feature/nested/branch");
    let mut vars = HashMap::new();
    vars.insert("main_worktree", "myrepo");
    vars.insert("branch", branch.as_str());
    let result = expand_template("{{ main_worktree }}.{{ branch }}", &vars, false).unwrap();
    assert_eq!(result, "myrepo.feature-nested-branch");
}

#[test]
#[cfg(unix)]
fn test_expand_template_literal_vs_escaped_unicode() {
    // Demonstrate the difference between literal and escaped expansion
    let branch = sanitize_branch_name("test-\u{2282}");
    let mut vars = HashMap::new();
    vars.insert("main_worktree", "myrepo");
    vars.insert("branch", branch.as_str());
    let template = "{{ main_worktree }}.{{ branch }}";

    let literal_result = expand_template(template, &vars, false).unwrap();
    let escaped_result = expand_template(template, &vars, true).unwrap();

    // Literal has no quotes
    assert_eq!(literal_result, "myrepo.test-\u{2282}");
    // Escaped has shell quotes around the unicode part
    // (shell_escape only quotes strings containing special chars)
    assert_eq!(escaped_result, "myrepo.'test-\u{2282}'");
}
