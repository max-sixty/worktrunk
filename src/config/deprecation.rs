//! Deprecated template variable detection and migration
//!
//! Scans config file content for deprecated template variables and generates
//! a migration file with replacements.
//!
//! Migration files are only written once per config file. The hint system tracks
//! whether a migration file has been written:
//! - Project config: uses `worktrunk.hints.deprecated-project-config` in git config
//! - User config: checks if the `.new` file already exists (no repo context)
//!
//! To regenerate a project config migration file, run `wt config state hints clear deprecated-project-config`.
//! To regenerate a user config migration file, delete the existing `.new` file.

use crate::styling::{eprintln, hint_message, warning_message};
use color_print::cformat;
use minijinja::Environment;
use shell_escape::escape;
use std::borrow::Cow;
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Tracks which config paths have already shown deprecation warnings this process.
/// Prevents repeated warnings when config is loaded multiple times.
static WARNED_PATHS: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

/// Hint name for project config deprecation warnings
const HINT_DEPRECATED_PROJECT_CONFIG: &str = "deprecated-project-config";

/// Mapping from deprecated variable name to its replacement
const DEPRECATED_VARS: &[(&str, &str)] = &[
    ("repo_root", "repo_path"),
    ("worktree", "worktree_path"),
    ("main_worktree", "repo"),
    ("main_worktree_path", "primary_worktree_path"),
];

/// Normalize a template string by replacing deprecated variables with their canonical names.
///
/// This allows approval matching to work regardless of whether the command was saved
/// with old or new variable names. For example, `{{ repo_root }}` and `{{ repo_path }}`
/// will both normalize to `{{ repo_path }}`.
///
/// Returns `Cow::Borrowed` if no replacements needed, avoiding allocation.
pub fn normalize_template_vars(template: &str) -> Cow<'_, str> {
    use regex::Regex;

    // Quick check: if none of the deprecated vars appear, return borrowed
    if !DEPRECATED_VARS
        .iter()
        .any(|(old, _)| template.contains(old))
    {
        return Cow::Borrowed(template);
    }

    let mut result = template.to_string();
    for &(old, new) in DEPRECATED_VARS {
        let re = Regex::new(&format!(r"\b{}\b", regex::escape(old))).unwrap();
        result = re.replace_all(&result, new).into_owned();
    }
    Cow::Owned(result)
}

/// Find all deprecated variables used in the content
///
/// Parses TOML to extract string values, then uses minijinja to detect
/// which template variables are referenced.
///
/// Returns a deduplicated list of (deprecated_name, replacement_name) pairs
pub fn find_deprecated_vars(content: &str) -> Vec<(&'static str, &'static str)> {
    // Parse TOML and extract all string values that might contain templates
    let template_strings = extract_template_strings(content);

    // Collect all variables used across all templates
    let mut used_vars = HashSet::new();
    let env = Environment::new();

    for template_str in template_strings {
        if let Ok(template) = env.template_from_str(&template_str) {
            used_vars.extend(template.undeclared_variables(false));
        }
    }

    // Check which deprecated variables are used
    DEPRECATED_VARS
        .iter()
        .filter(|(old, _)| used_vars.contains(*old))
        .copied()
        .collect()
}

/// Extract all string values from TOML content that might contain templates
fn extract_template_strings(content: &str) -> Vec<String> {
    let Ok(table) = content.parse::<toml::Table>() else {
        return vec![];
    };

    let mut strings = Vec::new();
    collect_strings_from_value(&toml::Value::Table(table), &mut strings);
    strings
}

/// Recursively collect all string values from a TOML value
fn collect_strings_from_value(value: &toml::Value, strings: &mut Vec<String>) {
    match value {
        toml::Value::String(s) => strings.push(s.clone()),
        toml::Value::Array(arr) => {
            for v in arr {
                collect_strings_from_value(v, strings);
            }
        }
        toml::Value::Table(table) => {
            for v in table.values() {
                collect_strings_from_value(v, strings);
            }
        }
        _ => {}
    }
}

/// Replace all deprecated variables with their new names
pub fn replace_deprecated_vars(content: &str) -> String {
    use regex::Regex;

    let strings = extract_template_strings(content);
    let mut result = content.to_string();

    for original in strings {
        let mut modified = original.clone();
        for &(old, new) in DEPRECATED_VARS {
            let re = Regex::new(&format!(r"\b{}\b", regex::escape(old))).unwrap();
            modified = re.replace_all(&modified, new).into_owned();
        }
        if modified != original {
            result = result.replace(&original, &modified);
        }
    }

    result
}

