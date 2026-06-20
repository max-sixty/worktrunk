//! User-defined `wt list` columns from `[list.custom-columns]` in user config.
//!
//! Resolved once per invocation, then expanded eagerly for every row before
//! the table skeleton renders: the template inputs (branch, worktree
//! identity, per-branch vars from the bulk config snapshot) are all in memory
//! by then, so cells paint with the skeleton, widths are measured from
//! content like Branch/Path, and a column that renders empty for every row is
//! dropped by the regular empty-column penalty. Layout participation lives in
//! `columns.rs` / `layout.rs` via `ColumnKind::Custom`.
//!
//! Expansion stays off the skeleton's critical costs: one minijinja
//! environment and one parse per column per invocation, one vars conversion
//! per branch, zero subprocesses per cell.

use std::collections::{BTreeMap, HashMap};

use ansi_str::AnsiStr;
use minijinja::Value;
use worktrunk::config::{
    ListColumnConfig, template_environment, validate_list_column_template, vars_map_to_value,
};
use worktrunk::git::Repository;
use worktrunk::path::to_posix_path;

use super::model::ListItem;

/// Default maximum display width for a custom column.
const DEFAULT_MAX_WIDTH: usize = 40;

/// Default drop priority — the URL band (see `COLUMN_SPECS`).
const DEFAULT_PRIORITY: u8 = 9;

/// A custom column resolved from config, ready to expand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCustomColumn {
    /// Header text (the TOML key).
    pub name: String,
    /// minijinja template; the rendered result is the cell text.
    pub template: String,
    /// Maximum display width; longer values truncate.
    pub max_width: usize,
    /// Drop order when the terminal narrows (lower = kept longer).
    pub priority: u8,
}

/// Resolve `[list.custom-columns]` config into expansion-ready columns.
///
/// Validates each definition and orders the result by (priority, name),
/// which is also the display order among custom columns. Errors abort
/// `wt list` — column config is consumed only here, so a broken template
/// can't affect other commands. (The picker degrades instead; see the
/// resolution site in `collect()`.)
pub fn resolve_custom_columns(
    columns: &BTreeMap<String, ListColumnConfig>,
    repo: &Repository,
) -> anyhow::Result<Vec<ResolvedCustomColumn>> {
    // `ColumnKind::Custom(u8)` indexes the resolved list; more entries than
    // u8 can address would silently alias columns.
    anyhow::ensure!(
        columns.len() <= usize::from(u8::MAX) + 1,
        "[list.custom-columns] supports at most 256 columns ({} configured)",
        columns.len()
    );
    let mut resolved: Vec<ResolvedCustomColumn> = columns
        .iter()
        .map(|(name, config)| {
            anyhow::ensure!(
                !name.trim().is_empty() && !name.chars().any(char::is_control),
                "Invalid [list.custom-columns] name {name:?}: must be non-empty without control characters"
            );
            anyhow::ensure!(
                config.width != Some(0),
                "Invalid [list.custom-columns.{name}] width: must be at least 1"
            );
            validate_list_column_template(&config.template, repo, &format!("list.custom-columns.{name}"))?;
            Ok(ResolvedCustomColumn {
                name: name.clone(),
                template: config.template.clone(),
                max_width: config.width.unwrap_or(DEFAULT_MAX_WIDTH),
                priority: config.priority.unwrap_or(DEFAULT_PRIORITY),
            })
        })
        .collect::<anyhow::Result<_>>()?;
    resolved.sort_by(|a, b| (a.priority, &a.name).cmp(&(b.priority, &b.name)));
    Ok(resolved)
}

