//! Deprecation detection and migration
//!
//! Scans config files for deprecated patterns and generates migration files:
//! - Deprecated template variables (repo_root → repo_path, etc.)
//! - Deprecated config sections (\[commit-generation\] → \[commit.generation\])
//! - Deprecated fields (args merged into command)
//! - Deprecated approved-commands in \[projects\] (moved to approvals.toml)
//!
//! Migration file write behavior:
//! - First time a deprecation is detected: file is written automatically
//! - Subsequent runs (for commands other than `wt config show`): brief warning only
//! - `wt config show`: always writes/regenerates the migration file with full details
//!
//! The hint system (`worktrunk.hints.deprecated-config` in git config) tracks whether
//! a deprecation has been warned about before. User config (no repo context) always
//! writes the migration file since there's no persistent hint tracking.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use color_print::cformat;
use minijinja::Environment;
use path_slash::PathExt as _;
use regex::Regex;
use shell_escape::unix::escape;

use crate::config::WorktrunkConfig;
use crate::shell_exec::Cmd;
use crate::styling::{
    eprintln, format_bash_with_gutter, format_with_gutter, hint_message, info_message,
    warning_message,
};

/// Tracks which config paths have already shown deprecation warnings this process.
/// Prevents repeated warnings when config is loaded multiple times.
static WARNED_DEPRECATED_PATHS: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Tracks which config paths have already shown unknown field warnings this process.
/// Prevents repeated warnings when config is loaded multiple times.
static WARNED_UNKNOWN_PATHS: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Hint name for config deprecation warnings
const HINT_DEPRECATED_CONFIG: &str = "deprecated-config";

/// Mapping from deprecated variable name to its replacement
const DEPRECATED_VARS: &[(&str, &str)] = &[
    ("repo_root", "repo_path"),
    ("worktree", "worktree_path"),
    ("main_worktree", "repo"),
    ("main_worktree_path", "primary_worktree_path"),
];

/// Top-level section keys that are deprecated and handled separately.
/// Callers should filter these out before calling `warn_unknown_fields` to avoid duplicate warnings.
pub const DEPRECATED_SECTION_KEYS: &[&str] = &["commit-generation"];

/// Normalize a template string by replacing deprecated variables with their canonical names.
///
/// This allows approval matching to work regardless of whether the command was saved
/// with old or new variable names. For example, `{{ repo_root }}` and `{{ repo_path }}`
/// will both normalize to `{{ repo_path }}`.
///
/// Returns `Cow::Borrowed` if no replacements needed, avoiding allocation.
pub fn normalize_template_vars(template: &str) -> Cow<'_, str> {
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

/// Information about deprecated commit-generation sections found in config
#[derive(Debug, Default, Clone)]
pub struct CommitGenerationDeprecations {
    /// Has top-level [commit-generation] section
    pub has_top_level: bool,
    /// Project keys that have deprecated [projects."...".commit-generation]
    pub project_keys: Vec<String>,
}

impl CommitGenerationDeprecations {
    pub fn is_empty(&self) -> bool {
        !self.has_top_level && self.project_keys.is_empty()
    }
}

/// All deprecation information detected in a config file.
///
/// This is a pure data struct with no path/label context. Used by both
/// config loading (brief warnings) and `wt config show` (full details).
#[derive(Debug, Default, Clone)]
pub struct Deprecations {
    /// Deprecated template variables found: (old_name, new_name)
    pub vars: Vec<(&'static str, &'static str)>,
    /// Deprecated commit-generation sections found
    pub commit_gen: CommitGenerationDeprecations,
    /// Has `approved-commands` in any `[projects."..."]` section (moved to approvals.toml)
    pub approved_commands: bool,
}

impl Deprecations {
    /// Returns true if any deprecations were found
    pub fn is_empty(&self) -> bool {
        self.vars.is_empty() && self.commit_gen.is_empty() && !self.approved_commands
    }
}

/// Detect deprecations in config content. Pure function, no I/O.
///
/// Returns a `Deprecations` struct containing all detected deprecation issues.
/// This is the recommended entry point for deprecation detection.
pub fn detect_deprecations(content: &str) -> Deprecations {
    Deprecations {
        vars: find_deprecated_vars(content),
        commit_gen: find_commit_generation_deprecations(content),
        approved_commands: find_approved_commands_deprecation(content),
    }
}

/// Check if any `[projects."..."]` section has a non-empty `approved-commands` array.
///
/// These have moved to `approvals.toml` and should be removed from config.toml.
pub fn find_approved_commands_deprecation(content: &str) -> bool {
    let Ok(doc) = content.parse::<toml_edit::DocumentMut>() else {
        return false;
    };

    let Some(projects) = doc.get("projects").and_then(|p| p.as_table()) else {
        return false;
    };

    for (_project_key, project_value) in projects.iter() {
        if let Some(project_table) = project_value.as_table()
            && let Some(approved) = project_table.get("approved-commands")
            && approved.as_array().is_some_and(|a| !a.is_empty())
        {
            return true;
        }
    }

    false
}

/// Find deprecated [commit-generation] sections in config
///
/// Returns information about:
/// - Top-level [commit-generation] section
/// - Project-level [projects."...".commit-generation] sections
pub fn find_commit_generation_deprecations(content: &str) -> CommitGenerationDeprecations {
    let Ok(doc) = content.parse::<toml_edit::DocumentMut>() else {
        return CommitGenerationDeprecations::default();
    };

    let mut result = CommitGenerationDeprecations::default();

    // Check if new [commit.generation] already exists as a valid table
    // (skip deprecation warning if so)
    let has_new_section = doc
        .get("commit")
        .and_then(|c| c.as_table())
        .and_then(|t| t.get("generation"))
        .is_some_and(|g| g.is_table() || g.is_inline_table());

    // Check top-level [commit-generation] - only flag if non-empty and new section doesn't exist
    // Handle both regular tables and inline tables
    if !has_new_section && let Some(section) = doc.get("commit-generation") {
        if let Some(table) = section.as_table() {
            if !table.is_empty() {
                result.has_top_level = true;
            }
        } else if let Some(inline) = section.as_inline_table()
            && !inline.is_empty()
        {
            result.has_top_level = true;
        }
    }

    // Check [projects."...".commit-generation]
    if let Some(projects) = doc.get("projects").and_then(|p| p.as_table()) {
        for (project_key, project_value) in projects.iter() {
            if let Some(project_table) = project_value.as_table() {
                // Check if this project has new section as a valid table
                let has_new_project_section = project_table
                    .get("commit")
                    .and_then(|c| c.as_table())
                    .and_then(|t| t.get("generation"))
                    .is_some_and(|g| g.is_table() || g.is_inline_table());

                // Only flag if old section exists, is non-empty, and new doesn't exist
                // Handle both regular tables and inline tables
                if !has_new_project_section
                    && let Some(old_section) = project_table.get("commit-generation")
                {
                    let is_non_empty = old_section.as_table().is_some_and(|t| !t.is_empty())
                        || old_section.as_inline_table().is_some_and(|t| !t.is_empty());
                    if is_non_empty {
                        result.project_keys.push(project_key.to_string());
                    }
                }
            }
        }
    }

    result
}