/// Check config content for deprecated variables and optionally create migration file
///
/// If deprecated variables are found and `warn_and_migrate` is true:
/// 1. Emits a warning listing the deprecated variables
/// 2. Creates a `.new` file with replacements (only if not already written)
///
/// Set `warn_and_migrate` to false for project config on feature worktrees - the warning
/// is only actionable from the main worktree where the migration file can be applied.
///
/// The `label` is used in the warning message (e.g., "User config" or "Project config").
///
/// `repo` should be provided for project config to use the hint system. For user config
/// (global, not repo-specific), pass `None` and the function will check if the `.new`
/// file already exists instead.
///
/// Warnings are deduplicated per path per process.
///
/// Returns Ok(true) if deprecated variables were found, Ok(false) otherwise.
pub fn check_and_migrate(
    path: &Path,
    content: &str,
    warn_and_migrate: bool,
    label: &str,
    repo: Option<&crate::git::Repository>,
) -> anyhow::Result<bool> {
    let deprecated = find_deprecated_vars(content);
    if deprecated.is_empty() {
        return Ok(false);
    }

    // Skip warning entirely if not in main worktree (for project config)
    if !warn_and_migrate {
        return Ok(true);
    }

    // Deduplicate warnings per path per process
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    {
        let mut guard = WARNED_PATHS.lock().unwrap();
        let warned = guard.get_or_insert_with(HashSet::new);
        if warned.contains(&canonical_path) {
            return Ok(true); // Already warned, skip
        }
        warned.insert(canonical_path.clone());
    }

    // Build the .new path: "config.toml" -> "config.toml.new"
    let new_path = path.with_extension(format!(
        "{}.new",
        path.extension().unwrap_or_default().to_string_lossy()
    ));

    // Check if we should skip writing the migration file
    // - Always regenerate if .new file exists (user kept it, so overwrite with fresh version)
    // - For project config: skip if hint shown AND .new doesn't exist (user deleted it)
    // - For user config: always write (no persistent hint tracking without repo)
    let should_skip_write = !new_path.exists()
        && repo.is_some_and(|r| r.has_shown_hint(HINT_DEPRECATED_PROJECT_CONFIG));

    // Build inline list of deprecated variables: "repo_root → repo_path, worktree → worktree_path"
    let var_list: Vec<String> = deprecated
        .iter()
        .map(|(old, new)| cformat!("<dim>{}</> → <bold>{}</>", old, new))
        .collect();

    let warning = format!(
        "{} uses deprecated template variables: {}",
        label,
        var_list.join(", ")
    );
    eprintln!("{}", warning_message(warning));

    if should_skip_write {
        // User deleted the .new file but hint is set - they don't want the migration file
        // Show how to regenerate if they change their mind
        eprintln!(
            "{}",
            hint_message(cformat!(
                "to regenerate, rerun after <bright-black>wt config state hints clear {}</>",
                HINT_DEPRECATED_PROJECT_CONFIG
            ))
        );
    } else {
        // Write migration file
        let new_content = replace_deprecated_vars(content);

        match std::fs::write(&new_path, &new_content) {
            Ok(()) => {
                // Mark hint as shown for project config
                if let Some(repo) = repo {
                    let _ = repo.mark_hint_shown(HINT_DEPRECATED_PROJECT_CONFIG);
                }

                // Show just the filename in the message, full paths in the command
                let new_filename = new_path
                    .file_name()
                    .map(|n| n.to_string_lossy())
                    .unwrap_or_default();

                // Shell-escape paths for safe copy-paste
                let new_path_str = new_path.to_string_lossy();
                let path_str = path.to_string_lossy();
                let new_path_escaped = escape(Cow::Borrowed(new_path_str.as_ref()));
                let path_escaped = escape(Cow::Borrowed(path_str.as_ref()));
                eprintln!(
                    "{}",
                    hint_message(cformat!(
                        "Wrote migrated {}; to apply: <bright-black>mv -- {} {}</>",
                        new_filename,
                        new_path_escaped,
                        path_escaped
                    ))
                );
            }
            Err(e) => {
                // Warn about write failure but don't block config loading
                eprintln!(
                    "{}",
                    hint_message(cformat!(
                        "Could not write migration file: <bright-black>{}</>",
                        e
                    ))
                );
            }
        }
    }

    // Flush stderr to ensure output appears before any subsequent messages
    std::io::stderr().flush().ok();

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_deprecated_vars_empty() {
        let content = r#"
worktree-path = "../{{ repo }}.{{ branch | sanitize }}"
"#;
        let found = find_deprecated_vars(content);
        assert!(found.is_empty());
    }

    #[test]
    fn test_find_deprecated_vars_repo_root() {
        let content = r#"
post-create = "ln -sf {{ repo_root }}/node_modules node_modules"
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(found, vec![("repo_root", "repo_path")]);
    }

    #[test]
    fn test_find_deprecated_vars_worktree() {
        let content = r#"
post-create = "cd {{ worktree }} && npm install"
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(found, vec![("worktree", "worktree_path")]);
    }

    #[test]
    fn test_find_deprecated_vars_main_worktree() {
        let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch | sanitize }}"
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(found, vec![("main_worktree", "repo")]);
    }

    #[test]
    fn test_find_deprecated_vars_main_worktree_path() {
        let content = r#"
post-create = "ln -sf {{ main_worktree_path }}/node_modules ."
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(found, vec![("main_worktree_path", "primary_worktree_path")]);
    }

    #[test]
    fn test_find_deprecated_vars_multiple() {
        let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch | sanitize }}"
