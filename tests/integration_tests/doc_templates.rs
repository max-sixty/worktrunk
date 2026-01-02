//! Tests for template examples shown in documentation.
//!
//! These tests verify that template expressions documented in docs/content/ behave
//! as described. This catches operator precedence issues like the one fixed in PR #373
//! where `{{ 'db-' ~ branch | hash_port }}` was incorrectly documented without parentheses.
//!
//! Run with: `cargo test --test integration doc_templates`

use std::collections::HashMap;
use worktrunk::config::expand_template;

/// Helper to compute hash_port for a string.
///
/// Must match `string_to_port()` in `src/config/expansion.rs`.
fn hash_port(s: &str) -> u16 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    10000 + (h.finish() % 10000) as u16
}

// =============================================================================
// Basic Variables (docs/content/hook.md: Template variables table)
// =============================================================================

#[test]
fn test_doc_basic_variables() {
    let mut vars = HashMap::new();
    vars.insert("repo", "myproject");
    vars.insert("branch", "feature/auth");
    vars.insert("worktree", "/home/user/myproject.feature-auth");
    vars.insert("default_branch", "main");

    // Each variable substitutes correctly
    assert_eq!(
        expand_template("{{ repo }}", &vars, false).unwrap(),
        "myproject"
    );
    assert_eq!(
        expand_template("{{ branch }}", &vars, false).unwrap(),
        "feature/auth"
    );
    assert_eq!(
        expand_template("{{ worktree }}", &vars, false).unwrap(),
        "/home/user/myproject.feature-auth"
    );
    assert_eq!(
        expand_template("{{ default_branch }}", &vars, false).unwrap(),
        "main"
    );
}

// =============================================================================
// Sanitize Filter (docs/content/hook.md: Filters table)
// "Replace `/` and `\` with `-`"
// =============================================================================

#[test]
fn test_doc_sanitize_filter() {
    let mut vars = HashMap::new();

    // From docs: {{ branch | sanitize }} replaces / and \ with -
    vars.insert("branch", "feature/foo");
    assert_eq!(
        expand_template("{{ branch | sanitize }}", &vars, false).unwrap(),
        "feature-foo",
        "sanitize should replace / with -"
    );

    vars.insert("branch", "user\\task");
    assert_eq!(
        expand_template("{{ branch | sanitize }}", &vars, false).unwrap(),
        "user-task",
        "sanitize should replace \\ with -"
    );

    // Nested paths
    vars.insert("branch", "user/feature/task");
    assert_eq!(
        expand_template("{{ branch | sanitize }}", &vars, false).unwrap(),
        "user-feature-task",
        "sanitize should handle multiple slashes"
    );
}

// =============================================================================
// Hash Port Filter (docs/content/hook.md: Filters table)
// "Hash to port 10000-19999"
// =============================================================================

#[test]
fn test_doc_hash_port_filter() {
    let mut vars = HashMap::new();
    vars.insert("branch", "feature-foo");

    let result = expand_template("{{ branch | hash_port }}", &vars, false).unwrap();
    let port: u16 = result.parse().expect("hash_port should produce a number");

    assert!(
        (10000..20000).contains(&port),
        "hash_port should produce port in range 10000-19999, got {port}"
    );

    // Deterministic
    let result2 = expand_template("{{ branch | hash_port }}", &vars, false).unwrap();
    assert_eq!(result, result2, "hash_port should be deterministic");
}

// =============================================================================
// Concatenation with hash_port (docs/content/tips-patterns.md)
// CRITICAL: These test the operator precedence issue from PR #373
// =============================================================================

#[test]
fn test_doc_hash_port_concatenation_precedence() {
    // From docs/content/tips-patterns.md:
    // "The `'db-' ~ branch` concatenation hashes differently than plain `branch`"
    //
    // The docs show: {{ ('db-' ~ branch) | hash_port }}
    // This should hash the concatenated string "db-feature", not "db-" + hash("feature")

    let mut vars = HashMap::new();
    vars.insert("branch", "feature");

    // With parentheses (correct, as documented)
    let with_parens = expand_template("{{ ('db-' ~ branch) | hash_port }}", &vars, false).unwrap();
    let port_with_parens: u16 = with_parens.parse().unwrap();

    // Verify it hashes the concatenated string
    let expected_port = hash_port("db-feature");
    assert_eq!(
        port_with_parens, expected_port,
        "('db-' ~ branch) | hash_port should hash 'db-feature', not just 'feature'"
    );

    // Without parentheses (what the bug was) - this hashes just "branch" and prepends "db-"
    let without_parens = expand_template("{{ 'db-' ~ branch | hash_port }}", &vars, false).unwrap();

    // The result should be different because of precedence
    // Without parens: 'db-' ~ (branch | hash_port) = 'db-' ~ hash("feature")
    let port_just_branch = hash_port("feature");
    assert_eq!(
        without_parens,
        format!("db-{}", port_just_branch),
        "Without parens, 'db-' ~ branch | hash_port means 'db-' ~ (hash_port(branch))"
    );

    // The two results should NOT be equal
    assert_ne!(
        with_parens, without_parens,
        "Parentheses change the result - this is the PR #373 issue"
    );
}