/// Expand every custom column for every item, populating
/// `ListItem::custom_values`.
///
/// `all_vars` is the pre-fetched vars map for all branches (one snapshot
/// read covers the whole table — no subprocess per cell). Render errors
/// produce an empty cell, logged at `-vv`: a vars key absent on this branch
/// is the expected sparse-column shape, and config-level typos were already
/// rejected at resolution.
pub fn expand_custom_columns(
    columns: &[ResolvedCustomColumn],
    items: &mut [ListItem],
    all_vars: &HashMap<String, BTreeMap<String, String>>,
    repo: &Repository,
) {
    let env = template_environment(repo);
    let names: Vec<String> = columns
        .iter()
        .map(|column| format!("list.custom-columns.{}", column.name))
        .collect();
    // Parse once per column; resolution already validated syntax, so a
    // failure here is unreachable in practice and renders empty cells.
    let templates: Vec<Option<minijinja::Template>> = columns
        .iter()
        .zip(&names)
        .map(
            |(column, name)| match env.template_from_named_str(name, &column.template) {
                Ok(template) => Some(template),
                Err(e) => {
                    log::debug!("[{name}] parse failed after validation: {e}");
                    None
                }
            },
        )
        .collect();

    // Convert each branch's vars to a template value once, not per cell
    let branch_values: HashMap<&str, Value> = all_vars
        .iter()
        .map(|(branch, entries)| (branch.as_str(), vars_map_to_value(entries)))
        .collect();
    let empty_vars = vars_map_to_value(&BTreeMap::new());

    // Worktree identity is only computed when a template references it:
    // `to_posix_path` shells out to cygpath per call on Windows (Git Bash),
    // which would put O(worktrees) subprocesses on the time-to-skeleton path
    // for columns that never mention the worktree.
    let needs_worktree = templates.iter().flatten().any(|template| {
        let referenced = template.undeclared_variables(false);
        referenced.contains("worktree_path") || referenced.contains("worktree_name")
    });

    for item in items {
        let (worktree_path, worktree_name) = if needs_worktree {
            item.worktree_data()
                .map(|data| {
                    let path = to_posix_path(&data.path.to_string_lossy());
                    let name = data
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    (path, name)
                })
                .unwrap_or_default()
        } else {
            Default::default()
        };
        let branch = item.branch.as_deref().unwrap_or("");

        let mut context: HashMap<String, Value> = HashMap::new();
        context.insert("branch".to_string(), Value::from(branch));
        context.insert("worktree_path".to_string(), Value::from(worktree_path));
        context.insert("worktree_name".to_string(), Value::from(worktree_name));
        // Always inject vars (empty for unknown branches) so
        // `{{ vars.key | default(...) }}` works in SemiStrict mode
        let vars = branch_values.get(branch).unwrap_or(&empty_vars).clone();
        context.insert("vars".to_string(), vars);
        let context = Value::from_object(context);

        item.custom_values = templates
            .iter()
            .zip(&names)
            .map(|(template, name)| {
                let Some(template) = template else {
                    return String::new();
                };
                match template.render(&context) {
                    Ok(value) => sanitize_cell(&value),
                    Err(e) => {
                        log::debug!("[{name}] render failed for row {branch:?}: {e}");
                        String::new()
                    }
                }
            })
            .collect();
    }
}

