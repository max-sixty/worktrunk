//! Tests for template expansion with special characters and edge cases
//!
//! These tests target potential shell injection vulnerabilities and
//! edge cases in template variable substitution.

use super::expand_template;
use std::collections::HashMap;

#[test]
fn test_expand_template_normal() {
    let extras = HashMap::new();
    let result = expand_template(
        "echo {{ branch }} {{ main_worktree }}",
        "myrepo",
        "feature",
        &extras,
    )
    .unwrap();
    assert_eq!(result, "echo feature myrepo");
}

#[test]
fn test_expand_template_branch_with_slashes() {
    // Bug hypothesis: Branch names with slashes are sanitized to dashes
    let extras = HashMap::new();
    let result = expand_template(
        "echo {{ branch }}",
        "myrepo",
        "feature/nested/branch",
        &extras,
    )
    .unwrap();

    // Line 459: safe_branch = branch.replace(['/', '\\'], "-")
    assert_eq!(result, "echo feature-nested-branch");
}

// Tests with platform-specific shell escaping (Unix uses single quotes, Windows uses double quotes)
#[test]
#[cfg(unix)]
fn test_expand_template_branch_with_spaces() {
    // Branch names with spaces are shell-escaped
    let extras = HashMap::new();
    let result = expand_template("echo {{ branch }}", "myrepo", "feature name", &extras).unwrap();

    // Shell-escaped with single quotes
    assert_eq!(result, "echo 'feature name'");
}

#[test]
#[cfg(unix)]
fn test_expand_template_branch_with_special_shell_chars() {
    // Special shell characters are escaped
    let extras = HashMap::new();
    let result =
        expand_template("echo {{ branch }}", "myrepo", "feature$(whoami)", &extras).unwrap();

    // Shell-escaped, prevents command substitution
    assert_eq!(result, "echo 'feature$(whoami)'");
    // Shell executes: echo 'feature$(whoami)' (literal string, no command execution)
}

#[test]
#[cfg(unix)]
fn test_expand_template_branch_with_backticks() {
    // Backticks are escaped
    let extras = HashMap::new();
    let result = expand_template("echo {{ branch }}", "myrepo", "feature`id`", &extras).unwrap();

    assert_eq!(result, "echo 'feature`id`'");
}

#[test]
#[cfg(unix)]
fn test_expand_template_branch_with_quotes() {
    // Quotes are shell-escaped to prevent injection
    let extras = HashMap::new();
    let result = expand_template("echo '{{ branch }}'", "myrepo", "feature'test", &extras).unwrap();

    // Shell escapes single quotes as '\''
    assert_eq!(result, "echo ''feature'\\''test''");
}

#[test]
#[cfg(unix)]
fn test_expand_template_extra_vars_with_spaces() {
    // Extra variables with spaces are shell-escaped
    let mut extras = HashMap::new();
    extras.insert("worktree", "/path with spaces/to/worktree");
    let result = expand_template("cd {{ worktree }}", "myrepo", "main", &extras).unwrap();

    assert_eq!(result, "cd '/path with spaces/to/worktree'");
}

#[test]
#[cfg(unix)]
fn test_expand_template_extra_vars_with_dollar_sign() {
    // Dollar signs are shell-escaped to prevent variable expansion
    let mut extras = HashMap::new();
    extras.insert("worktree", "/path/$USER/worktree");
    let result = expand_template("cd {{ worktree }}", "myrepo", "main", &extras).unwrap();

    assert_eq!(result, "cd '/path/$USER/worktree'");
    // Shell-escaped, prevents $USER from being expanded
}

#[test]
#[cfg(unix)]
fn test_expand_template_extra_vars_with_command_substitution() {
    // Special shell characters are shell-escaped to prevent injection
    let mut extras = HashMap::new();
    extras.insert("target", "main; rm -rf /");
    let result = expand_template("git merge {{ target }}", "myrepo", "feature", &extras).unwrap();

    assert_eq!(result, "git merge 'main; rm -rf /'");
    // Shell-escaped, prevents semicolon from being executed as command separator
}

#[test]
fn test_expand_template_variable_collision() {
    // What if extra vars contain "branch"? With minijinja, extra vars added later override built-ins
    let mut extras = HashMap::new();
    extras.insert("branch", "hacked");
    let result = expand_template("echo {{ branch }}", "myrepo", "feature", &extras).unwrap();

    // Extra vars are added to context after built-ins, so they override
    assert_eq!(result, "echo hacked");
}

#[test]
fn test_expand_template_extra_var_named_branch() {
    // What if we have both {{ branch }} in template and "branch" in extras?
    let mut extras = HashMap::new();
    extras.insert("branch", "extra-branch");
    let result = expand_template(
        "echo {{ branch }} from {{ branch }}",
        "myrepo",
        "main",
        &extras,
    )
    .unwrap();

    // Extra vars override built-ins, so both occurrences use "extra-branch"
    assert_eq!(result, "echo extra-branch from extra-branch");
}