#[test]
fn test_doc_hash_port_repo_branch_concatenation() {
    // From docs/content/hook.md line 176:
    // dev = "npm run dev --port {{ (repo ~ '-' ~ branch) | hash_port }}"

    let mut vars = HashMap::new();
    vars.insert("repo", "myapp");
    vars.insert("branch", "feature");

    let result = expand_template("{{ (repo ~ '-' ~ branch) | hash_port }}", &vars, false).unwrap();
    let port: u16 = result.parse().unwrap();

    // Should hash the full concatenated string
    let expected = hash_port("myapp-feature");
    assert_eq!(
        port, expected,
        "Should hash the concatenated string 'myapp-feature'"
    );
}

// =============================================================================
// Full Command Examples from Docs
// These test complete template strings from the documentation
// =============================================================================

#[test]
fn test_doc_example_docker_postgres() {
    // From docs/content/tips-patterns.md lines 75-84:
    // docker run ... -p {{ ('db-' ~ branch) | hash_port }}:5432

    let mut vars = HashMap::new();
    vars.insert("repo", "myproject");
    vars.insert("branch", "feature-auth");

    let template = r#"docker run -d --rm \
  --name {{ repo }}-{{ branch | sanitize }}-postgres \
  -p {{ ('db-' ~ branch) | hash_port }}:5432 \
  postgres:16"#;

    let result = expand_template(template, &vars, false).unwrap();

    // Check the container name uses sanitized branch
    assert!(
        result.contains("--name myproject-feature-auth-postgres"),
        "Container name should use sanitized branch"
    );

    // Check the port is a hash of "db-feature-auth"
    let expected_port = hash_port("db-feature-auth");
    assert!(
        result.contains(&format!("-p {expected_port}:5432")),
        "Port should be hash of 'db-feature-auth', expected {expected_port}"
    );
}

#[test]
fn test_doc_example_database_url() {
    // From docs/content/tips-patterns.md lines 96-101:
    // DATABASE_URL=postgres://postgres:dev@localhost:{{ ('db-' ~ branch) | hash_port }}/{{ repo }}

    let mut vars = HashMap::new();
    vars.insert("repo", "myproject");
    vars.insert("branch", "feature");

    let template = "DATABASE_URL=postgres://postgres:dev@localhost:{{ ('db-' ~ branch) | hash_port }}/{{ repo }}";

    let result = expand_template(template, &vars, false).unwrap();

    let expected_port = hash_port("db-feature");
    assert_eq!(
        result,
        format!("DATABASE_URL=postgres://postgres:dev@localhost:{expected_port}/myproject")
    );
}

#[test]
fn test_doc_example_dev_server() {
    // From docs/content/hook.md lines 168-170:
    // dev = "npm run dev -- --host {{ branch }}.lvh.me --port {{ branch | hash_port }}"

    let mut vars = HashMap::new();
    vars.insert("branch", "feature-auth");

    let template = "npm run dev -- --host {{ branch }}.lvh.me --port {{ branch | hash_port }}";

    let result = expand_template(template, &vars, false).unwrap();

    let expected_port = hash_port("feature-auth");
    assert_eq!(
        result,
        format!("npm run dev -- --host feature-auth.lvh.me --port {expected_port}")
    );
}

#[test]
fn test_doc_example_worktree_path_sanitize() {
    // From docs/content/tips-patterns.md line 217:
    // worktree-path = "{{ branch | sanitize }}"

    let mut vars = HashMap::new();
    vars.insert("branch", "feature/user/auth");
    vars.insert("main_worktree", "/home/user/project");

    let template = "{{ main_worktree }}.{{ branch | sanitize }}";

    let result = expand_template(template, &vars, false).unwrap();
    assert_eq!(result, "/home/user/project.feature-user-auth");
}

// =============================================================================
// Edge Cases
// =============================================================================

#[test]
fn test_doc_hash_port_empty_string() {
    let mut vars = HashMap::new();
    vars.insert("branch", "");

    let result = expand_template("{{ branch | hash_port }}", &vars, false).unwrap();
    let port: u16 = result.parse().unwrap();

    assert!(
        (10000..20000).contains(&port),
        "hash_port of empty string should still produce valid port"
    );
}

#[test]
fn test_doc_sanitize_no_slashes() {
    let mut vars = HashMap::new();
    vars.insert("branch", "simple-branch");

    let result = expand_template("{{ branch | sanitize }}", &vars, false).unwrap();
    assert_eq!(
        result, "simple-branch",
        "sanitize should be no-op without slashes"
    );
}

#[test]
fn test_doc_combined_filters() {
    // sanitize then hash_port (not currently documented, but should work)
    let mut vars = HashMap::new();
    vars.insert("branch", "feature/auth");

    let result = expand_template("{{ branch | sanitize | hash_port }}", &vars, false).unwrap();
    let port: u16 = result.parse().unwrap();

    // Should hash the sanitized version
    let expected = hash_port("feature-auth");
    assert_eq!(port, expected);
}