/// Flatten a rendered value to one trimmed line: ANSI escape sequences are
/// removed, newlines and tabs become spaces, other control characters drop.
fn sanitize_cell(value: &str) -> String {
    value
        .ansi_strip()
        .chars()
        .filter_map(|c| match c {
            '\n' | '\r' | '\t' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use worktrunk::testing::TestRepo;

    #[test]
    fn test_sanitize_cell() {
        assert_eq!(sanitize_cell("plain"), "plain");
        assert_eq!(sanitize_cell("  padded  "), "padded");
        assert_eq!(sanitize_cell("two\nlines\there"), "two lines here");
        assert_eq!(sanitize_cell("bell\u{7}gone"), "bellgone");
        assert_eq!(sanitize_cell("\u{1b}[31mred\u{1b}[0m"), "red");
        assert_eq!(sanitize_cell("\n\n"), "");
    }

    fn column_config(template: &str) -> ListColumnConfig {
        ListColumnConfig {
            template: template.to_string(),
            width: None,
            priority: None,
        }
    }

    #[test]
    fn test_resolve_orders_by_priority_then_name() {
        let test = TestRepo::new();
        let mut columns = BTreeMap::new();
        columns.insert("B".to_string(), column_config("{{ branch }}"));
        columns.insert(
            "A".to_string(),
            ListColumnConfig {
                template: "{{ branch }}".to_string(),
                width: Some(10),
                priority: Some(12),
            },
        );
        columns.insert(
            "C".to_string(),
            ListColumnConfig {
                template: "{{ branch }}".to_string(),
                width: None,
                priority: Some(3),
            },
        );

        let resolved = resolve_custom_columns(&columns, &test.repo).unwrap();
        let names: Vec<&str> = resolved.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["C", "B", "A"]);
        assert_eq!(resolved[0].priority, 3);
        assert_eq!(resolved[1].priority, DEFAULT_PRIORITY);
        assert_eq!(resolved[1].max_width, DEFAULT_MAX_WIDTH);
        assert_eq!(resolved[2].max_width, 10);
    }

    #[test]
    fn test_resolve_rejects_invalid_definitions() {
        let test = TestRepo::new();

        let mut blank_name = BTreeMap::new();
        blank_name.insert("  ".to_string(), column_config("x"));
        let err = resolve_custom_columns(&blank_name, &test.repo).unwrap_err();
        assert!(err.to_string().contains("name"), "got: {err}");

        let mut zero_width = BTreeMap::new();
        zero_width.insert(
            "Ok".to_string(),
            ListColumnConfig {
                template: "x".to_string(),
                width: Some(0),
                priority: None,
            },
        );
        let err = resolve_custom_columns(&zero_width, &test.repo).unwrap_err();
        assert!(err.to_string().contains("width"), "got: {err}");

        let mut unknown_var = BTreeMap::new();
        unknown_var.insert("Ok".to_string(), column_config("{{ nope }}"));
        let err = resolve_custom_columns(&unknown_var, &test.repo).unwrap_err();
        assert!(err.to_string().contains("nope"), "got: {err}");

        let too_many: BTreeMap<String, ListColumnConfig> = (0..=256)
            .map(|i| (format!("C{i:03}"), column_config("x")))
            .collect();
        let err = resolve_custom_columns(&too_many, &test.repo).unwrap_err();
        assert!(err.to_string().contains("256"), "got: {err}");
    }

    #[test]
    fn test_expand_custom_columns_per_row() {
        let test = TestRepo::new();
        let columns = vec![
            ResolvedCustomColumn {
                name: "Ticket".to_string(),
                template: "{{ vars.ticket }}".to_string(),
                max_width: 40,
                priority: 9,
            },
            ResolvedCustomColumn {
                name: "Port".to_string(),
                template: "{{ vars.config.port }}".to_string(),
                max_width: 40,
                priority: 9,
            },
            ResolvedCustomColumn {
                name: "Tag".to_string(),
                template: "{{ branch }}!".to_string(),
                max_width: 40,
                priority: 9,
            },
        ];
        let mut all_vars = HashMap::new();
        all_vars.insert(
            "feature".to_string(),
            BTreeMap::from([
                ("ticket".to_string(), "JIRA-9\nwrapped".to_string()),
                ("config".to_string(), r#"{"port": 8080}"#.to_string()),
            ]),
        );

        let mut items = vec![
            ListItem::new_branch("abc12345".to_string(), "feature".to_string()),
            ListItem::new_branch("abc12345".to_string(), "other".to_string()),
        ];
        expand_custom_columns(&columns, &mut items, &all_vars, &test.repo);

        // Vars-backed cells are sanitized to one line; nested JSON access
        // works; identity vars expand
        assert_eq!(
            items[0].custom_values,
            ["JIRA-9 wrapped", "8080", "feature!"]
        );
        // A branch without the vars keys renders empty cells, not errors
        assert_eq!(items[1].custom_values, ["", "", "other!"]);
    }
}
