//! Template expansion utilities for worktrunk
//!
//! Uses minijinja for template rendering. Single generic function with escaping flag:
//! - `shell_escape: true` — Shell-escaped for safe command execution
//! - `shell_escape: false` — Literal values for filesystem paths
//!
//! All templates support Jinja2 syntax including filters, conditionals, and loops.
//!
//! See `wt hook --help` for available filters and functions.

use std::borrow::Cow;

use color_print::cformat;
use minijinja::{Environment, UndefinedBehavior, Value};
use regex::Regex;
use shell_escape::escape;

use crate::git::Repository;
use crate::path::to_posix_path;
use crate::styling::{eprintln, format_with_gutter, info_message, verbosity};

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
    "primary_worktree_path",
    "commit",
    "short_commit",
    "remote",
    "remote_url",
    "upstream",
    "target",             // Added by merge/rebase hooks via extra_vars
    "base",               // Added by creation hooks via extra_vars
    "base_worktree_path", // Added by creation hooks via extra_vars
];

/// Deprecated template variable aliases (still valid for backward compatibility).
///
/// These map to current variables:
/// - `main_worktree` → `repo`
/// - `repo_root` → `repo_path`
/// - `worktree` → `worktree_path`
/// - `main_worktree_path` → `primary_worktree_path`
pub const DEPRECATED_TEMPLATE_VARS: &[&str] = &[
    "main_worktree",
    "repo_root",
    "worktree",
    "main_worktree_path",
];

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
/// 5. Append 3-character hash suffix for uniqueness (avoids reserved words and collisions)
/// 6. Truncate to 63 characters (PostgreSQL limit; MySQL=64, SQL Server=128)
///
/// The hash suffix ensures that:
/// - SQL reserved words are avoided (e.g., `user` → `user_abc`, not a reserved word)
/// - Different inputs don't collide (e.g., `a-b` and `a_b` get different suffixes)
///
/// # Limitations
/// - Empty input produces empty output (not a valid identifier in most DBs)
///
/// # Examples
/// ```
/// use worktrunk::config::sanitize_db;
///
/// // Hash suffix ensures uniqueness
/// assert!(sanitize_db("feature/auth").starts_with("feature_auth_"));
/// assert!(sanitize_db("123-bug-fix").starts_with("_123_bug_fix_"));
/// assert!(sanitize_db("UPPERCASE.Branch").starts_with("uppercase_branch_"));
///
/// // Different inputs get different suffixes even if base transforms are identical
/// assert_ne!(sanitize_db("a-b"), sanitize_db("a_b"));
/// ```
pub fn sanitize_db(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }

    // Single pass: lowercase, replace non-alphanumeric with underscore, collapse consecutive
    let mut result = String::with_capacity(s.len() + 4); // +4 for _xxx suffix
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

    // Truncate base to leave room for hash suffix (4 chars: _ + 3 hash chars)
    // PostgreSQL limit is 63, so max base is 59
    if result.len() > 59 {
        result.truncate(59);
    }

    // Append 3-character hash suffix for collision avoidance and reserved word safety
    // Hash is computed from original input, ensuring unique suffixes for colliding transforms
    if !result.ends_with('_') {
        result.push('_');
    }
    result.push_str(&short_hash(s));

    result
}

/// Generate a 3-character hash suffix from a string.
///
/// Uses base36 (0-9, a-z) for a compact representation with 46,656 unique values.
/// Used by `sanitize_db` and `sanitize_for_filename` to avoid collisions.
pub fn short_hash(s: &str) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    let hash = h.finish();

    // Convert to base36 and take 3 characters
    const CHARS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let c0 = CHARS[(hash % 36) as usize];
    let c1 = CHARS[((hash / 36) % 36) as usize];
    let c2 = CHARS[((hash / 1296) % 36) as usize];
    String::from_utf8(vec![c0, c1, c2]).unwrap()
}