post-create = "ln -sf {{ repo_root }}/node_modules {{ worktree }}/node_modules"
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(
            found,
            vec![
                ("repo_root", "repo_path"),
                ("worktree", "worktree_path"),
                ("main_worktree", "repo"),
            ]
        );
    }

    #[test]
    fn test_find_deprecated_vars_with_filter() {
        let content = r#"
post-create = "ln -sf {{ repo_root | something }}/node_modules"
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(found, vec![("repo_root", "repo_path")]);
    }

    #[test]
    fn test_find_deprecated_vars_deduplicates() {
        let content = r#"
post-create = "{{ repo_root }}/a {{ repo_root }}/b"
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(found, vec![("repo_root", "repo_path")]);
    }

    #[test]
    fn test_find_deprecated_vars_does_not_match_suffix() {
        // Should NOT match "worktree_path" when looking for "worktree"
        let content = r#"
post-create = "cd {{ worktree_path }} && npm install"
"#;
        let found = find_deprecated_vars(content);
        assert!(
            found.is_empty(),
            "Should not match worktree_path as worktree"
        );
    }

    #[test]
    fn test_replace_deprecated_vars_simple() {
        let content = r#"cmd = "{{ repo_root }}""#;
        let result = replace_deprecated_vars(content);
        assert_eq!(result, r#"cmd = "{{ repo_path }}""#);
    }

    #[test]
    fn test_replace_deprecated_vars_with_filter() {
        let content = r#"cmd = "{{ repo_root | sanitize }}""#;
        let result = replace_deprecated_vars(content);
        assert_eq!(result, r#"cmd = "{{ repo_path | sanitize }}""#);
    }

    #[test]
    fn test_replace_deprecated_vars_no_spaces() {
        let content = r#"cmd = "{{repo_root}}""#;
        let result = replace_deprecated_vars(content);
        assert_eq!(result, r#"cmd = "{{repo_path}}""#); // Preserves original formatting
    }

    #[test]
    fn test_replace_deprecated_vars_filter_no_spaces() {
        let content = r#"cmd = "{{repo_root|sanitize}}""#;
        let result = replace_deprecated_vars(content);
        assert_eq!(result, r#"cmd = "{{repo_path|sanitize}}""#); // Preserves original formatting
    }

    #[test]
    fn test_replace_deprecated_vars_multiple() {
        let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch | sanitize }}"
post-create = "ln -sf {{ repo_root }}/node_modules {{ worktree }}/node_modules"
"#;
        let result = replace_deprecated_vars(content);
        assert_eq!(
            result,
            r#"
worktree-path = "../{{ repo }}.{{ branch | sanitize }}"
post-create = "ln -sf {{ repo_path }}/node_modules {{ worktree_path }}/node_modules"
"#
        );
    }

    #[test]
    fn test_replace_deprecated_vars_preserves_other_content() {
        let content = r#"
# This is a comment
worktree-path = "../{{ repo }}.{{ branch }}"

[hooks]
post-create = "echo hello"
"#;
        let result = replace_deprecated_vars(content);
        assert_eq!(result, content); // No changes since no deprecated vars
    }

    #[test]
    fn test_replace_deprecated_vars_preserves_whitespace() {
        let content = r#"cmd = "{{  repo_root  }}""#;
        let result = replace_deprecated_vars(content);
        assert_eq!(result, r#"cmd = "{{  repo_path  }}""#); // Preserves original formatting
    }

    #[test]
    fn test_replace_does_not_match_suffix() {
        // Should NOT replace "worktree_path" when looking for "worktree"
        let content = r#"cmd = "{{ worktree_path }}""#;
        let result = replace_deprecated_vars(content);
        assert_eq!(
            result, r#"cmd = "{{ worktree_path }}""#,
            "Should not modify worktree_path"
        );
    }

    #[test]
    fn test_replace_in_statement_blocks() {
        // Word boundary replacement handles {% %} blocks too
        let content = r#"cmd = "{% if repo_root %}echo {{ repo_root }}{% endif %}""#;
        let result = replace_deprecated_vars(content);
        assert_eq!(
            result,
            r#"cmd = "{% if repo_path %}echo {{ repo_path }}{% endif %}""#
        );
    }

    // Tests for normalize_template_vars (single template string normalization)

    #[test]
    fn test_normalize_no_deprecated_vars() {
        let template = "ln -sf {{ repo_path }}/node_modules";
        let result = normalize_template_vars(template);
        assert!(matches!(result, Cow::Borrowed(_)), "Should not allocate");
        assert_eq!(result, template);
    }

    #[test]
    fn test_normalize_repo_root() {
        let template = "ln -sf {{ repo_root }}/node_modules";
        let result = normalize_template_vars(template);
        assert_eq!(result, "ln -sf {{ repo_path }}/node_modules");
    }

    #[test]
    fn test_normalize_worktree() {
        let template = "cd {{ worktree }} && npm install";
        let result = normalize_template_vars(template);
        assert_eq!(result, "cd {{ worktree_path }} && npm install");
    }

    #[test]
    fn test_normalize_main_worktree() {
        let template = "../{{ main_worktree }}.{{ branch }}";
        let result = normalize_template_vars(template);
        assert_eq!(result, "../{{ repo }}.{{ branch }}");
    }

    #[test]
    fn test_normalize_multiple_vars() {
        let template = "ln -sf {{ repo_root }}/node_modules {{ worktree }}/node_modules";
        let result = normalize_template_vars(template);
        assert_eq!(
            result,
            "ln -sf {{ repo_path }}/node_modules {{ worktree_path }}/node_modules"
        );
    }

    #[test]
    fn test_normalize_does_not_match_suffix() {
        // Should NOT replace "worktree_path" when looking for "worktree"
        let template = "cd {{ worktree_path }}";
        let result = normalize_template_vars(template);
        // Note: may allocate due to coarse quick check, but result is unchanged
        assert_eq!(result, template);
    }

    #[test]
    fn test_normalize_with_filter() {
        let template = "{{ repo_root | sanitize }}";
        let result = normalize_template_vars(template);
        assert_eq!(result, "{{ repo_path | sanitize }}");
    }

    // Tests for approved-commands array handling

    #[test]
    fn test_find_deprecated_vars_in_approved_commands() {
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = [
    "ln -sf {{ repo_root }}/node_modules",
    "cd {{ worktree }} && npm install",
]
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(
            found,
            vec![("repo_root", "repo_path"), ("worktree", "worktree_path"),]
        );
    }

    #[test]
    fn test_replace_deprecated_vars_in_approved_commands() {
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = [
    "ln -sf {{ repo_root }}/node_modules",
    "cd {{ worktree }} && npm install",
]
"#;
        let result = replace_deprecated_vars(content);
        assert_eq!(
            result,
            r#"
[projects."github.com/user/repo"]
approved-commands = [
    "ln -sf {{ repo_path }}/node_modules",
    "cd {{ worktree_path }} && npm install",
]
"#
        );
    }

    #[test]
    fn test_check_and_migrate_write_failure() {
        // Test the write error path by using a non-existent directory
        let content = r#"post-create = "{{ repo_root }}/script.sh""#;
        let non_existent_path = std::path::Path::new("/nonexistent/dir/config.toml");

        // Should return Ok(true) even if write fails - the function logs error but doesn't fail
        let result = check_and_migrate(non_existent_path, content, true, "Test config", None);
        assert!(result.is_ok());
        assert!(result.unwrap());
    }
}
