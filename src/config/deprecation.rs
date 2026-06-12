//! Deprecation detection and migration.
//!
//! Scans config files for deprecated patterns and surfaces them to the user:
//! - Deprecated template variables (repo_root → repo_path, etc.)
//! - Deprecated config sections (\[commit-generation\] → \[commit.generation\])
//! - Deprecated fields (args merged into command)
//! - Deprecated approved-commands in \[projects\] (moved to approvals.toml)
//!
//! Each deprecated pattern is one row in [`DEPRECATION_RULES`]: a detection
//! function plus an idempotent migration function. The table order is both
//! the warning-emission order and the migration order.
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

use anyhow::Context;
use color_print::cformat;
use minijinja::Environment;
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

/// Top-level keys that are deprecated and handled by the deprecation system —
/// renamed sections (`[commit-generation]` → `[commit.generation]`).
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

    let env = Environment::new();
    let Ok(parsed) = env.template_from_str(template) else {
        return Cow::Borrowed(template);
    };
    let used_vars = parsed.undeclared_variables(false);
    let replacements: Vec<_> = DEPRECATED_VARS
        .iter()
        .copied()
        .filter(|(old, _)| used_vars.contains(*old))
        .collect();
    if replacements.is_empty() {
        return Cow::Borrowed(template);
    }

    rewrite_template_var_identifiers(template, &replacements)
        .map(Cow::Owned)
        .unwrap_or(Cow::Borrowed(template))
}