/// Redact credentials from URLs for safe logging.
///
/// URLs with embedded credentials (e.g., `https://token@github.com/...`) have
/// the credential portion replaced with `[REDACTED]`.
///
/// # Examples
/// ```
/// use worktrunk::config::redact_credentials;
///
/// // URLs with credentials are redacted
/// assert_eq!(
///     redact_credentials("https://ghp_token123@github.com/owner/repo"),
///     "https://[REDACTED]@github.com/owner/repo"
/// );
///
/// // URLs without credentials are unchanged
/// assert_eq!(
///     redact_credentials("https://github.com/owner/repo"),
///     "https://github.com/owner/repo"
/// );
///
/// // Non-URL values pass through unchanged
/// assert_eq!(redact_credentials("main"), "main");
/// ```
pub fn redact_credentials(s: &str) -> String {
    // Pattern: scheme://credentials@host where credentials don't contain @
    // This matches URLs like https://token@github.com or https://user:pass@host.com
    thread_local! {
        static CREDENTIAL_URL: Regex = Regex::new(r"^([a-z][a-z0-9+.-]*://)([^@/]+)@").unwrap();
    }
    CREDENTIAL_URL.with(|re| re.replace(s, "${1}[REDACTED]@").into_owned())
}

/// Expand a template with variable substitution.
///
/// # Arguments
/// * `template` - Template string using Jinja2 syntax (e.g., `{{ branch }}`)
/// * `vars` - Variables to substitute
/// * `shell_escape` - If true, shell-escape all values for safe command execution.
///   If false, substitute values literally (for filesystem paths).
/// * `repo` - Repository for looking up worktree paths
///
/// # Filters
/// - `sanitize` — Replace `/` and `\` with `-` for filesystem-safe paths
/// - `sanitize_db` — Transform to database-safe identifier (`[a-z0-9_]`, max 63 chars)
/// - `hash_port` — Hash to deterministic port number (10000-19999)
///
/// # Functions
/// - `worktree_path_of_branch(branch)` — Look up the filesystem path of a branch's worktree
///   Returns empty string if branch has no worktree.
///
/// The `name` parameter appears in error messages to help identify which template failed.
pub fn expand_template(
    template: &str,
    vars: &HashMap<&str, &str>,
    shell_escape: bool,
    repo: &Repository,
    name: &str,
) -> Result<String, String> {
    // Build context map with raw values (shell escaping is applied at output time via formatter)
    let mut context = HashMap::new();
    for (key, value) in vars {
        context.insert(
            key.to_string(),
            minijinja::Value::from((*value).to_string()),
        );
    }

    // Render template with minijinja
    let mut env = Environment::new();
    // SemiStrict: errors on undefined variable use (printing, iteration) but allows
    // truthiness checks ({% if var %}). This catches typos while supporting optional vars.
    env.set_undefined_behavior(UndefinedBehavior::SemiStrict);
    if shell_escape {
        // Preserve trailing newlines in templates (important for multiline shell commands)
        env.set_keep_trailing_newline(true);

        // Shell-escape values at output time, not before template rendering.
        // This ensures filters (sanitize, sanitize_db, etc.) operate on raw values
        // and the escaping is applied to the final output, preventing corruption
        // when filters modify already-escaped strings.
        env.set_formatter(|out, _state, value| {
            if value.is_none() {
                return Ok(());
            }
            let s = value.to_string();
            let escaped = escape(Cow::Borrowed(&s));
            write!(out, "{escaped}")?;
            Ok(())
        });
    }

    // Register custom filters
    env.add_filter("sanitize", |value: Value| -> String {
        sanitize_branch_name(value.as_str().unwrap_or_default())
    });
    env.add_filter("sanitize_db", |value: Value| -> String {
        sanitize_db(value.as_str().unwrap_or_default())
    });
    env.add_filter("hash_port", |value: String| string_to_port(&value));

    // Register worktree_path_of_branch function for looking up branch worktree paths.
    // Returns raw paths — shell escaping is applied by the formatter at output time.
    let repo_clone = repo.clone();
    env.add_function("worktree_path_of_branch", move |branch: String| -> String {
        repo_clone
            .worktree_for_branch(&branch)
            .ok()
            .flatten()
            .map(|p| to_posix_path(&p.to_string_lossy()))
            .unwrap_or_default()
    });

    // Cache verbosity level for consistent behavior within this call
    let verbose = verbosity();

    // -vv: Full debug logging with vars
    // Redact credentials from values to prevent leaking tokens in logs
    if verbose >= 2 {
        log::debug!("[template:{name}] template={template:?}");
        // Sort keys for deterministic output in tests
        let mut sorted_vars: Vec<_> = vars.iter().collect();
        sorted_vars.sort_by_key(|(k, _)| *k);
        log::debug!(
            "[template:{name}] vars={{{}}}",
            sorted_vars
                .iter()
                .map(|(k, v)| format!("{k}={:?}", redact_credentials(v)))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let tmpl = env
        .template_from_named_str(name, template)
        .map_err(|e| format!("Template syntax error: {}", e))?;

    let result = tmpl
        .render(minijinja::Value::from_object(context))
        .map_err(|e| format!("Template render error: {}", e))?;

    // -vv: Full debug logging with result
    // Redact credentials from result to prevent leaking tokens in logs
    if verbose >= 2 {
        log::debug!("[template:{name}] result={:?}", redact_credentials(&result));
    }

    // -v: Nice styled output showing template expansion
    // Info message for header, gutter for quoted content (template → result)
    // Single atomic write to avoid interleaving in multi-threaded execution
    if verbose == 1 {
        let header = info_message(cformat!("Expanding <bold>{name}</>"));
        let content = if template.contains('\n') || result.contains('\n') {
            // Multiline: template lines, dim →, result lines
            cformat!("{template}\n<dim>→</>\n{result}")
        } else {
            // Single line: template → result
            cformat!("{template} <dim>→</> {result}")
        };
        let gutter = format_with_gutter(&content, None);
        eprintln!("{header}\n{gutter}");
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test fixture that creates a real temporary git repository.
    struct TestRepo {
        _dir: tempfile::TempDir,
        repo: Repository,
    }

    impl TestRepo {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            std::process::Command::new("git")
                .args(["init"])
                .current_dir(dir.path())
                .output()
                .unwrap();
            let repo = Repository::at(dir.path()).unwrap();
            Self { _dir: dir, repo }
        }
    }

    fn test_repo() -> TestRepo {
        TestRepo::new()
    }

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
        // Test that base transformations are correct (ignore hash suffix)
        let cases = [
            // Examples from spec
            ("feature/auth-oauth2", "feature_auth_oauth2_"),
            ("123-bug-fix", "_123_bug_fix_"),
            ("UPPERCASE.Branch", "uppercase_branch_"),
            // Lowercase conversion
            ("MyBranch", "mybranch_"),
            ("ALLCAPS", "allcaps_"),
            // Non-alphanumeric replacement
            ("feature/foo", "feature_foo_"),
            ("feature-bar", "feature_bar_"),
            ("feature.baz", "feature_baz_"),
            ("feature@qux", "feature_qux_"),
            // Consecutive underscore collapse
            ("a--b", "a_b_"),
            ("a///b", "a_b_"),
            ("a...b", "a_b_"),
            ("a-/-b", "a_b_"),
            // Leading digit prefix
            ("1branch", "_1branch_"),
            ("123", "_123_"),
            ("0test", "_0test_"),
            // No prefix needed
            ("branch1", "branch1_"),
            ("_already", "_already_"),
            // Edge cases (non-empty)
            ("a", "a_"),
            // Mixed cases
            ("Feature/Auth-OAuth2", "feature_auth_oauth2_"),
            ("user/TASK/123", "user_task_123_"),
            // Non-ASCII characters become underscores
            ("naïve-impl", "na_ve_impl_"),
            ("über-feature", "_ber_feature_"),
        ];
        for (input, expected_prefix) in cases {
            let result = sanitize_db(input);
            assert!(
                result.starts_with(expected_prefix),
                "input: {input}, expected prefix: {expected_prefix}, got: {result}"
            );
            // Result should be prefix + 3-char hash
            assert_eq!(
                result.len(),
                expected_prefix.len() + 3,
                "input: {input}, result: {result}"
            );
        }

        // Empty input stays empty (no hash suffix)
        assert_eq!(sanitize_db(""), "");

        // Special cases that collapse to just underscore + hash
        for input in ["_", "-", "---", "日本語"] {
            let result = sanitize_db(input);
            assert!(result.starts_with('_'), "input: {input}, got: {result}");
            assert_eq!(result.len(), 4, "input: {input}, got: {result}"); // _xxx
        }
    }

    #[test]
    fn test_sanitize_db_collision_avoidance() {
        // Different inputs that would collide without hash suffix now differ
        assert_ne!(sanitize_db("a-b"), sanitize_db("a_b"));
        assert_ne!(sanitize_db("feature/auth"), sanitize_db("feature-auth"));
        assert_ne!(sanitize_db("UPPERCASE"), sanitize_db("uppercase"));

        // Same input always produces same output (deterministic)
        assert_eq!(sanitize_db("test"), sanitize_db("test"));
        assert_eq!(sanitize_db("feature/foo"), sanitize_db("feature/foo"));
    }

    #[test]
    fn test_sanitize_db_reserved_words() {
        // Reserved words get hash suffix, making them safe
        let user = sanitize_db("user");
        assert!(user.starts_with("user_"), "got: {user}");
        assert_ne!(user, "user"); // Not a bare reserved word

        let select = sanitize_db("select");
        assert!(select.starts_with("select_"), "got: {select}");
        assert_ne!(select, "select");
    }

    #[test]
    fn test_sanitize_db_truncation() {
        // Total output is always max 63 characters
        // Base is truncated to 59 chars, then _xxx suffix (4 chars) is added

        // Very long input: base truncated to 59, + 4 = 63
        let long_input = "a".repeat(100);
        let result = sanitize_db(&long_input);
        assert_eq!(result.len(), 63, "result: {result}");
        assert!(result.starts_with(&"a".repeat(58)), "result: {result}");
        assert!(!result.ends_with('_'), "should end with hash chars");

        // Short input: base + _ + hash
        let short = "test";
        let result = sanitize_db(short);
        assert!(result.starts_with("test_"), "result: {result}");
        assert_eq!(result.len(), 8, "result: {result}"); // test_ + 3 hash chars

        // Truncation happens after prefix is added for digit-starting inputs
        let digit_start = format!("1{}", "x".repeat(100));
        let result = sanitize_db(&digit_start);
        assert_eq!(result.len(), 63, "result: {result}");
        assert!(result.starts_with("_1"), "result: {result}");
    }

    #[test]
    fn test_expand_template_basic() {
        let test = test_repo();

        // Single variable
        let mut vars = HashMap::new();
        vars.insert("name", "world");
        assert_eq!(
            expand_template("Hello {{ name }}", &vars, false, &test.repo, "test").unwrap(),
            "Hello world"
        );

        // Multiple variables
        vars.insert("repo", "myrepo");
        assert_eq!(
            expand_template("{{ repo }}/{{ name }}", &vars, false, &test.repo, "test").unwrap(),
            "myrepo/world"
        );

        // Empty/static cases
        let empty: HashMap<&str, &str> = HashMap::new();
        assert_eq!(
            expand_template("", &empty, false, &test.repo, "test").unwrap(),
            ""
        );
        assert_eq!(
            expand_template("static text", &empty, false, &test.repo, "test").unwrap(),
            "static text"
        );
        // Undefined variables now error in SemiStrict mode
        assert!(
            expand_template("no {{ variables }} here", &empty, false, &test.repo, "test")
                .unwrap_err()
                .contains("undefined")
        );
    }

    #[test]
    fn test_expand_template_shell_escape() {
        let test = test_repo();
        let mut vars = HashMap::new();
        vars.insert("path", "my path");
        let expanded = expand_template("cd {{ path }}", &vars, true, &test.repo, "test").unwrap();
        assert!(expanded.contains("'my path'") || expanded.contains("my\\ path"));

        // Command injection prevention
        vars.insert("arg", "test;rm -rf");
        let expanded = expand_template("echo {{ arg }}", &vars, true, &test.repo, "test").unwrap();
        assert!(!expanded.contains(";rm") || expanded.contains("'"));

        // No escape for literal mode
        vars.insert("branch", "feature/foo");
        assert_eq!(
            expand_template("{{ branch }}", &vars, false, &test.repo, "test").unwrap(),
            "feature/foo"
        );
    }

    #[test]
    fn test_expand_template_errors() {
        let test = test_repo();
        let vars = HashMap::new();
        assert!(
            expand_template("{{ unclosed", &vars, false, &test.repo, "test")
                .unwrap_err()
                .contains("syntax error")
        );
        assert!(expand_template("{{ 1 + }}", &vars, false, &test.repo, "test").is_err());
    }

    #[test]
    fn test_expand_template_jinja_features() {
        let test = test_repo();
        let mut vars = HashMap::new();
        vars.insert("debug", "true");
        assert_eq!(
            expand_template(
                "{% if debug %}DEBUG{% endif %}",
                &vars,
                false,
                &test.repo,
                "test"
            )
            .unwrap(),
            "DEBUG"
        );

        vars.insert("debug", "");
        assert_eq!(
            expand_template(
                "{% if debug %}DEBUG{% endif %}",
                &vars,
                false,
                &test.repo,
                "test"
            )
            .unwrap(),
            ""
        );

        let empty: HashMap<&str, &str> = HashMap::new();
        assert_eq!(
            expand_template(
                "{{ missing | default('fallback') }}",
                &empty,
                false,
                &test.repo,
                "test",
            )
            .unwrap(),
            "fallback"
        );

        vars.insert("name", "hello");
        assert_eq!(
            expand_template("{{ name | upper }}", &vars, false, &test.repo, "test").unwrap(),
            "HELLO"
        );
    }

    #[test]
    fn test_expand_template_strip_prefix() {
        let test = test_repo();
        let mut vars = HashMap::new();

        // Built-in replace filter strips prefix (replaces all occurrences)
        vars.insert("branch", "feature/foo");
        assert_eq!(
            expand_template(
                "{{ branch | replace('feature/', '') }}",
                &vars,
                false,
                &test.repo,
                "test"
            )
            .unwrap(),
            "foo"
        );

        // Replace + sanitize for worktree paths
        assert_eq!(
            expand_template(
                "{{ branch | replace('feature/', '') | sanitize }}",
                &vars,
                false,
                &test.repo,
                "test"
            )
            .unwrap(),
            "foo"
        );

        // Branch without prefix passes through unchanged
        vars.insert("branch", "main");
        assert_eq!(
            expand_template(
                "{{ branch | replace('feature/', '') }}",
                &vars,
                false,
                &test.repo,
                "test"
            )
            .unwrap(),
            "main"
        );

        // Slicing for prefix-only removal (avoids replacing mid-string)
        vars.insert("branch", "feature/nested/feature/deep");
        assert_eq!(
            expand_template("{{ branch[8:] }}", &vars, false, &test.repo, "test").unwrap(),
            "nested/feature/deep"
        );

        // Conditional slicing for safe prefix removal
        assert_eq!(
            expand_template(
                "{% if branch[:8] == 'feature/' %}{{ branch[8:] }}{% else %}{{ branch }}{% endif %}",
                &vars,
                false,
                &test.repo,
                "test"
            )
            .unwrap(),
            "nested/feature/deep"
        );

        // Conditional passes through non-matching branches
        vars.insert("branch", "bugfix/bar");
        assert_eq!(
            expand_template(
                "{% if branch[:8] == 'feature/' %}{{ branch[8:] }}{% else %}{{ branch }}{% endif %}",
                &vars,
                false,
                &test.repo,
                "test"
            )
            .unwrap(),
            "bugfix/bar"
        );
    }

    #[test]
    fn test_expand_template_sanitize_filter() {
        let test = test_repo();
        let mut vars = HashMap::new();
        vars.insert("branch", "feature/foo");
        assert_eq!(
            expand_template("{{ branch | sanitize }}", &vars, false, &test.repo, "test").unwrap(),
            "feature-foo"
        );

        // Backslashes are also sanitized
        vars.insert("branch", "feature\\bar");
        assert_eq!(
            expand_template("{{ branch | sanitize }}", &vars, false, &test.repo, "test").unwrap(),
            "feature-bar"
        );

        // Multiple slashes
        vars.insert("branch", "user/feature/task");
        assert_eq!(
            expand_template("{{ branch | sanitize }}", &vars, false, &test.repo, "test").unwrap(),
            "user-feature-task"
        );

        // Raw branch is unchanged
        vars.insert("branch", "feature/foo");
        assert_eq!(
            expand_template("{{ branch }}", &vars, false, &test.repo, "test").unwrap(),
            "feature/foo"
        );

        // Shell escaping + sanitize: filters operate on raw values, escaping applied at output.
        // Previously, shell escaping was applied BEFORE filters, corrupting the result
        // when values contained shell-special characters (quotes, backslashes).
        vars.insert("branch", "user's/feature");
        let result =
            expand_template("{{ branch | sanitize }}", &vars, true, &test.repo, "test").unwrap();
        // sanitize replaces / with -, producing "user's-feature"
        // shell_escape wraps it: 'user'\''s-feature' (valid shell for user's-feature)
        assert_eq!(result, "'user'\\''s-feature'", "sanitize + shell escape");

        // Without the fix, pre-escaping would produce corrupted output because
        // sanitize would replace the / and \ in the already-escaped value.

        // Shell escaping without filter: raw value with special chars
        let result = expand_template("{{ branch }}", &vars, true, &test.repo, "test").unwrap();
        // shell_escape wraps: 'user'\''s/feature' (valid shell for user's/feature)
        assert_eq!(
            result, "'user'\\''s/feature'",
            "shell escape without filter"
        );

        // Shell-escape formatter handles none values (renders as empty string)
        let result =
            expand_template("prefix-{{ none }}-suffix", &vars, true, &test.repo, "test").unwrap();
        assert_eq!(result, "prefix--suffix", "none renders as empty");
    }

    #[test]
    fn test_expand_template_sanitize_db_filter() {
        let test = test_repo();
        let mut vars = HashMap::new();

        // Basic transformation (with hash suffix)
        vars.insert("branch", "feature/auth-oauth2");
        let result = expand_template(
            "{{ branch | sanitize_db }}",
            &vars,
            false,
            &test.repo,
            "test",
        )
        .unwrap();
        assert!(result.starts_with("feature_auth_oauth2_"), "got: {result}");

        // Leading digit gets underscore prefix
        vars.insert("branch", "123-bug-fix");
        let result = expand_template(
            "{{ branch | sanitize_db }}",
            &vars,
            false,
            &test.repo,
            "test",
        )
        .unwrap();
        assert!(result.starts_with("_123_bug_fix_"), "got: {result}");

        // Uppercase conversion
        vars.insert("branch", "UPPERCASE.Branch");
        let result = expand_template(
            "{{ branch | sanitize_db }}",
            &vars,
            false,
            &test.repo,
            "test",
        )
        .unwrap();
        assert!(result.starts_with("uppercase_branch_"), "got: {result}");

        // Raw branch is unchanged
        vars.insert("branch", "feature/foo");
        assert_eq!(
            expand_template("{{ branch }}", &vars, false, &test.repo, "test").unwrap(),
            "feature/foo"
        );
    }

    #[test]
    fn test_expand_template_trailing_newline() {
        let test = test_repo();
        let mut vars = HashMap::new();
        vars.insert("cmd", "echo hello");
        assert!(
            expand_template("{{ cmd }}\n", &vars, true, &test.repo, "test")
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
        let test = test_repo();
        let mut vars = HashMap::new();
        vars.insert("branch", "feature-foo");
        vars.insert("repo", "myrepo");

        // Filter produces a number in range
        let result =
            expand_template("{{ branch | hash_port }}", &vars, false, &test.repo, "test").unwrap();
        let port: u16 = result.parse().expect("should be a number");
        assert!((10000..20000).contains(&port));

        // Concatenation produces different (but deterministic) result
        let r1 = expand_template(
            "{{ (repo ~ '-' ~ branch) | hash_port }}",
            &vars,
            false,
            &test.repo,
            "test",
        )
        .unwrap();
        let r1_port: u16 = r1.parse().expect("should be a number");
        let r2 = expand_template(
            "{{ (repo ~ '-' ~ branch) | hash_port }}",
            &vars,
            false,
            &test.repo,
            "test",
        )
        .unwrap();
        let r2_port: u16 = r2.parse().expect("should be a number");

        assert!((10000..20000).contains(&r1_port));
        assert!((10000..20000).contains(&r2_port));

        assert_eq!(r1, r2);
    }

    #[test]
    fn test_redact_credentials_https_token() {
        // GitHub-style personal access token
        assert_eq!(
            redact_credentials("https://ghp_token123@github.com/owner/repo"),
            "https://[REDACTED]@github.com/owner/repo"
        );
        // GitLab-style token
        assert_eq!(
            redact_credentials("https://glpat-xxxxxxxxxxxx@gitlab.com/owner/repo.git"),
            "https://[REDACTED]@gitlab.com/owner/repo.git"
        );
    }

    #[test]
    fn test_redact_credentials_https_user_pass() {
        // Username:password format
        assert_eq!(
            redact_credentials("https://user:password123@github.com/owner/repo"),
            "https://[REDACTED]@github.com/owner/repo"
        );
    }

    #[test]
    fn test_redact_credentials_no_credentials() {
        // Normal HTTPS URL without credentials - unchanged
        assert_eq!(
            redact_credentials("https://github.com/owner/repo"),
            "https://github.com/owner/repo"
        );
        // SSH URL - unchanged (no credentials in URL format)
        assert_eq!(
            redact_credentials("git@github.com:owner/repo.git"),
            "git@github.com:owner/repo.git"
        );
    }

    #[test]
    fn test_redact_credentials_non_url() {
        // Non-URL values pass through unchanged
        assert_eq!(redact_credentials("main"), "main");
        assert_eq!(redact_credentials("feature/auth"), "feature/auth");
        assert_eq!(redact_credentials("/path/to/worktree"), "/path/to/worktree");
        assert_eq!(redact_credentials(""), "");
    }

    #[test]
    fn test_redact_credentials_git_protocol() {
        // git:// protocol with credentials
        assert_eq!(
            redact_credentials("git://token@github.com/owner/repo.git"),
            "git://[REDACTED]@github.com/owner/repo.git"
        );
    }

    #[test]
    fn test_redact_credentials_preserves_path() {
        // Full URL with path and query should preserve everything after host
        assert_eq!(
            redact_credentials("https://token@github.com/owner/repo.git?ref=main"),
            "https://[REDACTED]@github.com/owner/repo.git?ref=main"
        );
    }
}
