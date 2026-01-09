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
//! - `sanitize_db` — Transform to database-safe identifier (`[a-z0-9_]`, max 63 chars)
//!   ```text
//!   {{ branch | sanitize_db }} → feature_auth_oauth2
//!   ```
//!
//! - `hash_port` — Hash a string to a deterministic port number (10000-19999)
//!   ```text
//!   {{ branch | hash_port }}              → 12472
//!   {{ (repo ~ "-" ~ branch) | hash_port }} → 15839
//!   ```

use minijinja::{Environment, Value};

/// Known template variables available in hook commands.
///
/// These are populated by `build_hook_context()` in `command_executor.rs`.
/// Some variables are conditional (e.g., `upstream` only exists if tracking is configured).
///
/// This list is the single source of truth for `--var` validation in CLI.
pub const TEMPLATE_VARS: &[&str] = &[
    "repo",
    "branch",
    "worktree_name",
    "repo_path",
    "worktree_path",
    "default_branch",
    "main_worktree_path",
    "commit",
    "short_commit",
    "remote",
    "remote_url",
    "upstream",
    "target", // Added by merge/rebase hooks via extra_vars
];

/// Deprecated template variable aliases (still valid for backward compatibility).
///
/// These map to current variables:
/// - `main_worktree` → `repo`
/// - `repo_root` → `repo_path`
/// - `worktree` → `worktree_path`
pub const DEPRECATED_TEMPLATE_VARS: &[&str] = &["main_worktree", "repo_root", "worktree"];

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

/// Sanitize a string for use as a database identifier.
///
/// Transforms input into an identifier compatible with most SQL databases
/// (PostgreSQL, MySQL, SQL Server). The transformation is more aggressive than
/// `sanitize_branch_name` to ensure compatibility with database identifier rules.
///
/// # Transformation Rules (applied in order)
/// 1. Convert to lowercase (ensures portability across case-sensitive systems)
/// 2. Replace non-alphanumeric characters with `_` (only `[a-z0-9_]` are safe)
/// 3. Collapse consecutive underscores into single underscore
/// 4. Add `_` prefix if identifier starts with a digit (SQL prohibits leading digits)
/// 5. Truncate to 63 characters (PostgreSQL limit; MySQL=64, SQL Server=128)
///
/// # Limitations
/// - Empty input produces empty output (not a valid identifier in most DBs)
/// - SQL reserved words (e.g., `user`, `select`) are not escaped
/// - Different inputs may collide after transformation (e.g., `a-b` and `a_b`)
///
/// # Examples
/// ```
/// use worktrunk::config::sanitize_db;
///
/// assert_eq!(sanitize_db("feature/auth-oauth2"), "feature_auth_oauth2");
/// assert_eq!(sanitize_db("123-bug-fix"), "_123_bug_fix");
/// assert_eq!(sanitize_db("UPPERCASE.Branch"), "uppercase_branch");
/// ```
pub fn sanitize_db(s: &str) -> String {
    // Single pass: lowercase, replace non-alphanumeric with underscore, collapse consecutive
    let mut result = String::with_capacity(s.len());
    let mut prev_underscore = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            result.push(c.to_ascii_lowercase());
            prev_underscore = false;
        } else if !prev_underscore {
            result.push('_');
            prev_underscore = true;
        }
    }

    // Prefix with underscore if starts with digit
    if result.starts_with(|c: char| c.is_ascii_digit()) {
        result.insert(0, '_');
    }

    // Truncate to 63 characters (PostgreSQL limit)
    // Safe to slice by bytes since output is ASCII-only
    if result.len() > 63 {
        result.truncate(63);
    }

    result
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
/// - `sanitize` — Replace `/` and `\` with `-` for filesystem-safe paths
/// - `sanitize_db` — Transform to database-safe identifier (`[a-z0-9_]`, max 63 chars)
/// - `hash_port` — Hash to deterministic port number (10000-19999)
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
    env.add_filter("sanitize_db", |value: Value| -> String {
        sanitize_db(value.as_str().unwrap_or_default())
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
    fn test_sanitize_db() {
        let cases = [
            // Examples from spec
            ("feature/auth-oauth2", "feature_auth_oauth2"),
            ("123-bug-fix", "_123_bug_fix"),
            ("UPPERCASE.Branch", "uppercase_branch"),
            // Lowercase conversion
            ("MyBranch", "mybranch"),
            ("ALLCAPS", "allcaps"),
            // Non-alphanumeric replacement
            ("feature/foo", "feature_foo"),
            ("feature-bar", "feature_bar"),
            ("feature.baz", "feature_baz"),
            ("feature@qux", "feature_qux"),
            // Consecutive underscore collapse
            ("a--b", "a_b"),
            ("a///b", "a_b"),
            ("a...b", "a_b"),
            ("a-/-b", "a_b"),
            // Leading digit prefix
            ("1branch", "_1branch"),
            ("123", "_123"),
            ("0test", "_0test"),
            // No prefix needed
            ("branch1", "branch1"),
            ("_already", "_already"),
            // Edge cases
            ("", ""),
            ("a", "a"),
            ("_", "_"),
            ("-", "_"),
            ("---", "_"),
            // Mixed cases
            ("Feature/Auth-OAuth2", "feature_auth_oauth2"),
            ("user/TASK/123", "user_task_123"),
            // Non-ASCII characters become underscores
            ("naïve-impl", "na_ve_impl"),
            ("日本語", "_"),
            ("über-feature", "_ber_feature"),
        ];
        for (input, expected) in cases {
            assert_eq!(sanitize_db(input), expected, "input: {input}");
        }
    }

    #[test]
    fn test_sanitize_db_truncation() {
        // Test truncation at 63 characters
        let long_input = "a".repeat(100);
        let result = sanitize_db(&long_input);
        assert_eq!(result.len(), 63);
        assert_eq!(result, "a".repeat(63));

        // Exactly 63 should not be truncated
        let exact = "b".repeat(63);
        assert_eq!(sanitize_db(&exact), exact);

        // 64 should be truncated
        let over = "c".repeat(64);
        assert_eq!(sanitize_db(&over).len(), 63);

        // Truncation happens after prefix is added
        let digit_start = format!("1{}", "x".repeat(100));
        let result = sanitize_db(&digit_start);
        assert_eq!(result.len(), 63);
        assert!(result.starts_with("_1"));
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
    fn test_expand_template_sanitize_db_filter() {
        let mut vars = HashMap::new();

        // Basic transformation
        vars.insert("branch", "feature/auth-oauth2");
        assert_eq!(
            expand_template("{{ branch | sanitize_db }}", &vars, false).unwrap(),
            "feature_auth_oauth2"
        );

        // Leading digit gets underscore prefix
        vars.insert("branch", "123-bug-fix");
        assert_eq!(
            expand_template("{{ branch | sanitize_db }}", &vars, false).unwrap(),
            "_123_bug_fix"
        );

        // Uppercase conversion
        vars.insert("branch", "UPPERCASE.Branch");
        assert_eq!(
            expand_template("{{ branch | sanitize_db }}", &vars, false).unwrap(),
            "uppercase_branch"
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
        let r1 = expand_template("{{ (repo ~ '-' ~ branch) | hash_port }}", &vars, false).unwrap();
        let r1_port: u16 = r1.parse().expect("should be a number");
        let r2 = expand_template("{{ (repo ~ '-' ~ branch) | hash_port }}", &vars, false).unwrap();
        let r2_port: u16 = r2.parse().expect("should be a number");

        assert!((10000..20000).contains(&r1_port));
        assert!((10000..20000).contains(&r2_port));

        assert_eq!(r1, r2);
    }
}
