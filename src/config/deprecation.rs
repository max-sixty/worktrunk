//! Deprecation detection and migration.
//!
//! Scans config files for deprecated patterns and surfaces them to the user:
//! - Deprecated template variables (repo_root → repo_path, etc.)
//! - Deprecated config sections (\[commit-generation\] → \[commit.generation\])
//! - Deprecated fields (args merged into command)
//! - Deprecated approved-commands in \[projects\] (moved to approvals.toml)
//!
//! Detection is purely in-memory — nothing writes to the filesystem from a
//! config load path. `check_and_migrate` returns the structurally migrated
//! content (for serde) and a `DeprecationInfo` describing what needs fixing.
//! Users materialize migrations explicitly via `wt config update` (which
//! overwrites the config file and copies approved-commands to `approvals.toml`)
//! or inspect them via `wt config show` / `wt config update --print`.
//!
//! Per-path warning dedup still applies within a process so `wt list` doesn't
//! spam the same deprecation message from multiple config layers.

use std::borrow::Cow;
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock};

use color_print::cformat;
use minijinja::Environment;
use regex::Regex;
use shell_escape::unix::escape;

use crate::config::WorktrunkConfig;
use crate::shell_exec::Cmd;
use crate::styling::{
    eprintln, format_with_gutter, hint_message, info_message, suggest_command_in_dir,
    warning_message,
};

/// Tracks which config paths have already shown deprecation warnings this process.
/// Prevents repeated warnings when config is loaded multiple times.
static WARNED_DEPRECATED_PATHS: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Set once the "Run wt config show..." hint has been emitted this process,
/// so multiple deprecated configs (user + project) share a single hint line.
static DEPRECATION_HINT_EMITTED: OnceLock<()> = OnceLock::new();

/// Latch that silences config deprecation/unknown-field warnings for the rest
/// of the process. Set by shell completion, picker, statusline, and help paths
/// — surfaces where stderr output would appear above the user's prompt or TUI.
static SUPPRESS_WARNINGS: OnceLock<()> = OnceLock::new();

pub fn suppress_warnings() {
    let _ = SUPPRESS_WARNINGS.set(());
}

fn warnings_suppressed() -> bool {
    SUPPRESS_WARNINGS.get().is_some()
}

/// Pre-compiled regexes for deprecated variable word-boundary matching.
/// Compiled once on first use, shared across all calls to normalize/replace.
static DEPRECATED_VAR_REGEXES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    DEPRECATED_VARS
        .iter()
        .map(|&(old, new)| {
            let re = Regex::new(&format!(r"\b{}\b", regex::escape(old))).unwrap();
            (re, new)
        })
        .collect()
});

/// Tracks which config paths have already shown unknown field warnings this process.
/// Prevents repeated warnings when config is loaded multiple times.
static WARNED_UNKNOWN_PATHS: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Mapping from deprecated variable name to its replacement
const DEPRECATED_VARS: &[(&str, &str)] = &[
    ("repo_root", "repo_path"),
    ("worktree", "worktree_path"),
    ("main_worktree", "repo"),
    ("main_worktree_path", "primary_worktree_path"),
];

/// Metadata for a deprecated top-level section key.
#[derive(Debug)]
pub struct DeprecatedSection {
    /// The deprecated key name (e.g., "commit-generation")
    pub key: &'static str,
    /// The canonical top-level key that replaces this, for determining which config type
    /// it belongs to via `WorktrunkConfig::is_valid_key()` (e.g., "commit")
    pub canonical_top_key: &'static str,
    /// Human-readable canonical form for display (e.g., "[commit.generation]")
    pub canonical_display: &'static str,
}

/// Top-level section keys that are deprecated and handled by the deprecation system.
///
/// When a deprecated key appears in the config type where its canonical replacement
/// is valid, `warn_unknown_fields` skips it (the deprecation system provides better
/// messaging). When it appears in the wrong config type, `warn_unknown_fields`
/// warns that it belongs in the other config with the canonical form.
pub const DEPRECATED_SECTION_KEYS: &[DeprecatedSection] = &[
    DeprecatedSection {
        key: "commit-generation",
        canonical_top_key: "commit",
        canonical_display: "[commit.generation]",
    },
    DeprecatedSection {
        key: "select",
        canonical_top_key: "switch",
        canonical_display: "[switch.picker]",
    },
    DeprecatedSection {
        key: "ci",
        canonical_top_key: "forge",
        canonical_display: "[forge]",
    },
];

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
    for (re, new) in DEPRECATED_VAR_REGEXES.iter() {
        result = re.replace_all(&result, *new).into_owned();
    }
    Cow::Owned(result)
}

/// Core logic for deprecated var detection, operating on pre-extracted template strings
fn find_deprecated_vars_from_strings(
    template_strings: &[String],
) -> Vec<(&'static str, &'static str)> {
    let mut used_vars = HashSet::new();
    let env = Environment::new();

    for template_str in template_strings {
        if let Ok(template) = env.template_from_str(template_str) {
            used_vars.extend(template.undeclared_variables(false));
        }
    }

    DEPRECATED_VARS
        .iter()
        .filter(|(old, _)| used_vars.contains(*old))
        .copied()
        .collect()
}

/// Extract all string values from an already-parsed TOML document
fn extract_template_strings_from_doc(doc: &toml_edit::DocumentMut) -> Vec<String> {
    let mut strings = Vec::new();
    collect_strings_from_edit_table(doc.as_table(), &mut strings);
    strings
}

/// Recursively collect all string values from a toml_edit table
fn collect_strings_from_edit_table(table: &toml_edit::Table, strings: &mut Vec<String>) {
    for (_, item) in table.iter() {
        collect_strings_from_edit_item(item, strings);
    }
}

/// Recursively collect all string values from a toml_edit item
fn collect_strings_from_edit_item(item: &toml_edit::Item, strings: &mut Vec<String>) {
    match item {
        toml_edit::Item::Value(v) => collect_strings_from_edit_value(v, strings),
        toml_edit::Item::Table(t) => collect_strings_from_edit_table(t, strings),
        toml_edit::Item::ArrayOfTables(arr) => {
            for t in arr.iter() {
                collect_strings_from_edit_table(t, strings);
            }
        }
        _ => {}
    }
}

/// Recursively collect all string values from a toml_edit value
fn collect_strings_from_edit_value(value: &toml_edit::Value, strings: &mut Vec<String>) {
    match value {
        toml_edit::Value::String(s) => strings.push(s.value().clone()),
        toml_edit::Value::Array(arr) => {
            for v in arr.iter() {
                collect_strings_from_edit_value(v, strings);
            }
        }
        toml_edit::Value::InlineTable(t) => {
            for (_, v) in t.iter() {
                collect_strings_from_edit_value(v, strings);
            }
        }
        _ => {}
    }
}

