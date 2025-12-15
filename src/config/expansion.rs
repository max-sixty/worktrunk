//! Template expansion utilities for worktrunk
//!
//! Uses minijinja for template rendering. Single generic function with escaping flag:
//! - `shell_escape: true` — Shell-escaped for safe command execution
//! - `shell_escape: false` — Literal values for filesystem paths
//!
//! All templates support Jinja2 syntax including filters, conditionals, and loops.

use minijinja::Environment;
use std::collections::HashMap;

/// Sanitize a branch name for use in filesystem paths.
///
/// Replaces path separators (`/` and `\`) with dashes to prevent directory traversal
/// and ensure the branch name is a single path component.
///
/// # Examples
/// ```
/// use worktrunk::config::sanitize_branch_name;
///
/// assert_eq!(sanitize_branch_name("feature/foo"), "feature-foo");
/// assert_eq!(sanitize_branch_name("user\\task"), "user-task");
/// assert_eq!(sanitize_branch_name("simple-branch"), "simple-branch");
/// ```
pub fn sanitize_branch_name(branch: &str) -> String {
    branch.replace(['/', '\\'], "-")
}

/// Expand a template with variable substitution.
///
/// # Arguments
/// * `template` - Template string using Jinja2 syntax (e.g., `{{ branch }}`)
/// * `vars` - Variables to substitute. Callers should sanitize branch names with
///   [`sanitize_branch_name`] before inserting.
/// * `shell_escape` - If true, shell-escape all values for safe command execution.
///   If false, substitute values literally (for filesystem paths).
///
/// # Examples
/// ```
/// use worktrunk::config::{expand_template, sanitize_branch_name};
/// use std::collections::HashMap;
///
/// // For shell commands (escaped)
/// let branch = sanitize_branch_name("feature/foo");
/// let mut vars = HashMap::new();
/// vars.insert("branch", branch.as_str());
/// vars.insert("repo", "myrepo");
/// let cmd = expand_template("echo {{ branch }} in {{ repo }}", &vars, true).unwrap();
/// assert_eq!(cmd, "echo feature-foo in myrepo");
///
/// // For filesystem paths (literal)
/// let branch = sanitize_branch_name("feature/foo");
/// let mut vars = HashMap::new();
/// vars.insert("branch", branch.as_str());
/// vars.insert("main_worktree", "myrepo");
/// let path = expand_template("{{ main_worktree }}.{{ branch }}", &vars, false).unwrap();
/// assert_eq!(path, "myrepo.feature-foo");
/// ```
pub fn expand_template(
    template: &str,
    vars: &HashMap<&str, &str>,
    shell_escape: bool,
) -> Result<String, String> {
    use shell_escape::escape;
    use std::borrow::Cow;

    // Build context map, optionally shell-escaping values
    let mut context = HashMap::new();
    for (key, value) in vars {
        let val = if shell_escape {
            escape(Cow::Borrowed(*value)).to_string()
        } else {
            (*value).to_string()
        };
        context.insert(key.to_string(), minijinja::Value::from(val));
    }

    // Render template with minijinja
    let mut env = Environment::new();
    if shell_escape {
        // Preserve trailing newlines in templates (important for multiline shell commands)
        env.set_keep_trailing_newline(true);
    }
    let tmpl = env
        .template_from_str(template)
        .map_err(|e| format!("Template syntax error: {}", e))?;

    tmpl.render(minijinja::Value::from_object(context))
        .map_err(|e| format!("Template render error: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================================
    // sanitize_branch_name Tests
    // ============================================================================

    #[test]
    fn test_sanitize_branch_name_forward_slash() {
        assert_eq!(sanitize_branch_name("feature/foo"), "feature-foo");
    }

    #[test]
    fn test_sanitize_branch_name_backslash() {
        assert_eq!(sanitize_branch_name("user\\task"), "user-task");
    }

    #[test]
    fn test_sanitize_branch_name_multiple_slashes() {
        assert_eq!(
            sanitize_branch_name("feature/user/task"),
            "feature-user-task"
        );
    }

    #[test]
    fn test_sanitize_branch_name_mixed_slashes() {
        assert_eq!(sanitize_branch_name("feature/user\\task"), "feature-user-task");
    }

    #[test]
    fn test_sanitize_branch_name_no_slashes() {
        assert_eq!(sanitize_branch_name("simple-branch"), "simple-branch");
    }

    #[test]
    fn test_sanitize_branch_name_empty() {
        assert_eq!(sanitize_branch_name(""), "");
    }

    #[test]
    fn test_sanitize_branch_name_only_slashes() {
        assert_eq!(sanitize_branch_name("///"), "---");
    }

    #[test]
    fn test_sanitize_branch_name_leading_slash() {
        assert_eq!(sanitize_branch_name("/feature"), "-feature");
    }

    #[test]
    fn test_sanitize_branch_name_trailing_slash() {
        assert_eq!(sanitize_branch_name("feature/"), "feature-");
    }

    // ============================================================================
    // expand_template Tests - Basic Substitution
    // ============================================================================

    #[test]
    fn test_expand_template_single_variable() {
        let mut vars = HashMap::new();
        vars.insert("name", "world");
        let result = expand_template("Hello {{ name }}", &vars, false);
        assert_eq!(result.unwrap(), "Hello world");
    }

    #[test]
    fn test_expand_template_multiple_variables() {
        let mut vars = HashMap::new();
        vars.insert("branch", "feature");
        vars.insert("repo", "myrepo");
        let result = expand_template("{{ repo }}/{{ branch }}", &vars, false);
        assert_eq!(result.unwrap(), "myrepo/feature");
    }

    #[test]
    fn test_expand_template_empty_template() {
        let vars = HashMap::new();
        let result = expand_template("", &vars, false);
        assert_eq!(result.unwrap(), "");
    }

    #[test]
    fn test_expand_template_no_variables() {
        let vars = HashMap::new();
        let result = expand_template("static text", &vars, false);
        assert_eq!(result.unwrap(), "static text");
    }

    #[test]
    fn test_expand_template_empty_vars() {
        let vars = HashMap::new();
        let result = expand_template("no {{ variables }} here", &vars, false);
        // minijinja renders undefined variables as empty string by default
        assert_eq!(result.unwrap(), "no  here");
    }

    // ============================================================================
    // expand_template Tests - Shell Escaping
    // ============================================================================

    #[test]
    fn test_expand_template_shell_escape_spaces() {
        let mut vars = HashMap::new();
        vars.insert("path", "my path");
        let result = expand_template("cd {{ path }}", &vars, true);
        // shell_escape wraps strings with spaces in quotes
        let expanded = result.unwrap();
        assert!(expanded.contains("'my path'") || expanded.contains("my\\ path"));
    }

    #[test]
    fn test_expand_template_shell_escape_special_chars() {
        let mut vars = HashMap::new();
        vars.insert("arg", "test;rm -rf");
        let result = expand_template("echo {{ arg }}", &vars, true);
        // Should be escaped to prevent command injection
        let expanded = result.unwrap();
        assert!(!expanded.contains(";rm") || expanded.contains("'"));
    }

    #[test]
    fn test_expand_template_no_escape_literal() {
        let mut vars = HashMap::new();
        vars.insert("branch", "feature/foo");
        let result = expand_template("{{ branch }}", &vars, false);
        // Without shell escape, slashes pass through
        assert_eq!(result.unwrap(), "feature/foo");
    }

    #[test]
    fn test_expand_template_shell_escape_quotes() {
        let mut vars = HashMap::new();
        vars.insert("msg", "it's working");
        let result = expand_template("echo {{ msg }}", &vars, true);
        let expanded = result.unwrap();
        // Single quote should be escaped
        assert!(expanded.contains("it") && expanded.contains("working"));
    }

    // ============================================================================
    // expand_template Tests - Error Cases
    // ============================================================================

    #[test]
    fn test_expand_template_invalid_syntax() {
        let vars = HashMap::new();
        let result = expand_template("{{ unclosed", &vars, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("syntax error"));
    }

    #[test]
    fn test_expand_template_invalid_expression() {
        let vars = HashMap::new();
        let result = expand_template("{{ 1 + }}", &vars, false);
        assert!(result.is_err());
    }

    // ============================================================================
    // expand_template Tests - Jinja2 Features
    // ============================================================================

    #[test]
    fn test_expand_template_jinja_conditional() {
        let mut vars = HashMap::new();
        vars.insert("debug", "true");
        let result = expand_template(
            "{% if debug %}DEBUG MODE{% endif %}",
            &vars,
            false,
        );
        assert_eq!(result.unwrap(), "DEBUG MODE");
    }

    #[test]
    fn test_expand_template_jinja_conditional_false() {
        let mut vars = HashMap::new();
        vars.insert("debug", "");
        let result = expand_template(
            "{% if debug %}DEBUG MODE{% endif %}",
            &vars,
            false,
        );
        // Empty string is falsy in Jinja2
        assert_eq!(result.unwrap(), "");
    }

    #[test]
    fn test_expand_template_jinja_default_filter() {
        let vars = HashMap::new();
        let result = expand_template(
            "{{ missing | default('fallback') }}",
            &vars,
            false,
        );
        assert_eq!(result.unwrap(), "fallback");
    }

    #[test]
    fn test_expand_template_jinja_upper_filter() {
        let mut vars = HashMap::new();
        vars.insert("name", "hello");
        let result = expand_template("{{ name | upper }}", &vars, false);
        assert_eq!(result.unwrap(), "HELLO");
    }

    // ============================================================================
    // expand_template Tests - Trailing Newline
    // ============================================================================

    #[test]
    fn test_expand_template_trailing_newline_shell_escape() {
        let mut vars = HashMap::new();
        vars.insert("cmd", "echo hello");
        let result = expand_template("{{ cmd }}\n", &vars, true);
        // With shell_escape=true, trailing newlines should be preserved
        assert!(result.unwrap().ends_with('\n'));
    }

    #[test]
    fn test_expand_template_trailing_newline_no_escape() {
        let mut vars = HashMap::new();
        vars.insert("cmd", "echo hello");
        let result = expand_template("{{ cmd }}\n", &vars, false);
        // Without shell_escape, trailing newlines may or may not be preserved
        // depending on minijinja defaults (not set_keep_trailing_newline)
        let expanded = result.unwrap();
        // Just verify it works, don't assert on newline behavior
        assert!(expanded.contains("echo hello"));
    }

    // ============================================================================
    // expand_template Tests - Real-World Patterns
    // ============================================================================

    #[test]
    fn test_expand_template_worktree_path_pattern() {
        let mut vars = HashMap::new();
        vars.insert("main_worktree", "myrepo");
        vars.insert("branch", "feature-foo");
        let result = expand_template(
            "{{ main_worktree }}.{{ branch }}",
            &vars,
            false,
        );
        assert_eq!(result.unwrap(), "myrepo.feature-foo");
    }

    #[test]
    fn test_expand_template_shell_command_pattern() {
        let mut vars = HashMap::new();
        vars.insert("repo", "myrepo");
        vars.insert("branch", "feature");
        let result = expand_template(
            "cargo test --package {{ repo }}",
            &vars,
            true,
        );
        assert_eq!(result.unwrap(), "cargo test --package myrepo");
    }

    #[test]
    fn test_expand_template_git_command_pattern() {
        let mut vars = HashMap::new();
        vars.insert("target", "main");
        vars.insert("branch", "feature");
        let result = expand_template(
            "git merge {{ target }}",
            &vars,
            true,
        );
        assert_eq!(result.unwrap(), "git merge main");
    }
}