/// Migrate [commit-generation] sections to [commit.generation]
///
/// Performs the following migrations:
/// - Renames [commit-generation] to [commit.generation]
/// - Merges args field into command (if present)
/// - Renames [projects."...".commit-generation] to [projects."...".commit.generation]
pub fn migrate_commit_generation_sections(content: &str) -> String {
    let Ok(mut doc) = content.parse::<toml_edit::DocumentMut>() else {
        return content.to_string();
    };

    let mut modified = false;

    // Check if new [commit.generation] already exists as a valid table - if so, skip migration
    // (new format takes precedence, don't overwrite it)
    let has_new_section = doc
        .get("commit")
        .and_then(|c| c.as_table())
        .and_then(|t| t.get("generation"))
        .is_some_and(|g| g.is_table() || g.is_inline_table());

    // Migrate top-level [commit-generation] → [commit.generation]
    // Only if new section doesn't already exist
    // Handle both regular tables and inline tables
    if !has_new_section && let Some(old_section) = doc.remove("commit-generation") {
        // Convert to table - works for both regular tables and inline tables
        let table_opt = match old_section {
            toml_edit::Item::Table(t) => Some(t),
            toml_edit::Item::Value(toml_edit::Value::InlineTable(it)) => Some(it.into_table()),
            _ => None,
        };

        if let Some(mut table) = table_opt {
            // Merge args into command if present
            merge_args_into_command(&mut table);

            // Ensure [commit] section exists.
            // Mark as implicit so it doesn't render a separate [commit] header
            // (only [commit.generation] will render)
            if !doc.contains_key("commit") {
                let mut commit_table = toml_edit::Table::new();
                commit_table.set_implicit(true);
                doc.insert("commit", toml_edit::Item::Table(commit_table));
            }

            // Move to [commit.generation]
            if let Some(commit_table) = doc["commit"].as_table_mut() {
                commit_table.insert("generation", toml_edit::Item::Table(table));
            }

            modified = true;
        }
    }

    // Migrate [projects."...".commit-generation] → [projects."...".commit.generation]
    if let Some(projects) = doc.get_mut("projects").and_then(|p| p.as_table_mut()) {
        for (_project_key, project_value) in projects.iter_mut() {
            if let Some(project_table) = project_value.as_table_mut() {
                // Check if new section already exists as a valid table for this project
                let has_new_project_section = project_table
                    .get("commit")
                    .and_then(|c| c.as_table())
                    .and_then(|t| t.get("generation"))
                    .is_some_and(|g| g.is_table() || g.is_inline_table());

                if !has_new_project_section
                    && let Some(old_section) = project_table.remove("commit-generation")
                {
                    // Convert to table - works for both regular tables and inline tables
                    let table_opt = match old_section {
                        toml_edit::Item::Table(t) => Some(t),
                        toml_edit::Item::Value(toml_edit::Value::InlineTable(it)) => {
                            Some(it.into_table())
                        }
                        _ => None,
                    };

                    if let Some(mut table) = table_opt {
                        // Merge args into command if present
                        merge_args_into_command(&mut table);

                        // Ensure [projects."...".commit] section exists.
                        // Mark as implicit so it doesn't render a separate header
                        if !project_table.contains_key("commit") {
                            let mut commit_table = toml_edit::Table::new();
                            commit_table.set_implicit(true);
                            project_table.insert("commit", toml_edit::Item::Table(commit_table));
                        }

                        // Move to [projects."...".commit.generation]
                        if let Some(commit_table) = project_table["commit"].as_table_mut() {
                            commit_table.insert("generation", toml_edit::Item::Table(table));
                        }

                        modified = true;
                    }
                }
            }
        }
    }

    if modified {
        doc.to_string()
    } else {
        content.to_string()
    }
}

/// Remove `approved-commands` from all `\[projects."..."\]` sections.
///
/// For each project section, removes the `approved-commands` key.
/// If a project section becomes empty after removal, removes the project entry.
/// If the `\[projects\]` table becomes empty, removes it.
pub fn remove_approved_commands_from_config(content: &str) -> String {
    let Ok(mut doc) = content.parse::<toml_edit::DocumentMut>() else {
        return content.to_string();
    };

    let mut modified = false;

    if let Some(projects) = doc.get_mut("projects").and_then(|p| p.as_table_mut()) {
        // Collect project keys that should have approved-commands removed
        let mut remove_from: Vec<String> = Vec::new();
        let mut emptied: Vec<String> = Vec::new();

        for (project_key, project_value) in projects.iter() {
            if let Some(project_table) = project_value.as_table()
                && project_table.contains_key("approved-commands")
            {
                remove_from.push(project_key.to_string());
                // Will be empty after removal if approved-commands is the only key
                if project_table.len() == 1 {
                    emptied.push(project_key.to_string());
                }
            }
        }

        for key in &remove_from {
            if let Some(project_value) = projects.get_mut(key)
                && let Some(project_table) = project_value.as_table_mut()
            {
                project_table.remove("approved-commands");
                modified = true;
            }
        }

        for key in &emptied {
            projects.remove(key);
        }
    }

    // Remove empty [projects] table
    if doc
        .get("projects")
        .and_then(|p| p.as_table())
        .is_some_and(|t| t.is_empty())
    {
        doc.remove("projects");
        modified = true;
    }

    if modified {
        doc.to_string()
    } else {
        content.to_string()
    }
}

