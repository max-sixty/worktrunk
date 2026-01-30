//! Tests for template examples shown in documentation.
//!
//! These tests verify that template expressions documented in docs/content/ behave
//! as described. This catches operator precedence issues like the one fixed in PR #373
//! where `{{ 'db-' ~ branch | hash_port }}` was incorrectly documented without parentheses.
//!
//! Run with: `cargo test --test integration doc_templates`

use std::collections::HashMap;

use rstest::rstest;
use worktrunk::config::expand_template;
use worktrunk::git::Repository;

use crate::common::{TestRepo, repo};

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

#[rstest]
fn test_doc_basic_variables(repo: TestRepo) {
    let repository = Repository::at(repo.root_path()).unwrap();
    let mut vars = HashMap::new();
    vars.insert("repo", "myproject");
    vars.insert("branch", "feature/auth");
    vars.insert("worktree", "/home/user/myproject.feature-auth");
    vars.insert("default_branch", "main");

    // Each variable substitutes correctly
    assert_eq!(
        expand_template("{{ repo }}", &vars, false, &repository, "test").unwrap(),
        "myproject"
    );
    assert_eq!(
        expand_template("{{ branch }}", &vars, false, &repository, "test").unwrap(),
        "feature/auth"
    );
    assert_eq!(
        expand_template("{{ worktree }}", &vars, false, &repository, "test").unwrap(),
        "/home/user/myproject.feature-auth"
    );
    assert_eq!(
        expand_template("{{ default_branch }}", &vars, false, &repository, "test").unwrap(),
        "main"
    );
}

// =============================================================================
// Sanitize Filter (docs/content/hook.md: Filters table)
// "Replace `/` and `\` with `-`"
// =============================================================================

#[rstest]
fn test_doc_sanitize_filter(repo: TestRepo) {
    let mut vars = HashMap::new();
    let repository = Repository::at(repo.root_path()).unwrap();

    // From docs: {{ branch | sanitize }} replaces / and \ with -
    vars.insert("branch", "feature/foo");
    assert_eq!(
        expand_template("{{ branch | sanitize }}", &vars, false, &repository, "test").unwrap(),
        "feature-foo",
        "sanitize should replace / with -"
    );

    vars.insert("branch", "user\\task");
    assert_eq!(
        expand_template("{{ branch | sanitize }}", &vars, false, &repository, "test").unwrap(),
        "user-task",
        "sanitize should replace \\ with -"
    );

    // Nested paths
    vars.insert("branch", "user/feature/task");
    assert_eq!(
        expand_template("{{ branch | sanitize }}", &vars, false, &repository, "test").unwrap(),
        "user-feature-task",
        "sanitize should handle multiple slashes"
    );
}

// =============================================================================
// Sanitize DB Filter (docs/content/hook.md: Filters table)
// "Transform to database-safe identifier ([a-z0-9_], max 63 chars)"
// =============================================================================