#[test]
fn test_expand_template_missing_variable() {
    // What happens with undefined variables?
    let extras = HashMap::new();
    let result = expand_template("echo {{ undefined }}", "myrepo", "main", &extras).unwrap();

    // minijinja will render undefined variables as empty string
    assert_eq!(result, "echo ");
}

#[test]
#[cfg(unix)]
fn test_expand_template_empty_branch() {
    let extras = HashMap::new();
    let result = expand_template("echo {{ branch }}", "myrepo", "", &extras).unwrap();

    // Empty string is shell-escaped to ''
    assert_eq!(result, "echo ''");
}

#[test]
#[cfg(unix)]
fn test_expand_template_unicode_in_branch() {
    // Unicode characters in branch name are shell-escaped
    let extras = HashMap::new();
    let result = expand_template("echo {{ branch }}", "myrepo", "feature-🚀", &extras).unwrap();

    // Unicode is preserved but quoted for shell safety
    assert_eq!(result, "echo 'feature-🚀'");
}

#[test]
fn test_expand_template_backslash_in_branch() {
    // Windows-style path separators
    let extras = HashMap::new();
    let result =
        expand_template("echo {{ branch }}", "myrepo", "feature\\branch", &extras).unwrap();

    // Line 459: backslashes also replaced with dashes
    assert_eq!(result, "echo feature-branch");
}

#[test]
fn test_expand_template_multiple_replacements() {
    let mut extras = HashMap::new();
    extras.insert("worktree", "/path/to/wt");
    extras.insert("target", "develop");

    let result = expand_template(
        "cd {{ worktree }} && git merge {{ target }} from {{ branch }}",
        "myrepo",
        "feature",
        &extras,
    )
    .unwrap();

    assert_eq!(result, "cd /path/to/wt && git merge develop from feature");
}

#[test]
fn test_expand_template_curly_braces_without_variables() {
    // Just curly braces, not variables
    let extras = HashMap::new();
    let result = expand_template("echo {}", "myrepo", "main", &extras).unwrap();

    assert_eq!(result, "echo {}");
}

#[test]
fn test_expand_template_nested_curly_braces() {
    // Nested braces - minijinja doesn't support {{{ syntax, use literal curly braces instead
    let extras = HashMap::new();
    let result =
        expand_template("echo {{ '{' ~ branch ~ '}' }}", "myrepo", "main", &extras).unwrap();

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
    let extras = HashMap::new();

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
    for (name, branch) in test_cases {
        let result = expand_template("echo {{ branch }}", "myrepo", branch, &extras).unwrap();
        results.push((name, branch, result));
    }

    insta::assert_yaml_snapshot!(results);
}

#[test]
#[cfg(unix)]
fn snapshot_shell_escaping_quotes() {
    let extras = HashMap::new();

    // Test quote handling
    let test_cases = vec![
        ("single_quote", "feature'test"),
        ("double_quote", "feature\"test"),
        ("mixed_quotes", "feature'test\"mixed"),
        ("multiple_single", "don't'panic"),
    ];

    let mut results = Vec::new();
    for (name, branch) in test_cases {
        let result = expand_template("echo {{ branch }}", "myrepo", branch, &extras).unwrap();
        results.push((name, branch, result));
    }

    insta::assert_yaml_snapshot!(results);
}

#[test]
#[cfg(unix)]
fn snapshot_shell_escaping_paths() {
    let mut extras = HashMap::new();

    // Test path escaping with various special characters
    let test_cases = vec![
        ("spaces", "/path with spaces/to/worktree"),
        ("dollar", "/path/$USER/worktree"),
        ("tilde", "~/worktree"),
        ("special_chars", "/path/to/worktree (new)"),
        ("unicode", "/path/to/🚀/worktree"),
    ];

    let mut results = Vec::new();
    for (name, path) in test_cases {
        extras.clear();
        extras.insert("worktree", path);
        let result = expand_template(
            "cd {{ worktree }} && echo {{ branch }}",
            "myrepo",
            "main",
            &extras,
        )
        .unwrap();
        results.push((name, path, result));
    }

    insta::assert_yaml_snapshot!(results);
}

#[test]
#[cfg(unix)]
fn snapshot_complex_templates() {
    let mut extras = HashMap::new();
    extras.insert("worktree", "/path with spaces/wt");
    extras.insert("target", "main; rm -rf /");

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
    for (name, template, branch) in test_cases {
        let result = expand_template(template, "/repo/path", branch, &extras).unwrap();
        results.push((name, template, branch, result));
    }

    insta::assert_yaml_snapshot!(results);
}