/// Merge args array into command string
///
/// Converts: command = "llm", args = ["-m", "haiku"]
/// To: command = "llm -m haiku"
///
/// Only removes `args` if it can be successfully merged into `command`.
/// Preserves `args` if:
/// - `command` is missing or not a string
/// - `args` is not an array
fn merge_args_into_command(table: &mut toml_edit::Table) {
    // Validate preconditions before removing args
    let can_merge = table.get("args").is_some_and(|a| a.as_array().is_some())
        && table
            .get("command")
            .and_then(|c| c.as_value())
            .is_some_and(|v| v.as_str().is_some());

    if !can_merge {
        return;
    }

    // Now safe to remove and merge
    let args = table.remove("args").unwrap();
    let args_array = args.as_array().unwrap();
    let command = table
        .get_mut("command")
        .and_then(|c| c.as_value_mut())
        .unwrap();
    let cmd_str = command.as_str().unwrap();

    // Filter to string args only (non-strings are dropped)
    let args_str: Vec<&str> = args_array.iter().filter_map(|a| a.as_str()).collect();
    if !args_str.is_empty() {
        // Only add space if command is non-empty
        let new_command = if cmd_str.is_empty() {
            shell_join(&args_str)
        } else {
            format!("{} {}", cmd_str, shell_join(&args_str))
        };
        *command = toml_edit::Value::from(new_command);
    }
}