#[rstest]
fn test_doc_sanitize_db_filter(repo: TestRepo) {
    let mut vars = HashMap::new();
    let repository = Repository::at(repo.root_path()).unwrap();

    // From docs: {{ branch | sanitize_db }} transforms to database-safe identifier
    // Output includes a 3-character hash suffix for uniqueness
    vars.insert("branch", "feature/auth-oauth2");
    let result = expand_template(
        "{{ branch | sanitize_db }}",
        &vars,
        false,
        &repository,
        "test",
    )
    .unwrap();
    assert!(
        result.starts_with("feature_auth_oauth2_"),
        "sanitize_db should replace non-alphanumeric with _ and lowercase, got: {result}"
    );

    // Leading digits get underscore prefix
    vars.insert("branch", "123-bug-fix");
    let result = expand_template(
        "{{ branch | sanitize_db }}",
        &vars,
        false,
        &repository,
        "test",
    )
    .unwrap();
    assert!(
        result.starts_with("_123_bug_fix_"),
        "sanitize_db should prefix leading digits with _, got: {result}"
    );

    // Uppercase conversion
    vars.insert("branch", "UPPERCASE.Branch");
    let result = expand_template(
        "{{ branch | sanitize_db }}",
        &vars,
        false,
        &repository,
        "test",
    )
    .unwrap();
    assert!(
        result.starts_with("uppercase_branch_"),
        "sanitize_db should convert to lowercase, got: {result}"
    );

    // Consecutive underscores collapsed
    vars.insert("branch", "a--b//c");
    let result = expand_template(
        "{{ branch | sanitize_db }}",
        &vars,
        false,
        &repository,
        "test",
    )
    .unwrap();
    assert!(
        result.starts_with("a_b_c_"),
        "sanitize_db should collapse consecutive underscores, got: {result}"
    );

    // Different inputs that would otherwise collide get different suffixes
    vars.insert("branch", "a-b");
    let result1 = expand_template(
        "{{ branch | sanitize_db }}",
        &vars,
        false,
        &repository,
        "test",
    )
    .unwrap();
    vars.insert("branch", "a_b");
    let result2 = expand_template(
        "{{ branch | sanitize_db }}",
        &vars,
        false,
        &repository,
        "test",
    )
    .unwrap();
    assert_ne!(
        result1, result2,
        "a-b and a_b should produce different outputs"
    );
}

#[rstest]
fn test_doc_sanitize_db_truncation(repo: TestRepo) {
    let repository = Repository::at(repo.root_path()).unwrap();
    let mut vars = HashMap::new();

    // Truncates to 63 characters (PostgreSQL limit)
    let long_branch = "a".repeat(100);
    vars.insert("branch", long_branch.as_str());
    let result = expand_template(
        "{{ branch | sanitize_db }}",
        &vars,
        false,
        &repository,
        "test",
    )
    .unwrap();
    assert_eq!(
        result.len(),
        63,
        "sanitize_db should truncate to 63 characters"
    );
}

// =============================================================================
// Hash Port Filter (docs/content/hook.md: Filters table)
// "Hash to port 10000-19999"
// =============================================================================

#[rstest]
fn test_doc_hash_port_filter(repo: TestRepo) {
    let mut vars = HashMap::new();
    vars.insert("branch", "feature-foo");
    let repository = Repository::at(repo.root_path()).unwrap();

    let result = expand_template(
        "{{ branch | hash_port }}",
        &vars,
        false,
        &repository,
        "test",
    )
    .unwrap();
    let port: u16 = result.parse().expect("hash_port should produce a number");

    assert!(
        (10000..20000).contains(&port),
        "hash_port should produce port in range 10000-19999, got {port}"
    );

    // Deterministic
    let result2 = expand_template(
        "{{ branch | hash_port }}",
        &vars,
        false,
        &repository,
        "test",
    )
    .unwrap();
    assert_eq!(result, result2, "hash_port should be deterministic");
}

// =============================================================================
// Concatenation with hash_port (docs/content/tips-patterns.md)
// CRITICAL: These test the operator precedence issue from PR #373
// =============================================================================