fn rewrite_template_var_identifiers(
    template: &str,
    replacements: &[(&str, &'static str)],
) -> Option<String> {
    let mut out = String::with_capacity(template.len());
    let mut cursor = 0;
    let mut changed = false;
    let mut in_raw = false;

    while let Some((tag_start, tag_kind)) = find_next_template_tag(template, cursor) {
        out.push_str(&template[cursor..tag_start]);

        let (body_start, close_delim) = match tag_kind {
            TemplateTagKind::Variable => (tag_start + 2, "}}"),
            TemplateTagKind::Block => (tag_start + 2, "%}"),
            TemplateTagKind::Comment => {
                let end = template[tag_start + 2..].find("#}")? + tag_start + 4;
                out.push_str(&template[tag_start..end]);
                cursor = end;
                continue;
            }
        };
        let tag_end = template[body_start..].find(close_delim)? + body_start;
        let full_tag_end = tag_end + close_delim.len();

        if tag_kind == TemplateTagKind::Block
            && matches!(
                template_block_name(&template[body_start..tag_end]),
                Some("raw")
            )
        {
            in_raw = true;
        }

        if in_raw {
            out.push_str(&template[tag_start..full_tag_end]);
            if tag_kind == TemplateTagKind::Block
                && matches!(
                    template_block_name(&template[body_start..tag_end]),
                    Some("endraw")
                )
            {
                in_raw = false;
            }
        } else {
            let body_start =
                body_start + usize::from(template[body_start..tag_end].starts_with('-'));
            let body_end = tag_end - usize::from(template[body_start..tag_end].ends_with('-'));
            let (rewritten_body, body_changed) =
                rewrite_template_tag_body(&template[body_start..body_end], replacements);
            out.push_str(&template[tag_start..body_start]);
            out.push_str(&rewritten_body);
            out.push_str(&template[body_end..full_tag_end]);
            changed |= body_changed;
        }

        cursor = full_tag_end;
    }

    out.push_str(&template[cursor..]);
    changed.then_some(out)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TemplateTagKind {
    Variable,
    Block,
    Comment,
}

fn find_next_template_tag(template: &str, from: usize) -> Option<(usize, TemplateTagKind)> {
    let mut search_from = from;
    loop {
        let rel = template[search_from..].find('{')?;
        let idx = search_from + rel;
        let rest = &template[idx..];
        let kind = if rest.starts_with("{{") {
            TemplateTagKind::Variable
        } else if rest.starts_with("{%") {
            TemplateTagKind::Block
        } else if rest.starts_with("{#") {
            TemplateTagKind::Comment
        } else {
            search_from = idx + 1;
            continue;
        };
        return Some((idx, kind));
    }
}

fn template_block_name(body: &str) -> Option<&str> {
    let body = body.strip_prefix('-').unwrap_or(body).trim_start();
    let end = body
        .find(|c: char| !is_template_identifier_char(c))
        .unwrap_or(body.len());
    (end > 0).then_some(&body[..end])
}

fn rewrite_template_tag_body(body: &str, replacements: &[(&str, &'static str)]) -> (String, bool) {
    let mut out = String::with_capacity(body.len());
    let mut cursor = 0;
    let mut changed = false;

    while let Some(ch) = body.get(cursor..).and_then(|s| s.chars().next()) {
        if ch == '"' || ch == '\'' {
            let end = quoted_template_string_end(body, cursor, ch);
            out.push_str(&body[cursor..end]);
            cursor = end;
        } else if is_template_identifier_start(ch) {
            let end = identifier_end(body, cursor);
            let ident = &body[cursor..end];
            if !is_template_attribute_or_assignment(body, cursor, end)
                && let Some((_, new)) = replacements.iter().find(|(old, _)| *old == ident)
            {
                out.push_str(new);
                changed = true;
            } else {
                out.push_str(ident);
            }
            cursor = end;
        } else {
            out.push(ch);
            cursor += ch.len_utf8();
        }
    }

    (out, changed)
}

fn quoted_template_string_end(body: &str, start: usize, quote: char) -> usize {
    let mut escaped = false;
    let mut cursor = start + quote.len_utf8();
    while let Some(ch) = body.get(cursor..).and_then(|s| s.chars().next()) {
        cursor += ch.len_utf8();
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == quote {
            break;
        }
    }
    cursor
}

fn identifier_end(body: &str, start: usize) -> usize {
    let mut cursor = start;
    while let Some(ch) = body.get(cursor..).and_then(|s| s.chars().next()) {
        if !is_template_identifier_char(ch) {
            break;
        }
        cursor += ch.len_utf8();
    }
    cursor
}

fn is_template_attribute_or_assignment(body: &str, start: usize, end: usize) -> bool {
    let previous = body[..start].chars().rev().find(|c| !c.is_whitespace());
    if previous == Some('.') {
        return true;
    }

    let next = body[end..].trim_start();
    next.starts_with('=') && !next.starts_with("==")
}

fn is_template_identifier_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_template_identifier_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
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

/// Rewrite deprecated template variables inside one string, returning the
/// new value when it changed.
fn rewrite_deprecated_vars(original: &str) -> Option<String> {
    match normalize_template_vars(original) {
        Cow::Borrowed(_) => None,
        Cow::Owned(modified) => Some(modified),
    }
}

/// Replace deprecated template vars in every string value of the document,
/// mutating the `toml_edit` tree in place.
///
/// Operating on the parsed tree (rather than a raw `str::replace` against the
/// file text) is correct when the TOML source uses escapes: the decoded value
/// would not appear verbatim in the file, so a raw replace silently skipped
/// the migration while detection still warned. `toml_edit` re-serializes the
/// changed string with proper escaping.
fn replace_deprecated_vars_in_doc(doc: &mut toml_edit::DocumentMut) -> bool {
    fn walk_table(table: &mut toml_edit::Table, changed: &mut bool) {
        for (_, item) in table.iter_mut() {
            walk_item(item, changed);
        }
    }
    fn walk_item(item: &mut toml_edit::Item, changed: &mut bool) {
        match item {
            toml_edit::Item::Value(v) => walk_value(v, changed),
            toml_edit::Item::Table(t) => walk_table(t, changed),
            toml_edit::Item::ArrayOfTables(arr) => {
                for t in arr.iter_mut() {
                    walk_table(t, changed);
                }
            }
            _ => {}
        }
    }
    fn walk_value(value: &mut toml_edit::Value, changed: &mut bool) {
        match value {
            toml_edit::Value::String(s) => {
                if let Some(new) = rewrite_deprecated_vars(s.value()) {
                    let decor = s.decor().clone();
                    let mut formatted = toml_edit::Formatted::new(new);
                    *formatted.decor_mut() = decor;
                    *value = toml_edit::Value::String(formatted);
                    *changed = true;
                }
            }
            toml_edit::Value::Array(arr) => {
                for v in arr.iter_mut() {
                    walk_value(v, changed);
                }
            }
            toml_edit::Value::InlineTable(t) => {
                for (_, v) in t.iter_mut() {
                    walk_value(v, changed);
                }
            }
            _ => {}
        }
    }

    let mut changed = false;
    walk_table(doc.as_table_mut(), &mut changed);
    changed
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

/// One deprecated config pattern, carrying the payload its warning needs.
///
/// Each variant maps to a single `format_deprecation_warnings` arm. Silently
/// migrated patterns (the `-create` → `-start` hook rename) have NO variant —
/// they produce no warning by construction.
#[derive(Debug, Clone)]
pub enum DeprecationKind {
    /// Deprecated template variable `old` replaced by `new`.
    TemplateVar {
        old: &'static str,
        new: &'static str,
    },
    /// `[commit-generation]` sections → `[commit.generation]`. Carries the
    /// top-level flag plus per-project keys so each emits its own warning line.
    CommitGeneration(CommitGenerationDeprecations),
    /// `approved-commands` under `[projects."..."]` (moved to approvals.toml).
    ApprovedCommands,
    /// `[select]` section (moved to `[switch.picker]`).
    Select,
    /// `[ci]` section (moved to `[forge]`).
    CiSection,
    /// `no-ff` in `[merge]` (use `ff` instead).
    NoFf,
    /// `no-cd` in `[switch]` (use `cd` instead).
    NoCd,
    /// `timeout-ms` under `[switch.picker]` (removed — picker renders progressively).
    SwitchPickerTimeout,
    /// Pre-* hooks using multi-entry table form, by display path.
    PreHookTableForm(Vec<String>),
}

/// All deprecation patterns detected in a config file, in the order their
/// warnings are emitted.
///
/// Pure data with no path/label context. Used by both config loading (brief
/// warnings) and `wt config show` (full details). Empty when nothing is
/// deprecated.
pub type Deprecations = Vec<DeprecationKind>;

/// Detection half of a [`DeprecationRule`]: append any detected kinds to
/// `kinds`. Rules run in table order, so each pushes its kinds in
/// warning-emission order.
type DetectFn = fn(&toml_edit::DocumentMut, &mut Deprecations);

/// Migration half of a [`DeprecationRule`]: rewrite the deprecated pattern
/// into canonical form, returning whether the document changed. Idempotent —
/// feeding a migrated document back in is a no-op.
type MigrateFn = fn(&mut toml_edit::DocumentMut) -> bool;

/// How a [`DeprecationRule`] participates in detection and migration.
///
/// The shape makes invalid rules unrepresentable: a rule that migrates only
/// on `wt config update` always has a detection to gate on, and a silent rule
/// has no detection — so it emits no warning by construction.
enum RuleMode {
    /// Warns, and is rewritten on every config load before serde parses.
    Structural(DetectFn),
    /// Warns, but the deprecated form still works at runtime (deprecated
    /// template variables resolve via [`normalize_template_vars`];
    /// `approved-commands` is still a valid serde field), so the load path
    /// leaves it alone. Rewritten only via [`compute_migrated_content`]
    /// (`wt config show` / `wt config update`), and only when detection
    /// fires — e.g. an empty `approved-commands = []` is not deprecated and
    /// must survive the rewrite untouched. The gate re-runs detection on the
    /// partially migrated document, so key an `UpdateOnly` detection on
    /// sections no earlier rule rewrites — otherwise the rule could migrate
    /// without ever having warned.
    UpdateOnly(DetectFn),
    /// Silently-migrated rename: rewritten on every load like `Structural`,
    /// but with no `DeprecationKind` and no warning by construction.
    Silent,
}

/// One deprecated config pattern: how to detect it and how to rewrite it.
struct DeprecationRule {
    mode: RuleMode,
    migrate: MigrateFn,
}

/// Every deprecation, one row each. The table order is the contract:
/// detection ([`detect_deprecations_from_doc`]) and migration
/// ([`migrate_content_doc`], [`compute_migrated_content`]) iterate top to
/// bottom, so a row's position is both its warning-emission position and its
/// migration position.
///
/// Most rows rewrite disjoint keys, but two orderings are load-bearing:
/// - The silent `-create` → `-start` rename precedes the pre-hook table-form
///   rule, so a renamed `[pre-create]` multi-entry table still gets
///   pipeline-migrated.
/// - `[select]` → `[switch.picker]` precedes the rules that edit keys under
///   `[switch]`: it moves a `timeout-ms` written under `[select]` to where
///   the strip rule looks, and converts an inline `switch` table to a
///   standard one, which the `no-cd` rule's `as_table()` match requires.
///
/// The `[ci]` → `[forge]` rule is order-independent: `[forge]` takes over
/// `[ci]`'s explicit document position (see [`migrate_ci_doc`]), so its
/// rendered placement doesn't depend on which tables other rules re-append.
///
/// Adding a deprecation: a detection fn and an idempotent migration fn
/// (one-line `any_config_table` / `for_each_config_table_mut` compositions
/// live directly in the row), a [`DeprecationKind`] variant with its
/// `format_deprecation_warnings` arm, and a row here (plus a
/// [`DeprecatedSection`] entry for a removed top-level section). A
/// silently-migrated rename is just a [`RuleMode::Silent`] row.
const DEPRECATION_RULES: &[DeprecationRule] = &[
    // Template variables: {{ repo_root }} → {{ repo_path }} etc., inside any
    // string value.
    DeprecationRule {
        mode: RuleMode::UpdateOnly(|doc, kinds| {
            let template_strings = extract_template_strings_from_doc(doc);
            for (old, new) in find_deprecated_vars_from_strings(&template_strings) {
                kinds.push(DeprecationKind::TemplateVar { old, new });
            }
        }),
        migrate: replace_deprecated_vars_in_doc,
    },
    // [commit-generation] → [commit.generation], top-level and per-project.
    DeprecationRule {
        mode: RuleMode::Structural(|doc, kinds| {
            let commit_gen = find_commit_generation_from_doc(doc);
            if !commit_gen.is_empty() {
                kinds.push(DeprecationKind::CommitGeneration(commit_gen));
            }
        }),
        migrate: |doc| {
            for_each_config_table_mut(doc, |_, table| migrate_commit_generation_in(table))
        },
    },
    // approved-commands under [projects."..."] → approvals.toml. The rule only
    // removes; `wt config update` copies the entries to approvals.toml first
    // (see `copy_approved_commands_to_approvals_file`).
    DeprecationRule {
        mode: RuleMode::UpdateOnly(|doc, kinds| {
            if find_approved_commands_from_doc(doc) {
                kinds.push(DeprecationKind::ApprovedCommands);
            }
        }),
        migrate: remove_approved_commands_doc,
    },
    // [select] → [switch.picker].
    DeprecationRule {
        mode: RuleMode::Structural(|doc, kinds| {
            if any_config_table(doc, |_, table| has_select_without_picker(table)) {
                kinds.push(DeprecationKind::Select);
            }
        }),
        migrate: |doc| for_each_config_table_mut(doc, |_, table| migrate_select_table(table)),
    },
    // pre-create/post-create → pre-start/post-start. Silent: the creation
    // hook rename is paused (see #2838) — both names load via serde aliases,
    // but in-memory migration to canonical keeps round-trip analysis
    // (`unknown_tree`) coherent for the table and array-of-tables forms,
    // where serde aliases on the field don't cover every shape. Must precede
    // the pre-hook table-form rule — see the ordering notes above.
    DeprecationRule {
        mode: RuleMode::Silent,
        migrate: |doc| {
            let pre = rename_hook_key(doc, "pre-create", "pre-start");
            let post = rename_hook_key(doc, "post-create", "post-start");
            pre || post
        },
    },
    // [ci] → [forge].
    DeprecationRule {
        mode: RuleMode::Structural(|doc, kinds| {
            if find_ci_section_from_doc(doc) {
                kinds.push(DeprecationKind::CiSection);
            }
        }),
        migrate: migrate_ci_doc,
    },
    // merge.no-ff → merge.ff (inverted).
    DeprecationRule {
        mode: RuleMode::Structural(|doc, kinds| {
            if find_negated_bool_from_doc(doc, "merge", "no-ff", "ff") {
                kinds.push(DeprecationKind::NoFf);
            }
        }),
        migrate: |doc| migrate_negated_bool_doc(doc, "merge", "no-ff", "ff"),
    },
    // switch.no-cd → switch.cd (inverted).
    DeprecationRule {
        mode: RuleMode::Structural(|doc, kinds| {
            if find_negated_bool_from_doc(doc, "switch", "no-cd", "cd") {
                kinds.push(DeprecationKind::NoCd);
            }
        }),
        migrate: |doc| migrate_negated_bool_doc(doc, "switch", "no-cd", "cd"),
    },
    // switch.picker.timeout-ms — removed; the picker renders progressively.
    DeprecationRule {
        mode: RuleMode::Structural(|doc, kinds| {
            if any_config_table(doc, |_, table| has_switch_picker_timeout(table)) {
                kinds.push(DeprecationKind::SwitchPickerTimeout);
            }
        }),
        migrate: |doc| {
            for_each_config_table_mut(doc, |_, table| remove_switch_picker_timeout_in(table))
        },
    },
    // Multi-entry pre-* hook tables → array-of-tables pipeline form.
    DeprecationRule {
        mode: RuleMode::Structural(|doc, kinds| {
            let pre_hook_table_form = find_pre_hook_table_form_from_doc(doc);
            if !pre_hook_table_form.is_empty() {
                kinds.push(DeprecationKind::PreHookTableForm(pre_hook_table_form));
            }
        }),
        migrate: |doc| for_each_config_table_mut(doc, |_, table| migrate_pre_hook_table_in(table)),
    },
];

/// Detect deprecations in config content. Pure function, no I/O.
///
/// Returns the detected deprecation patterns. This is the recommended entry
/// point for deprecation detection.
pub fn detect_deprecations(content: &str) -> Deprecations {
    let Ok(doc) = content.parse::<toml_edit::DocumentMut>() else {
        return Vec::new();
    };
    detect_deprecations_from_doc(&doc)
}

/// Detect deprecations from an already-parsed document.
///
/// Pushes kinds in [`DEPRECATION_RULES`] order — the warning-emission order —
/// so iterating the returned `Vec` reproduces the warning text byte-for-byte.
fn detect_deprecations_from_doc(doc: &toml_edit::DocumentMut) -> Deprecations {
    let mut kinds = Vec::new();
    for rule in DEPRECATION_RULES {
        match rule.mode {
            RuleMode::Structural(detect) | RuleMode::UpdateOnly(detect) => detect(doc, &mut kinds),
            RuleMode::Silent => {}
        }
    }
    kinds
}

/// Detect-side scope walk: apply `f` to the top-level table (scope `None`) and
/// to each `[projects."key"]` table (scope `Some(key)`), short-circuiting on the
/// first `true`. The scope key feeds callers that build display paths.
fn any_config_table(
    doc: &toml_edit::DocumentMut,
    mut f: impl FnMut(Option<&str>, &toml_edit::Table) -> bool,
) -> bool {
    if f(None, doc.as_table()) {
        return true;
    }
    if let Some(projects) = doc.get("projects").and_then(|p| p.as_table()) {
        for (key, value) in projects.iter() {
            if let Some(table) = value.as_table()
                && f(Some(key), table)
            {
                return true;
            }
        }
    }
    false
}

/// Migrate-side scope walk: apply `f` mutably to the top-level table (scope
/// `None`) and to each `[projects."key"]` table (scope `Some(key)`), returning
/// whether any scope reported a change.
fn for_each_config_table_mut(
    doc: &mut toml_edit::DocumentMut,
    mut f: impl FnMut(Option<&str>, &mut toml_edit::Table) -> bool,
) -> bool {
    let mut modified = f(None, doc.as_table_mut());
    if let Some(projects) = doc.get_mut("projects").and_then(|p| p.as_table_mut()) {
        for (key, value) in projects.iter_mut() {
            if let Some(table) = value.as_table_mut() {
                modified |= f(Some(key.get()), table);
            }
        }
    }
    modified
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

/// Whether a scope has a non-empty deprecated `commit-generation` section and
/// no canonical `[commit.generation]` to supersede it. Shared by detection and
/// migration so both agree on which scopes need migrating.
fn has_deprecated_commit_generation(table: &toml_edit::Table) -> bool {
    if has_table_like_child(table.get("commit"), "generation") {
        return false;
    }
    table.get("commit-generation").is_some_and(|section| {
        section.as_table().is_some_and(|t| !t.is_empty())
            || section.as_inline_table().is_some_and(|t| !t.is_empty())
    })
}

fn find_commit_generation_from_doc(doc: &toml_edit::DocumentMut) -> CommitGenerationDeprecations {
    let mut result = CommitGenerationDeprecations::default();
    // Top-level sets a bool; each project records its key — a per-scope fork, so
    // the closure branches on the scope rather than returning a uniform bool.
    any_config_table(doc, |scope, table| {
        if has_deprecated_commit_generation(table) {
            match scope {
                None => result.has_top_level = true,
                Some(key) => result.project_keys.push(key.to_string()),
            }
        }
        false
    });
    result
}

/// Whether a TOML item is a table or inline table (can be migrated as a section).
fn is_table_like(item: &toml_edit::Item) -> bool {
    matches!(
        item,
        toml_edit::Item::Table(_) | toml_edit::Item::Value(toml_edit::Value::InlineTable(_))
    )
}

/// Whether a slot can host a freshly-inserted subtable: absent, or already a
/// (possibly inline) table. A scalar/array occupant blocks insertion — the
/// migration would remove the deprecated source but have nowhere to put
/// `[parent.child]`, silently dropping the user's data.
fn can_host_subtable(item: Option<&toml_edit::Item>) -> bool {
    item.is_none_or(is_table_like)
}

fn has_table_like_child(item: Option<&toml_edit::Item>, key: &str) -> bool {
    match item {
        Some(toml_edit::Item::Table(t)) => t.get(key).is_some_and(is_table_like),
        Some(toml_edit::Item::Value(toml_edit::Value::InlineTable(t))) => t
            .get(key)
            .is_some_and(|v| matches!(v, toml_edit::Value::InlineTable(_))),
        _ => false,
    }
}

/// Ensure a table-like parent is writable as a standard table.
///
/// Inline tables can deserialize like tables, but TOML forbids extending them
/// with later subtables. Convert before inserting migrated nested sections so
/// existing inline parent fields survive alongside the new child table.
fn ensure_standard_table_parent<'a>(
    table: &'a mut toml_edit::Table,
    key: &str,
) -> Option<&'a mut toml_edit::Table> {
    if !table.contains_key(key) {
        let mut parent = toml_edit::Table::new();
        parent.set_implicit(true);
        table.insert(key, toml_edit::Item::Table(parent));
    }

    let item = table.get_mut(key)?;
    if let Some(inline) = item.as_inline_table().cloned() {
        *item = toml_edit::Item::Table(inline.into_table());
    }
    item.as_table_mut()
}

/// Convert a table-like TOML item into a `Table`. Returns `None` for other shapes.
fn into_table(item: toml_edit::Item) -> Option<toml_edit::Table> {
    match item {
        toml_edit::Item::Table(t) => Some(t),
        toml_edit::Item::Value(toml_edit::Value::InlineTable(it)) => Some(it.into_table()),
        _ => None,
    }
}

/// Migrate one scope's `[commit-generation]` → `[commit.generation]`.
///
/// Skips when a canonical `[commit.generation]` already exists (new format takes
/// precedence). Peeks before removing so a malformed value (e.g. a bare string)
/// is left in place rather than silently dropped when a sibling migration also
/// serializes the doc. Requires `commit` be absent or a table — a scalar
/// `commit = "x"` blocks insertion of `[commit.generation]`, and removing the
/// source then would silently drop the deprecated section.
fn migrate_commit_generation_in(table: &mut toml_edit::Table) -> bool {
    if has_table_like_child(table.get("commit"), "generation")
        || !table.get("commit-generation").is_some_and(is_table_like)
        || !can_host_subtable(table.get("commit"))
    {
        return false;
    }
    let Some(old_section) = table.remove("commit-generation") else {
        return false;
    };
    let mut generation = into_table(old_section).expect("checked is_table_like above");

    // Merge args into command if present.
    merge_args_into_command(&mut generation);

    // Ensure [commit] exists (implicit, so only [commit.generation] renders a
    // header) and move the migrated section under it.
    if let Some(commit_table) = ensure_standard_table_parent(table, "commit") {
        commit_table.insert("generation", toml_edit::Item::Table(generation));
    }
    true
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

/// Check if a table has a non-empty `select` section without `switch.picker`.
fn has_select_without_picker(table: &toml_edit::Table) -> bool {
    let has_new_section = has_table_like_child(table.get("switch"), "picker");

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

/// Migrate a `select` key to `switch.picker` within a table. Returns whether a
/// migration was performed. Skips when `[switch.picker]` already exists.
///
/// Leaves a malformed `select` (e.g., a string) in place rather than removing
/// it — silently dropping it would lose user config when a sibling migration
/// also rewrites the document.
fn migrate_select_table(table: &mut toml_edit::Table) -> bool {
    let has_new_section = has_table_like_child(table.get("switch"), "picker");

    if has_new_section {
        return false;
    }

    if !table.get("select").is_some_and(is_table_like) {
        return false;
    }

    // A scalar `switch = "x"` blocks insertion of `[switch.picker]`; removing
    // `select` then would silently drop the user's picker config.
    if !can_host_subtable(table.get("switch")) {
        return false;
    }

    let select_table =
        into_table(table.remove("select").unwrap()).expect("checked is_table_like above");

    if let Some(switch_table) = ensure_standard_table_parent(table, "switch") {
        switch_table.insert("picker", toml_edit::Item::Table(select_table));
    }

    true
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
            && table_like_len(item).is_some_and(|len| len >= 2)
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
    any_config_table(doc, |scope, table| {
        let prefix = scope.map_or_else(String::new, |key| format!("projects.\"{key}\""));
        collect_pre_hook_table_form_keys(table, &prefix, &mut found);
        false
    });
    found
}

fn table_like_len(item: &toml_edit::Item) -> Option<usize> {
    match item {
        toml_edit::Item::Table(t) => Some(t.len()),
        toml_edit::Item::Value(toml_edit::Value::InlineTable(t)) => Some(t.len()),
        _ => None,
    }
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
///
/// `[forge]` takes over `[ci]`'s document position — a fresh table has no
/// position and would render at the end of the file instead of in the user's
/// original spot. The `platform` entry moves wholesale (key and item), so
/// comments attached to the line survive. When `[ci]` is fully consumed, its
/// decor (comments and blank lines above the header) moves to `[forge]` too;
/// when other keys keep `[ci]` alive, the decor stays there and `[forge]`
/// renders directly after the remainder — it shares `[ci]`'s position, the
/// position sort is stable, and `[forge]` is inserted later in visit order.
fn migrate_ci_doc(doc: &mut toml_edit::DocumentMut) -> bool {
    // Skip if [forge] already exists
    if doc
        .get("forge")
        .is_some_and(|f| f.is_table() || f.is_inline_table())
    {
        return false;
    }

    let Some(ci_table) = doc.get_mut("ci").and_then(|ci| ci.as_table_mut()) else {
        return false;
    };
    // Gate before mutating: a missing or non-string platform is left untouched.
    if ci_table
        .get("platform")
        .is_none_or(|p| p.as_str().is_none())
    {
        return false;
    }

    // Move only the migrated entry; keep any other keys so we don't silently
    // drop config that wasn't part of the migration.
    let (key, item) = ci_table
        .remove_entry("platform")
        .expect("checked platform exists above");
    let mut forge_table = toml_edit::Table::new();
    forge_table.insert_formatted(&key, item);
    forge_table.set_position(ci_table.position());
    if ci_table.is_empty() {
        *forge_table.decor_mut() = ci_table.decor().clone();
        doc.remove("ci");
    }

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
    any_config_table(doc, |_, scope| {
        scope
            .get(section)
            .and_then(|s| s.as_table())
            .is_some_and(|table| !table.contains_key(new_key) && table.contains_key(old_key))
    })
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
    for_each_config_table_mut(doc, |_, scope| {
        scope
            .get_mut(section)
            .and_then(|s| s.as_table_mut())
            .is_some_and(|table| migrate_negated_bool(table, old_key, new_key))
    })
}

/// Convert a multi-entry pre-* table section into an array-of-tables pipeline.
///
/// Removes `[key]` as a table section and inserts `[[key]]` blocks —
/// one block per named step, preserving insertion order.
///
/// Iterates pre-* keys in document order (not [`PRE_HOOK_KEYS`] order) so
/// migrated sections land in the same relative position they had in the
/// source file.
fn migrate_pre_hook_table_in(table: &mut toml_edit::Table) -> bool {
    let keys_to_migrate: Vec<String> = table
        .iter()
        .filter(|(k, v)| {
            PRE_HOOK_KEYS.contains(k)
                && pre_hook_pipeline_entries(v).is_some_and(|entries| entries.len() >= 2)
        })
        .map(|(k, _)| k.to_string())
        .collect();

    let mut modified = false;
    for key in keys_to_migrate {
        let item = table.get_mut(&key).unwrap();
        let entries = pre_hook_pipeline_entries(item).unwrap();

        let mut arr = toml_edit::ArrayOfTables::new();
        for (name, value) in entries.iter() {
            let mut block = toml_edit::Table::new();
            block.insert(name, toml_edit::value(value.as_str()));
            arr.push(block);
        }

        *item = toml_edit::Item::ArrayOfTables(arr);
        modified = true;
    }
    modified
}

fn pre_hook_pipeline_entries(item: &toml_edit::Item) -> Option<Vec<(String, String)>> {
    match item {
        toml_edit::Item::Table(t) => {
            let entries = t
                .iter()
                .map(|(name, value)| Some((name.to_string(), value.as_str()?.to_string())))
                .collect::<Option<Vec<_>>>()?;
            Some(entries)
        }
        toml_edit::Item::Value(toml_edit::Value::InlineTable(t)) => {
            let entries = t
                .iter()
                .map(|(name, value)| Some((name.to_string(), value.as_str()?.to_string())))
                .collect::<Option<Vec<_>>>()?;
            Some(entries)
        }
        _ => None,
    }
}

/// Apply the load-path migrations — every [`RuleMode::Structural`] and
/// [`RuleMode::Silent`] rule — to a parsed document, in table order. Returns
/// true if any modifications were made.
///
/// [`RuleMode::UpdateOnly`] rules are excluded — template variable renaming
/// is cosmetic (would break `--var` overrides), and approved-commands is
/// still a valid serde field. They apply in [`compute_migrated_content`].
fn migrate_content_doc(doc: &mut toml_edit::DocumentMut) -> bool {
    let mut modified = false;
    for rule in DEPRECATION_RULES {
        if matches!(rule.mode, RuleMode::Structural(_) | RuleMode::Silent) {
            modified |= (rule.migrate)(doc);
        }
    }
    modified
}

/// Rename `old_key` to `new_key` at the top level and under each `[projects."..."]`.
///
/// Skips any location where `new_key` already exists — the user has already
/// consolidated there, and clobbering their canonical value would lose config.
/// The rewrite preserves the value shape (string, `[table]`, or
/// `[[array-of-tables]]`) since it moves the `Item` unchanged.
fn rename_hook_key(doc: &mut toml_edit::DocumentMut, old_key: &str, new_key: &str) -> bool {
    let mut modified = false;

    if doc.get(new_key).is_none()
        && let Some(value) = doc.remove(old_key)
    {
        doc.insert(new_key, value);
        modified = true;
    }

    if let Some(projects) = doc.get_mut("projects").and_then(|p| p.as_table_mut()) {
        for (_key, project_value) in projects.iter_mut() {
            if let Some(project_table) = project_value.as_table_mut()
                && project_table.get(new_key).is_none()
                && let Some(value) = project_table.remove(old_key)
            {
                project_table.insert(new_key, value);
                modified = true;
            }
        }
    }

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

/// Remove `timeout-ms` from `[switch.picker]` in a table (top-level or project).
/// An emptied `[switch.picker]` section is left in place — it round-trips
/// harmlessly.
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
/// content, so the approvals data survives the rewrite. `Ok(None)` for the
/// benign no-op cases (a valid `approvals.toml` already exists, or the config
/// has no approved-commands entries). Returns `Err` when the existing
/// approvals file cannot be validated or a copy was attempted but failed — the
/// caller must abort before rewriting config.toml, otherwise the legacy
/// approvals are silently lost.
pub fn copy_approved_commands_to_approvals_file(
    config_path: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    let approvals_path = config_path.with_file_name("approvals.toml");
    let _lock = super::user::mutation::acquire_config_lock(&approvals_path)?;
    if approvals_path.exists() {
        validate_existing_approvals_file(&approvals_path)?;
        return Ok(None); // Already authoritative, don't overwrite
    }

    let approvals =
        super::approvals::Approvals::load_from_config_file(config_path).with_context(|| {
            format!(
                "Failed to read approved-commands from {} for migration",
                config_path.display()
            )
        })?;
    if approvals.projects().next().is_none() {
        return Ok(None); // Nothing to copy
    }

    approvals.save_to(&approvals_path).with_context(|| {
        format!(
            "Failed to write migrated approvals to {}",
            approvals_path.display()
        )
    })?;
    Ok(Some(approvals_path))
}

fn validate_existing_approvals_file(approvals_path: &Path) -> anyhow::Result<()> {
    let content = std::fs::read_to_string(approvals_path).with_context(|| {
        format!(
            "Failed to read existing approvals file {}",
            crate::path::format_path_for_display(approvals_path)
        )
    })?;
    toml::from_str::<super::approvals::Approvals>(&content).with_context(|| {
        format!(
            "Failed to parse existing approvals file {}",
            crate::path::format_path_for_display(approvals_path)
        )
    })?;
    Ok(())
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
    // Validate preconditions before removing args. Every element must be a
    // string — a single non-string (e.g. `args = [1, "--ok"]`) would otherwise
    // be silently filtered out while `args` was removed, dropping user data.
    let can_merge = table
        .get("args")
        .and_then(|a| a.as_array())
        .is_some_and(|a| a.iter().all(|v| v.as_str().is_some()))
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

    // `can_merge` guarantees every element is a string; `filter_map` here just
    // extracts them.
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
            let deprecations = detect_deprecations_from_doc(&doc);
            let migrated_content = migrate_content_from_doc(content, doc);
            (deprecations, migrated_content)
        }
        Err(_) => (Vec::new(), content.to_string()),
    };

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
    //
    // Some deprecations migrate silently (e.g. the `-create` → `-start` hook
    // rename): `format_deprecation_warnings` emits nothing for them. The hint is
    // gated on the warning text being non-empty so a silently-migrated config
    // produces no stray "run wt config show" line with nothing above it.
    if emit_inline_warnings && !warnings_suppressed() {
        let warnings = format_deprecation_warnings(&info);
        if !warnings.is_empty() {
            eprint!("{warnings}");
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
    // Callers (`wt config show`, `wt config update`, `format_deprecation_details`)
    // all run content through `check_and_migrate` first, so it is known to parse.
    let mut doc = content
        .parse::<toml_edit::DocumentMut>()
        .expect("compute_migrated_content called with content that failed TOML parse; callers must funnel through check_and_migrate first");

    let mut modified = false;
    for rule in DEPRECATION_RULES {
        let apply = match rule.mode {
            RuleMode::Structural(_) | RuleMode::Silent => true,
            // Gated on detection: the migration may rewrite more than what
            // counts as deprecated (e.g. `remove_approved_commands_doc` also
            // strips an *empty* `approved-commands = []`, which detection
            // deliberately ignores).
            RuleMode::UpdateOnly(detect) => {
                let mut kinds = Vec::new();
                detect(&doc, &mut kinds);
                !kinds.is_empty()
            }
        };
        if apply {
            modified |= (rule.migrate)(&mut doc);
        }
    }
    if modified {
        doc.to_string()
    } else {
        content.to_string()
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
    let label = &info.label;
    let mut out = String::new();

    // One `warning_message` line per emitted message. The kinds are stored in
    // emission order, so a single pass reproduces the original output verbatim.
    // Each arm pushes its own newline so a multi-line kind (commit-generation)
    // can emit several lines.
    for kind in &info.deprecations {
        match kind {
            DeprecationKind::TemplateVar { old, new } => {
                let _ = writeln!(
                    out,
                    "{}",
                    warning_message(cformat!(
                        "{label}: template variable <bold>{old}</> is deprecated in favor of <bold>{new}</>"
                    ))
                );
            }
            DeprecationKind::CommitGeneration(commit_gen) => {
                if commit_gen.has_top_level {
                    let _ = writeln!(
                        out,
                        "{}",
                        warning_message(cformat!(
                            "{label}: <bold>[commit-generation]</> is deprecated in favor of <bold>[commit.generation]</>"
                        ))
                    );
                }
                for k in &commit_gen.project_keys {
                    let _ = writeln!(
                        out,
                        "{}",
                        warning_message(cformat!(
                            "{label}: <bold>[projects.\"{k}\".commit-generation]</> is deprecated in favor of <bold>[projects.\"{k}\".commit.generation]</>"
                        ))
                    );
                }
            }
            DeprecationKind::ApprovedCommands => {
                let _ = writeln!(
                    out,
                    "{}",
                    warning_message(cformat!(
                        "{label}: <bold>approved-commands</> under <bold>[projects]</> is deprecated in favor of <bold>approvals.toml</>"
                    ))
                );
            }
            DeprecationKind::Select => {
                let _ = writeln!(
                    out,
                    "{}",
                    warning_message(cformat!(
                        "{label}: <bold>[select]</> is deprecated in favor of <bold>[switch.picker]</>"
                    ))
                );
            }
            DeprecationKind::CiSection => {
                let _ = writeln!(
                    out,
                    "{}",
                    warning_message(cformat!(
                        "{label}: <bold>[ci]</> is deprecated in favor of <bold>[forge]</>"
                    ))
                );
            }
            DeprecationKind::NoFf => {
                let _ = writeln!(
                    out,
                    "{}",
                    warning_message(cformat!(
                        "{label}: <bold>merge.no-ff</> is deprecated in favor of <bold>merge.ff</> (inverted)"
                    ))
                );
            }
            DeprecationKind::NoCd => {
                let _ = writeln!(
                    out,
                    "{}",
                    warning_message(cformat!(
                        "{label}: <bold>switch.no-cd</> is deprecated in favor of <bold>switch.cd</> (inverted)"
                    ))
                );
            }
            DeprecationKind::SwitchPickerTimeout => {
                let _ = writeln!(
                    out,
                    "{}",
                    warning_message(cformat!(
                        "{label}: <bold>switch.picker.timeout-ms</> is no longer used — the picker now renders progressively"
                    ))
                );
            }
            DeprecationKind::PreHookTableForm(hooks) => {
                let hook_list = hooks
                    .iter()
                    .map(|h| cformat!("<bold>{h}</>"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let _ = writeln!(
                    out,
                    "{}",
                    warning_message(cformat!(
                        "{label}: table form for {hook_list} is deprecated in favor of the pipeline form. \
                     We're unifying pre-hooks, post-hooks, and aliases so that list form always runs serially \
                     and table form always runs in parallel — migrate now to keep the current serial behavior \
                     once the table form is repurposed."
                    ))
                );
            }
        }
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

/// `[commit.generation]` sub-keys (canonical/migrated form) that belong only
/// in user config.
///
/// `[commit.generation]` is itself a valid *project* config section — but only
/// for `template-append`, the project-wide commit convention shared across the
/// team. The LLM `command` and the full prompt templates are resolved from
/// user/system config only. Putting them in a project `.config/wt.toml` is a
/// common, easily-missed mistake (see #2774).
///
/// `is_valid_key` only knows top-level keys, so misplaced *nested* keys can't
/// be classified by [`key_belongs_in`]. Since `commit` is now a legitimate
/// project section, the round-trip flags just these offending leaves; without
/// this list they'd surface as a bare "unknown field" instead of a redirect.
///
/// These paths are schema-valid in user config, so they only ever surface as
/// unknown-nested under *project* config; [`nested_key_belongs_in`] therefore
/// needs no config-type gate. The list is kept in sync with
/// `CommitGenerationConfig` by `user_only_commit_generation_paths_track_schema`.
const USER_ONLY_COMMIT_GENERATION_PATHS: &[&str] = &[
    "commit.generation.command",
    "commit.generation.template",
    "commit.generation.template-file",
    "commit.generation.squash-template",
    "commit.generation.squash-template-file",
];

/// Returns the config where a misplaced *nested* key belongs.
///
/// The nested analog of [`key_belongs_in`], for the one case that needs a
/// redirect: user-only `[commit.generation]` keys placed in project config
/// (see `USER_ONLY_COMMIT_GENERATION_PATHS`). Returns `None` for ordinary
/// unknown nested paths (typos), which stay "unknown field".
pub fn nested_key_belongs_in<C: WorktrunkConfig>(path: &str) -> Option<&'static str> {
    USER_ONLY_COMMIT_GENERATION_PATHS
        .contains(&path)
        .then(C::Other::description)
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
        UnknownWarning::NestedWrongConfig {
            path,
            other_description,
        } => cformat!(
            "{label} has key <bold>{path}</> which belongs in {other_description} (will be ignored)"
        ),
        UnknownWarning::NestedUnknown { path } => {
            cformat!("{label} has unknown field <bold>{path}</> (will be ignored)")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `USER_ONLY_COMMIT_GENERATION_PATHS` must stay in sync with
    /// `CommitGenerationConfig`: every key except the project-valid
    /// `template-append`. If a field is added to that struct without updating
    /// the list, a misplaced key would silently degrade to "unknown field".
    #[test]
    fn user_only_commit_generation_paths_track_schema() {
        let schema = schemars::SchemaGenerator::default()
            .into_root_schema_for::<crate::config::CommitGenerationConfig>();
        let mut expected: Vec<String> = schema
            .as_object()
            .and_then(|o| o.get("properties"))
            .and_then(|p| p.as_object())
            .map(|props| props.keys().cloned().collect())
            .unwrap_or_default();
        expected.retain(|k| k != "template-append");
        let mut expected: Vec<String> = expected
            .iter()
            .map(|k| format!("commit.generation.{k}"))
            .collect();
        expected.sort();

        let mut actual: Vec<String> = USER_ONLY_COMMIT_GENERATION_PATHS
            .iter()
            .map(|s| s.to_string())
            .collect();
        actual.sort();

        assert_eq!(actual, expected);
    }

    // Test helpers bridging string fixtures to the entry points. The find_*
    // helpers extract one kind from `detect_deprecations`; migration tests
    // call `migrate_content` / `compute_migrated_content` directly. The
    // template-var and approved-commands helpers wrap internal functions
    // whose isolation or parse-failure semantics have no public seam.

    fn extract_template_strings(content: &str) -> Vec<String> {
        let Ok(doc) = content.parse::<toml_edit::DocumentMut>() else {
            return vec![];
        };
        extract_template_strings_from_doc(&doc)
    }

    fn replace_deprecated_vars(content: &str) -> String {
        let Ok(mut doc) = content.parse::<toml_edit::DocumentMut>() else {
            return content.to_string();
        };
        if !replace_deprecated_vars_in_doc(&mut doc) {
            return content.to_string();
        }
        // `toml_edit` always serializes a document with a trailing newline.
        // These helper tests pass fragments without one and assert on the
        // substituted text, not serialization shape — mirror the input.
        let out = doc.to_string();
        if !content.ends_with('\n') {
            out.strip_suffix('\n').map(str::to_owned).unwrap_or(out)
        } else {
            out
        }
    }

    fn find_deprecated_vars(content: &str) -> Vec<(&'static str, &'static str)> {
        let strings = extract_template_strings(content);
        find_deprecated_vars_from_strings(&strings)
    }

    /// True when `deprecations` carries a kind matching `pred`. Lets the
    /// per-kind tests assert presence of a single deprecation against the
    /// `Vec<DeprecationKind>`, the way they used to read a boolean field.
    fn has_kind(deprecations: &Deprecations, pred: impl Fn(&DeprecationKind) -> bool) -> bool {
        deprecations.iter().any(pred)
    }

    fn find_commit_generation_deprecations(content: &str) -> CommitGenerationDeprecations {
        detect_deprecations(content)
            .into_iter()
            .find_map(|k| match k {
                DeprecationKind::CommitGeneration(found) => Some(found),
                _ => None,
            })
            .unwrap_or_default()
    }

    fn find_approved_commands_deprecation(content: &str) -> bool {
        has_kind(&detect_deprecations(content), |k| {
            matches!(k, DeprecationKind::ApprovedCommands)
        })
    }

    fn find_select_deprecation(content: &str) -> bool {
        has_kind(&detect_deprecations(content), |k| {
            matches!(k, DeprecationKind::Select)
        })
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
post-start = "ln -sf {{ repo_root }}/node_modules node_modules"
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(found, vec![("repo_root", "repo_path")]);
    }

    #[test]
    fn test_find_deprecated_vars_worktree() {
        let content = r#"
post-start = "cd {{ worktree }} && npm install"
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
post-start = "ln -sf {{ main_worktree_path }}/node_modules ."
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(found, vec![("main_worktree_path", "primary_worktree_path")]);
    }

    #[test]
    fn test_find_deprecated_vars_multiple() {
        let content = r#"
worktree-path = "../{{ main_worktree }}.{{ branch | sanitize }}"
post-start = "ln -sf {{ repo_root }}/node_modules {{ worktree }}/node_modules"
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
post-start = "ln -sf {{ repo_root | something }}/node_modules"
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(found, vec![("repo_root", "repo_path")]);
    }

    #[test]
    fn test_find_deprecated_vars_deduplicates() {
        let content = r#"
post-start = "{{ repo_root }}/a {{ repo_root }}/b"
"#;
        let found = find_deprecated_vars(content);
        assert_eq!(found, vec![("repo_root", "repo_path")]);
    }

    #[test]
    fn test_find_deprecated_vars_does_not_match_suffix() {
        // Should NOT match "worktree_path" when looking for "worktree"
        let content = r#"
post-start = "cd {{ worktree_path }} && npm install"
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

    /// Regression: when the deprecated var sits next to an escaped quote, the
    /// decoded string value does not appear verbatim in the raw file text, so
    /// the old `str::replace`-on-content path silently skipped the migration
    /// while detection still warned. The toml_edit-tree rewrite handles it.
    #[test]
    fn test_replace_deprecated_vars_with_escaped_quotes() {
        // Source TOML: pre-start = "echo \"{{ repo_root }}\""
        let content = r#"pre-start = "echo \"{{ repo_root }}\"""#;
        let result = replace_deprecated_vars(content);
        assert!(
            !result.contains("repo_root"),
            "deprecated var must be migrated even with escaped quotes; got: {result}"
        );
        assert!(
            result.contains("repo_path"),
            "migrated var must be present; got: {result}"
        );
    }

    /// Same, exercised through the public `compute_migrated_content` entry.
    #[test]
    fn test_compute_migrated_content_escaped_quotes() {
        let content = "pre-start = \"echo \\\"{{ repo_root }}\\\"\"\n";
        let migrated = compute_migrated_content(content);
        assert!(
            !migrated.contains("repo_root"),
            "compute_migrated_content must migrate vars inside escaped strings; got: {migrated}"
        );
        assert!(migrated.contains("repo_path"));
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
post-start = "ln -sf {{ repo_root }}/node_modules {{ worktree }}/node_modules"
"#;
        let result = replace_deprecated_vars(content);
        assert_eq!(
            result,
            r#"
worktree-path = "../{{ repo }}.{{ branch | sanitize }}"
post-start = "ln -sf {{ repo_path }}/node_modules {{ worktree_path }}/node_modules"
"#
        );
    }

    #[test]
    fn test_replace_deprecated_vars_preserves_other_content() {
        let content = r#"
# This is a comment
worktree-path = "../{{ repo }}.{{ branch }}"

[hooks]
post-start = "echo hello"
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

    /// The tree walker must recurse into arrays of tables and inline tables —
    /// not just top-level tables — so a deprecated var is migrated wherever it
    /// appears, while non-string scalars are left untouched.
    #[test]
    fn test_replace_deprecated_vars_walks_array_of_tables_and_inline_table() {
        let content = r#"
[[steps]]
run = "build {{ repo_root }}"

[env]
script = { cmd = "{{ repo_root }}/x" }
timeout = 30
"#;
        let result = replace_deprecated_vars(content);
        assert!(
            result.contains("build {{ repo_path }}"),
            "array-of-tables var migrated: {result}"
        );
        assert!(
            result.contains("{{ repo_path }}/x"),
            "inline-table var migrated: {result}"
        );
        assert!(
            result.contains("timeout = 30"),
            "non-string scalar left untouched: {result}"
        );
    }

    /// `into_table` underpins every "peek before remove" structural migration:
    /// callers gate on `is_table_like`, so the non-table arm must return `None`
    /// rather than panic if that contract is ever violated.
    #[test]
    fn test_into_table_returns_none_for_non_table() {
        let scalar = toml_edit::Item::Value(toml_edit::Value::from(5));
        assert!(into_table(scalar).is_none());
    }

    /// Canonical config with no deprecations must round-trip through
    /// `compute_migrated_content` byte-for-byte (the unmodified branch).
    #[test]
    fn test_compute_migrated_content_noop_returns_input_unchanged() {
        let content = "pre-start = \"echo {{ repo_path }}\"\n";
        assert_eq!(compute_migrated_content(content), content);
    }

    #[test]
    fn test_compute_migrated_content_does_not_rewrite_literal_text_when_other_template_uses_deprecated_var()
     {
        let content = "pre-merge = \"echo repo_root\"\npost-merge = \"echo {{ repo_root }}\"\n";
        let migrated = compute_migrated_content(content);
        assert_eq!(
            migrated,
            "pre-merge = \"echo repo_root\"\npost-merge = \"echo {{ repo_path }}\"\n"
        );
    }

    /// The `replace_deprecated_vars` helper must return the input untouched
    /// when it cannot be parsed as TOML, rather than panicking.
    #[test]
    fn test_replace_deprecated_vars_returns_input_on_parse_error() {
        let content = "this is = = not valid toml";
        assert_eq!(replace_deprecated_vars(content), content);
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
    fn test_normalize_does_not_rewrite_literal_text() {
        let template = "echo repo_root";
        let result = normalize_template_vars(template);
        assert!(matches!(result, Cow::Borrowed(_)), "Should not allocate");
        assert_eq!(result, template);
    }

    #[test]
    fn test_normalize_only_rewrites_template_identifiers() {
        let template = "echo repo_root && echo {{ repo_root }}";
        let result = normalize_template_vars(template);
        assert_eq!(result, "echo repo_root && echo {{ repo_path }}");
    }

    /// When `repo_root` is bound as a `{% set %}` local it is no longer the
    /// deprecated global, so minijinja reports no undeclared `repo_root` and
    /// the template is left untouched — the local name is not silently renamed.
    #[test]
    fn test_normalize_skips_set_assignment_target() {
        let template = "{% set repo_root = \"x\" %}{{ repo_root }}";
        let result = normalize_template_vars(template);
        assert!(matches!(result, Cow::Borrowed(_)), "Should not allocate");
        assert_eq!(result, template);
    }

    /// Identifiers inside `{# #}` comments must not be rewritten.
    #[test]
    fn test_normalize_skips_comment_tags() {
        let template = "{# repo_root #}{{ repo_root }}";
        let result = normalize_template_vars(template);
        assert_eq!(result, "{# repo_root #}{{ repo_path }}");
    }

    /// Identifiers inside `{% raw %}…{% endraw %}` blocks are verbatim text,
    /// not template references, so they must be left alone — only a genuine
    /// reference outside the raw block is rewritten.
    #[test]
    fn test_normalize_skips_raw_blocks() {
        let template = "{% raw %}{{ repo_root }}{% endraw %}{{ repo_root }}";
        let result = normalize_template_vars(template);
        assert_eq!(
            result,
            "{% raw %}{{ repo_root }}{% endraw %}{{ repo_path }}"
        );
    }

    /// A deprecated name that appears as a quoted string literal inside a tag
    /// is not an identifier and must not be rewritten.
    #[test]
    fn test_normalize_skips_string_literals_in_tags() {
        let template = "{{ \"repo_root\" }} {{ repo_root }}";
        let result = normalize_template_vars(template);
        assert_eq!(result, "{{ \"repo_root\" }} {{ repo_path }}");
    }

    /// A deprecated name used as an attribute (`obj.repo_root`) is a member of
    /// another value, not the deprecated global, so it must not be rewritten.
    #[test]
    fn test_normalize_skips_attribute_access() {
        let template = "{{ obj.repo_root }} {{ repo_root }}";
        let result = normalize_template_vars(template);
        assert_eq!(result, "{{ obj.repo_root }} {{ repo_path }}");
    }

    /// A bare `{` that does not open a tag is literal text; the scan steps past
    /// it and still rewrites a genuine reference later in the string.
    #[test]
    fn test_normalize_skips_bare_brace() {
        let template = "{ literal {{ repo_root }}";
        let result = normalize_template_vars(template);
        assert_eq!(result, "{ literal {{ repo_path }}");
    }

    /// A backslash-escaped quote inside an in-tag string literal does not end
    /// the literal early, so its contents are preserved verbatim.
    #[test]
    fn test_normalize_handles_escaped_quote_in_tag_string() {
        let template = "{{ \"a\\\"repo_root\" }} {{ repo_root }}";
        let result = normalize_template_vars(template);
        assert_eq!(result, "{{ \"a\\\"repo_root\" }} {{ repo_path }}");
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
        let result = migrate_content(content);
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
        let result = migrate_content(content);
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
        let result = migrate_content(content);
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
        let result = migrate_content(content);
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
        let step2 = migrate_content(&step1);
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
        let result = migrate_content(content);
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
        let result = migrate_content(content);
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
        let result = migrate_content(content);
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
        let result = migrate_content(content);
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
        let result = migrate_content(content);
        insta::assert_snapshot!(result, @r#"

        [commit.generation]
        command = "-m haiku"
        "#);
    }

    #[test]
    fn test_migrate_malformed_string_value_unchanged() {
        // When commit-generation is a string (malformed), migration must leave
        // it in place — silently dropping it would lose user config.
        let content = r#"
commit-generation = "not a table"
other = "value"
"#;
        let result = migrate_content(content);
        assert!(
            !result.contains("[commit.generation]"),
            "Should not create new section for malformed input"
        );
        assert!(
            result.contains("commit-generation = \"not a table\""),
            "Malformed value must be preserved; got: {result}"
        );
    }

    #[test]
    fn test_migrate_malformed_project_level_string_unchanged() {
        // When project-level commit-generation is a string, migration must
        // leave it in place rather than dropping it.
        let content = r#"
[projects."github.com/user/repo"]
commit-generation = "not a table"
other = "value"
"#;
        let result = migrate_content(content);
        assert!(
            !result.contains("[projects.\"github.com/user/repo\".commit.generation]"),
            "Should not create new section for malformed project-level input"
        );
        assert!(
            result.contains("commit-generation = \"not a table\""),
            "Malformed project-level value must be preserved; got: {result}"
        );
    }

    /// Malformed deprecated section + a valid sibling migration: the bug was
    /// that doc.remove() happened before the malformed-value check, so a
    /// sibling migration would serialize the doc with the section already
    /// dropped. The fix peeks before removing.
    #[test]
    fn test_malformed_section_preserved_with_sibling_migration() {
        let content = r#"commit-generation = "keep me"

[merge]
no-ff = true
"#;
        let result = migrate_content(content);
        assert!(
            result.contains(r#"commit-generation = "keep me""#),
            "Malformed commit-generation must survive sibling migrations; got:\n{result}"
        );
        // Sibling migration should still apply.
        assert!(
            result.contains("ff = false"),
            "merge.no-ff should have migrated to merge.ff = false; got:\n{result}"
        );
    }

    /// Same shape for [select]: a malformed select value next to a valid
    /// sibling migration must be preserved.
    #[test]
    fn test_malformed_select_preserved_with_sibling_migration() {
        let content = r#"select = "not a table"

[merge]
no-ff = true
"#;
        let result = migrate_content(content);
        assert!(
            result.contains(r#"select = "not a table""#),
            "Malformed select must survive sibling migrations; got:\n{result}"
        );
        assert!(
            result.contains("ff = false"),
            "merge.no-ff should have migrated; got:\n{result}"
        );
    }

    /// Scalar `commit = "x"` blocks `[commit.generation]` insertion. The
    /// migration must NOT remove `[commit-generation]` — doing so previously
    /// dropped the section since the new key could not be written under a
    /// scalar parent.
    #[test]
    fn test_commit_generation_preserved_when_commit_is_scalar() {
        let content = r#"commit = "x"

[commit-generation]
template = "tpl"

[merge]
no-ff = true
"#;
        let result = migrate_content(content);
        assert!(
            result.contains("[commit-generation]") && result.contains(r#"template = "tpl""#),
            "[commit-generation] must survive when scalar `commit` blocks the new key; got:\n{result}"
        );
        // Source must still be the scalar — nothing inserted under it.
        assert!(
            result.contains(r#"commit = "x""#),
            "scalar `commit` must be preserved unchanged; got:\n{result}"
        );
        // Sibling migration still applies.
        assert!(
            result.contains("ff = false"),
            "merge.no-ff should have migrated; got:\n{result}"
        );
    }

    #[test]
    fn test_commit_generation_migrates_when_commit_parent_is_inline_table() {
        let content = r#"commit = { stage = "tracked" }

[commit-generation]
command = "llm"
"#;
        let result = migrate_content(content);
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        let commit = doc["commit"].as_table().expect("commit table");
        assert_eq!(
            commit["stage"].as_str(),
            Some("tracked"),
            "inline parent fields must survive: {result}"
        );
        assert_eq!(
            commit["generation"]["command"].as_str(),
            Some("llm"),
            "deprecated section should move under commit.generation: {result}"
        );
        assert!(
            doc.get("commit-generation").is_none(),
            "old section should be removed after migration: {result}"
        );
    }

    #[test]
    fn test_project_commit_generation_migrates_when_commit_parent_is_inline_table() {
        let content = r#"
[projects."github.com/user/repo"]
commit = { stage = "tracked" }
commit-generation = { command = "llm" }
"#;
        let result = migrate_content(content);
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        let project = doc["projects"]["github.com/user/repo"]
            .as_table()
            .expect("project table");
        let commit = project["commit"].as_table().expect("project commit table");
        assert_eq!(
            commit["stage"].as_str(),
            Some("tracked"),
            "inline project parent fields must survive: {result}"
        );
        assert_eq!(
            commit["generation"]["command"].as_str(),
            Some("llm"),
            "project deprecated section should move under commit.generation: {result}"
        );
        assert!(
            project.get("commit-generation").is_none(),
            "old project section should be removed after migration: {result}"
        );
    }

    /// Same shape for `[select]` when `switch = "x"` is scalar.
    #[test]
    fn test_select_preserved_when_switch_is_scalar() {
        let content = r#"switch = "x"

[select]
preview = "p"

[merge]
no-ff = true
"#;
        let result = migrate_content(content);
        assert!(
            result.contains("[select]") && result.contains(r#"preview = "p""#),
            "[select] must survive when scalar `switch` blocks the new key; got:\n{result}"
        );
        assert!(
            result.contains(r#"switch = "x""#),
            "scalar `switch` must be preserved unchanged; got:\n{result}"
        );
        assert!(
            result.contains("ff = false"),
            "merge.no-ff should have migrated; got:\n{result}"
        );
    }

    /// `args = [1, "--ok"]`: a single non-string element would previously be
    /// filtered out while `args` was removed, dropping user data. The whole
    /// `args` array must be preserved unchanged when any element isn't a
    /// string.
    #[test]
    fn test_commit_generation_args_preserved_when_non_string_element() {
        let content = r#"[commit-generation]
command = "echo"
args = [1, "--ok"]
"#;
        let result = migrate_content(content);
        // Section migrated to [commit.generation], but `args` preserved
        // because it contains a non-string element.
        assert!(
            result.contains("[commit.generation]"),
            "section should still migrate; got:\n{result}"
        );
        assert!(
            result.contains("args = [1, \"--ok\"]") || result.contains("args = [ 1, \"--ok\" ]"),
            "args must be preserved unchanged when any element is non-string; got:\n{result}"
        );
        // command must NOT have been mutated to include the partial join.
        assert!(
            result.contains(r#"command = "echo""#),
            "command must not be mutated when args is preserved; got:\n{result}"
        );
    }

    /// `[ci]` migration only owns `platform`; other keys in the same section
    /// must be preserved, not dropped along with the section. The new
    /// `[forge]` lands directly after the surviving `[ci]` remainder, not at
    /// the end of the file.
    #[test]
    fn test_ci_migration_preserves_other_keys() {
        let content = r#"[ci]
platform = "github"
hostname = "ghe.example"

[merge]
ff = false
"#;
        let result = migrate_content(content);
        insta::assert_snapshot!(result, @r#"
        [ci]
        hostname = "ghe.example"

        [forge]
        platform = "github"

        [merge]
        ff = false
        "#);
    }

    /// The migrated `[forge]` takes over `[ci]`'s file position — and its
    /// decor (the comment above) when the section is fully consumed — instead
    /// of rendering as a fresh position-less table at the end of the file.
    /// Comments on the `platform` line itself survive the move too.
    #[test]
    fn test_ci_migration_keeps_section_position() {
        let content = r#"# which forge to talk to
[ci]
platform = "github" # not gitlab

[merge]
ff = false
"#;
        let result = migrate_content(content);
        insta::assert_snapshot!(result, @r#"
        # which forge to talk to
        [forge]
        platform = "github" # not gitlab

        [merge]
        ff = false
        "#);
    }

    #[test]
    fn test_migrate_invalid_toml_returns_unchanged() {
        // When content is not valid TOML, return it unchanged
        let content = "this is [not valid {toml";
        let result = migrate_content(content);
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
        let result = migrate_content(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    #[test]
    fn snapshot_migrate_commit_generation_with_args() {
        let content = r#"
[commit-generation]
command = "llm"
args = ["-m", "claude-haiku-4.5"]
"#;
        let result = migrate_content(content);
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
        let result = migrate_content(content);
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
        let result = migrate_content(content);
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
        let result = migrate_content(content);
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
        let result = migrate_content(content);
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
        assert!(has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::ApprovedCommands
        )));
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
            deprecations: vec![DeprecationKind::ApprovedCommands],
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

        let result =
            copy_approved_commands_to_approvals_file(&config_path).expect("copy should succeed");
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

        let result = copy_approved_commands_to_approvals_file(&config_path)
            .expect("skip should not surface error");
        assert!(result.is_none(), "Should skip when approvals.toml exists");

        // Verify existing file was not overwritten
        let existing = std::fs::read_to_string(&approvals_path).unwrap();
        assert_eq!(existing, "# existing approvals\n");
    }

    #[test]
    fn test_copy_approved_commands_errors_when_existing_approvals_invalid() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        let approvals_path = temp_dir.path().join("approvals.toml");
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = ["npm install"]
"#;
        std::fs::write(&config_path, content).unwrap();
        std::fs::write(&approvals_path, "this is = = not valid toml\n").unwrap();

        let result = copy_approved_commands_to_approvals_file(&config_path);
        assert!(
            result.is_err(),
            "Invalid existing approvals.toml must surface as Err; got {result:?}"
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Failed to parse existing approvals file"),
            "Error should identify the invalid approvals file"
        );
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

        let result = copy_approved_commands_to_approvals_file(&config_path)
            .expect("empty case should not surface error");
        assert!(
            result.is_none(),
            "Should skip when no approved-commands exist"
        );
    }

    /// Regression: when approvals.toml cannot be written (e.g. the directory
    /// is read-only), the copy must return Err rather than silently signaling
    /// "nothing to copy", otherwise the caller would proceed to rewrite
    /// config.toml and drop the legacy approvals.
    #[cfg(unix)]
    #[test]
    fn test_copy_approved_commands_surfaces_write_failure() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        let content = r#"
[projects."github.com/user/repo"]
approved-commands = ["npm install"]
"#;
        std::fs::write(&config_path, content).unwrap();

        // Make the directory read-only so approvals.toml creation fails.
        let mut perms = std::fs::metadata(temp_dir.path()).unwrap().permissions();
        perms.set_mode(0o555);
        std::fs::set_permissions(temp_dir.path(), perms).unwrap();

        // Root ignores directory permissions, so the write would succeed and
        // the assertion below would spuriously fail (Claude Code web, Docker).
        // Probe and skip when not actually restricted — matching the pattern
        // in tests/integration_tests/approval_save.rs.
        if std::fs::write(temp_dir.path().join("__probe"), "").is_ok() {
            let mut perms = std::fs::metadata(temp_dir.path()).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(temp_dir.path(), perms).unwrap();
            std::eprintln!("Skipping permission test - running with elevated privileges");
            return;
        }

        let result = copy_approved_commands_to_approvals_file(&config_path);

        // Restore writable perms so the tempdir can be cleaned up.
        let mut perms = std::fs::metadata(temp_dir.path()).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(temp_dir.path(), perms).unwrap();

        assert!(
            result.is_err(),
            "Write failure must surface as Err, not Ok(None); got {result:?}"
        );
    }

    /// Regression: when the source config cannot be read or parsed, the copy
    /// must surface the error (with context) rather than silently signaling
    /// "nothing to copy" — same data-loss class as the write-failure case.
    #[test]
    fn test_copy_approved_commands_surfaces_read_failure() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        std::fs::write(&config_path, "this is = = not valid toml\n").unwrap();

        let result = copy_approved_commands_to_approvals_file(&config_path);
        assert!(
            result.is_err(),
            "Unparsable source config must surface as Err; got {result:?}"
        );
    }

    #[test]
    fn test_set_implicit_suppresses_parent_header() {
        // Verifies that set_implicit(true) prevents an empty parent table from
        // rendering its own header. This is the key technique used in
        // ensure_standard_table_parent to avoid creating spurious [commit]
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
        let result = migrate_content(content);
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
    fn test_migrate_select_when_switch_parent_is_inline_table() {
        let content = r#"switch = { cd = false }

[select]
pager = "delta"
"#;
        let result = migrate_content(content);
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        let switch = doc["switch"].as_table().expect("switch table");
        assert_eq!(
            switch["cd"].as_bool(),
            Some(false),
            "inline switch fields must survive: {result}"
        );
        assert_eq!(
            switch["picker"]["pager"].as_str(),
            Some("delta"),
            "select should move under switch.picker: {result}"
        );
        assert!(
            doc.get("select").is_none(),
            "old select section should be removed after migration: {result}"
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
        let result = migrate_content(content);
        assert_eq!(
            result, content,
            "Should not migrate when new section exists"
        );
    }

    #[test]
    fn test_migrate_select_invalid_toml() {
        let content = "this is { not valid toml";
        let result = migrate_content(content);
        assert_eq!(result, content, "Invalid TOML should be returned unchanged");
    }

    #[test]
    fn test_migrate_select_no_select_section() {
        let content = r#"
[list]
full = true
"#;
        let result = migrate_content(content);
        assert_eq!(result, content, "No [select] section means no migration");
    }

    #[test]
    fn test_detect_deprecations_includes_select() {
        let content = r#"
[select]
pager = "delta"
"#;
        let deprecations = detect_deprecations(content);
        assert!(has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::Select
        )));
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
        let result = migrate_content(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    #[test]
    fn test_format_deprecation_details_select() {
        let content = r#"[select]
pager = "delta --paging=never"
"#;
        let info = DeprecationInfo {
            config_path: std::path::PathBuf::from("/tmp/test-config.toml"),
            deprecations: vec![DeprecationKind::Select],
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

    /// The silent create-hooks rule renames the deprecated `pre-create`/`post-create`
    /// keys to canonical `pre-start`/`post-start`, preserving the value shape
    /// (string, `[table]`, `[[array-of-tables]]`) at both the top level and
    /// inside `[projects."..."]`.
    #[test]
    fn test_migrate_create_hooks_renames_every_shape() {
        let content = r#"pre-create = "npm install"

[[post-create]]
lint = "cargo clippy"

[projects."my-project"]
pre-create = "cargo build"

[projects."my-project".post-create]
server = "npm run dev"
"#;
        let result = migrate_content(content);
        assert!(
            !result.contains("pre-create") && !result.contains("post-create"),
            "no deprecated key may remain; got:\n{result}"
        );
        assert!(
            result.contains(r#"pre-start = "npm install""#),
            "top-level string renamed; got:\n{result}"
        );
        assert!(
            result.contains("[[post-start]]"),
            "top-level array-of-tables renamed; got:\n{result}"
        );
        assert!(
            result.contains(r#"pre-start = "cargo build""#),
            "per-project string renamed; got:\n{result}"
        );
        assert!(
            result.contains(r#"[projects."my-project".post-start]"#),
            "per-project table renamed; got:\n{result}"
        );
    }

    /// When the canonical `-start` key already exists, the migrator leaves the
    /// deprecated `-create` key alone rather than clobbering the user's value.
    #[test]
    fn test_migrate_create_hooks_skips_when_start_exists() {
        let content = r#"pre-create = "old"
pre-start = "new"

[projects."my-project"]
post-create = "old"
post-start = "new"
"#;
        assert_eq!(
            migrate_content(content),
            content,
            "must not clobber an existing canonical key"
        );
    }

    #[test]
    fn test_migrate_create_hooks_invalid_toml() {
        let content = "this is { not valid toml";
        assert_eq!(migrate_content(content), content);
    }

    #[test]
    fn snapshot_migrate_create_to_start() {
        let content = r#"pre-create = "npm install"

[post-create]
server = "npm run dev"
"#;
        let migrated = compute_migrated_content(content);
        insta::assert_snapshot!(migration_diff(content, &migrated));
    }

    #[test]
    fn test_detect_switch_picker_timeout_top_level() {
        let content = r#"
[switch.picker]
pager = "delta"
timeout-ms = 500
"#;
        let deprecations = detect_deprecations(content);
        assert!(has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::SwitchPickerTimeout
        )));
        assert!(!deprecations.is_empty());
    }

    #[test]
    fn test_detect_switch_picker_timeout_project_level() {
        let content = r#"
[projects."github.com/user/repo".switch.picker]
timeout-ms = 300
"#;
        let deprecations = detect_deprecations(content);
        assert!(has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::SwitchPickerTimeout
        )));
    }

    #[test]
    fn test_detect_switch_picker_timeout_inline_table() {
        let content = r#"
[switch]
picker = { pager = "delta", timeout-ms = 500 }
"#;
        let deprecations = detect_deprecations(content);
        assert!(has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::SwitchPickerTimeout
        )));
    }

    #[test]
    fn test_migrate_switch_picker_timeout_inline_table() {
        let content = r#"
[switch]
picker = { pager = "delta", timeout-ms = 500 }
"#;
        let result = migrate_content(content);
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
        assert!(!has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::SwitchPickerTimeout
        )));
    }

    #[test]
    fn test_migrate_switch_picker_timeout_removes_key() {
        let content = r#"
[switch.picker]
pager = "delta"
timeout-ms = 500
"#;
        let result = migrate_content(content);
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
        let result = migrate_content(content);
        assert!(!result.contains("timeout-ms"));
        assert!(result.contains("pager"));
    }

    #[test]
    fn test_migrate_switch_picker_timeout_noop_when_absent() {
        let content = r#"
[switch.picker]
pager = "delta"
"#;
        let result = migrate_content(content);
        assert_eq!(result, content);
    }

    #[test]
    fn test_migrate_switch_picker_timeout_invalid_toml() {
        let content = "this is { not valid toml";
        let result = migrate_content(content);
        assert_eq!(result, content);
    }

    #[test]
    fn test_format_deprecation_warnings_switch_picker_timeout() {
        let info = DeprecationInfo {
            config_path: std::path::PathBuf::from("/tmp/test-config.toml"),
            deprecations: vec![DeprecationKind::SwitchPickerTimeout],
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

    // ==================== negated bool format + migration tests ====================

    #[test]
    fn test_format_deprecation_warnings_no_ff_and_no_cd() {
        let info = DeprecationInfo {
            config_path: std::path::PathBuf::from("/tmp/test-config.toml"),
            deprecations: vec![DeprecationKind::NoFf, DeprecationKind::NoCd],
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
        assert!(has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::NoFf
        )));
    }

    #[test]
    fn test_detect_no_ff_not_flagged_when_ff_exists() {
        let deprecations = detect_deprecations("[merge]\nff = true\nno-ff = true\n");
        assert!(!has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::NoFf
        )));
    }

    #[test]
    fn test_detect_no_cd_deprecation() {
        let deprecations = detect_deprecations("[switch]\nno-cd = true\n");
        assert!(has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::NoCd
        )));
    }

    #[test]
    fn test_detect_no_ff_project_level() {
        let content = r#"
[projects."github.com/user/repo".merge]
no-ff = true
"#;
        let deprecations = detect_deprecations(content);
        assert!(has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::NoFf
        )));
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
        assert!(has_kind(&deprecations, |k| matches!(
            k,
            DeprecationKind::Select
        )));
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
        use crate::config::{ProjectConfig, UnknownWarning, UserConfig, collect_unknown_warnings};

        // User-only commit-generation key in project config → nested redirect
        // to user config (the load path that `config show` mirrors).
        let warnings =
            collect_unknown_warnings::<ProjectConfig>("[commit-generation]\ncommand = \"llm\"\n");
        assert!(
            matches!(
                warnings.as_slice(),
                [UnknownWarning::NestedWrongConfig { path, other_description }]
                    if path == "commit.generation.command" && *other_description == "user config"
            ),
            "expected one NestedWrongConfig → user config, got {warnings:?}"
        );

        // ci in user config → top-level deprecated-section redirect.
        let warnings = collect_unknown_warnings::<UserConfig>("[ci]\nplatform = \"github\"\n");
        assert!(
            matches!(
                warnings.as_slice(),
                [UnknownWarning::TopLevelDeprecatedWrongConfig { other_description, .. }]
                    if *other_description == "project config"
            ),
            "expected one TopLevelDeprecatedWrongConfig → project config, got {warnings:?}"
        );

        // Exercise the stderr/dedup side-effect path itself.
        let path = std::env::temp_dir().join("test-deprecated-wrong-config-project.toml");
        warn_unknown_fields::<ProjectConfig>(
            "[commit-generation]\ncommand = \"llm\"\n",
            &path,
            "Project config",
        );
    }

    // ==================== pre-hook table form tests ====================

    fn find_pre_hook_table_form(content: &str) -> Vec<String> {
        detect_deprecations(content)
            .into_iter()
            .find_map(|k| match k {
                DeprecationKind::PreHookTableForm(found) => Some(found),
                _ => None,
            })
            .unwrap_or_default()
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

        // Inline table form → detected like section table form
        let found = find_pre_hook_table_form("pre-merge = { test = \"t\", lint = \"l\" }\n");
        assert_eq!(found, vec!["pre-merge"]);

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
        let result = migrate_content(content);
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
    fn test_migrate_pre_hook_inline_table_form_converts_to_pipeline() {
        let content = r#"pre-merge = { test = "cargo test", lint = "cargo clippy" }
"#;
        let result = migrate_content(content);
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        let arr = doc["pre-merge"]
            .as_array_of_tables()
            .expect("should be array of tables");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr.get(0).unwrap()["test"].as_str(), Some("cargo test"));
        assert_eq!(arr.get(1).unwrap()["lint"].as_str(), Some("cargo clippy"));
    }

    #[test]
    fn test_migrate_pre_hook_table_form_preserves_order() {
        let content = r#"
[pre-merge]
first = "1"
second = "2"
third = "3"
"#;
        let result = migrate_content(content);
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        let arr = doc["pre-merge"].as_array_of_tables().unwrap();
        let names: Vec<&str> = arr.iter().map(|t| t.iter().next().unwrap().0).collect();
        assert_eq!(names, vec!["first", "second", "third"]);
    }

    #[test]
    fn test_migrate_pre_hook_table_form_single_entry_untouched() {
        let content = "[pre-merge]\ntest = \"t\"\n";
        let result = migrate_content(content);
        assert_eq!(result, content, "Single-entry table should not be migrated");
    }

    #[test]
    fn test_migrate_pre_hook_table_form_per_project() {
        let content = r#"
[projects."web".pre-start]
install = "npm ci"
build = "npm run build"
"#;
        let result = migrate_content(content);
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
        // The pipeline migration only transforms pre-* hooks; post-start is a
        // post-* hook (table form is canonical there) and must pass through
        // untouched.
        let result = migrate_content(content);
        insta::assert_snapshot!(migration_diff(content, &result));
    }

    /// Every `DEPRECATION_RULES` row's migration fires on one config, pinning
    /// cross-rule interactions the per-rule tests can't see — in particular
    /// the inserted `[forge]` staying in the mid-file spot where the user
    /// wrote `[ci]` (it would render at the end without an explicit position;
    /// see [`migrate_ci_doc`]) and a `timeout-ms` under `[select]` being
    /// moved into `[switch.picker]` and then stripped.
    #[test]
    fn snapshot_migrate_all_rules_combined() {
        let content = r#"worktree-path = "../{{ repo_root }}.{{ branch }}"
pre-create = "npm install"

[pre-merge]
test = "cargo test"
lint = "cargo clippy"

[commit-generation]
command = "llm"
args = ["-m", "haiku"]

[ci]
platform = "github"

[select]
pager = "delta"
timeout-ms = 500

[merge]
no-ff = true

[switch]
no-cd = true

[post-create]
server = "npm run dev"

[projects."github.com/user/repo"]
approved-commands = ["npm test"]
"#;
        let migrated = compute_migrated_content(content);
        assert_eq!(
            compute_migrated_content(&migrated),
            migrated,
            "migration must be idempotent"
        );
        insta::assert_snapshot!(migration_diff(content, &migrated));
    }
}