/// Core logic for variable replacement, operating on pre-extracted template strings
fn replace_deprecated_vars_from_strings(content: &str, template_strings: &[String]) -> String {
    let mut result = content.to_string();

    for original in template_strings {
        let mut modified = original.clone();
        for (re, new) in DEPRECATED_VAR_REGEXES.iter() {
            modified = re.replace_all(&modified, *new).into_owned();
        }
        if modified != *original {
            result = result.replace(original, &modified);
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
    /// Has `[select]` section (moved to `[switch.picker]`)
    pub select: bool,
    /// Has `[hooks.post-create]` (renamed to `[hooks.pre-start]`)
    pub post_create: bool,
    /// Has `[ci]` section (moved to `[forge]`)
    pub ci_section: bool,
    /// Has `no-ff` in `[merge]` section (use `ff` instead)
    pub no_ff: bool,
    /// Has `no-cd` in `[switch]` section (use `cd` instead)
    pub no_cd: bool,
    /// Pre-* hooks using multi-entry table form (will become concurrent in a future version)
    pub pre_hook_table_form: Vec<String>,
    /// Has `timeout-ms` under `[switch.picker]` (removed — picker now renders progressively)
    pub switch_picker_timeout_ms: bool,
}

impl Deprecations {
    /// Returns true if any deprecations were found
    pub fn is_empty(&self) -> bool {
        self.vars.is_empty()
            && self.commit_gen.is_empty()
            && !self.approved_commands
            && !self.select
            && !self.post_create
            && !self.ci_section
            && !self.no_ff
            && !self.no_cd
            && self.pre_hook_table_form.is_empty()
            && !self.switch_picker_timeout_ms
    }
}

/// Detect deprecations in config content. Pure function, no I/O.
///
/// Parses the TOML content once and checks all deprecation types against the
/// parsed document.
///
/// Returns a `Deprecations` struct containing all detected deprecation issues.
/// This is the recommended entry point for deprecation detection.
pub fn detect_deprecations(content: &str) -> Deprecations {
    let Ok(doc) = content.parse::<toml_edit::DocumentMut>() else {
        return Deprecations::default();
    };
    let template_strings = extract_template_strings_from_doc(&doc);
    detect_deprecations_from_doc(&doc, &template_strings)
}

/// Detect deprecations from an already-parsed document and pre-extracted template strings.
fn detect_deprecations_from_doc(
    doc: &toml_edit::DocumentMut,
    template_strings: &[String],
) -> Deprecations {
    Deprecations {
        vars: find_deprecated_vars_from_strings(template_strings),
        commit_gen: find_commit_generation_from_doc(doc),
        approved_commands: find_approved_commands_from_doc(doc),
        select: find_select_from_doc(doc),
        post_create: find_post_create_from_doc(doc),
        ci_section: find_ci_section_from_doc(doc),
        no_ff: find_negated_bool_from_doc(doc, "merge", "no-ff", "ff"),
        no_cd: find_negated_bool_from_doc(doc, "switch", "no-cd", "cd"),
        pre_hook_table_form: find_pre_hook_table_form_from_doc(doc),
        switch_picker_timeout_ms: find_switch_picker_timeout_from_doc(doc),
    }
}

fn find_approved_commands_from_doc(doc: &toml_edit::DocumentMut) -> bool {
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

fn find_commit_generation_from_doc(doc: &toml_edit::DocumentMut) -> CommitGenerationDeprecations {
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

fn migrate_commit_generation_doc(doc: &mut toml_edit::DocumentMut) -> bool {
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

    modified
}

/// Remove `approved-commands` from all `\[projects."..."\]` sections.
///
/// For each project section, removes the `approved-commands` key.
/// If a project section becomes empty after removal, removes the project entry.
/// If the `\[projects\]` table becomes empty, removes it.
fn remove_approved_commands_doc(doc: &mut toml_edit::DocumentMut) -> bool {
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

    modified
}

fn find_select_from_doc(doc: &toml_edit::DocumentMut) -> bool {
    if has_select_without_picker(doc) {
        return true;
    }

    // Check project-level sections
    if let Some(projects) = doc.get("projects").and_then(|p| p.as_table()) {
        for (_key, project_value) in projects.iter() {
            if let Some(project_table) = project_value.as_table()
                && has_select_without_picker(project_table)
            {
                return true;
            }
        }
    }

    false
}

/// Check if a table has a non-empty `select` section without `switch.picker`.
fn has_select_without_picker(table: &toml_edit::Table) -> bool {
    let has_new_section = table
        .get("switch")
        .and_then(|s| s.as_table())
        .and_then(|t| t.get("picker"))
        .is_some_and(|p| p.is_table() || p.is_inline_table());

    if has_new_section {
        return false;
    }

    if let Some(section) = table.get("select") {
        if let Some(t) = section.as_table() {
            return !t.is_empty();
        }
        if let Some(t) = section.as_inline_table() {
            return !t.is_empty();
        }
    }

    false
}

fn find_post_create_from_doc(doc: &toml_edit::DocumentMut) -> bool {
    // Top-level (user config or project config): hooks are flattened here
    if doc.get("pre-start").is_none() && doc.get("post-create").is_some_and(is_non_empty_item) {
        return true;
    }

    // Per-project overrides (user config): hooks are flattened into `[projects."id"]`
    if let Some(projects) = doc.get("projects").and_then(|p| p.as_table()) {
        for (_key, project_value) in projects.iter() {
            if let Some(project_table) = project_value.as_table()
                && project_table.get("pre-start").is_none()
                && project_table
                    .get("post-create")
                    .is_some_and(is_non_empty_item)
            {
                return true;
            }
        }
    }

    false
}

/// Check if a TOML item is non-empty (strings are always non-empty, tables must have entries).
fn is_non_empty_item(item: &toml_edit::Item) -> bool {
    match item {
        toml_edit::Item::Value(toml_edit::Value::InlineTable(t)) => !t.is_empty(),
        toml_edit::Item::Table(t) => !t.is_empty(),
        _ => true, // strings and other values are always "non-empty"
    }
}

/// Error message emitted when a config contains the removed `post-create`
/// hook key. Matches the wording used by `check_and_migrate` when the load
/// path converts detection into a fatal error.
const POST_CREATE_REMOVED_MSG: &str = "`post-create` hook was renamed to `pre-start` in v0.32.0 and the silent rewrite has been removed. Rename `post-create` to `pre-start` in your config.";

/// Migrate `[select]` sections to `[switch.picker]`.
///
/// Handles both top-level and project-level `[projects."...".select]` sections.
/// Skips each migration if `[switch.picker]` already exists at that level.
fn migrate_select_doc(doc: &mut toml_edit::DocumentMut) -> bool {
    let mut modified = false;

    // Migrate top-level [select] → [switch.picker]
    migrate_select_table(doc.as_table_mut(), &mut modified);

    // Migrate project-level [projects."...".select] → [projects."...".switch.picker]
    if let Some(projects) = doc.get_mut("projects").and_then(|p| p.as_table_mut()) {
        for (_key, project_value) in projects.iter_mut() {
            if let Some(project_table) = project_value.as_table_mut() {
                migrate_select_table(project_table, &mut modified);
            }
        }
    }

    modified
}

/// Migrate a `select` key to `switch.picker` within a table.
fn migrate_select_table(table: &mut toml_edit::Table, modified: &mut bool) {
    let has_new_section = table
        .get("switch")
        .and_then(|s| s.as_table())
        .and_then(|t| t.get("picker"))
        .is_some_and(|p| p.is_table() || p.is_inline_table());

    if has_new_section {
        return;
    }

    let Some(old_section) = table.remove("select") else {
        return;
    };

    let table_opt = match old_section {
        toml_edit::Item::Table(t) => Some(t),
        toml_edit::Item::Value(toml_edit::Value::InlineTable(it)) => Some(it.into_table()),
        _ => None,
    };

    let Some(select_table) = table_opt else {
        return;
    };

    if !table.contains_key("switch") {
        let mut switch_table = toml_edit::Table::new();
        switch_table.set_implicit(true);
        table.insert("switch", toml_edit::Item::Table(switch_table));
    }

    if let Some(switch_table) = table["switch"].as_table_mut() {
        switch_table.insert("picker", toml_edit::Item::Table(select_table));
    }

    *modified = true;
}

/// The 5 canonical pre-* hook keys.
const PRE_HOOK_KEYS: &[&str] = &[
    "pre-switch",
    "pre-start",
    "pre-commit",
    "pre-merge",
    "pre-remove",
];

/// Check if a table has a multi-entry pre-* hook (table form with 2+ named commands).
fn collect_pre_hook_table_form_keys(
    table: &toml_edit::Table,
    prefix: &str,
    found: &mut Vec<String>,
) {
    for &key in PRE_HOOK_KEYS {
        if let Some(item) = table.get(key)
            && item.as_table().is_some_and(|t| t.len() >= 2)
        {
            if prefix.is_empty() {
                found.push(key.to_string());
            } else {
                found.push(format!("{prefix}.{key}"));
            }
        }
    }
}

/// Find pre-* hooks using multi-entry table form.
///
/// Hooks are flattened into the top level of user config, project config, and
/// each `[projects."id"]` subtree. Returns display paths for each deprecated
/// hook found.
fn find_pre_hook_table_form_from_doc(doc: &toml_edit::DocumentMut) -> Vec<String> {
    let mut found = Vec::new();

    // Top-level (user config or project config)
    collect_pre_hook_table_form_keys(doc.as_table(), "", &mut found);

    // Per-project overrides (user config)
    if let Some(projects) = doc.get("projects").and_then(|p| p.as_table()) {
        for (project_key, project_value) in projects.iter() {
            if let Some(project_table) = project_value.as_table() {
                let prefix = format!("projects.\"{project_key}\"");
                collect_pre_hook_table_form_keys(project_table, &prefix, &mut found);
            }
        }
    }

    found
}

fn find_ci_section_from_doc(doc: &toml_edit::DocumentMut) -> bool {
    // Skip if [forge] already exists
    if doc
        .get("forge")
        .is_some_and(|f| f.is_table() || f.is_inline_table())
    {
        return false;
    }

    // Check if [ci] exists with a non-empty platform field
    doc.get("ci")
        .and_then(|ci| ci.as_table())
        .and_then(|t| t.get("platform"))
        .is_some_and(|p| p.as_str().is_some_and(|s| !s.is_empty()))
}

/// Migrate `[ci]` section to `[forge]`.
///
/// Moves `platform` from `[ci]` to `[forge]`, preserving the value.
/// Removes `[ci]` if `platform` was its only field.
/// Skips migration if `[forge]` already exists.
fn migrate_ci_doc(doc: &mut toml_edit::DocumentMut) -> bool {
    // Skip if [forge] already exists
    if doc
        .get("forge")
        .is_some_and(|f| f.is_table() || f.is_inline_table())
    {
        return false;
    }

    // Get platform value from [ci]
    let platform = doc
        .get("ci")
        .and_then(|ci| ci.as_table())
        .and_then(|t| t.get("platform"))
        .and_then(|p| p.as_str())
        .map(String::from);

    let Some(platform) = platform else {
        return false;
    };

    // Remove [ci] section (it only has platform)
    doc.remove("ci");

    // Create [forge] section with platform
    let mut forge_table = toml_edit::Table::new();
    forge_table.insert("platform", toml_edit::value(platform));
    doc.insert("forge", toml_edit::Item::Table(forge_table));

    true
}

/// Check if a section has a deprecated negated boolean field (e.g., `no-ff` without `ff`).
///
/// Checks both the top-level section and project-level sections.
fn find_negated_bool_from_doc(
    doc: &toml_edit::DocumentMut,
    section: &str,
    old_key: &str,
    new_key: &str,
) -> bool {
    // Check top-level section
    if let Some(table) = doc.get(section).and_then(|s| s.as_table())
        && !table.contains_key(new_key)
        && table.contains_key(old_key)
    {
        return true;
    }

    // Check project-level sections
    if let Some(projects) = doc.get("projects").and_then(|p| p.as_table()) {
        for (_key, project_value) in projects.iter() {
            if let Some(table) = project_value
                .as_table()
                .and_then(|t| t.get(section))
                .and_then(|s| s.as_table())
                && !table.contains_key(new_key)
                && table.contains_key(old_key)
            {
                return true;
            }
        }
    }

    false
}

/// Migrate a negated boolean field within a table (e.g., `no-ff = true` → `ff = false`).
///
/// Returns true if a migration was performed.
fn migrate_negated_bool(table: &mut toml_edit::Table, old_key: &str, new_key: &str) -> bool {
    if table.contains_key(new_key) {
        // New key takes precedence; remove the old one if present
        return table.remove(old_key).is_some();
    }
    let Some(old_item) = table.remove(old_key) else {
        return false;
    };
    if let Some(bool_val) = old_item.as_value().and_then(|v| v.as_bool()) {
        table.insert(new_key, toml_edit::value(!bool_val));
        true
    } else {
        // Put it back if we can't parse it
        table.insert(old_key, old_item);
        false
    }
}

/// Migrate a negated boolean field in a section and its project-level counterparts.
fn migrate_negated_bool_doc(
    doc: &mut toml_edit::DocumentMut,
    section: &str,
    old_key: &str,
    new_key: &str,
) -> bool {
    let mut modified = false;

    // Top-level section
    if let Some(table) = doc.get_mut(section).and_then(|s| s.as_table_mut())
        && migrate_negated_bool(table, old_key, new_key)
    {
        modified = true;
    }

    // Project-level sections
    if let Some(projects) = doc.get_mut("projects").and_then(|p| p.as_table_mut()) {
        for (_key, project_value) in projects.iter_mut() {
            if let Some(table) = project_value
                .as_table_mut()
                .and_then(|t| t.get_mut(section))
                .and_then(|s| s.as_table_mut())
                && migrate_negated_bool(table, old_key, new_key)
            {
                modified = true;
            }
        }
    }

    modified
}

/// Convert a multi-entry pre-* table section into an array-of-tables pipeline.
///
/// Removes `[key]` as a table section and inserts `[[key]]` blocks —
/// one block per named step, preserving insertion order.
///
/// Iterates pre-* keys in document order (not [`PRE_HOOK_KEYS`] order) so
/// migrated sections land in the same relative position they had in the
/// source file.
fn migrate_pre_hook_table_in(table: &mut toml_edit::Table, modified: &mut bool) {
    let keys_to_migrate: Vec<String> = table
        .iter()
        .filter(|(k, v)| {
            PRE_HOOK_KEYS.contains(k)
                && v.as_table()
                    .is_some_and(|t| t.len() >= 2 && t.iter().all(|(_, v)| v.as_str().is_some()))
        })
        .map(|(k, _)| k.to_string())
        .collect();

    for key in keys_to_migrate {
        let item = table.get_mut(&key).unwrap();
        let entries = item.as_table().unwrap();

        let mut arr = toml_edit::ArrayOfTables::new();
        for (name, value) in entries.iter() {
            let mut block = toml_edit::Table::new();
            block.insert(name, toml_edit::value(value.as_str().unwrap()));
            arr.push(block);
        }

        *item = toml_edit::Item::ArrayOfTables(arr);
        *modified = true;
    }
}

/// Migrate multi-entry pre-* hook table sections to pipeline arrays.
///
/// Hooks are flattened into the top level of user config, project config, and
/// each `[projects."id"]` subtree.
fn migrate_pre_hook_table_form_doc(doc: &mut toml_edit::DocumentMut) -> bool {
    let mut modified = false;

    // Top-level (user config or project config)
    migrate_pre_hook_table_in(doc.as_table_mut(), &mut modified);

    // Per-project overrides (user config)
    if let Some(projects) = doc.get_mut("projects").and_then(|p| p.as_table_mut()) {
        for (_key, project_value) in projects.iter_mut() {
            if let Some(project_table) = project_value.as_table_mut() {
                migrate_pre_hook_table_in(project_table, &mut modified);
            }
        }
    }

    modified
}

/// Apply all structural TOML migrations to a parsed document.
///
/// This is the single source of truth for config migration. Returns true if
/// any modifications were made.
///
/// Note: `replace_deprecated_vars` and `remove_approved_commands` are NOT
/// included here — template variable renaming is cosmetic (would break
/// `--var` overrides), and approved-commands is still a valid serde field.
fn migrate_content_doc(doc: &mut toml_edit::DocumentMut) -> bool {
    let mut modified = false;
    modified |= migrate_commit_generation_doc(doc);
    modified |= migrate_select_doc(doc);
    modified |= migrate_pre_hook_table_form_doc(doc);
    modified |= migrate_ci_doc(doc);
    modified |= migrate_negated_bool_doc(doc, "merge", "no-ff", "ff");
    modified |= migrate_negated_bool_doc(doc, "switch", "no-cd", "cd");
    modified |= migrate_switch_picker_timeout_doc(doc);
    modified
}

/// Check if a table has `timeout-ms` under `[switch.picker]`.
///
/// `[switch.picker]` can be written either as a section (regular table) or
/// inline (`picker = { ... }`); `toml_edit` surfaces these as different node
/// types, so both branches are needed.
fn has_switch_picker_timeout(table: &toml_edit::Table) -> bool {
    table
        .get("switch")
        .and_then(|s| s.as_table())
        .and_then(|t| t.get("picker"))
        .and_then(|p| match p {
            toml_edit::Item::Table(t) => Some(t.contains_key("timeout-ms")),
            toml_edit::Item::Value(toml_edit::Value::InlineTable(it)) => {
                Some(it.contains_key("timeout-ms"))
            }
            _ => None,
        })
        .unwrap_or(false)
}

fn find_switch_picker_timeout_from_doc(doc: &toml_edit::DocumentMut) -> bool {
    if has_switch_picker_timeout(doc.as_table()) {
        return true;
    }
    if let Some(projects) = doc.get("projects").and_then(|p| p.as_table()) {
        for (_key, project_value) in projects.iter() {
            if let Some(project_table) = project_value.as_table()
                && has_switch_picker_timeout(project_table)
            {
                return true;
            }
        }
    }
    false
}

/// Remove `timeout-ms` from `[switch.picker]` in a table (top-level or project).
fn remove_switch_picker_timeout_in(table: &mut toml_edit::Table) -> bool {
    let Some(picker) = table
        .get_mut("switch")
        .and_then(|s| s.as_table_mut())
        .and_then(|t| t.get_mut("picker"))
    else {
        return false;
    };
    match picker {
        toml_edit::Item::Table(t) => t.remove("timeout-ms").is_some(),
        toml_edit::Item::Value(toml_edit::Value::InlineTable(it)) => {
            it.remove("timeout-ms").is_some()
        }
        _ => false,
    }
}

/// Remove deprecated `timeout-ms` from `[switch.picker]` sections.
///
/// Strips the key at both the top level and under `[projects."..."]`. Empty
/// `[switch.picker]` sections are left in place — they round-trip harmlessly.
fn migrate_switch_picker_timeout_doc(doc: &mut toml_edit::DocumentMut) -> bool {
    let mut modified = remove_switch_picker_timeout_in(doc.as_table_mut());
    if let Some(projects) = doc.get_mut("projects").and_then(|p| p.as_table_mut()) {
        for (_key, project_value) in projects.iter_mut() {
            if let Some(project_table) = project_value.as_table_mut() {
                modified |= remove_switch_picker_timeout_in(project_table);
            }
        }
    }
    modified
}

fn migrate_content_from_doc(content: &str, mut doc: toml_edit::DocumentMut) -> String {
    if migrate_content_doc(&mut doc) {
        doc.to_string()
    } else {
        content.to_string()
    }
}

/// Apply all TOML-level migrations to config content.
///
/// Parses the TOML, applies all structural migrations, and returns the result.
/// Called by load paths that only need structural migration. `check_and_migrate()`
/// reuses the same migration path when it also needs to emit warnings.
pub fn migrate_content(content: &str) -> String {
    let Ok(doc) = content.parse::<toml_edit::DocumentMut>() else {
        return content.to_string();
    };
    migrate_content_from_doc(content, doc)
}

/// Copy approved-commands from config.toml to approvals.toml.
///
/// Called by `wt config update` before overwriting the config with migrated
/// content, so the approvals data survives the rewrite. No-op if
/// `approvals.toml` already exists (already authoritative) or the config has
/// no approved-commands entries.
///
/// Returns `Some(path)` if approvals.toml was created, `None` otherwise.
pub fn copy_approved_commands_to_approvals_file(config_path: &Path) -> Option<PathBuf> {
    let approvals_path = config_path.with_file_name("approvals.toml");
    if approvals_path.exists() {
        return None; // Already authoritative, don't overwrite
    }

    let approvals = super::approvals::Approvals::load_from_config_file(config_path).ok()?;
    approvals.projects().next()?; // Nothing to copy if empty

    approvals.save_to(&approvals_path).ok()?;
    Some(approvals_path)
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
/// Detection result plus display context (paths, labels). No filesystem side
/// effects — `check_and_migrate` never touches the filesystem; `wt config
/// update` rewrites the config and copies approvals under an explicit user
/// action.
#[derive(Debug)]
pub struct DeprecationInfo {
    /// Path to the config file with deprecations
    pub config_path: PathBuf,
    /// All detected deprecations
    pub deprecations: Deprecations,
    /// Label for this config (e.g., "User config", "Project config")
    pub label: String,
    /// Main worktree path when viewing from a linked worktree (for `-C` in hints)
    pub main_worktree_path: Option<PathBuf>,
}

impl DeprecationInfo {
    /// Returns true if any deprecations were found
    pub fn has_deprecations(&self) -> bool {
        !self.deprecations.is_empty()
    }
}

/// Result of checking config content for deprecations.
///
/// `migrated_content` is the structurally migrated TOML used for serde loading.
/// `info` is present only when user-visible deprecations were detected.
#[derive(Debug)]
pub struct CheckAndMigrateResult {
    pub info: Option<DeprecationInfo>,
    pub migrated_content: String,
}

/// Check config content for deprecated patterns.
///
/// Detects:
/// - Deprecated template variables (repo_root → repo_path, etc.)
/// - Deprecated [commit-generation] sections → [commit.generation]
/// - Deprecated args field (merged into command)
/// - Deprecated approved-commands in \[projects\] (moved to approvals.toml)
///
/// Pure with respect to the filesystem — never rewrites config or copies
/// approvals. The user materializes migrations by running `wt config update`
/// (or `wt config update --print`). Deprecation warnings still go to stderr
/// when `emit_inline_warnings` is set.
///
/// Set `warn_and_migrate` to false for project config on feature worktrees —
/// the warning is only actionable from the main worktree where the user would
/// run `wt config update`.
///
/// The `label` is used in the warning message (e.g., "User config" or "Project config").
///
/// `repo` is used to resolve the primary worktree path for the "run this from
/// the main worktree" hint when viewing project config from a linked worktree.
///
/// When `emit_inline_warnings` is true, per-kind deprecation warnings are printed to stderr
/// with a hint pointing at `wt config show`/`wt config update`. When false, nothing is
/// printed and the caller is expected to render via `format_deprecation_details`. Use this for commands other than `config show`.
///
/// Warnings are deduplicated per path per process.
///
/// Returns the structurally migrated content for serde loading, plus optional
/// deprecation info when user-visible deprecations were found.
pub fn check_and_migrate(
    path: &Path,
    content: &str,
    warn_and_migrate: bool,
    label: &str,
    repo: Option<&crate::git::Repository>,
    emit_inline_warnings: bool,
) -> anyhow::Result<CheckAndMigrateResult> {
    // Parse once — shared by detection and migration.
    // Contract: unparsable content collapses to empty deprecations so downstream
    // `compute_migrated_content` (invoked by `config show`/`config update` only when
    // `info` is `Some`) can assume the content parses.
    let (deprecations, migrated_content) = match content.parse::<toml_edit::DocumentMut>() {
        Ok(doc) => {
            let template_strings = extract_template_strings_from_doc(&doc);
            let deprecations = detect_deprecations_from_doc(&doc, &template_strings);
            let migrated_content = migrate_content_from_doc(content, doc);
            (deprecations, migrated_content)
        }
        Err(_) => (Deprecations::default(), content.to_string()),
    };

    if deprecations.post_create {
        return Err(anyhow::anyhow!("{label}: {POST_CREATE_REMOVED_MSG}"));
    }

    if deprecations.is_empty() {
        return Ok(CheckAndMigrateResult {
            info: None,
            migrated_content,
        });
    }

    let info = DeprecationInfo {
        config_path: path.to_path_buf(),
        deprecations,
        label: label.to_string(),
        main_worktree_path: if !warn_and_migrate {
            repo.and_then(|r| r.repo_path().ok())
                .map(|p| p.to_path_buf())
        } else {
            None
        },
    };

    // Skip warning entirely if not in main worktree (for project config)
    if !warn_and_migrate {
        return Ok(CheckAndMigrateResult {
            info: Some(info),
            migrated_content,
        });
    }

    // Deduplicate warnings per path per process
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    {
        let mut guard = WARNED_DEPRECATED_PATHS
            .lock()
            .map_err(|e| anyhow::anyhow!("failed to lock deprecation warning tracker: {e}"))?;
        if guard.contains(&canonical_path) {
            return Ok(CheckAndMigrateResult {
                info: Some(info),
                migrated_content,
            });
        }
        guard.insert(canonical_path);
    }

    // For non-config-show commands, emit per-kind warnings but skip the diff.
    // The diff is reserved for `wt config show`, where the user has opted into details.
    if emit_inline_warnings && !warnings_suppressed() {
        eprint!("{}", format_deprecation_warnings(&info));
        if DEPRECATION_HINT_EMITTED.set(()).is_ok() {
            eprintln!(
                "{}",
                hint_message(cformat!(
                    "To see details, run <underline>wt config show</>; to apply updates, run <underline>wt config update</>"
                ))
            );
        }
        std::io::stderr().flush().ok();
    }

    Ok(CheckAndMigrateResult {
        info: Some(info),
        migrated_content,
    })
}

/// Apply all deprecation fixes to `content` in memory and return the migrated
/// TOML string.
///
/// Applies variable renames (cosmetic, string-level), structural section and
/// field migrations, and removes `approved-commands` under `[projects]` (which
/// `wt config update` copies to `approvals.toml` before overwriting).
///
/// Pure function — no filesystem access. Idempotent: feeding its own output
/// back in is a no-op. Callers materialize the result via `wt config update`
/// or display it via `wt config show`.
pub fn compute_migrated_content(content: &str) -> String {
    // Parse once to extract template strings and detect what needs migrating.
    // Callers (`wt config show`, `wt config update`, `format_deprecation_details`)
    // all run content through `check_and_migrate` first, so it is known to parse.
    let doc = content
        .parse::<toml_edit::DocumentMut>()
        .expect("compute_migrated_content called with content that failed TOML parse; callers must funnel through check_and_migrate first");
    let template_strings = extract_template_strings_from_doc(&doc);
    let deprecations = detect_deprecations_from_doc(&doc, &template_strings);

    // Apply string-level var replacement first (cosmetic, operates on raw content)
    let after_vars = if !deprecations.vars.is_empty() {
        replace_deprecated_vars_from_strings(content, &template_strings)
    } else {
        content.to_string()
    };

    // Re-parse for structural migrations (which operate on toml_edit::DocumentMut).
    // `replace_deprecated_vars_from_strings` substitutes one identifier for another
    // inside `template_strings`, which are values extracted from string literals —
    // they cannot collide with TOML syntactic tokens, so the replacement preserves
    // validity.
    let mut doc = after_vars
        .parse::<toml_edit::DocumentMut>()
        .expect("template-var replacement preserves TOML structure");
    let mut modified = migrate_content_doc(&mut doc);
    // Additionally remove approved-commands (not part of migrate_content because
    // approved-commands is still a valid serde field at runtime).
    if deprecations.approved_commands {
        modified |= remove_approved_commands_doc(&mut doc);
    }
    if modified {
        doc.to_string()
    } else {
        after_vars
    }
}

/// Render a colored unified diff between `original` and `migrated`, with
/// `label` shown as the file name in the diff header (e.g. `config.toml`).
///
/// Uses a private tempdir containing two files named `<label>/current` and
/// `<label>/migrated`; `git diff --no-index` is invoked from inside that
/// tempdir so the diff header shows clean relative paths. The tempdir is
/// dropped on return. Returns `None` when the contents match.
pub fn format_migration_diff(original: &str, migrated: &str, label: &str) -> Option<String> {
    let dir = tempfile::tempdir().expect("failed to create tempdir for migration diff");
    let subdir = dir.path().join(label);
    std::fs::create_dir(&subdir).expect("failed to create subdir in fresh tempdir");
    let current = subdir.join("current");
    let migrated_path = subdir.join("migrated");
    std::fs::write(&current, original).expect("failed to write current config to tempfile");
    std::fs::write(&migrated_path, migrated).expect("failed to write migrated config to tempfile");

    let output = Cmd::new("git")
        .args(["diff", "--no-index", "--color=always", "-U3", "--"])
        .arg(format!("{label}/current"))
        .arg(format!("{label}/migrated"))
        .current_dir(dir.path())
        .run()
        .expect("git diff --no-index failed");

    // git diff --no-index exits 1 when files differ, which is expected.
    let diff_output = String::from_utf8_lossy(&output.stdout);
    if diff_output.is_empty() {
        return None;
    }
    Some(format_with_gutter(diff_output.trim_end(), None))
}

/// Format deprecation warning lines (without apply hints or diff).
///
/// Lists which deprecated patterns were found: template variables, config sections,
/// approved-commands. Used by both `format_deprecation_details` (which adds the
/// `wt config update` hint and diff) and `wt config update` (which applies directly).
pub fn format_deprecation_warnings(info: &DeprecationInfo) -> String {
    use std::fmt::Write;
    let mut out = String::new();

    for (old, new) in &info.deprecations.vars {
        let _ = writeln!(
            out,
            "{}",
            warning_message(cformat!(
                "{label}: template variable <bold>{old}</> is deprecated in favor of <bold>{new}</>",
                label = info.label,
            ))
        );
    }

    if info.deprecations.commit_gen.has_top_level {
        let _ = writeln!(
            out,
            "{}",
            warning_message(cformat!(
                "{}: <bold>[commit-generation]</> is deprecated in favor of <bold>[commit.generation]</>",
                info.label
            ))
        );
    }
    for project_key in &info.deprecations.commit_gen.project_keys {
        let _ = writeln!(
            out,
            "{}",
            warning_message(cformat!(
                "{label}: <bold>[projects.\"{k}\".commit-generation]</> is deprecated in favor of <bold>[projects.\"{k}\".commit.generation]</>",
                label = info.label,
                k = project_key
            ))
        );
    }

    if info.deprecations.approved_commands {
        let _ = writeln!(
            out,
            "{}",
            warning_message(cformat!(
                "{}: <bold>approved-commands</> under <bold>[projects]</> is deprecated in favor of <bold>approvals.toml</>",
                info.label
            ))
        );
    }

    if info.deprecations.select {
        let _ = writeln!(
            out,
            "{}",
            warning_message(cformat!(
                "{}: <bold>[select]</> is deprecated in favor of <bold>[switch.picker]</>",
                info.label
            ))
        );
    }

    if info.deprecations.ci_section {
        let _ = writeln!(
            out,
            "{}",
            warning_message(cformat!(
                "{}: <bold>[ci]</> is deprecated in favor of <bold>[forge]</>",
                info.label
            ))
        );
    }

    if info.deprecations.no_ff {
        let _ = writeln!(
            out,
            "{}",
            warning_message(cformat!(
                "{}: <bold>merge.no-ff</> is deprecated in favor of <bold>merge.ff</> (inverted)",
                info.label
            ))
        );
    }

    if info.deprecations.no_cd {
        let _ = writeln!(
            out,
            "{}",
            warning_message(cformat!(
                "{}: <bold>switch.no-cd</> is deprecated in favor of <bold>switch.cd</> (inverted)",
                info.label
            ))
        );
    }

    if info.deprecations.switch_picker_timeout_ms {
        let _ = writeln!(
            out,
            "{}",
            warning_message(cformat!(
                "{}: <bold>switch.picker.timeout-ms</> is no longer used — the picker now renders progressively",
                info.label
            ))
        );
    }

    if !info.deprecations.pre_hook_table_form.is_empty() {
        let hook_list = info
            .deprecations
            .pre_hook_table_form
            .iter()
            .map(|h| cformat!("<bold>{h}</>"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "{}",
            warning_message(cformat!(
                "{}: table form for {} is deprecated in favor of the pipeline form. \
                 We're unifying pre-hooks, post-hooks, and aliases so that list form always runs serially \
                 and table form always runs in parallel — migrate now to keep the current serial behavior \
                 once the table form is repurposed.",
                info.label,
                hook_list
            ))
        );
    }

    out
}

/// Format deprecation details for display (for use by `wt config show`).
///
/// Returns formatted output including:
/// - Warning message listing deprecated patterns
/// - Migration hint with apply command
/// - Inline diff showing the changes
///
/// `original_content` is the current on-disk config; the migrated content is
/// derived in memory via [`compute_migrated_content`] so this function has no
/// filesystem side effects other than the tempdir used briefly for `git diff`.
pub fn format_deprecation_details(info: &DeprecationInfo, original_content: &str) -> String {
    use std::fmt::Write;
    let mut out = format_deprecation_warnings(info);

    if let Some(main_path) = &info.main_worktree_path {
        // In a linked worktree — the user needs to run update from the primary.
        let cmd = suggest_command_in_dir(main_path, "config", &["update"], &[]);
        let _ = writeln!(
            out,
            "{}",
            hint_message(cformat!("To apply: <underline>{cmd}</>"))
        );
        return out;
    }

    let _ = writeln!(
        out,
        "{}",
        hint_message(cformat!("To apply: <underline>wt config update</>"))
    );

    let migrated = compute_migrated_content(original_content);
    let label = info
        .config_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "config".to_string());
    if let Some(diff) = format_migration_diff(original_content, &migrated, &label) {
        let _ = writeln!(out, "{}", info_message("Proposed diff:"));
        let _ = writeln!(out, "{diff}");
    }

    out
}

/// Returns the config location where this key belongs, if it's in the wrong config.
///
/// Generic over `C`, the config type where the key was found. If the key would
/// be valid in `C::Other`, returns that config's description.
///
/// For example, `key_belongs_in::<ProjectConfig>("skip-shell-integration-prompt")`
/// returns `Some("user config")`.
/// Returns `None` if the key is truly unknown (not valid in either config).
pub fn key_belongs_in<C: WorktrunkConfig>(key: &str) -> Option<&'static str> {
    C::Other::is_valid_key(key).then(C::Other::description)
}

/// Classification of an unknown config key for warning purposes.
pub enum UnknownKeyKind {
    /// Deprecated key in its correct config type — deprecation system handles it
    DeprecatedHandled,
    /// Deprecated key in the wrong config type
    DeprecatedWrongConfig {
        other_description: &'static str,
        canonical_display: &'static str,
    },
    /// Non-deprecated key that belongs in the other config type
    WrongConfig { other_description: &'static str },
    /// Truly unknown key (not valid in either config type)
    Unknown,
}

/// Classify an unknown config key: deprecated (right/wrong file), misplaced, or unknown.
pub fn classify_unknown_key<C: WorktrunkConfig>(key: &str) -> UnknownKeyKind {
    if let Some(dep) = DEPRECATED_SECTION_KEYS.iter().find(|d| d.key == key) {
        return if C::is_valid_key(dep.canonical_top_key) {
            UnknownKeyKind::DeprecatedHandled
        } else {
            UnknownKeyKind::DeprecatedWrongConfig {
                other_description: C::Other::description(),
                canonical_display: dep.canonical_display,
            }
        };
    }
    match key_belongs_in::<C>(key) {
        Some(other) => UnknownKeyKind::WrongConfig {
            other_description: other,
        },
        None => UnknownKeyKind::Unknown,
    }
}

/// Warn about unknown fields in a config file.
///
/// Generic over `C`, the config type being loaded. Classification is shared
/// with `config show` via [`collect_unknown_warnings`](crate::config::collect_unknown_warnings);
/// this wrapper adds per-path deduplication and stderr emission.
///
/// The `label` is used in the warning message (e.g., "User config" or
/// "Project config").
pub fn warn_unknown_fields<C: WorktrunkConfig>(raw_contents: &str, path: &Path, label: &str) {
    if warnings_suppressed() {
        return;
    }

    let warnings = crate::config::collect_unknown_warnings::<C>(raw_contents);
    if warnings.is_empty() {
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

    for warning in warnings {
        eprintln!("{}", warning_message(format_load_warning(label, &warning)));
    }

    // Flush stderr to ensure output appears before any subsequent messages
    std::io::stderr().flush().ok();
}

fn format_load_warning(label: &str, warning: &crate::config::UnknownWarning) -> String {
    use crate::config::UnknownWarning;
    match warning {
        UnknownWarning::TopLevelUnknown { key } => {
            cformat!("{label} has unknown field <bold>{key}</> (will be ignored)")
        }
        UnknownWarning::TopLevelWrongConfig {
            key,
            other_description,
        } => cformat!(
            "{label} has key <bold>{key}</> which belongs in {other_description} (will be ignored)"
        ),
        UnknownWarning::TopLevelDeprecatedWrongConfig {
            key,
            other_description,
            canonical_display,
        } => cformat!(
            "{label} has key <bold>{key}</> which belongs in {other_description} as {canonical_display}"
        ),
        UnknownWarning::NestedUnknown { path } => {
            cformat!("{label} has unknown field <bold>{path}</> (will be ignored)")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test helpers that parse from string and delegate to the internal functions.
    // These mirror the former pub wrappers that were inlined into
    // `detect_deprecations` and `compute_migrated_content`.

    fn extract_template_strings(content: &str) -> Vec<String> {
        let Ok(doc) = content.parse::<toml_edit::DocumentMut>() else {
            return vec![];
        };
        extract_template_strings_from_doc(&doc)
    }

    fn replace_deprecated_vars(content: &str) -> String {
        let strings = extract_template_strings(content);
        replace_deprecated_vars_from_strings(content, &strings)
    }

    fn find_deprecated_vars(content: &str) -> Vec<(&'static str, &'static str)> {
        let strings = extract_template_strings(content);
        find_deprecated_vars_from_strings(&strings)
    }

    fn find_commit_generation_deprecations(content: &str) -> CommitGenerationDeprecations {
        content
            .parse::<toml_edit::DocumentMut>()
            .map(|doc| find_commit_generation_from_doc(&doc))
            .unwrap_or_default()
    }

    fn find_approved_commands_deprecation(content: &str) -> bool {
        content
            .parse::<toml_edit::DocumentMut>()
            .ok()
            .is_some_and(|doc| find_approved_commands_from_doc(&doc))
    }

    fn find_select_deprecation(content: &str) -> bool {
        content
            .parse::<toml_edit::DocumentMut>()
            .ok()
            .is_some_and(|doc| find_select_from_doc(&doc))
    }

    fn find_post_create_deprecation(content: &str) -> bool {
        content
            .parse::<toml_edit::DocumentMut>()
            .ok()
            .is_some_and(|doc| find_post_create_from_doc(&doc))
    }

    fn migrate_commit_generation_sections(content: &str) -> String {
        let Ok(mut doc) = content.parse::<toml_edit::DocumentMut>() else {
            return content.to_string();
        };
        if migrate_commit_generation_doc(&mut doc) {
            doc.to_string()
        } else {
            content.to_string()
        }
    }

    fn remove_approved_commands_from_config(content: &str) -> String {
        let Ok(mut doc) = content.parse::<toml_edit::DocumentMut>() else {
            return content.to_string();
        };
        if remove_approved_commands_doc(&mut doc) {
            doc.to_string()
        } else {
            content.to_string()
        }
    }

    fn migrate_select_to_switch_picker(content: &str) -> String {
        let Ok(mut doc) = content.parse::<toml_edit::DocumentMut>() else {
            return content.to_string();
        };
        if migrate_select_doc(&mut doc) {
            doc.to_string()
        } else {
            content.to_string()
        }
    }

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
    fn test_find_deprecated_vars_in_array_of_tables() {
        // Exercises the ArrayOfTables arm in collect_strings_from_edit_item
        let content = r#"
[[hooks]]
command = "ln -sf {{ repo_root }}/node_modules"
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(found, vec![("repo_root", "repo_path")]);
    }

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
        let content = "[merge]\nno-ff = true\n";
        let non_existent_path = std::path::Path::new("/nonexistent/dir/config.toml");

        // Should return Ok(Some(_)) even if write fails - the function logs error but doesn't fail
        let result =
            check_and_migrate(non_existent_path, content, true, "Test config", None, false);
        assert!(result.is_ok());
        assert!(result.unwrap().info.is_some());
    }

    #[test]
    fn test_check_and_migrate_deduplicates_warnings() {
        // Test that calling twice with same path skips the second warning
        let content = "[merge]\nno-ff = true\n";
        // Use a unique path that won't collide with other tests
        let unique_path = std::path::Path::new("/nonexistent/dedup_test_12345/config.toml");

        // First call should process normally
        let result1 = check_and_migrate(unique_path, content, true, "Test config", None, false);
        assert!(result1.is_ok());
        assert!(result1.unwrap().info.is_some());

        // Second call with same path should early-return (hits the deduplication branch)
        let result2 = check_and_migrate(unique_path, content, true, "Test config", None, false);
        assert!(result2.is_ok());
        assert!(result2.unwrap().info.is_some());
    }

    #[test]
    fn test_check_and_migrate_returns_migrated_content() {
        let content = r#"
[select]
pager = "delta"
"#;

        let result = check_and_migrate(
            std::path::Path::new("/tmp/config.toml"),
            content,
            true,
            "Test config",
            None,
            false,
        )
        .unwrap();

        assert_eq!(result.migrated_content, migrate_content(content));
        assert!(result.info.is_some());
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
    fn test_migrate_commit_generation_args_with_spaces() {
        let content = r#"
[commit-generation]
command = "llm"
args = ["-m", "claude haiku 4.5"]
"#;
        let result = migrate_commit_generation_sections(content);
        insta::assert_snapshot!(result, @r#"

        [commit.generation]
        command = "llm -m 'claude haiku 4.5'"
        "#);
    }

    #[test]
    fn test_migrate_commit_generation_preserves_other_fields() {
        let content = r#"
[commit-generation]
command = "llm -m haiku"
template = "Write commit: {{ diff }}"
"#;
        let result = migrate_commit_generation_sections(content);
        insta::assert_snapshot!(result, @r#"

        [commit.generation]
        command = "llm -m haiku"
        template = "Write commit: {{ diff }}"
        "#);
    }

    #[test]
    fn test_migrate_no_changes_needed() {
        let content = r#"
[commit.generation]
command = "llm -m haiku"
"#;
        let result = migrate_commit_generation_sections(content);
        assert_eq!(result, content);
    }

    #[test]
    fn test_migrate_skips_when_new_section_exists() {
        let content = r#"
[commit.generation]
command = "new-command"

[commit-generation]
command = "old-command"
"#;
        let result = migrate_commit_generation_sections(content);
        // Old section left as-is since new already exists
        insta::assert_snapshot!(result, @r#"

        [commit.generation]
        command = "new-command"

        [commit-generation]
        command = "old-command"
        "#);
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
        assert_eq!(shell_join(&["echo", "it's"]), r"echo 'it'\''s'");
    }

    #[test]
    fn test_combined_migrations_template_vars_and_section_rename() {
        let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch }}"

[commit-generation]
command = "llm"
args = ["-m", "haiku"]
"#;
        let step1 = replace_deprecated_vars(content);
        let step2 = migrate_commit_generation_sections(&step1);
        insta::assert_snapshot!(step2, @r#"

        worktree-path = "../{{ repo }}.{{ branch }}"

        [commit.generation]
        command = "llm -m haiku"
        "#);
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
        // Args preserved when no command to merge into
        let content = r#"
[commit-generation]
args = ["-m", "haiku"]
template = "some template"
"#;
        let result = migrate_commit_generation_sections(content);
        insta::assert_snapshot!(result, @r#"

        [commit.generation]
        args = ["-m", "haiku"]
        template = "some template"
        "#);
    }

    #[test]
    fn test_migrate_args_with_non_string_command() {
        // Args preserved when command is not a string
        let content = r#"
[commit-generation]
command = 123
args = ["-m", "haiku"]
"#;
        let result = migrate_commit_generation_sections(content);
        insta::assert_snapshot!(result, @r#"

        [commit.generation]
        command = 123
        args = ["-m", "haiku"]
        "#);
    }

    #[test]
    fn test_migrate_empty_command_with_args() {
        let content = r#"
[commit-generation]
command = ""
args = ["-m", "haiku"]
"#;
        let result = migrate_commit_generation_sections(content);
        insta::assert_snapshot!(result, @r#"

        [commit.generation]
        command = "-m haiku"
        "#);
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
    fn test_remove_approved_commands_multiple_projects() {
        let content = r#"
[projects."github.com/user/repo1"]
approved-commands = ["npm install"]

[projects."github.com/user/repo2"]
approved-commands = ["cargo test"]
worktree-path = ".worktrees/{{ branch | sanitize }}"
"#;
        let result = remove_approved_commands_from_config(content);
        insta::assert_snapshot!(result, @r#"

        [projects."github.com/user/repo2"]
        worktree-path = ".worktrees/{{ branch | sanitize }}"
        "#);
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
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = ["npm install"]
"#;
        let info = DeprecationInfo {
            config_path: std::path::PathBuf::from("/tmp/test-config.toml"),
            deprecations: Deprecations {
                vars: vec![],
                commit_gen: CommitGenerationDeprecations::default(),
                approved_commands: true,
                select: false,
                post_create: false,
                ci_section: false,
                no_ff: false,
                no_cd: false,
                pre_hook_table_form: vec![],
                switch_picker_timeout_ms: false,
            },
            label: "User config".to_string(),
            main_worktree_path: None,
        };
        let output = format_deprecation_details(&info, content);
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
    fn test_compute_migrated_content_removes_approved_commands() {
        let content = r#"worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

[projects."github.com/user/repo"]
approved-commands = ["npm install"]
"#;
        let migrated = compute_migrated_content(content);
        assert!(!migrated.contains("approved-commands"));
    }

    #[test]
    fn test_copy_approved_commands_creates_approvals_file() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = ["npm install", "npm test"]

[projects."github.com/other/repo"]
approved-commands = ["cargo build"]
"#;
        std::fs::write(&config_path, content).unwrap();

        let result = copy_approved_commands_to_approvals_file(&config_path);
        assert!(result.is_some(), "Should create approvals.toml");

        let approvals_path = result.unwrap();
        assert_eq!(approvals_path, temp_dir.path().join("approvals.toml"));

        let approvals_content = std::fs::read_to_string(&approvals_path).unwrap();
        assert!(
            approvals_content.contains("npm install"),
            "Should contain npm install: {}",
            approvals_content
        );
        assert!(
            approvals_content.contains("npm test"),
            "Should contain npm test: {}",
            approvals_content
        );
        assert!(
            approvals_content.contains("cargo build"),
            "Should contain cargo build: {}",
            approvals_content
        );
    }

    #[test]
    fn test_copy_approved_commands_skips_when_approvals_exists() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        let approvals_path = temp_dir.path().join("approvals.toml");
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = ["npm install"]
"#;
        std::fs::write(&config_path, content).unwrap();
        std::fs::write(&approvals_path, "# existing approvals\n").unwrap();

        let result = copy_approved_commands_to_approvals_file(&config_path);
        assert!(result.is_none(), "Should skip when approvals.toml exists");

        // Verify existing file was not overwritten
        let existing = std::fs::read_to_string(&approvals_path).unwrap();
        assert_eq!(existing, "# existing approvals\n");
    }

    #[test]
    fn test_copy_approved_commands_skips_when_empty() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        let content = r#"
[projects."github.com/user/repo"]
worktree-path = ".worktrees/{{ branch | sanitize }}"
"#;
        std::fs::write(&config_path, content).unwrap();

        let result = copy_approved_commands_to_approvals_file(&config_path);
        assert!(
            result.is_none(),
            "Should skip when no approved-commands exist"
        );
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

    // Tests for [select] → [switch.picker] deprecation

    #[test]
    fn test_find_select_deprecation_none() {
        let content = r#"
[switch.picker]
pager = "delta --paging=never"
"#;
        assert!(!find_select_deprecation(content));
    }

    #[test]
    fn test_find_select_deprecation_present() {
        let content = r#"
[select]
pager = "delta --paging=never"
"#;
        assert!(find_select_deprecation(content));
    }

    #[test]
    fn test_find_select_deprecation_empty_not_flagged() {
        let content = r#"
[select]
"#;
        assert!(!find_select_deprecation(content));
    }

    #[test]
    fn test_find_select_deprecation_skips_when_new_exists() {
        // When both [select] and [switch.picker] exist, don't flag
        let content = r#"
[select]
pager = "old"

[switch.picker]
pager = "new"
"#;
        assert!(!find_select_deprecation(content));
    }

    #[test]
    fn test_find_select_deprecation_inline_table() {
        let content = r#"
select = { pager = "delta" }
"#;
        assert!(find_select_deprecation(content));
    }

    #[test]
    fn test_find_select_deprecation_empty_inline_table() {
        let content = r#"
select = {}
"#;
        assert!(!find_select_deprecation(content));
    }

    #[test]
    fn test_migrate_select_simple() {
        let content = r#"
[select]
pager = "delta --paging=never"
"#;
        let result = migrate_select_to_switch_picker(content);
        assert!(
            result.contains("[switch.picker]"),
            "Should have [switch.picker]: {result}"
        );
        assert!(
            result.contains("pager = \"delta --paging=never\""),
            "Should preserve pager: {result}"
        );
        assert!(
            !result.contains("[select]"),
            "Should remove [select]: {result}"
        );
    }

    #[test]
    fn test_migrate_select_skips_when_new_exists() {
        let content = r#"
[select]
pager = "old"

[switch.picker]
pager = "new"
"#;
        let result = migrate_select_to_switch_picker(content);
        assert_eq!(
            result, content,
            "Should not migrate when new section exists"
        );
    }

    #[test]
    fn test_migrate_select_invalid_toml() {
        let content = "this is { not valid toml";
        let result = migrate_select_to_switch_picker(content);
        assert_eq!(result, content, "Invalid TOML should be returned unchanged");
    }

    #[test]
    fn test_migrate_select_no_select_section() {
        let content = r#"
[list]
full = true
"#;
        let result = migrate_select_to_switch_picker(content);
        assert_eq!(result, content, "No [select] section means no migration");
    }

    #[test]
    fn test_detect_deprecations_includes_select() {
        let content = r#"
[select]
pager = "delta"
"#;
        let deprecations = detect_deprecations(content);
        assert!(deprecations.select);
        assert!(!deprecations.is_empty());
    }

    #[test]
    fn snapshot_migrate_select_to_switch_picker() {
        let content = r#"worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

[select]
pager = "delta --paging=never"

[list]
branches = true
"#;
        let result = migrate_select_to_switch_picker(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    #[test]
    fn test_format_deprecation_details_select() {
        let content = r#"[select]
pager = "delta --paging=never"
"#;
        let info = DeprecationInfo {
            config_path: std::path::PathBuf::from("/tmp/test-config.toml"),
            deprecations: Deprecations {
                vars: vec![],
                commit_gen: CommitGenerationDeprecations::default(),
                approved_commands: false,
                select: true,
                post_create: false,
                ci_section: false,
                no_ff: false,
                no_cd: false,
                pre_hook_table_form: vec![],
                switch_picker_timeout_ms: false,
            },
            label: "User config".to_string(),
            main_worktree_path: None,
        };
        let output = format_deprecation_details(&info, content);
        assert!(
            output.contains("[select]"),
            "Should mention [select] in output: {output}"
        );
        assert!(
            output.contains("[switch.picker]"),
            "Should mention [switch.picker]: {output}"
        );
    }

    #[test]
    fn test_compute_migrated_content_renames_select() {
        let content = r#"worktree-path = "../{{ repo }}.{{ branch | sanitize }}"

[select]
pager = "delta --paging=never"
"#;
        let migrated = compute_migrated_content(content);
        assert!(
            migrated.contains("[switch.picker]"),
            "Migrated content should have [switch.picker]: {migrated}"
        );
        assert!(
            !migrated.contains("[select]"),
            "Migrated content should not have [select]: {migrated}"
        );
    }

    // --- post-create → pre-start deprecation tests ---

    #[test]
    fn test_find_post_create_deprecation_none() {
        // No post-create, no deprecation
        let content = r#"
pre-start = "npm install"
"#;
        assert!(!find_post_create_deprecation(content));
    }

    #[test]
    fn test_find_post_create_deprecation_top_level() {
        // Project config format: bare key
        let content = r#"
post-create = "npm install"
"#;
        assert!(find_post_create_deprecation(content));
    }

    #[test]
    fn test_find_post_create_deprecation_project_level() {
        // User config format: hooks flattened into [projects."..."]
        let content = r#"
[projects."my-project"]
post-create = "npm install"
"#;
        assert!(find_post_create_deprecation(content));
    }

    #[test]
    fn test_find_post_create_deprecation_named_commands() {
        // Named command table format
        let content = r#"
[post-create]
lint = "npm run lint"
build = "npm run build"
"#;
        assert!(find_post_create_deprecation(content));
    }

    #[test]
    fn test_find_post_create_deprecation_empty_table_not_flagged() {
        // Empty [post-create] table is a no-op — don't warn
        let content = r#"
[post-create]
"#;
        assert!(!find_post_create_deprecation(content));
    }

    #[test]
    fn test_find_post_create_deprecation_skips_when_pre_start_exists_top_level() {
        // Both present at top level — don't flag
        let content = r#"
post-create = "old"
pre-start = "new"
"#;
        assert!(!find_post_create_deprecation(content));
    }

    #[test]
    fn test_find_post_create_deprecation_skips_when_pre_start_exists_project() {
        // Both present in project hooks — don't flag
        let content = r#"
[projects."my-project"]
post-create = "old"
pre-start = "new"
"#;
        assert!(!find_post_create_deprecation(content));
    }

    fn migrate_switch_picker_timeout(content: &str) -> String {
        let Ok(mut doc) = content.parse::<toml_edit::DocumentMut>() else {
            return content.to_string();
        };
        if migrate_switch_picker_timeout_doc(&mut doc) {
            doc.to_string()
        } else {
            content.to_string()
        }
    }

    #[test]
    fn test_detect_switch_picker_timeout_top_level() {
        let content = r#"
[switch.picker]
pager = "delta"
timeout-ms = 500
"#;
        let deprecations = detect_deprecations(content);
        assert!(deprecations.switch_picker_timeout_ms);
        assert!(!deprecations.is_empty());
    }

    #[test]
    fn test_detect_switch_picker_timeout_project_level() {
        let content = r#"
[projects."github.com/user/repo".switch.picker]
timeout-ms = 300
"#;
        let deprecations = detect_deprecations(content);
        assert!(deprecations.switch_picker_timeout_ms);
    }

    #[test]
    fn test_detect_switch_picker_timeout_inline_table() {
        let content = r#"
[switch]
picker = { pager = "delta", timeout-ms = 500 }
"#;
        let deprecations = detect_deprecations(content);
        assert!(deprecations.switch_picker_timeout_ms);
    }

    #[test]
    fn test_migrate_switch_picker_timeout_inline_table() {
        let content = r#"
[switch]
picker = { pager = "delta", timeout-ms = 500 }
"#;
        let result = migrate_switch_picker_timeout(content);
        assert!(!result.contains("timeout-ms"));
        assert!(result.contains("pager"));
    }

    #[test]
    fn test_detect_switch_picker_timeout_absent() {
        let content = r#"
[switch.picker]
pager = "delta"
"#;
        let deprecations = detect_deprecations(content);
        assert!(!deprecations.switch_picker_timeout_ms);
    }

    #[test]
    fn test_migrate_switch_picker_timeout_removes_key() {
        let content = r#"
[switch.picker]
pager = "delta"
timeout-ms = 500
"#;
        let result = migrate_switch_picker_timeout(content);
        assert!(
            !result.contains("timeout-ms"),
            "Should strip timeout-ms: {result}"
        );
        assert!(
            result.contains("pager"),
            "Should preserve sibling keys: {result}"
        );
    }

    #[test]
    fn test_migrate_switch_picker_timeout_project_level() {
        let content = r#"
[projects."github.com/user/repo".switch.picker]
pager = "bat"
timeout-ms = 100
"#;
        let result = migrate_switch_picker_timeout(content);
        assert!(!result.contains("timeout-ms"));
        assert!(result.contains("pager"));
    }

    #[test]
    fn test_migrate_switch_picker_timeout_noop_when_absent() {
        let content = r#"
[switch.picker]
pager = "delta"
"#;
        let result = migrate_switch_picker_timeout(content);
        assert_eq!(result, content);
    }

    #[test]
    fn test_migrate_switch_picker_timeout_invalid_toml() {
        let content = "this is { not valid toml";
        let result = migrate_switch_picker_timeout(content);
        assert_eq!(result, content);
    }

    #[test]
    fn test_format_deprecation_warnings_switch_picker_timeout() {
        let info = DeprecationInfo {
            config_path: std::path::PathBuf::from("/tmp/test-config.toml"),
            deprecations: Deprecations {
                switch_picker_timeout_ms: true,
                ..Deprecations::default()
            },
            label: "User config".to_string(),
            main_worktree_path: None,
        };
        let output = format_deprecation_warnings(&info);
        assert!(
            output.contains("switch.picker.timeout-ms"),
            "Should mention the field: {output}"
        );
        assert!(
            output.contains("no longer used"),
            "Should explain deprecation reason: {output}"
        );
    }

    #[test]
    fn test_detect_deprecations_includes_post_create() {
        let content = r#"
post-create = "npm install"
"#;
        let deprecations = detect_deprecations(content);
        assert!(deprecations.post_create);
        assert!(!deprecations.is_empty());
    }

    // ==================== negated bool format + migration tests ====================

    #[test]
    fn test_format_deprecation_warnings_no_ff_and_no_cd() {
        let info = DeprecationInfo {
            config_path: std::path::PathBuf::from("/tmp/test-config.toml"),
            deprecations: Deprecations {
                vars: vec![],
                commit_gen: CommitGenerationDeprecations::default(),
                approved_commands: false,
                select: false,
                post_create: false,
                ci_section: false,
                no_ff: true,
                no_cd: true,
                pre_hook_table_form: vec![],
                switch_picker_timeout_ms: false,
            },
            label: "User config".to_string(),
            main_worktree_path: None,
        };
        let output = format_deprecation_warnings(&info);
        assert!(output.contains("no-ff"), "Should mention no-ff: {output}");
        assert!(output.contains("no-cd"), "Should mention no-cd: {output}");
    }

    #[test]
    fn test_detect_no_ff_deprecation() {
        let deprecations = detect_deprecations("[merge]\nno-ff = true\n");
        assert!(deprecations.no_ff);
    }

    #[test]
    fn test_detect_no_ff_not_flagged_when_ff_exists() {
        let deprecations = detect_deprecations("[merge]\nff = true\nno-ff = true\n");
        assert!(!deprecations.no_ff);
    }

    #[test]
    fn test_detect_no_cd_deprecation() {
        let deprecations = detect_deprecations("[switch]\nno-cd = true\n");
        assert!(deprecations.no_cd);
    }

    #[test]
    fn test_detect_no_ff_project_level() {
        let content = r#"
[projects."github.com/user/repo".merge]
no-ff = true
"#;
        let deprecations = detect_deprecations(content);
        assert!(deprecations.no_ff);
    }

    #[test]
    fn test_migrate_no_ff_to_ff() {
        let content = "[merge]\nno-ff = true\n";
        let result = migrate_content(content);
        assert!(result.contains("ff = false"), "Should invert: {result}");
        assert!(!result.contains("no-ff"), "Should remove no-ff: {result}");
    }

    #[test]
    fn test_migrate_no_cd_to_cd() {
        let content = "[switch]\nno-cd = false\n";
        let result = migrate_content(content);
        assert!(result.contains("cd = true"), "Should invert: {result}");
        assert!(!result.contains("no-cd"), "Should remove no-cd: {result}");
    }

    #[test]
    fn test_migrate_no_ff_project_level() {
        let content = r#"
[projects."github.com/user/repo".merge]
no-ff = true
"#;
        let result = migrate_content(content);
        assert!(result.contains("ff = false"), "Should migrate: {result}");
        assert!(!result.contains("no-ff"), "Should remove no-ff: {result}");
    }

    #[test]
    fn test_migrate_negated_bool_non_boolean_value_preserved() {
        // Non-boolean `no-ff` value should be left alone
        let content = "[merge]\nno-ff = \"not-a-bool\"\n";
        let result = migrate_content(content);
        assert!(
            result.contains("no-ff"),
            "Non-boolean value should be preserved: {result}"
        );
    }

    #[test]
    fn test_migrate_no_ff_skips_when_ff_exists() {
        let content = "[merge]\nff = true\nno-ff = true\n";
        let result = migrate_content(content);
        assert!(result.contains("ff = true"), "ff should be kept: {result}");
        assert!(
            !result.contains("no-ff"),
            "no-ff should be removed: {result}"
        );
    }

    // ==================== project-level select migration tests ====================

    #[test]
    fn test_detect_select_project_level() {
        let content = r#"
[projects."github.com/user/repo".select]
pager = "bat"
"#;
        let deprecations = detect_deprecations(content);
        assert!(deprecations.select);
    }

    #[test]
    fn test_migrate_select_project_level() {
        let content = r#"
[projects."github.com/user/repo".select]
pager = "bat"
"#;
        let result = migrate_content(content);
        assert!(
            result.contains("[projects.\"github.com/user/repo\".switch.picker]"),
            "Should migrate project select: {result}"
        );
        assert!(
            !result.contains("[projects.\"github.com/user/repo\".select]"),
            "Should remove project select: {result}"
        );
    }

    // ==================== migrate_content tests ====================

    #[test]
    fn test_migrate_content_applies_all_structural_migrations() {
        let content = r#"
[commit-generation]
command = "llm"

[select]
pager = "delta"

[merge]
no-ff = true

[switch]
no-cd = true
"#;
        let result = migrate_content(content);
        assert!(
            result.contains("[commit.generation]"),
            "commit-generation: {result}"
        );
        assert!(
            result.contains("[switch.picker]"),
            "select to switch.picker: {result}"
        );
        assert!(result.contains("ff = false"), "no-ff to ff: {result}");
        assert!(result.contains("cd = false"), "no-cd to cd: {result}");
    }

    #[test]
    fn test_migrate_content_is_no_op_for_canonical_config() {
        let content = r#"
[commit.generation]
command = "llm"

[merge]
ff = true
"#;
        let result = migrate_content(content);
        assert_eq!(result, content);
    }

    #[test]
    fn test_warn_unknown_fields_deprecated_key_in_wrong_config() {
        use crate::config::{ProjectConfig, UserConfig};

        // commit-generation in project config → should warn "belongs in user config"
        let path = std::env::temp_dir().join("test-deprecated-wrong-config-project.toml");
        warn_unknown_fields::<ProjectConfig>(
            "[commit-generation]\ncommand = \"llm\"\n",
            &path,
            "Project config",
        );

        // ci in user config → should warn "belongs in project config"
        let path = std::env::temp_dir().join("test-deprecated-wrong-config-user.toml");
        warn_unknown_fields::<UserConfig>("[ci]\nplatform = \"github\"\n", &path, "User config");
    }

    // ==================== pre-hook table form tests ====================

    fn find_pre_hook_table_form(content: &str) -> Vec<String> {
        content
            .parse::<toml_edit::DocumentMut>()
            .map(|doc| find_pre_hook_table_form_from_doc(&doc))
            .unwrap_or_default()
    }

    fn migrate_pre_hook_table_form(content: &str) -> String {
        let Ok(mut doc) = content.parse::<toml_edit::DocumentMut>() else {
            return content.to_string();
        };
        if migrate_pre_hook_table_form_doc(&mut doc) {
            doc.to_string()
        } else {
            content.to_string()
        }
    }

    #[test]
    fn test_detect_pre_hook_table_form() {
        // Multi-entry table → detected
        let found = find_pre_hook_table_form("[pre-merge]\ntest = \"t\"\nlint = \"l\"\n");
        assert_eq!(found, vec!["pre-merge"]);

        // Single-entry table → not detected
        let found = find_pre_hook_table_form("[pre-merge]\ntest = \"t\"\n");
        assert!(found.is_empty());

        // String form → not detected
        let found = find_pre_hook_table_form("pre-merge = \"cargo test\"\n");
        assert!(found.is_empty());

        // Array/pipeline form → not detected
        let found = find_pre_hook_table_form("pre-merge = [{test = \"t\"}, {lint = \"l\"}]\n");
        assert!(found.is_empty());

        // Post-* hooks → not detected (table form is canonical for post-*)
        let found = find_pre_hook_table_form("[post-merge]\ntest = \"t\"\nlint = \"l\"\n");
        assert!(found.is_empty());

        // All 5 pre-* keys detected
        let content = r#"
[pre-switch]
a = "1"
b = "2"

[pre-start]
a = "1"
b = "2"

[pre-commit]
a = "1"
b = "2"

[pre-merge]
a = "1"
b = "2"

[pre-remove]
a = "1"
b = "2"
"#;
        let found = find_pre_hook_table_form(content);
        assert_eq!(
            found,
            vec![
                "pre-switch",
                "pre-start",
                "pre-commit",
                "pre-merge",
                "pre-remove"
            ]
        );
    }

    #[test]
    fn test_detect_pre_hook_table_form_per_project() {
        // Per-project overrides: hooks are flattened under [projects."id"]
        let content = r#"
[projects."github.com/user/repo".pre-start]
install = "npm ci"
build = "npm run build"
"#;
        let found = find_pre_hook_table_form(content);
        assert_eq!(found, vec!["projects.\"github.com/user/repo\".pre-start"]);
    }

    #[test]
    fn test_migrate_pre_hook_table_form_converts_to_pipeline() {
        let content = r#"
[pre-merge]
test = "cargo test"
lint = "cargo clippy"
"#;
        let result = migrate_pre_hook_table_form(content);
        // Should produce `[[pre-merge]]` array-of-tables blocks
        assert!(
            result.contains("[[pre-merge]]"),
            "Should emit [[pre-merge]] blocks: {result}"
        );
        // Verify it parses back as valid TOML with the right structure
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        let arr = doc["pre-merge"]
            .as_array_of_tables()
            .expect("should be array of tables");
        assert_eq!(arr.len(), 2);
        let first = arr.get(0).unwrap();
        assert_eq!(first.get("test").unwrap().as_str().unwrap(), "cargo test");
        let second = arr.get(1).unwrap();
        assert_eq!(
            second.get("lint").unwrap().as_str().unwrap(),
            "cargo clippy"
        );
    }

    #[test]
    fn test_migrate_pre_hook_table_form_preserves_order() {
        let content = r#"
[pre-merge]
first = "1"
second = "2"
third = "3"
"#;
        let result = migrate_pre_hook_table_form(content);
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        let arr = doc["pre-merge"].as_array_of_tables().unwrap();
        let names: Vec<&str> = arr.iter().map(|t| t.iter().next().unwrap().0).collect();
        assert_eq!(names, vec!["first", "second", "third"]);
    }

    #[test]
    fn test_migrate_pre_hook_table_form_single_entry_untouched() {
        let content = "[pre-merge]\ntest = \"t\"\n";
        let result = migrate_pre_hook_table_form(content);
        assert_eq!(result, content, "Single-entry table should not be migrated");
    }

    #[test]
    fn test_migrate_pre_hook_table_form_per_project() {
        let content = r#"
[projects."web".pre-start]
install = "npm ci"
build = "npm run build"
"#;
        let result = migrate_pre_hook_table_form(content);
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        let project = doc["projects"]["web"].as_table().unwrap();
        let arr = project["pre-start"]
            .as_array_of_tables()
            .expect("should be array of tables");
        assert_eq!(arr.len(), 2);
    }

    #[test]
    fn test_migrate_content_includes_pre_hook_table_form() {
        let content = r#"
[pre-merge]
test = "cargo test"
lint = "cargo clippy"

[merge]
no-ff = true
"#;
        let result = migrate_content(content);
        assert!(
            result.contains("[[pre-merge]]"),
            "Table section should become [[pre-merge]] blocks: {result}"
        );
        assert!(
            result.contains("ff = false"),
            "no-ff should also migrate: {result}"
        );
    }

    #[test]
    fn snapshot_migrate_pre_hook_table_form() {
        let content = r#"[pre-merge]
test = "cargo test"
lint = "cargo clippy"

[post-start]
server = "npm run dev"
"#;
        let result = migrate_pre_hook_table_form(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }
}