/// Join arguments with proper shell quoting using shell_escape
fn shell_join(args: &[&str]) -> String {
    args.iter()
        .map(|arg| escape(Cow::Borrowed(*arg)).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Information about deprecated config patterns that were found.
///
/// Used by `wt config show` to display full deprecation details including inline diff.
/// This struct combines detection results with context (paths, labels) for display.
#[derive(Debug)]
pub struct DeprecationInfo {
    /// Path to the config file with deprecations
    pub config_path: PathBuf,
    /// Path to the generated migration file (if written)
    pub migration_path: Option<PathBuf>,
    /// All detected deprecations
    pub deprecations: Deprecations,
    /// Label for this config (e.g., "User config", "Project config")
    pub label: String,
    /// True if in a linked worktree (migration file written to main worktree only)
    pub in_linked_worktree: bool,
}

impl DeprecationInfo {
    /// Returns true if any deprecations were found
    pub fn has_deprecations(&self) -> bool {
        !self.deprecations.is_empty()
    }
}

/// Check config content for deprecated patterns and optionally create migration file
///
/// Detects and migrates:
/// - Deprecated template variables (repo_root → repo_path, etc.)
/// - Deprecated [commit-generation] sections → [commit.generation]
/// - Deprecated args field (merged into command)
/// - Deprecated approved-commands in \[projects\] (moved to approvals.toml)
///
/// If deprecations are found and `warn_and_migrate` is true:
/// 1. Emits warnings listing the deprecated patterns
/// 2. Creates a single `.new` file with all migrations applied
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
/// When `show_brief_warning` is true, only a brief pointer to `wt config show` is emitted
/// instead of full deprecation details. Use this for commands other than `config show`.
///
/// Warnings are deduplicated per path per process.
///
/// Returns `Ok(Some(info))` if deprecations were found, `Ok(None)` otherwise.
pub fn check_and_migrate(
    path: &Path,
    content: &str,
    warn_and_migrate: bool,
    label: &str,
    repo: Option<&crate::git::Repository>,
    show_brief_warning: bool,
) -> anyhow::Result<Option<DeprecationInfo>> {
    // Detect all deprecation types
    let deprecations = detect_deprecations(content);

    if deprecations.is_empty() {
        // Config is clean - clear hint so future deprecations get full treatment.
        // This handles the case where a user fixes their config today, then months
        // later a new deprecation is introduced - they should get the full warning.
        // TODO: We want to avoid gunking up config loading with too many checks,
        // but a single git config --unset seems acceptable for now.
        if let Some(repo) = repo {
            let _ = repo.clear_hint(HINT_DEPRECATED_CONFIG);
        }
        return Ok(None);
    }

    // Build the .new path: "config.toml" -> "config.toml.new"
    // For files without extension: "config" -> "config.new" (not "config..new")
    let new_path = match path.extension() {
        Some(ext) => path.with_extension(format!("{}.new", ext.to_string_lossy())),
        None => path.with_extension("new"),
    };

    // Skip writing if: (a) this is a brief warning (not `wt config show`), AND
    //                  (b) migration file already exists
    // This means first-time deprecation gets automatic file write, after that
    // users run `wt config show` to get/update the migration file.
    let should_skip_write = show_brief_warning && new_path.exists();

    // Build deprecation info for return
    let mut info = DeprecationInfo {
        config_path: path.to_path_buf(),
        migration_path: None,
        deprecations,
        label: label.to_string(),
        in_linked_worktree: !warn_and_migrate,
    };

    // Skip warning entirely if not in main worktree (for project config)
    if !warn_and_migrate {
        return Ok(Some(info));
    }

    // Deduplicate warnings per path per process
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    {
        let mut guard = WARNED_DEPRECATED_PATHS.lock().unwrap();
        if guard.contains(&canonical_path) {
            // Already warned, but still set migration_path if file exists
            if new_path.exists() {
                info.migration_path = Some(new_path);
            }
            return Ok(Some(info));
        }
        guard.insert(canonical_path.clone());
    }

    // For brief warnings (non-config-show commands), just show a pointer
    if show_brief_warning {
        eprintln!("{}", format_brief_warning(label));

        // Still write migration file if needed (first time only)
        if !should_skip_write
            && let Some(new_path) = write_migration_file(path, content, &info.deprecations, repo)
        {
            // Show apply hint for first-time migration file write
            let new_filename = new_path
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default();
            let new_path_str = escape(new_path.to_slash_lossy());
            let path_str = escape(path.to_slash_lossy());

            eprintln!(
                "{}",
                info_message(cformat!(
                    "Wrote migrated <bold>{new_filename}</>. To apply:"
                ))
            );
            eprintln!(
                "{}",
                format_bash_with_gutter(&format!("mv -- {} {}", new_path_str, path_str))
            );

            info.migration_path = Some(new_path);
        }

        std::io::stderr().flush().ok();
        return Ok(Some(info));
    }

    // Silent mode for `wt config show` - just write migration file and return info
    // The caller will use format_deprecation_details() to add output to its buffer
    if !should_skip_write {
        info.migration_path = write_migration_file(path, content, &info.deprecations, repo);
    }

    Ok(Some(info))
}

/// Format brief warning for normal config loading.
///
/// Returns a formatted warning string. Does not print anything;
/// caller is responsible for output.
pub fn format_brief_warning(label: &str) -> String {
    warning_message(cformat!(
        "{} has deprecated settings. To see details, run <bold>wt config show</>",
        label
    ))
    .to_string()
}

/// Write migration file with all fixes applied.
///
/// Applies all deprecation fixes (variable renames, section migrations) and writes
/// the result to a `.new` file alongside the original config.
///
/// Returns `Some(path)` if file was written successfully, `None` otherwise.
///
/// # Arguments
/// * `path` - Path to the original config file
/// * `content` - Content of the config file
/// * `deprecations` - Detected deprecations to fix
/// * `repo` - Optional repository for hint tracking (project config only)
pub fn write_migration_file(
    path: &Path,
    content: &str,
    deprecations: &Deprecations,
    repo: Option<&crate::git::Repository>,
) -> Option<PathBuf> {
    // Build the .new path: "config.toml" -> "config.toml.new"
    let new_path = path.with_extension(format!(
        "{}.new",
        path.extension().unwrap_or_default().to_string_lossy()
    ));

    // Apply all migrations to generate new content
    let mut new_content = content.to_string();
    if !deprecations.vars.is_empty() {
        new_content = replace_deprecated_vars(&new_content);
    }
    if !deprecations.commit_gen.is_empty() {
        new_content = migrate_commit_generation_sections(&new_content);
    }
    if deprecations.approved_commands {
        new_content = remove_approved_commands_from_config(&new_content);
    }

    if let Err(e) = std::fs::write(&new_path, &new_content) {
        // Log write failure but don't block config loading
        log::warn!("Could not write migration file: {}", e);
        return None;
    }

    // Mark hint as shown for project config
    if let Some(repo) = repo {
        let _ = repo.mark_hint_shown(HINT_DEPRECATED_CONFIG);
    }

    Some(new_path)
}

/// Format the diff between original and migrated config files as a string
pub fn format_migration_diff(original_path: &Path, new_path: &Path) -> Option<String> {
    // Run git diff and return the formatted output
    // -U3: Show 3 lines of context (git default)
    // Use -- to separate options from file paths (guards against filenames starting with -)
    if let Ok(output) = Cmd::new("git")
        .args(["diff", "--no-index", "--color=always", "-U3", "--"])
        .arg(original_path.to_slash_lossy().into_owned())
        .arg(new_path.to_slash_lossy().into_owned())
        .run()
    {
        // git diff --no-index exits 1 when files differ, which is expected
        let diff_output = String::from_utf8_lossy(&output.stdout);
        if !diff_output.is_empty() {
            return Some(format_with_gutter(diff_output.trim_end(), None));
        }
    }
    None
}

/// Format deprecation details for display (for use by wt config show)
///
/// Returns formatted output including:
/// - Warning message listing deprecated patterns
/// - Migration hint with apply command
/// - Inline diff showing the changes
pub fn format_deprecation_details(info: &DeprecationInfo) -> String {
    use std::fmt::Write;
    let mut out = String::new();

    // Warning message listing deprecated patterns
    if !info.deprecations.vars.is_empty() {
        let var_list: Vec<String> = info
            .deprecations
            .vars
            .iter()
            .map(|(old, new)| cformat!("<dim>{}</> → <bold>{}</>", old, new))
            .collect();
        let _ = writeln!(
            out,
            "{}",
            warning_message(format!(
                "{} uses deprecated template variables: {}",
                info.label,
                var_list.join(", ")
            ))
        );
    }

    if !info.deprecations.commit_gen.is_empty() {
        let mut parts = Vec::new();
        if info.deprecations.commit_gen.has_top_level {
            parts.push("[commit-generation] → [commit.generation]".to_string());
        }
        for project_key in &info.deprecations.commit_gen.project_keys {
            parts.push(format!(
                "[projects.\"{}\".commit-generation] → [projects.\"{}\".commit.generation]",
                project_key, project_key
            ));
        }
        let _ = writeln!(
            out,
            "{}",
            warning_message(format!(
                "{} uses deprecated config sections: {}",
                info.label,
                parts.join(", ")
            ))
        );
    }

    if info.deprecations.approved_commands {
        let _ = writeln!(
            out,
            "{}",
            warning_message(format!(
                "{} has approved-commands in [projects] sections (moved to approvals.toml)",
                info.label
            ))
        );
    }

    // Migration hint with apply command
    if let Some(new_path) = &info.migration_path {
        let new_filename = new_path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();
        // Shell-escape paths for safe copy/paste
        let new_path_str = escape(new_path.to_slash_lossy());
        let path_str = escape(info.config_path.to_slash_lossy());

        let _ = writeln!(
            out,
            "{}",
            info_message(cformat!(
                "Wrote migrated <bold>{new_filename}</>. To apply:"
            ))
        );
        let _ = writeln!(
            out,
            "{}",
            format_bash_with_gutter(&format!("mv -- {} {}", new_path_str, path_str))
        );

        // Inline diff with intro
        if let Some(diff) = format_migration_diff(&info.config_path, new_path) {
            let _ = writeln!(out, "{}", info_message("Diff:"));
            let _ = writeln!(out, "{}", diff);
        }
    } else if info.in_linked_worktree {
        // In linked worktree - migration file is written to main worktree
        let _ = writeln!(
            out,
            "{}",
            hint_message(cformat!(
                "To generate migration file, run <bright-black>wt config show</> from main worktree",
            ))
        );
    }

    out
}

/// Returns the config location where this key belongs, if it's in the wrong config.
///
/// Generic over `C`, the config type where the key was found. If the key would
/// be valid in `C::Other`, returns that config's description.
///
/// For example, `key_belongs_in::<ProjectConfig>("commit-generation")` returns
/// `Some("user config")`.
/// Returns `None` if the key is truly unknown (not valid in either config).
pub fn key_belongs_in<C: WorktrunkConfig>(key: &str) -> Option<&'static str> {
    C::Other::is_valid_key(key).then(C::Other::description)
}

/// Warn about unknown fields in config file
///
/// Generic over `C`, the config type being loaded. Emits a warning for each
/// unknown field, deduplicated per path per process.
///
/// When an unknown key belongs in the other config type (`C::Other`),
/// the warning includes a hint about where to move it.
///
/// The `label` is used in the warning message (e.g., "User config" or "Project config").
pub fn warn_unknown_fields<C: WorktrunkConfig>(
    path: &Path,
    unknown_keys: &HashMap<String, toml::Value>,
    label: &str,
) {
    if unknown_keys.is_empty() {
        return;
    }

    // Deduplicate warnings per path per process
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    {
        let mut guard = WARNED_UNKNOWN_PATHS.lock().unwrap();
        if guard.contains(&canonical_path) {
            return; // Already warned, skip
        }
        guard.insert(canonical_path);
    }

    // Sort keys for deterministic output order
    let mut keys: Vec<_> = unknown_keys.keys().collect();
    keys.sort();

    for key in keys {
        if let Some(other_location) = key_belongs_in::<C>(key) {
            eprintln!(
                "{}",
                warning_message(cformat!(
                    "{label} has key <bold>{key}</> which belongs in {other_location} (will be ignored)"
                ))
            );
        } else {
            eprintln!(
                "{}",
                warning_message(cformat!(
                    "{label} has unknown field <bold>{key}</> (will be ignored)"
                ))
            );
        }
    }

    // Flush stderr to ensure output appears before any subsequent messages
    std::io::stderr().flush().ok();
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

        // Should return Ok(Some(_)) even if write fails - the function logs error but doesn't fail
        let result =
            check_and_migrate(non_existent_path, content, true, "Test config", None, false);
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }

    #[test]
    fn test_check_and_migrate_deduplicates_warnings() {
        // Test that calling twice with same path skips the second warning
        let content = r#"post-create = "{{ repo_root }}/script.sh""#;
        // Use a unique path that won't collide with other tests
        let unique_path = std::path::Path::new("/nonexistent/dedup_test_12345/config.toml");

        // First call should process normally
        let result1 = check_and_migrate(unique_path, content, true, "Test config", None, false);
        assert!(result1.is_ok());
        assert!(result1.unwrap().is_some());

        // Second call with same path should early-return (hits the deduplication branch)
        let result2 = check_and_migrate(unique_path, content, true, "Test config", None, false);
        assert!(result2.is_ok());
        assert!(result2.unwrap().is_some());
    }

    #[test]
    fn test_migration_path_with_extension() {
        // config.toml -> config.toml.new
        let path = std::path::Path::new("/tmp/config.toml");
        let new_path = match path.extension() {
            Some(ext) => path.with_extension(format!("{}.new", ext.to_string_lossy())),
            None => path.with_extension("new"),
        };
        assert_eq!(new_path.to_str().unwrap(), "/tmp/config.toml.new");
    }

    #[test]
    fn test_migration_path_without_extension() {
        // config -> config.new (not config..new)
        let path = std::path::Path::new("/tmp/config");
        let new_path = match path.extension() {
            Some(ext) => path.with_extension(format!("{}.new", ext.to_string_lossy())),
            None => path.with_extension("new"),
        };
        assert_eq!(new_path.to_str().unwrap(), "/tmp/config.new");
    }

    #[test]
    fn test_migration_path_hidden_file() {
        // .hidden -> .hidden.new (not .hidden..new)
        // Note: .hidden has no extension (the dot is part of the filename)
        let path = std::path::Path::new("/tmp/.hidden");
        let new_path = match path.extension() {
            Some(ext) => path.with_extension(format!("{}.new", ext.to_string_lossy())),
            None => path.with_extension("new"),
        };
        assert_eq!(new_path.to_str().unwrap(), "/tmp/.hidden.new");
    }

    // Tests for commit-generation section migration

    #[test]
    fn test_find_commit_generation_deprecations_none() {
        let content = r#"
[commit.generation]
command = "llm -m haiku"
"#;
        let result = find_commit_generation_deprecations(content);
        assert!(result.is_empty());
    }

    #[test]
    fn test_find_commit_generation_deprecations_top_level() {
        let content = r#"
[commit-generation]
command = "llm -m haiku"
"#;
        let result = find_commit_generation_deprecations(content);
        assert!(result.has_top_level);
        assert!(result.project_keys.is_empty());
    }

    #[test]
    fn test_find_commit_generation_deprecations_project_level() {
        let content = r#"
[projects."github.com/user/repo".commit-generation]
command = "llm -m gpt-4"
"#;
        let result = find_commit_generation_deprecations(content);
        assert!(!result.has_top_level);
        assert_eq!(result.project_keys, vec!["github.com/user/repo"]);
    }

    #[test]
    fn test_find_commit_generation_deprecations_multiple_projects() {
        let content = r#"
[commit-generation]
command = "llm -m haiku"

[projects."github.com/user/repo1".commit-generation]
command = "llm -m gpt-4"

[projects."github.com/user/repo2".commit-generation]
command = "llm -m opus"
"#;
        let result = find_commit_generation_deprecations(content);
        assert!(result.has_top_level);
        assert_eq!(result.project_keys.len(), 2);
        assert!(
            result
                .project_keys
                .contains(&"github.com/user/repo1".to_string())
        );
        assert!(
            result
                .project_keys
                .contains(&"github.com/user/repo2".to_string())
        );
    }

    #[test]
    fn test_migrate_commit_generation_simple() {
        let content = r#"
[commit-generation]
command = "llm -m haiku"
"#;
        let result = migrate_commit_generation_sections(content);
        assert!(result.contains("[commit.generation]"));
        assert!(result.contains("command = \"llm -m haiku\""));
        assert!(!result.contains("[commit-generation]"));
    }

    #[test]
    fn test_migrate_commit_generation_with_args() {
        let content = r#"
[commit-generation]
command = "llm"
args = ["-m", "haiku"]
"#;
        let result = migrate_commit_generation_sections(content);
        assert!(result.contains("[commit.generation]"));
        assert!(result.contains("command = \"llm -m haiku\""));
        assert!(!result.contains("args"));
    }

    #[test]
    fn test_migrate_commit_generation_args_with_spaces() {
        let content = r#"
[commit-generation]
command = "llm"
args = ["-m", "claude haiku 4.5"]
"#;
        let result = migrate_commit_generation_sections(content);
        assert!(result.contains("[commit.generation]"));
        // Args with spaces should be quoted
        assert!(result.contains("command = \"llm -m 'claude haiku 4.5'\""));
    }

    #[test]
    fn test_migrate_commit_generation_project_level() {
        let content = r#"
[projects."github.com/user/repo".commit-generation]
command = "llm -m gpt-4"
"#;
        let result = migrate_commit_generation_sections(content);
        assert!(result.contains("[projects.\"github.com/user/repo\".commit.generation]"));
        assert!(result.contains("command = \"llm -m gpt-4\""));
        assert!(!result.contains("commit-generation"));
    }

    #[test]
    fn test_migrate_commit_generation_preserves_other_fields() {
        let content = r#"
[commit-generation]
command = "llm -m haiku"
template = "Write commit: {{ diff }}"
"#;
        let result = migrate_commit_generation_sections(content);
        assert!(result.contains("[commit.generation]"));
        assert!(result.contains("command = \"llm -m haiku\""));
        assert!(result.contains("template = \"Write commit: {{ diff }}\""));
    }

    #[test]
    fn test_migrate_commit_generation_preserves_existing_commit_section() {
        let content = r#"
[commit]
stage = "all"

[commit-generation]
command = "llm -m haiku"
"#;
        let result = migrate_commit_generation_sections(content);
        assert!(result.contains("[commit]"));
        assert!(result.contains("stage = \"all\""));
        assert!(result.contains("[commit.generation]"));
        assert!(result.contains("command = \"llm -m haiku\""));
    }

    #[test]
    fn test_migrate_no_changes_needed() {
        let content = r#"
[commit.generation]
command = "llm -m haiku"
"#;
        let result = migrate_commit_generation_sections(content);
        // Should return unchanged content
        assert_eq!(result, content);
    }

    #[test]
    fn test_migrate_skips_when_new_section_exists() {
        // When both old and new sections exist, migration should NOT overwrite
        // the new section (new takes precedence)
        let content = r#"
[commit.generation]
command = "new-command"

[commit-generation]
command = "old-command"
"#;
        let result = migrate_commit_generation_sections(content);
        // New section should be preserved, old section should be removed but not migrated
        assert!(
            result.contains("command = \"new-command\""),
            "New command should be preserved"
        );
        // Old section is left alone (not migrated since new exists)
        assert!(
            result.contains("[commit-generation]"),
            "Old section is left as-is since new already exists"
        );
    }

    #[test]
    fn test_find_deprecations_skips_when_new_section_exists() {
        // When new section exists, don't flag old section as deprecated
        let content = r#"
[commit.generation]
command = "new-command"

[commit-generation]
command = "old-command"
"#;
        let result = find_commit_generation_deprecations(content);
        assert!(
            !result.has_top_level,
            "Should not flag deprecation when new section exists"
        );
    }

    #[test]
    fn test_find_deprecations_skips_empty_section() {
        // Empty old section should not be flagged
        let content = r#"
[commit-generation]
"#;
        let result = find_commit_generation_deprecations(content);
        assert!(
            !result.has_top_level,
            "Should not flag empty deprecated section"
        );
    }

    #[test]
    fn test_shell_join_simple() {
        assert_eq!(shell_join(&["-m", "haiku"]), "-m haiku");
    }

    #[test]
    fn test_shell_join_with_spaces() {
        assert_eq!(shell_join(&["-m", "claude haiku"]), "-m 'claude haiku'");
    }

    #[test]
    fn test_shell_join_with_quotes() {
        assert_eq!(shell_join(&["echo", "it's"]), "echo 'it'\\''s'");
    }

    #[test]
    fn test_combined_migrations_template_vars_and_section_rename() {
        // Test that both deprecated template variables AND deprecated
        // [commit-generation] section are migrated in a single pass
        let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"

[commit-generation]
command = "llm"
args = ["-m", "haiku"]
"#;
        // First apply template var replacements
        let step1 = replace_deprecated_vars(content);
        assert!(step1.contains("{{ repo }}"), "main_worktree → repo");

        // Then apply section migration
        let step2 = migrate_commit_generation_sections(&step1);
        assert!(step2.contains("[commit.generation]"), "Section renamed");
        assert!(
            step2.contains("command = \"llm -m haiku\""),
            "Args merged into command"
        );
        assert!(
            !step2.contains("[commit-generation]"),
            "Old section removed"
        );
        assert!(!step2.contains("args"), "Args field removed");
    }

    // Tests for inline table handling

    #[test]
    fn test_find_deprecations_inline_table_top_level() {
        // Inline table format: commit-generation = { command = "llm" }
        let content = r#"
commit-generation = { command = "llm -m haiku" }
"#;
        let result = find_commit_generation_deprecations(content);
        assert!(result.has_top_level, "Should detect inline table format");
    }

    #[test]
    fn test_find_deprecations_inline_table_project_level() {
        let content = r#"
[projects."github.com/user/repo"]
commit-generation = { command = "llm -m gpt-4" }
"#;
        let result = find_commit_generation_deprecations(content);
        assert_eq!(
            result.project_keys,
            vec!["github.com/user/repo"],
            "Should detect project-level inline table"
        );
    }

    #[test]
    fn test_migrate_inline_table_top_level() {
        let content = r#"
commit-generation = { command = "llm", args = ["-m", "haiku"] }
"#;
        let result = migrate_commit_generation_sections(content);
        assert!(
            result.contains("[commit.generation]") || result.contains("[commit]"),
            "Should migrate inline table"
        );
        assert!(
            result.contains("command = \"llm -m haiku\""),
            "Should merge args into command"
        );
        assert!(
            !result.contains("commit-generation"),
            "Should remove old inline table"
        );
    }

    #[test]
    fn test_find_deprecations_malformed_generation_not_table() {
        // If commit.generation is a string (malformed), should still warn about old format
        let content = r#"
[commit]
generation = "not a table"

[commit-generation]
command = "llm -m haiku"
"#;
        let result = find_commit_generation_deprecations(content);
        assert!(
            result.has_top_level,
            "Should flag deprecated section when new section is malformed"
        );
    }

    #[test]
    fn test_migrate_inline_table_project_level() {
        let content = r#"
[projects."github.com/user/repo"]
commit-generation = { command = "llm", args = ["-m", "gpt-4"] }
"#;
        let result = migrate_commit_generation_sections(content);
        assert!(
            result.contains("[projects.\"github.com/user/repo\".commit.generation]")
                || result.contains("[projects.\"github.com/user/repo\".commit]"),
            "Should migrate project-level inline table"
        );
        assert!(
            result.contains("command = \"llm -m gpt-4\""),
            "Should merge args into command"
        );
        assert!(
            !result.contains("commit-generation"),
            "Should remove old inline table"
        );
    }

    #[test]
    fn test_migrate_preserves_existing_commit_stage() {
        // When [commit] section already exists with other fields, preserve them
        let content = r#"
[commit]
stage = "all"

[commit-generation]
command = "llm -m haiku"
"#;
        let result = migrate_commit_generation_sections(content);
        assert!(result.contains("stage = \"all\""), "Should preserve stage");
        assert!(
            result.contains("[commit.generation]"),
            "Should add generation subsection"
        );
        assert!(
            result.contains("command = \"llm -m haiku\""),
            "Should migrate command"
        );
    }

    #[test]
    fn test_find_deprecations_empty_inline_table() {
        // Empty inline table should not be flagged
        let content = r#"
commit-generation = {}
"#;
        let result = find_commit_generation_deprecations(content);
        assert!(
            !result.has_top_level,
            "Should not flag empty inline table as deprecated"
        );
    }

    #[test]
    fn test_migrate_args_without_command_preserved() {
        // When args exists but command doesn't, args should be preserved
        // (merge_args_into_command won't run without a command)
        let content = r#"
[commit-generation]
args = ["-m", "haiku"]
template = "some template"
"#;
        let result = migrate_commit_generation_sections(content);
        assert!(
            result.contains("[commit.generation]"),
            "Section should be renamed"
        );
        // Args should be preserved since there's no command to merge into
        assert!(
            result.contains("args ="),
            "Args should be preserved when no command exists"
        );
    }

    #[test]
    fn test_migrate_args_with_non_string_command() {
        // When command is not a string (e.g., integer), args should be preserved
        let content = r#"
[commit-generation]
command = 123
args = ["-m", "haiku"]
"#;
        let result = migrate_commit_generation_sections(content);
        // Args should be preserved since command is not a string
        assert!(
            result.contains("args ="),
            "Args should be preserved when command is not a string"
        );
    }

    #[test]
    fn test_migrate_command_only_no_args() {
        // When only command exists (no args), it should migrate cleanly
        let content = r#"
[commit-generation]
command = "llm -m haiku"
"#;
        let result = migrate_commit_generation_sections(content);
        assert!(result.contains("[commit.generation]"));
        assert!(result.contains("command = \"llm -m haiku\""));
        assert!(!result.contains("args"));
    }

    #[test]
    fn test_migrate_empty_command_with_args() {
        // When command is empty string but args exist, args become the command
        let content = r#"
[commit-generation]
command = ""
args = ["-m", "haiku"]
"#;
        let result = migrate_commit_generation_sections(content);
        assert!(
            result.contains("[commit.generation]"),
            "Section should be renamed"
        );
        // Empty command + args should produce just args as command
        assert!(
            result.contains("command = \"-m haiku\""),
            "Empty command should be replaced with args"
        );
        assert!(
            !result.contains("args"),
            "Args field should be removed after merge"
        );
    }

    #[test]
    fn test_migrate_malformed_string_value_unchanged() {
        // When commit-generation is a string (malformed), migration skips it
        // This exercises the `_ => None` branch in the match
        let content = r#"
commit-generation = "not a table"
other = "value"
"#;
        let result = migrate_commit_generation_sections(content);
        // Malformed value is removed (doc.remove happens), but no migration occurs
        // The content stays mostly unchanged since we don't add [commit.generation]
        assert!(
            !result.contains("[commit.generation]"),
            "Should not create new section for malformed input"
        );
    }

    #[test]
    fn test_migrate_malformed_project_level_string_unchanged() {
        // When project-level commit-generation is a string, migration skips it
        let content = r#"
[projects."github.com/user/repo"]
commit-generation = "not a table"
other = "value"
"#;
        let result = migrate_commit_generation_sections(content);
        assert!(
            !result.contains("[projects.\"github.com/user/repo\".commit.generation]"),
            "Should not create new section for malformed project-level input"
        );
    }

    #[test]
    fn test_migrate_invalid_toml_returns_unchanged() {
        // When content is not valid TOML, return it unchanged
        let content = "this is [not valid {toml";
        let result = migrate_commit_generation_sections(content);
        assert_eq!(result, content, "Invalid TOML should be returned unchanged");
    }

    // Snapshot tests for migration output (showing diffs)

    /// Generate a unified diff between original and migrated content
    fn migration_diff(original: &str, migrated: &str) -> String {
        use similar::{ChangeTag, TextDiff};
        let diff = TextDiff::from_lines(original, migrated);
        let mut output = String::new();
        for change in diff.iter_all_changes() {
            let sign = match change.tag() {
                ChangeTag::Delete => "-",
                ChangeTag::Insert => "+",
                ChangeTag::Equal => " ",
            };
            output.push_str(&format!("{}{}", sign, change));
        }
        output
    }

    #[test]
    fn snapshot_migrate_commit_generation_simple() {
        let content = r#"
[commit-generation]
command = "llm -m haiku"
"#;
        let result = migrate_commit_generation_sections(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    #[test]
    fn snapshot_migrate_commit_generation_with_args() {
        let content = r#"
[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4.5"]
"#;
        let result = migrate_commit_generation_sections(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    #[test]
    fn snapshot_migrate_with_trailing_sections() {
        // This is the bug case: [commit-generation] in the middle of the file
        // followed by other sections. The migration should not add an extra
        // [commit] section at the end.
        let content = r#"# Config file
worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4.5"]

[list]
branches = true
remotes = false
"#;
        let result = migrate_commit_generation_sections(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    #[test]
    fn snapshot_migrate_preserves_existing_commit_section() {
        let content = r#"
[commit]
stage = "all"

[commit-generation]
command = "llm -m haiku"
"#;
        let result = migrate_commit_generation_sections(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    #[test]
    fn snapshot_migrate_project_level() {
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = ["npm test"]

[projects."github.com/user/repo".commit-generation]
command = "llm"
args = ["-m", "gpt-4"]
"#;
        let result = migrate_commit_generation_sections(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    #[test]
    fn snapshot_migrate_combined_top_and_project() {
        let content = r#"
[commit-generation]
command = "llm -m haiku"

[projects."github.com/user/repo".commit-generation]
command = "llm -m gpt-4"

[list]
branches = true
"#;
        let result = migrate_commit_generation_sections(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    // Tests for approved-commands deprecation detection

    #[test]
    fn test_find_approved_commands_deprecation_none() {
        let content = r#"
[commit.generation]
command = "llm -m haiku"
"#;
        assert!(!find_approved_commands_deprecation(content));
    }

    #[test]
    fn test_find_approved_commands_deprecation_present() {
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = ["npm install", "npm test"]
"#;
        assert!(find_approved_commands_deprecation(content));
    }

    #[test]
    fn test_find_approved_commands_deprecation_empty_array() {
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = []
"#;
        assert!(!find_approved_commands_deprecation(content));
    }

    #[test]
    fn test_find_approved_commands_deprecation_no_projects() {
        let content = r#"
worktree-path = "../{{ repo }}.{{ branch }}"
"#;
        assert!(!find_approved_commands_deprecation(content));
    }

    #[test]
    fn test_find_approved_commands_deprecation_project_without_approvals() {
        let content = r#"
[projects."github.com/user/repo"]
worktree-path = ".worktrees/{{ branch | sanitize }}"
"#;
        assert!(!find_approved_commands_deprecation(content));
    }

    // Tests for remove_approved_commands_from_config

    #[test]
    fn test_remove_approved_commands_simple() {
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = ["npm install", "npm test"]
"#;
        let result = remove_approved_commands_from_config(content);
        assert!(!result.contains("approved-commands"));
        // Empty project section and empty projects table should be removed
        assert!(!result.contains("[projects"));
    }

    #[test]
    fn test_remove_approved_commands_preserves_other_fields() {
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = ["npm install"]
worktree-path = ".worktrees/{{ branch | sanitize }}"
"#;
        let result = remove_approved_commands_from_config(content);
        assert!(!result.contains("approved-commands"));
        assert!(result.contains("worktree-path"));
        assert!(result.contains("projects"));
    }

    #[test]
    fn test_remove_approved_commands_multiple_projects() {
        let content = r#"
[projects."github.com/user/repo1"]
approved-commands = ["npm install"]

[projects."github.com/user/repo2"]
approved-commands = ["cargo test"]
worktree-path = ".worktrees/{{ branch | sanitize }}"
"#;
        let result = remove_approved_commands_from_config(content);
        assert!(!result.contains("approved-commands"));
        // repo1 had only approved-commands, so its section should be removed
        assert!(!result.contains("repo1"));
        // repo2 has other fields, so its section should remain
        assert!(result.contains("repo2"));
        assert!(result.contains("worktree-path"));
    }

    #[test]
    fn test_remove_approved_commands_no_change() {
        let content = r#"
[projects."github.com/user/repo"]
worktree-path = ".worktrees/{{ branch | sanitize }}"
"#;
        let result = remove_approved_commands_from_config(content);
        assert_eq!(result, content);
    }

    #[test]
    fn snapshot_remove_approved_commands() {
        let content = r#"worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

[projects."github.com/user/repo"]
approved-commands = ["npm install", "npm test"]
worktree-path = ".worktrees/{{ branch | sanitize }}"
"#;
        let result = remove_approved_commands_from_config(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    #[test]
    fn snapshot_remove_approved_commands_entire_section() {
        let content = r#"worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

[projects."github.com/user/repo"]
approved-commands = ["npm install"]
"#;
        let result = remove_approved_commands_from_config(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    #[test]
    fn test_detect_deprecations_includes_approved_commands() {
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = ["npm install"]
"#;
        let deprecations = detect_deprecations(content);
        assert!(deprecations.approved_commands);
        assert!(!deprecations.is_empty());
    }

    #[test]
    fn test_remove_approved_commands_invalid_toml() {
        let content = "this is { not valid toml";
        let result = remove_approved_commands_from_config(content);
        assert_eq!(result, content, "Invalid TOML should be returned unchanged");
    }

    #[test]
    fn test_format_deprecation_details_approved_commands() {
        let info = DeprecationInfo {
            config_path: std::path::PathBuf::from("/tmp/test-config.toml"),
            migration_path: None,
            deprecations: Deprecations {
                vars: vec![],
                commit_gen: CommitGenerationDeprecations::default(),
                approved_commands: true,
            },
            label: "User config".to_string(),
            in_linked_worktree: false,
        };
        let output = format_deprecation_details(&info);
        assert!(
            output.contains("approved-commands"),
            "Should mention approved-commands in output: {}",
            output
        );
        assert!(
            output.contains("approvals.toml"),
            "Should mention approvals.toml: {}",
            output
        );
    }

    #[test]
    fn test_write_migration_file_with_approved_commands() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        let content = r#"worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

[projects."github.com/user/repo"]
approved-commands = ["npm install"]
"#;
        std::fs::write(&config_path, content).unwrap();

        let deprecations = Deprecations {
            vars: vec![],
            commit_gen: CommitGenerationDeprecations::default(),
            approved_commands: true,
        };
        let result = write_migration_file(&config_path, content, &deprecations, None);
        assert!(
            result.is_some(),
            "Should write migration file for approved-commands"
        );
        let migration_path = result.unwrap();
        let migrated = std::fs::read_to_string(&migration_path).unwrap();
        assert!(!migrated.contains("approved-commands"));
    }

    #[test]
    fn test_set_implicit_suppresses_parent_header() {
        // Verifies that set_implicit(true) prevents an empty parent table from
        // rendering its own header. This is the key technique used in
        // migrate_commit_generation_sections to avoid creating spurious [commit]
        // headers when migrating [commit-generation] to [commit.generation].
        use toml_edit::{DocumentMut, Item, Table};

        let mut doc: DocumentMut = "[foo]\nbar = 1\n".parse().unwrap();
        let mut commit_table = Table::new();
        commit_table.set_implicit(true);
        let mut gen_table = Table::new();
        gen_table.insert("command", toml_edit::value("llm"));
        commit_table.insert("generation", Item::Table(gen_table));
        doc.insert("commit", Item::Table(commit_table));
        let result = doc.to_string();

        assert!(
            !result.contains("\n[commit]\n"),
            "set_implicit should suppress separate [commit] header"
        );
        assert!(
            result.contains("[commit.generation]"),
            "Should have [commit.generation] header"
        );
    }
}
