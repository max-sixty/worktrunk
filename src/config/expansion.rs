//! Template expansion utilities for worktrunk
//!
//! Uses minijinja for template rendering. Single generic function with escaping flag:
//! - `shell_escape: true` — Shell-escaped for safe command execution
//! - `shell_escape: false` — Literal values for filesystem paths
//!
//! All templates support Jinja2 syntax including filters, conditionals, and loops.
//!
//! # Custom Filters
//!
//! - `sanitize` — Replace `/` and `\` with `-` for filesystem-safe names
//!   ```text
//!   {{ branch | sanitize }} → feature-foo
//!   ```
//!
//! - `hash_port` — Hash a string to a deterministic port number (10000-19999)
//!   ```text
//!   {{ branch | hash_port }}              → 12472
//!   {{ repo ~ "-" ~ branch | hash_port }} → 15839
//!   ```

use minijinja::{Environment, Value};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// Hash a string to a port in range 10000-19999.
fn string_to_port(s: &str) -> u16 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    10000 + (h.finish() % 10000) as u16
}

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
/// * `vars` - Variables to substitute
/// * `shell_escape` - If true, shell-escape all values for safe command execution.
///   If false, substitute values literally (for filesystem paths).
///
/// # Filters
/// The `sanitize` filter is available for branch names, replacing `/` and `\` with `-`:
/// - `{{ branch }}` — raw branch name (e.g., `feature/foo`)
/// - `{{ branch | sanitize }}` — sanitized for paths (e.g., `feature-foo`)
///
/// # Examples
/// ```
/// use worktrunk::config::expand_template;
/// use std::collections::HashMap;
///
/// // Raw branch name
/// let mut vars = HashMap::new();
/// vars.insert("branch", "feature/foo");
/// vars.insert("repo", "myrepo");
/// let cmd = expand_template("echo {{ branch }} in {{ repo }}", &vars, true).unwrap();
/// assert_eq!(cmd, "echo feature/foo in myrepo");
///
/// // Sanitized branch name for filesystem paths
/// let mut vars = HashMap::new();
/// vars.insert("branch", "feature/foo");
/// vars.insert("main_worktree", "myrepo");
/// let path = expand_template("{{ main_worktree }}.{{ branch | sanitize }}", &vars, false).unwrap();
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

    // Register custom filters
    env.add_filter("sanitize", |value: Value| -> String {
        sanitize_branch_name(value.as_str().unwrap_or_default())
    });
    env.add_filter("hash_port", |value: String| string_to_port(&value));

    let tmpl = env
        .template_from_str(template)
        .map_err(|e| format!("Template syntax error: {}", e))?;

    tmpl.render(minijinja::Value::from_object(context))
        .map_err(|e| format!("Template render error: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_branch_name() {
        let cases = [
            ("feature/foo", "feature-foo"),
            ("user\\task", "user-task"),
            ("feature/user/task", "feature-user-task"),
            ("feature/user\\task", "feature-user-task"),
            ("simple-branch", "simple-branch"),
            ("", ""),
            ("///", "---"),
            ("/feature", "-feature"),
            ("feature/", "feature-"),
        ];
        for (input, expected) in cases {
            assert_eq!(sanitize_branch_name(input), expected, "input: {input}");
        }
    }

    #[test]
    fn test_expand_template_basic() {
        // Single variable
        let mut vars = HashMap::new();
        vars.insert("name", "world");
        assert_eq!(
            expand_template("Hello {{ name }}", &vars, false).unwrap(),
            "Hello world"
        );

        // Multiple variables
        vars.insert("repo", "myrepo");
        assert_eq!(
            expand_template("{{ repo }}/{{ name }}", &vars, false).unwrap(),
            "myrepo/world"
        );

        // Empty/static cases
        let empty: HashMap<&str, &str> = HashMap::new();
        assert_eq!(expand_template("", &empty, false).unwrap(), "");
        assert_eq!(
            expand_template("static text", &empty, false).unwrap(),
            "static text"
        );
        assert_eq!(
            expand_template("no {{ variables }} here", &empty, false).unwrap(),
            "no  here"
        );
    }

    #[test]
    fn test_expand_template_shell_escape() {
        let mut vars = HashMap::new();
        vars.insert("path", "my path");
        let expanded = expand_template("cd {{ path }}", &vars, true).unwrap();
        assert!(expanded.contains("'my path'") || expanded.contains("my\\ path"));

        // Command injection prevention
        vars.insert("arg", "test;rm -rf");
        let expanded = expand_template("echo {{ arg }}", &vars, true).unwrap();
        assert!(!expanded.contains(";rm") || expanded.contains("'"));

        // No escape for literal mode
        vars.insert("branch", "feature/foo");
        assert_eq!(
            expand_template("{{ branch }}", &vars, false).unwrap(),
            "feature/foo"
        );
    }

    #[test]
    fn test_expand_template_errors() {
        let vars = HashMap::new();
        assert!(
            expand_template("{{ unclosed", &vars, false)
                .unwrap_err()
                .contains("syntax error")
        );
        assert!(expand_template("{{ 1 + }}", &vars, false).is_err());
    }

    #[test]
    fn test_expand_template_jinja_features() {
        let mut vars = HashMap::new();
        vars.insert("debug", "true");
        assert_eq!(
            expand_template("{% if debug %}DEBUG{% endif %}", &vars, false).unwrap(),
            "DEBUG"
        );

        vars.insert("debug", "");
        assert_eq!(
            expand_template("{% if debug %}DEBUG{% endif %}", &vars, false).unwrap(),
            ""
        );

        let empty: HashMap<&str, &str> = HashMap::new();
        assert_eq!(
            expand_template("{{ missing | default('fallback') }}", &empty, false).unwrap(),
            "fallback"
        );

        vars.insert("name", "hello");
        assert_eq!(
            expand_template("{{ name | upper }}", &vars, false).unwrap(),
            "HELLO"
        );
    }

    #[test]
    fn test_expand_template_sanitize_filter() {
        let mut vars = HashMap::new();
        vars.insert("branch", "feature/foo");
        assert_eq!(
            expand_template("{{ branch | sanitize }}", &vars, false).unwrap(),
            "feature-foo"
        );

        // Backslashes are also sanitized
        vars.insert("branch", "feature\\bar");
        assert_eq!(
            expand_template("{{ branch | sanitize }}", &vars, false).unwrap(),
            "feature-bar"
        );

        // Multiple slashes
        vars.insert("branch", "user/feature/task");
        assert_eq!(
            expand_template("{{ branch | sanitize }}", &vars, false).unwrap(),
            "user-feature-task"
        );

        // Raw branch is unchanged
        vars.insert("branch", "feature/foo");
        assert_eq!(
            expand_template("{{ branch }}", &vars, false).unwrap(),
            "feature/foo"
        );
    }

    #[test]
    fn test_expand_template_trailing_newline() {
        let mut vars = HashMap::new();
        vars.insert("cmd", "echo hello");
        assert!(
            expand_template("{{ cmd }}\n", &vars, true)
                .unwrap()
                .ends_with('\n')
        );
    }

    #[test]
    fn test_string_to_port_deterministic_and_in_range() {
        for input in ["main", "feature-foo", "", "a", "long-branch-name-123"] {
            let p1 = string_to_port(input);
            let p2 = string_to_port(input);
            assert_eq!(p1, p2, "same input should produce same port");
            assert!((10000..20000).contains(&p1), "port {} out of range", p1);
        }
    }

    #[test]
    fn test_hash_port_filter() {
        let mut vars = HashMap::new();
        vars.insert("branch", "feature-foo");
        vars.insert("repo", "myrepo");

        // Filter produces a number in range
        let result = expand_template("{{ branch | hash_port }}", &vars, false).unwrap();
        let port: u16 = result.parse().expect("should be a number");
        assert!((10000..20000).contains(&port));

        // Concatenation produces different (but deterministic) result
        let r1 = expand_template("{{ repo ~ '-' ~ branch | hash_port }}", &vars, false).unwrap();
        let r2 = expand_template("{{ repo ~ '-' ~ branch | hash_port }}", &vars, false).unwrap();
        assert_eq!(r1, r2);
    }
}