#[rstest]
fn test_doc_hash_port_concatenation_precedence(repo: TestRepo) {
    // From docs/content/tips-patterns.md:
    // "The `'db-' ~ branch` concatenation hashes differently than plain `branch`"
    //
    // The docs show: {{ ('db-' ~ branch) | hash_port }}
    // This should hash the concatenated string "db-feature", not "db-" + hash("feature")

    let mut vars = HashMap::new();
    vars.insert("branch", "feature");
    let repository = Repository::at(repo.root_path()).unwrap();

    // With parentheses (correct, as documented)
    let with_parens = expand_template(
        "{{ ('db-' ~ branch) | hash_port }}",
        &vars,
        false,
        &repository,
        "test",
    )
    .unwrap();
    let port_with_parens: u16 = with_parens.parse().unwrap();

    // Verify it hashes the concatenated string
    let expected_port = hash_port("db-feature");
    assert_eq!(
        port_with_parens, expected_port,
        "('db-' ~ branch) | hash_port should hash 'db-feature', not just 'feature'"
    );

    // Without parentheses (what the bug was) - this hashes just "branch" and prepends "db-"
    let without_parens = expand_template(
        "{{ 'db-' ~ branch | hash_port }}",
        &vars,
        false,
        &repository,
        "test",
    )
    .unwrap();

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

#[rstest]
fn test_doc_hash_port_repo_branch_concatenation(repo: TestRepo) {
    // From docs/content/hook.md line 176:
    // dev = "npm run dev --port {{ (repo ~ '-' ~ branch) | hash_port }}"

    let repository = Repository::at(repo.root_path()).unwrap();
    let mut vars = HashMap::new();
    vars.insert("repo", "myapp");
    vars.insert("branch", "feature");

    let result = expand_template(
        "{{ (repo ~ '-' ~ branch) | hash_port }}",
        &vars,
        false,
        &repository,
        "test",
    )
    .unwrap();
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

#[rstest]
fn test_doc_example_docker_postgres(repo: TestRepo) {
    // From docs/content/tips-patterns.md lines 75-84:
    // docker run ... -p {{ ('db-' ~ branch) | hash_port }}:5432

    let repository = Repository::at(repo.root_path()).unwrap();
    let mut vars = HashMap::new();
    vars.insert("repo", "myproject");
    vars.insert("branch", "feature-auth");

    let template = r#"docker run -d --rm \
  --name {{ repo }}-{{ branch | sanitize }}-postgres \
  -p {{ ('db-' ~ branch) | hash_port }}:5432 \
  postgres:16"#;

    let result = expand_template(template, &vars, false, &repository, "test").unwrap();

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

#[rstest]
fn test_doc_example_database_url(repo: TestRepo) {
    // From docs/content/tips-patterns.md lines 96-101:
    // DATABASE_URL=postgres://postgres:dev@localhost:{{ ('db-' ~ branch) | hash_port }}/{{ repo }}

    let repository = Repository::at(repo.root_path()).unwrap();
    let mut vars = HashMap::new();
    vars.insert("repo", "myproject");
    vars.insert("branch", "feature");

    let template = "DATABASE_URL=postgres://postgres:dev@localhost:{{ ('db-' ~ branch) | hash_port }}/{{ repo }}";

    let result = expand_template(template, &vars, false, &repository, "test").unwrap();

    let expected_port = hash_port("db-feature");
    assert_eq!(
        result,
        format!("DATABASE_URL=postgres://postgres:dev@localhost:{expected_port}/myproject")
    );
}

#[rstest]
fn test_doc_example_dev_server(repo: TestRepo) {
    // From docs/content/hook.md lines 168-170:
    // dev = "npm run dev -- --host {{ branch }}.lvh.me --port {{ branch | hash_port }}"

    let repository = Repository::at(repo.root_path()).unwrap();
    let mut vars = HashMap::new();
    vars.insert("branch", "feature-auth");

    let template = "npm run dev -- --host {{ branch }}.lvh.me --port {{ branch | hash_port }}";

    let result = expand_template(template, &vars, false, &repository, "test").unwrap();

    let expected_port = hash_port("feature-auth");
    assert_eq!(
        result,
        format!("npm run dev -- --host feature-auth.lvh.me --port {expected_port}")
    );
}

#[rstest]
fn test_doc_example_worktree_path_sanitize(repo: TestRepo) {
    // From docs/content/tips-patterns.md line 217:
    // worktree-path = "{{ branch | sanitize }}"

    let repository = Repository::at(repo.root_path()).unwrap();
    let mut vars = HashMap::new();
    vars.insert("branch", "feature/user/auth");
    vars.insert("main_worktree", "/home/user/project");

    let template = "{{ main_worktree }}.{{ branch | sanitize }}";

    let result = expand_template(template, &vars, false, &repository, "test").unwrap();
    assert_eq!(result, "/home/user/project.feature-user-auth");
}

// =============================================================================
// Edge Cases
// =============================================================================

#[rstest]
fn test_doc_hash_port_empty_string(repo: TestRepo) {
    let repository = Repository::at(repo.root_path()).unwrap();
    let mut vars = HashMap::new();
    vars.insert("branch", "");

    let result = expand_template(
        "{{ branch | hash_port }}",
        &vars,
        false,
        &repository,
        "test",
    )
    .unwrap();
    let port: u16 = result.parse().unwrap();

    assert!(
        (10000..20000).contains(&port),
        "hash_port of empty string should still produce valid port"
    );
}

#[rstest]
fn test_doc_sanitize_no_slashes(repo: TestRepo) {
    let repository = Repository::at(repo.root_path()).unwrap();
    let mut vars = HashMap::new();
    vars.insert("branch", "simple-branch");

    let result =
        expand_template("{{ branch | sanitize }}", &vars, false, &repository, "test").unwrap();
    assert_eq!(
        result, "simple-branch",
        "sanitize should be no-op without slashes"
    );
}

#[rstest]
fn test_doc_combined_filters(repo: TestRepo) {
    // sanitize then hash_port (not currently documented, but should work)
    let repository = Repository::at(repo.root_path()).unwrap();
    let mut vars = HashMap::new();
    vars.insert("branch", "feature/auth");

    let result = expand_template(
        "{{ branch | sanitize | hash_port }}",
        &vars,
        false,
        &repository,
        "test",
    )
    .unwrap();
    let port: u16 = result.parse().unwrap();

    // Should hash the sanitized version
    let expected = hash_port("feature-auth");
    assert_eq!(port, expected);
}

// =============================================================================
// worktree_path_of_branch Function
// =============================================================================

#[rstest]
fn test_worktree_path_of_branch_function_registered(repo: TestRepo) {
    // Test that worktree_path_of_branch function is callable and returns empty for nonexistent branch
    let repository = Repository::at(repo.root_path()).unwrap();
    let vars: HashMap<&str, &str> = HashMap::new();
    let result = expand_template(
        "{{ worktree_path_of_branch('nonexistent') }}",
        &vars,
        false,
        &repository,
        "test",
    );
    assert_eq!(result.unwrap(), "");
}

/// Test that worktree_path_of_branch returns shell-escaped paths when shell_escape=true.
///
/// This is a regression test for the bug where worktree_path_of_branch returned raw paths
/// even when shell_escape=true, which could break hook commands on paths with spaces
/// like "/Users/john/My Projects/feature".
#[rstest]
fn test_worktree_path_of_branch_shell_escape(repo: TestRepo) {
    // Create a worktree with a space in the path to test shell escaping
    let worktree_path = repo.root_path().parent().unwrap().join("My Worktree");

    // Create a new branch and worktree
    repo.run_git(&["branch", "test-branch"]);
    repo.run_git(&[
        "worktree",
        "add",
        worktree_path.to_str().unwrap(),
        "test-branch",
    ]);

    let repository = Repository::at(repo.root_path()).unwrap();
    let vars: HashMap<&str, &str> = HashMap::new();

    // With shell_escape=false, path is returned literally
    let result_literal = expand_template(
        "{{ worktree_path_of_branch('test-branch') }}",
        &vars,
        false,
        &repository,
        "test",
    )
    .unwrap();
    // Path should contain "My Worktree" unescaped (spaces as-is, posix-style)
    assert!(
        result_literal.contains("My Worktree"),
        "Expected literal path with space, got: {result_literal}"
    );

    // With shell_escape=true, path should be escaped for safe shell usage
    let result_escaped = expand_template(
        "{{ worktree_path_of_branch('test-branch') }}",
        &vars,
        true,
        &repository,
        "test",
    )
    .unwrap();
    // Path should be quoted or escaped (shell_escape crate uses single quotes)
    assert!(
        result_escaped.contains('\'') || result_escaped.contains('\\'),
        "Expected shell-escaped path, got: {result_escaped}"
    );
    // The escaped path should still reference the worktree
    assert!(
        result_escaped.contains("My Worktree") || result_escaped.contains("My\\ Worktree"),
        "Escaped path should reference worktree: {result_escaped}"
    );

    // Clean up worktree
    repo.run_git(&[
        "worktree",
        "remove",
        "--force",
        worktree_path.to_str().unwrap(),
    ]);
}
