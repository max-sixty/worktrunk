//! State management commands.
//!
//! Commands for getting, setting, and clearing stored state. State lives in
//! git config (under `worktrunk.*`) and in the `.git/wt/` directory tree.
//!
//! # `state get` ↔ `state clear` parity
//!
//! The aggregate `wt config state get` (`handle_state_show`) MUST surface every
//! category that the aggregate `wt config state clear` (`handle_state_clear_all`)
//! removes. A user should never be able to run `state clear` and have something
//! disappear that `state get` never mentioned.
//!
//! Categories covered by both paths:
//!
//! - Default branch cache (git config `worktrunk.default_branch.*`)
//! - Previous branch (git config `worktrunk.history`)
//! - Branch markers (git config `worktrunk.state.<branch>.marker`)
//! - Vars (git config `worktrunk.state.<branch>.vars.*`)
//! - CI status cache (`.git/wt/cache/ci-status/`)
//! - Git commands cache (`.git/wt/cache/{merge-tree-conflicts,is-ancestor,…}/`)
//! - Hints (git config `worktrunk.hints.*`)
//! - Logs (`.git/wt/logs/`)
//! - Trash (`.git/wt/trash/`)
//!
//! When adding a new category, update BOTH `handle_state_show` and
//! `handle_state_clear_all`, plus the `after_long_help` blocks for `state get`
//! and `state clear` in `src/cli/config.rs`, in the same change.
//!
//! # Log layout invariant
//!
//! Inside `wt_logs_dir()`, top-level *files* are shared logs (`commands.jsonl*`,
//! `trace.log`, `output.log`, `diagnostic.md`) and top-level *directories* are
//! per-branch log trees (`{branch}/{source|internal}/{hook-type}/{name}.log`).
//! Categorization
//! relies on this file-vs-directory distinction: new top-level shared entries
//! must remain files. If a future category needs multiple files, it should live
//! under a single reserved subdirectory rather than adding sibling top-level dirs.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context;
use color_print::cformat;
use path_slash::PathExt as _;
use worktrunk::config::config_path;
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    eprintln, format_heading, format_with_gutter, info_message, println, success_message,
    warning_message,
};

use crate::cli::{OutputFormat, SwitchFormat};
use worktrunk::utils::epoch_now;

use super::super::list::ci_status::{CachedCiStatus, CiBranchName};
use crate::display::format_relative_time_short;
use crate::help_pager::show_help_in_pager;

// ==================== Path Helpers ====================

/// Get the user config path, or error if it cannot be determined.
///
/// Delegates to `config_path()` so that `config create` and `config show`
/// resolve the same path that config loading uses — including any CLI
/// (`--config`) or environment variable (`WORKTRUNK_CONFIG_PATH`) overrides.
pub fn require_user_config_path() -> anyhow::Result<PathBuf> {
    config_path().context("Cannot determine config directory")
}

// ==================== Log Management ====================

/// Top-level files created by `-vv` under `wt_logs_dir()`.
const DIAGNOSTIC_FILES: &[&str] = &["trace.log", "output.log", "diagnostic.md"];

fn is_diagnostic_file(name: &str) -> bool {
    DIAGNOSTIC_FILES.contains(&name)
}

/// Check if a top-level file belongs to the command audit log (`.jsonl` / `.jsonl.old`).
fn is_command_log_file(name: &str) -> bool {
    name.ends_with(".jsonl") || name.ends_with(".jsonl.old")
}

/// A hook-output log file discovered by walking the per-branch subtree.
struct HookOutputEntry {
    /// Path relative to `wt_logs_dir()`, used for display and JSON output.
    /// Always forward-slashed for cross-platform stability.
    relative_display: String,
    metadata: std::fs::Metadata,
}

/// Walk every per-branch log file under `log_dir`.
///
/// Top-level *directories* are treated as branch dirs; each is walked
/// recursively for `.log` files. Non-directory top-level entries are ignored
/// (those belong to command audit / diagnostic categories).
///
/// Returns entries sorted by modification time (newest first), with name as a
/// tie-breaker for stable ordering.
fn walk_hook_output_files(log_dir: &Path) -> anyhow::Result<Vec<HookOutputEntry>> {
    let mut out = Vec::new();
    if !log_dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(log_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        walk_branch_dir(log_dir, &entry.path(), &mut out)?;
    }
    sort_hook_entries(&mut out);
    Ok(out)
}

/// Recursively collect `.log` files under a branch directory.
fn walk_branch_dir(
    log_dir: &Path,
    current: &Path,
    out: &mut Vec<HookOutputEntry>,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(current)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_dir() {
            walk_branch_dir(log_dir, &path, out)?;
        } else if file_type.is_file() && path.extension().and_then(|e| e.to_str()) == Some("log") {
            let metadata = entry.metadata()?;
            let relative = path.strip_prefix(log_dir).unwrap_or(&path);
            out.push(HookOutputEntry {
                relative_display: relative.to_slash_lossy().into_owned(),
                metadata,
            });
        }
    }
    Ok(())
}

/// Sort hook entries by mtime (newest first), then by relative path for stability.
fn sort_hook_entries(entries: &mut [HookOutputEntry]) {
    entries.sort_by(|a, b| {
        let a_time = a.metadata.modified().ok();
        let b_time = b.metadata.modified().ok();
        b_time
            .cmp(&a_time)
            .then_with(|| a.relative_display.cmp(&b.relative_display))
    });
}

/// A top-level entry staged under `wt_trash_dir()`.
///
/// Worktree removal renames directories into `.git/wt/trash/<name>-<timestamp>`
/// and a background `rm -rf` cleans them up; entries still present here are
/// awaiting (or escaped) that sweep.
struct TrashEntry {
    /// Filename, e.g. `myproject.feature-1234567890`.
    name: String,
    /// Absolute path, forward-slashed for cross-platform display.
    path: String,
    metadata: std::fs::Metadata,
}

/// List top-level entries under `wt_trash_dir()`.
///
/// Only the first level matters — each entry is one staged worktree (a
/// directory) or a stray file. Sorted by mtime (newest first) with name as
/// tie-breaker. Individual dirent/metadata failures are skipped: `state get`
/// is a read-only inspector and can race with the background `rm -rf`, so a
/// partial listing is more useful than a hard failure.
fn list_trash_entries(repo: &Repository) -> anyhow::Result<Vec<TrashEntry>> {
    let trash_dir = repo.wt_trash_dir();
    if !trash_dir.exists() {
        return Ok(Vec::new());
    }

    let mut out: Vec<TrashEntry> = std::fs::read_dir(&trash_dir)?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            Some(TrashEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: entry.path().to_slash_lossy().into_owned(),
                metadata,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        let a_time = a.metadata.modified().ok();
        let b_time = b.metadata.modified().ok();
        b_time.cmp(&a_time).then_with(|| a.name.cmp(&b.name))
    });
    Ok(out)
}

/// Clear stale entries from the wt/trash directory.
///
/// Worktree removal renames directories into `.git/wt/trash/` for instant UX,
/// then deletes them in a background process. If the background `rm -rf` fails
/// or is killed, entries accumulate. This cleans them up.
fn clear_trash(repo: &Repository) -> anyhow::Result<usize> {
    let trash_dir = repo.wt_trash_dir();

    if !trash_dir.exists() {
        return Ok(0);
    }

    let mut cleared = 0;
    for entry in std::fs::read_dir(&trash_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            std::fs::remove_dir_all(&path)?;
        } else {
            std::fs::remove_file(&path)?;
        }
        cleared += 1;
    }

    // Remove the trash directory itself if empty
    if std::fs::read_dir(&trash_dir)?.next().is_none() {
        let _ = std::fs::remove_dir(&trash_dir);
    }

    Ok(cleared)
}

/// Count `.log` files recursively under `dir`.
///
/// Used by `clear_logs` to report how many logs are being swept when it
/// removes a whole branch subtree with `remove_dir_all`.
fn count_log_files_recursive(dir: &Path) -> anyhow::Result<usize> {
    let mut count = 0;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_dir() {
            count += count_log_files_recursive(&path)?;
        } else if file_type.is_file() && path.extension().and_then(|e| e.to_str()) == Some("log") {
            count += 1;
        }
    }
    Ok(count)
}

/// Clear all log files from the wt/logs directory.
///
/// Walks the two layers of log storage:
///
/// 1. **Top-level files**: `commands.jsonl*`, `trace.log`, `output.log`, `diagnostic.md`.
///    Also sweeps any legacy flat `.log` files left over from the pre-nested
///    layout so the transition is self-healing (no explicit migrator).
/// 2. **Top-level directories**: per-branch log trees — counted recursively
///    and removed with `remove_dir_all`.
fn clear_logs(repo: &Repository) -> anyhow::Result<usize> {
    let log_dir = repo.wt_logs_dir();

    if !log_dir.exists() {
        return Ok(0);
    }

    let mut cleared = 0;
    for entry in std::fs::read_dir(&log_dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            // Branch subtree — count logs within, then nuke the whole subtree.
            cleared += count_log_files_recursive(&path)?;
            std::fs::remove_dir_all(&path)?;
        } else if file_type.is_file() {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            // Known shared files + legacy flat `.log` files from the old layout.
            if is_command_log_file(name) || is_diagnostic_file(name) || name.ends_with(".log") {
                std::fs::remove_file(&path)?;
                cleared += 1;
            }
        }
    }

    // Remove the directory if empty
    if std::fs::read_dir(&log_dir)?.next().is_none() {
        let _ = std::fs::remove_dir(&log_dir);
    }

    Ok(cleared)
}

/// A row ready to render in the log listing table or emit as JSON.
struct LogRow {
    /// Path relative to `wt_logs_dir()` (forward-slashed), for compact display.
    /// For top-level shared files this is just the filename.
    display_name: String,
    /// Absolute path (forward-slashed), for consumers that want to open the file directly.
    path: String,
    size: u64,
    modified_at: Option<u64>,
    /// Structured hook-output segments — present for entries under branch subtrees,
    /// absent for shared top-level files (command log, diagnostic).
    hook_structure: Option<HookStructure>,
}

/// Structured view of a hook-output log path. Values are the on-disk (sanitized)
/// names, so filters like `select(.source == "user")` work without splitting
/// the relative path on `/`.
struct HookStructure {
    /// First path segment — sanitized branch directory (may include a short
    /// collision-avoidance hash).
    branch: String,
    /// `"user"`, `"project"`, or `"internal"`.
    source: String,
    /// Hook type (`post-start`, `post-switch`, …) for user/project hooks;
    /// `None` for internal operations.
    hook_type: Option<String>,
    /// Sanitized hook name for user/project hooks; internal op name
    /// (e.g., `"remove"`) for internal entries.
    name: String,
}

impl LogRow {
    fn to_json(&self) -> serde_json::Value {
        let mut obj = serde_json::json!({
            "file": self.display_name,
            "path": self.path,
            "size": self.size,
            "modified_at": self.modified_at,
        });
        if let Some(s) = &self.hook_structure {
            let map = obj.as_object_mut().expect("json! produced an object");
            map.insert("branch".into(), s.branch.clone().into());
            map.insert("source".into(), s.source.clone().into());
            map.insert(
                "hook_type".into(),
                s.hook_type
                    .clone()
                    .map_or(serde_json::Value::Null, Into::into),
            );
            map.insert("name".into(), s.name.clone().into());
        }
        obj
    }
}

/// Build a `LogRow` for a top-level shared file.
fn top_level_log_row(entry: &std::fs::DirEntry) -> LogRow {
    let name = entry.file_name().to_string_lossy().into_owned();
    let path = entry.path().to_slash_lossy().into_owned();
    let meta = entry.metadata().ok();
    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let modified_at = meta
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    LogRow {
        display_name: name,
        path,
        size,
        modified_at,
        hook_structure: None,
    }
}

/// Build a `LogRow` for a hook-output file (display uses relative path).
fn hook_output_log_row(log_dir: &Path, entry: &HookOutputEntry) -> LogRow {
    let size = entry.metadata.len();
    let modified_at = entry
        .metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    let path = log_dir
        .join(&entry.relative_display)
        .to_slash_lossy()
        .into_owned();
    LogRow {
        display_name: entry.relative_display.clone(),
        path,
        size,
        modified_at,
        hook_structure: parse_hook_structure(&entry.relative_display),
    }
}

/// Parse a hook-output relative path into its structured segments.
///
/// Expected layouts (enforced by the writers in `commands/process.rs`):
/// - `{branch}/{source}/{hook_type}/{name}.log` — user/project hooks
/// - `{branch}/internal/{op}.log` — internal operations
///
/// Unknown layouts (legacy flat logs, future shapes) return `None` so the
/// entry still appears in the listing, just without structured filtering.
fn parse_hook_structure(relative: &str) -> Option<HookStructure> {
    let parts: Vec<&str> = relative.split('/').collect();
    match parts.as_slice() {
        [branch, "internal", op_log] => Some(HookStructure {
            branch: (*branch).to_string(),
            source: "internal".to_string(),
            hook_type: None,
            name: op_log.strip_suffix(".log").unwrap_or(op_log).to_string(),
        }),
        [branch, source, hook_type, name_log] => Some(HookStructure {
            branch: (*branch).to_string(),
            source: (*source).to_string(),
            hook_type: Some((*hook_type).to_string()),
            name: name_log
                .strip_suffix(".log")
                .unwrap_or(name_log)
                .to_string(),
        }),
        _ => None,
    }
}

/// Read and partition log files into command log, hook output, and diagnostic categories.
///
/// Top-level files are classified by name; directories under `log_dir` are
/// walked as branch subtrees to collect hook output. All three categories are
/// sorted by modification time (newest first) with a stable tie-breaker.
fn partition_log_files_json(
    repo: &Repository,
) -> anyhow::Result<(
    Vec<serde_json::Value>,
    Vec<serde_json::Value>,
    Vec<serde_json::Value>,
)> {
    let log_dir = repo.wt_logs_dir();
    if !log_dir.exists() {
        return Ok((vec![], vec![], vec![]));
    }

    let mut cmd_rows = Vec::new();
    let mut diagnostic_rows = Vec::new();
    for entry in std::fs::read_dir(&log_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_command_log_file(&name) {
            cmd_rows.push(top_level_log_row(&entry));
        } else if is_diagnostic_file(&name) {
            diagnostic_rows.push(top_level_log_row(&entry));
        }
    }
    sort_log_rows(&mut cmd_rows);
    sort_log_rows(&mut diagnostic_rows);

    // Hook output comes from walking the branch subtrees.
    let hook_rows: Vec<LogRow> = walk_hook_output_files(&log_dir)?
        .iter()
        .map(|e| hook_output_log_row(&log_dir, e))
        .collect();

    Ok((
        cmd_rows.iter().map(LogRow::to_json).collect(),
        hook_rows.iter().map(LogRow::to_json).collect(),
        diagnostic_rows.iter().map(LogRow::to_json).collect(),
    ))
}

/// Sort log rows by mtime (newest first), stable on display name.
fn sort_log_rows(rows: &mut [LogRow]) {
    rows.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| a.display_name.cmp(&b.display_name))
    });
}

/// Render a table of log rows, or "(none)" if empty.
fn render_log_table(out: &mut String, rows: &[LogRow]) -> std::fmt::Result {
    if rows.is_empty() {
        writeln!(out, "{}", format_with_gutter("(none)", None))?;
        return Ok(());
    }

    let table_rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let size_str = if row.size < 1024 {
                format!("{}B", row.size)
            } else {
                format!("{}K", row.size / 1024)
            };
            let age = row
                .modified_at
                .map(|secs| format_relative_time_short(secs as i64))
                .unwrap_or_else(|| "?".to_string());
            vec![row.display_name.clone(), size_str, age]
        })
        .collect();

    let rendered = crate::md_help::render_data_table(&["File", "Size", "Age"], &table_rows);
    writeln!(out, "{}", rendered.trim_end())?;

    Ok(())
}

/// Render a section heading and the `(none)` placeholder if the log dir is missing.
fn render_log_heading(out: &mut String, log_dir: &Path, heading: &str) -> std::fmt::Result {
    let log_dir_display = format_path_for_display(log_dir);
    writeln!(
        out,
        "{}",
        format_heading(heading, Some(&format!("@ {log_dir_display}")))
    )
}

/// Render the command-log or diagnostic section: top-level files filtered by name.
fn render_top_level_section(
    out: &mut String,
    repo: &Repository,
    heading: &str,
    filter: impl Fn(&str) -> bool,
) -> anyhow::Result<()> {
    let log_dir = repo.wt_logs_dir();
    render_log_heading(out, &log_dir, heading)?;
    if !log_dir.exists() {
        writeln!(out, "{}", format_with_gutter("(none)", None))?;
        return Ok(());
    }

    let mut rows: Vec<LogRow> = std::fs::read_dir(&log_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|ft| ft.is_file()).unwrap_or(false))
        .filter(|e| filter(&e.file_name().to_string_lossy()))
        .map(|e| top_level_log_row(&e))
        .collect();
    sort_log_rows(&mut rows);
    render_log_table(out, &rows)?;
    Ok(())
}

/// Render the hook-output section: walk per-branch subtrees.
fn render_hook_output_section(out: &mut String, repo: &Repository) -> anyhow::Result<()> {
    let log_dir = repo.wt_logs_dir();
    render_log_heading(out, &log_dir, "HOOK OUTPUT")?;
    if !log_dir.exists() {
        writeln!(out, "{}", format_with_gutter("(none)", None))?;
        return Ok(());
    }

    let rows: Vec<LogRow> = walk_hook_output_files(&log_dir)?
        .iter()
        .map(|e| hook_output_log_row(&log_dir, e))
        .collect();
    render_log_table(out, &rows)?;
    Ok(())
}

/// Render all three log sections (command log, hook output, diagnostic) into a buffer.
pub(super) fn render_all_log_sections(out: &mut String, repo: &Repository) -> anyhow::Result<()> {
    render_top_level_section(out, repo, "COMMAND LOG", is_command_log_file)?;
    writeln!(out)?;
    render_hook_output_section(out, repo)?;
    writeln!(out)?;
    render_top_level_section(out, repo, "DIAGNOSTIC", is_diagnostic_file)?;
    Ok(())
}

// ==================== Logs List Command ====================

/// List all log files — command log, hook output, and diagnostics.
///
/// JSON output emits three arrays keyed by category, each entry carrying
/// `file`, `path`, `size`, and `modified_at`. Hook-output entries additionally
/// expose `branch`, `source`, `hook_type`, and `name` so consumers can filter
/// with `jq` rather than parsing the slash-delimited `file` path.
pub fn handle_logs_list(format: SwitchFormat) -> anyhow::Result<()> {
    let repo = Repository::current()?;

    if format == SwitchFormat::Json {
        let (command_log, hook_output, diagnostic) = partition_log_files_json(&repo)?;
        let output = serde_json::json!({
            "command_log": command_log,
            "hook_output": hook_output,
            "diagnostic": diagnostic,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    let mut out = String::new();
    render_all_log_sections(&mut out, &repo)?;

    // Display through pager; fall back to direct stdout if pager unavailable
    // (matches #2155 routing --help output to stdout).
    if show_help_in_pager(&out, true).is_err() {
        println!("{}", out);
    }
    Ok(())
}

// ==================== State Get/Set/Clear Commands ====================

/// Handle the state get command
pub fn handle_state_get(
    key: &str,
    branch: Option<String>,
    format: SwitchFormat,
) -> anyhow::Result<()> {
    use super::super::list::ci_status::PrStatus;

    let repo = Repository::current()?;

    match key {
        "default-branch" => {
            let branch_name = repo.default_branch().ok_or_else(|| {
                anyhow::anyhow!(cformat!(
                    "Cannot determine default branch. To configure, run <bold>wt config state default-branch set BRANCH</>"
                ))
            })?;
            println!("{branch_name}");
        }
        "previous-branch" => match repo.switch_previous() {
            Some(prev) => println!("{prev}"),
            None => println!(""),
        },
        "marker" => {
            let branch_name = match branch {
                Some(b) => b,
                None => repo.require_current_branch("get marker for current branch")?,
            };
            if format == SwitchFormat::Json {
                // Read raw config to get both marker and set_at
                let config_key = format!("worktrunk.state.{branch_name}.marker");
                let raw = repo
                    .config_value(&config_key)
                    .ok()
                    .flatten()
                    .filter(|s| !s.is_empty());
                let output = match raw {
                    Some(json_str) => {
                        let parsed: serde_json::Value =
                            serde_json::from_str(&json_str).unwrap_or_default();
                        serde_json::json!({
                            "branch": branch_name,
                            "marker": parsed.get("marker").and_then(|v| v.as_str()),
                            "set_at": parsed.get("set_at").and_then(|v| v.as_u64()),
                        })
                    }
                    None => serde_json::json!(null),
                };
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                match repo.branch_marker(&branch_name) {
                    Some(marker) => println!("{marker}"),
                    None => println!(""),
                }
            }
        }
        "ci-status" => {
            let branch_name = match branch {
                Some(b) => b,
                None => repo.require_current_branch("get ci-status for current branch")?,
            };

            // Determine if this is a remote ref by checking git refs directly.
            // This is authoritative - we check actual refs, not guessing from name.
            let is_remote = repo
                .run_command(&[
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/remotes/{}", branch_name),
                ])
                .is_ok();

            // Get the HEAD commit for this branch
            let head = repo
                .run_command(&["rev-parse", &branch_name])
                .map(|s| s.trim().to_string())
                .unwrap_or_default();

            if head.is_empty() {
                return Err(worktrunk::git::GitError::BranchNotFound {
                    branch: branch_name,
                    show_create_hint: true,
                    last_fetch_ago: None,
                }
                .into());
            }

            let ci_branch = CiBranchName::from_branch_ref(&branch_name, is_remote);
            let pr_status = PrStatus::detect(&repo, &ci_branch, &head);

            if format == SwitchFormat::Json {
                let output = pr_status
                    .as_ref()
                    .map(super::super::list::json_output::JsonCi::from);
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                let ci_status = pr_status
                    .map_or(super::super::list::ci_status::CiStatus::NoCI, |s| {
                        s.ci_status
                    });
                let status_str: &'static str = ci_status.into();
                println!("{status_str}");
            }
        }
        _ => {
            anyhow::bail!(
                "Unknown key: {key}. Valid keys: default-branch, previous-branch, ci-status, marker, logs"
            )
        }
    }

    Ok(())
}

/// Handle the state set command
pub fn handle_state_set(key: &str, value: String, branch: Option<String>) -> anyhow::Result<()> {
    let repo = Repository::current()?;

    match key {
        "default-branch" => {
            // Warn if the branch doesn't exist locally
            if !repo.branch(&value).exists_locally()? {
                eprintln!(
                    "{}",
                    warning_message(cformat!("Branch <bold>{value}</> does not exist locally"))
                );
            }
            repo.set_default_branch(&value)?;
            eprintln!(
                "{}",
                success_message(cformat!("Set default branch to <bold>{value}</>"))
            );
        }
        "previous-branch" => {
            repo.set_switch_previous(Some(&value))?;
            eprintln!(
                "{}",
                success_message(cformat!("Set previous branch to <bold>{value}</>"))
            );
        }
        "marker" => {
            let branch_name = match branch {
                Some(b) => b,
                None => repo.require_current_branch("set marker for current branch")?,
            };

            // Store as JSON with timestamp
            let now = epoch_now();
            let json = serde_json::json!({
                "marker": value,
                "set_at": now
            });

            let config_key = format!("worktrunk.state.{branch_name}.marker");
            repo.set_config(&config_key, &json.to_string())?;

            eprintln!(
                "{}",
                success_message(cformat!(
                    "Set marker for <bold>{branch_name}</> to <bold>{value}</>"
                ))
            );
        }
        _ => {
            anyhow::bail!("Unknown key: {key}. Valid keys: default-branch, previous-branch, marker")
        }
    }

    Ok(())
}

/// Handle the state clear command
pub fn handle_state_clear(key: &str, branch: Option<String>, all: bool) -> anyhow::Result<()> {
    let repo = Repository::current()?;

    match key {
        "default-branch" => {
            if repo.clear_default_branch_cache()? {
                eprintln!("{}", success_message("Cleared default branch cache"));
            } else {
                eprintln!("{}", info_message("No default branch cache to clear"));
            }
        }
        "previous-branch" => {
            if repo.unset_config("worktrunk.history").unwrap_or(false) {
                eprintln!("{}", success_message("Cleared previous branch"));
            } else {
                eprintln!("{}", info_message("No previous branch to clear"));
            }
        }
        "ci-status" => {
            if all {
                let cleared = CachedCiStatus::clear_all(&repo);
                if cleared == 0 {
                    eprintln!("{}", info_message("No CI cache entries to clear"));
                } else {
                    eprintln!(
                        "{}",
                        success_message(cformat!(
                            "Cleared <bold>{cleared}</> CI cache entr{}",
                            if cleared == 1 { "y" } else { "ies" }
                        ))
                    );
                }
            } else {
                // Clear CI status for specific branch
                let branch_name = match branch {
                    Some(b) => b,
                    None => repo.require_current_branch("clear ci-status for current branch")?,
                };
                let config_key = format!("worktrunk.state.{branch_name}.ci-status");
                if repo.unset_config(&config_key).unwrap_or(false) {
                    eprintln!(
                        "{}",
                        success_message(cformat!("Cleared CI cache for <bold>{branch_name}</>"))
                    );
                } else {
                    eprintln!(
                        "{}",
                        info_message(cformat!("No CI cache for <bold>{branch_name}</>"))
                    );
                }
            }
        }
        "marker" => {
            if all {
                let output = repo
                    .run_command(&["config", "--get-regexp", r"^worktrunk\.state\..+\.marker$"])
                    .unwrap_or_default();

                let mut cleared_count = 0;
                for line in output.lines() {
                    if let Some(config_key) = line.split_whitespace().next() {
                        repo.unset_config(config_key)?;
                        cleared_count += 1;
                    }
                }

                if cleared_count == 0 {
                    eprintln!("{}", info_message("No markers to clear"));
                } else {
                    eprintln!(
                        "{}",
                        success_message(cformat!(
                            "Cleared <bold>{cleared_count}</> marker{}",
                            if cleared_count == 1 { "" } else { "s" }
                        ))
                    );
                }
            } else {
                let branch_name = match branch {
                    Some(b) => b,
                    None => repo.require_current_branch("clear marker for current branch")?,
                };

                let config_key = format!("worktrunk.state.{branch_name}.marker");
                if repo.unset_config(&config_key).unwrap_or(false) {
                    eprintln!(
                        "{}",
                        success_message(cformat!("Cleared marker for <bold>{branch_name}</>"))
                    );
                } else {
                    eprintln!(
                        "{}",
                        info_message(cformat!("No marker set for <bold>{branch_name}</>"))
                    );
                }
            }
        }
        "logs" => {
            let cleared = clear_logs(&repo)?;
            if cleared == 0 {
                eprintln!("{}", info_message("No logs to clear"));
            } else {
                eprintln!(
                    "{}",
                    success_message(cformat!(
                        "Cleared <bold>{cleared}</> log file{}",
                        if cleared == 1 { "" } else { "s" }
                    ))
                );
            }
        }
        _ => {
            anyhow::bail!(
                "Unknown key: {key}. Valid keys: default-branch, previous-branch, ci-status, marker, logs"
            )
        }
    }

    Ok(())
}

/// Handle the state clear all command
pub fn handle_state_clear_all() -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let mut cleared_any = false;

    // Clear default branch cache
    if matches!(repo.clear_default_branch_cache(), Ok(true)) {
        eprintln!("{}", success_message("Cleared default branch cache"));
        cleared_any = true;
    }

    // Clear previous branch
    if repo.unset_config("worktrunk.history").unwrap_or(false) {
        eprintln!("{}", success_message("Cleared previous branch"));
        cleared_any = true;
    }

    // Clear all markers
    let markers_output = repo
        .run_command(&["config", "--get-regexp", r"^worktrunk\.state\..+\.marker$"])
        .unwrap_or_default();
    let mut markers_cleared = 0;
    for line in markers_output.lines() {
        if let Some(config_key) = line.split_whitespace().next() {
            let _ = repo.unset_config(config_key);
            markers_cleared += 1;
        }
    }
    if markers_cleared > 0 {
        eprintln!(
            "{}",
            success_message(cformat!(
                "Cleared <bold>{markers_cleared}</> marker{}",
                if markers_cleared == 1 { "" } else { "s" }
            ))
        );
        cleared_any = true;
    }

    // Clear all CI status cache
    let ci_cleared = CachedCiStatus::clear_all(&repo);
    if ci_cleared > 0 {
        eprintln!(
            "{}",
            success_message(cformat!(
                "Cleared <bold>{ci_cleared}</> CI cache entr{}",
                if ci_cleared == 1 { "y" } else { "ies" }
            ))
        );
        cleared_any = true;
    }

    // Clear git commands cache (merge-tree, ancestry, diff results)
    let sha_cleared = repo.clear_git_commands_cache();
    if sha_cleared > 0 {
        eprintln!(
            "{}",
            success_message(cformat!(
                "Cleared <bold>{sha_cleared}</> git commands cache entr{}",
                if sha_cleared == 1 { "y" } else { "ies" }
            ))
        );
        cleared_any = true;
    }

    // Clear all vars data
    let vars_cleared = clear_all_vars(&repo)?;
    if vars_cleared > 0 {
        eprintln!(
            "{}",
            success_message(cformat!(
                "Cleared <bold>{vars_cleared}</> variable{}",
                if vars_cleared == 1 { "" } else { "s" }
            ))
        );
        cleared_any = true;
    }

    // Clear all logs
    let logs_cleared = clear_logs(&repo)?;
    if logs_cleared > 0 {
        eprintln!(
            "{}",
            success_message(cformat!(
                "Cleared <bold>{logs_cleared}</> log file{}",
                if logs_cleared == 1 { "" } else { "s" }
            ))
        );
        cleared_any = true;
    }

    // Clear all hints
    let hints_cleared = repo.clear_all_hints()?;
    if hints_cleared > 0 {
        eprintln!(
            "{}",
            success_message(cformat!(
                "Cleared <bold>{hints_cleared}</> hint{}",
                if hints_cleared == 1 { "" } else { "s" }
            ))
        );
        cleared_any = true;
    }

    // Clear stale trash from worktree removal
    let trash_cleared = clear_trash(&repo)?;
    if trash_cleared > 0 {
        eprintln!(
            "{}",
            success_message(cformat!(
                "Cleared <bold>{trash_cleared}</> trash entr{}",
                if trash_cleared == 1 { "y" } else { "ies" }
            ))
        );
        cleared_any = true;
    }

    if !cleared_any {
        eprintln!("{}", info_message("No stored state to clear"));
    }

    Ok(())
}

// ==================== State Show Commands ====================

/// Handle the state get command (shows all state)
pub fn handle_state_show(format: OutputFormat) -> anyhow::Result<()> {
    let repo = Repository::current()?;

    match format {
        OutputFormat::Json => handle_state_show_json(&repo),
        OutputFormat::Table | OutputFormat::ClaudeCode => handle_state_show_table(&repo),
    }
}

/// Output state as JSON
fn handle_state_show_json(repo: &Repository) -> anyhow::Result<()> {
    // Get default branch
    let default_branch = repo.default_branch();

    // Get previous branch
    let previous_branch = repo.switch_previous();

    // Get markers
    let markers: Vec<serde_json::Value> = all_markers(repo)
        .into_iter()
        .map(|m| {
            serde_json::json!({
                "branch": m.branch,
                "marker": m.marker,
                "set_at": if m.set_at > 0 { Some(m.set_at) } else { None }
            })
        })
        .collect();

    // Get CI status cache
    let mut ci_entries = CachedCiStatus::list_all(repo);
    ci_entries.sort_by(|a, b| {
        b.1.checked_at
            .cmp(&a.1.checked_at)
            .then_with(|| a.0.cmp(&b.0))
    });
    let ci_status: Vec<serde_json::Value> = ci_entries
        .into_iter()
        .map(|(branch, cached)| {
            let status = cached
                .status
                .as_ref()
                .map(|s| -> &'static str { s.ci_status.into() });
            serde_json::json!({
                "branch": branch,
                "status": status,
                "checked_at": cached.checked_at,
                "head": cached.head
            })
        })
        .collect();

    let (command_log, hook_output, diagnostic) = partition_log_files_json(repo)?;

    // Get vars data (all branches) — collect into BTreeMap for sorted output
    let all_vars: std::collections::BTreeMap<_, _> = repo.all_vars_entries().into_iter().collect();
    let vars_data: Vec<serde_json::Value> = all_vars
        .into_iter()
        .flat_map(|(branch, entries)| {
            entries.into_iter().map(move |(key, value)| {
                serde_json::json!({
                    "branch": branch,
                    "key": key,
                    "value": value
                })
            })
        })
        .collect();

    // Get hints
    let hints = repo.list_shown_hints();

    // Get trash entries
    let trash: Vec<serde_json::Value> = list_trash_entries(repo)?
        .iter()
        .map(|e| {
            let modified_at = e
                .metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs());
            serde_json::json!({
                "name": e.name,
                "path": e.path,
                "modified_at": modified_at,
            })
        })
        .collect();

    let output = serde_json::json!({
        "default_branch": default_branch,
        "previous_branch": previous_branch,
        "markers": markers,
        "ci_status": ci_status,
        "git_commands_cache": repo.git_commands_cache_count(),
        "vars": vars_data,
        "command_log": command_log,
        "hook_output": hook_output,
        "diagnostic": diagnostic,
        "hints": hints,
        "trash": trash,
    });

    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

/// Output state as human-readable table
fn handle_state_show_table(repo: &Repository) -> anyhow::Result<()> {
    // Build complete output as a string
    let mut out = String::new();

    // Show default branch cache
    writeln!(out, "{}", format_heading("DEFAULT BRANCH", None))?;
    match repo.default_branch() {
        Some(branch) => writeln!(out, "{}", format_with_gutter(&branch, None))?,
        None => writeln!(out, "{}", format_with_gutter("(not available)", None))?,
    }
    writeln!(out)?;

    // Show previous branch (for `wt switch -`)
    writeln!(out, "{}", format_heading("PREVIOUS BRANCH", None))?;
    match repo.switch_previous() {
        Some(prev) => writeln!(out, "{}", format_with_gutter(&prev, None))?,
        None => writeln!(out, "{}", format_with_gutter("(none)", None))?,
    }
    writeln!(out)?;

    // Show branch markers
    writeln!(out, "{}", format_heading("BRANCH MARKERS", None))?;
    let markers = all_markers(repo);
    if markers.is_empty() {
        writeln!(out, "{}", format_with_gutter("(none)", None))?;
    } else {
        let rows: Vec<Vec<String>> = markers
            .iter()
            .map(|entry| {
                let age = format_relative_time_short(entry.set_at as i64);
                vec![entry.branch.clone(), entry.marker.clone(), age]
            })
            .collect();
        let rendered = crate::md_help::render_data_table(&["Branch", "Marker", "Age"], &rows);
        writeln!(out, "{}", rendered.trim_end())?;
    }
    writeln!(out)?;

    // Show vars data
    writeln!(out, "{}", format_heading("VARS", None))?;
    let all_vars: std::collections::BTreeMap<_, _> = repo.all_vars_entries().into_iter().collect();

    if all_vars.is_empty() {
        writeln!(out, "{}", format_with_gutter("(none)", None))?;
    } else {
        let headers = &["Branch", "Key", "Value"];
        let mut rows: Vec<Vec<String>> = Vec::new();
        for (branch, entries) in &all_vars {
            for (key, value) in entries {
                // Truncate long values for display
                let display_value = if value.len() > 40 {
                    format!("{}...", &value[..37])
                } else {
                    value.to_string()
                };
                rows.push(vec![branch.to_string(), key.to_string(), display_value]);
            }
        }
        let rendered = crate::md_help::render_data_table(headers, &rows);
        writeln!(out, "{}", rendered.trim_end())?;
    }
    writeln!(out)?;

    // Show CI status cache
    writeln!(out, "{}", format_heading("CI STATUS CACHE", None))?;
    let mut entries = CachedCiStatus::list_all(repo);
    // Sort by age (most recent first), then by branch name for ties
    entries.sort_by(|a, b| {
        b.1.checked_at
            .cmp(&a.1.checked_at)
            .then_with(|| a.0.cmp(&b.0))
    });
    if entries.is_empty() {
        writeln!(out, "{}", format_with_gutter("(none)", None))?;
    } else {
        let rows: Vec<Vec<String>> = entries
            .iter()
            .map(|(branch, cached)| {
                let status = match &cached.status {
                    Some(pr_status) => {
                        let s: &'static str = pr_status.ci_status.into();
                        s.to_string()
                    }
                    None => "none".to_string(),
                };
                let age = format_relative_time_short(cached.checked_at as i64);
                let head: String = cached.head.chars().take(8).collect();
                vec![branch.clone(), status, age, head]
            })
            .collect();
        let rendered =
            crate::md_help::render_data_table(&["Branch", "Status", "Age", "Head"], &rows);
        writeln!(out, "{}", rendered.trim_end())?;
    }
    writeln!(out)?;

    // Show git commands cache summary
    writeln!(out, "{}", format_heading("GIT COMMANDS CACHE", None))?;
    let sha_count = repo.git_commands_cache_count();
    if sha_count == 0 {
        writeln!(out, "{}", format_with_gutter("(none)", None))?;
    } else {
        let label = if sha_count == 1 { "entry" } else { "entries" };
        writeln!(
            out,
            "{}",
            format_with_gutter(&format!("{sha_count} {label}"), None)
        )?;
    }
    writeln!(out)?;

    // Show hints
    writeln!(out, "{}", format_heading("HINTS", None))?;
    let hints = repo.list_shown_hints();
    if hints.is_empty() {
        writeln!(out, "{}", format_with_gutter("(none)", None))?;
    } else {
        for hint in hints {
            writeln!(out, "{}", format_with_gutter(&hint, None))?;
        }
    }
    writeln!(out)?;

    // Show log files
    render_all_log_sections(&mut out, repo)?;
    writeln!(out)?;

    // Show trash (staged worktree removals awaiting background delete)
    let trash_dir = repo.wt_trash_dir();
    let trash_display = format_path_for_display(&trash_dir);
    writeln!(
        out,
        "{}",
        format_heading("TRASH", Some(&format!("@ {trash_display}")))
    )?;
    let trash = list_trash_entries(repo)?;
    if trash.is_empty() {
        writeln!(out, "{}", format_with_gutter("(none)", None))?;
    } else {
        let rows: Vec<Vec<String>> = trash
            .iter()
            .map(|e| {
                let age = e
                    .metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| format_relative_time_short(d.as_secs() as i64))
                    .unwrap_or_else(|| "?".to_string());
                vec![e.name.clone(), age]
            })
            .collect();
        let rendered = crate::md_help::render_data_table(&["Entry", "Age"], &rows);
        writeln!(out, "{}", rendered.trim_end())?;
    }

    // Display through pager; fall back to direct stdout if pager unavailable
    if let Err(e) = show_help_in_pager(&out, true) {
        log::debug!("Pager invocation failed: {}", e);
        println!("{}", out);
    }

    Ok(())
}

// ==================== Vars Operations ====================

/// Validate a vars key name: letters, digits, and hyphens only.
fn validate_vars_key(key: &str) -> anyhow::Result<()> {
    if key.is_empty() {
        anyhow::bail!("Key cannot be empty");
    }
    if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        anyhow::bail!("Invalid key {key:?}: keys must contain only letters, digits, and hyphens");
    }
    Ok(())
}

/// Handle vars get
pub fn handle_vars_get(key: &str, branch: Option<String>) -> anyhow::Result<()> {
    validate_vars_key(key)?;
    let repo = Repository::current()?;
    let branch_name = match branch {
        Some(b) => b,
        None => repo.require_current_branch("get variable for current branch")?,
    };

    let config_key = format!("worktrunk.state.{branch_name}.vars.{key}");
    if let Some(value) = repo.config_value(&config_key)? {
        println!("{value}");
    }
    Ok(())
}

/// Handle vars set
pub fn handle_vars_set(key: &str, value: &str, branch: Option<String>) -> anyhow::Result<()> {
    validate_vars_key(key)?;
    let repo = Repository::current()?;
    let branch_name = match branch {
        Some(b) => b,
        None => repo.require_current_branch("set variable for current branch")?,
    };

    let config_key = format!("worktrunk.state.{branch_name}.vars.{key}");
    repo.set_config(&config_key, value)?;

    eprintln!(
        "{}",
        success_message(cformat!("Set <bold>{key}</> for <bold>{branch_name}</>"))
    );
    Ok(())
}

/// Handle vars list
pub fn handle_vars_list(branch: Option<String>, format: SwitchFormat) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let branch_name = match branch {
        Some(b) => b,
        None => repo.require_current_branch("list variables for current branch")?,
    };

    let entries: Vec<_> = repo.vars_entries(&branch_name).into_iter().collect();

    if format == SwitchFormat::Json {
        let obj: serde_json::Map<String, serde_json::Value> = entries
            .into_iter()
            .map(|(k, v)| (k, serde_json::Value::String(v)))
            .collect();
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else if entries.is_empty() {
        eprintln!(
            "{}",
            info_message(cformat!("No variables for <bold>{branch_name}</>"))
        );
    } else {
        for (key, value) in &entries {
            println!("{key}\t{value}");
        }
    }
    Ok(())
}

/// Handle vars clear
pub fn handle_vars_clear(
    key: Option<&str>,
    all: bool,
    branch: Option<String>,
) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let branch_name = match branch {
        Some(b) => b,
        None => repo.require_current_branch("clear variable for current branch")?,
    };

    if !all && key.is_none() {
        anyhow::bail!("Specify a key to clear, or use --all to clear all keys");
    }

    if all {
        let entries: Vec<_> = repo.vars_entries(&branch_name).into_iter().collect();
        if entries.is_empty() {
            eprintln!(
                "{}",
                info_message(cformat!("No variables for <bold>{branch_name}</>"))
            );
        } else {
            let count = entries.len();
            for (key, _) in entries {
                let config_key = format!("worktrunk.state.{branch_name}.vars.{key}");
                let _ = repo.unset_config(&config_key);
            }
            eprintln!(
                "{}",
                success_message(cformat!(
                    "Cleared <bold>{count}</> variable{} for <bold>{branch_name}</>",
                    if count == 1 { "" } else { "s" }
                ))
            );
        }
    } else {
        let key = key.expect("key required when --all not set");
        validate_vars_key(key)?;
        let config_key = format!("worktrunk.state.{branch_name}.vars.{key}");
        if repo.unset_config(&config_key).unwrap_or(false) {
            eprintln!(
                "{}",
                success_message(cformat!(
                    "Cleared <bold>{key}</> for <bold>{branch_name}</>"
                ))
            );
        } else {
            eprintln!(
                "{}",
                info_message(cformat!(
                    "No variable <bold>{key}</> for <bold>{branch_name}</>"
                ))
            );
        }
    }
    Ok(())
}

/// Clear all vars entries across all branches (used by handle_state_clear_all).
fn clear_all_vars(repo: &Repository) -> anyhow::Result<usize> {
    let all_vars = repo.all_vars_entries();
    let mut cleared = 0;
    for (branch, entries) in &all_vars {
        for key in entries.keys() {
            let config_key = format!("worktrunk.state.{branch}.vars.{key}");
            let _ = repo.unset_config(&config_key);
            cleared += 1;
        }
    }
    Ok(cleared)
}

// ==================== Marker Helpers ====================

/// Marker entry with branch, text, and timestamp
pub(super) struct MarkerEntry {
    pub branch: String,
    pub marker: String,
    pub set_at: u64,
}

/// Get all branch markers from git config with timestamps
pub(super) fn all_markers(repo: &Repository) -> Vec<MarkerEntry> {
    let output = repo
        .run_command(&["config", "--get-regexp", r"^worktrunk\.state\..+\.marker$"])
        .unwrap_or_default();

    let mut markers = Vec::new();
    for line in output.lines() {
        // Format: "worktrunk.state.<branch>.marker json_value"
        let Some((key, value)) = line.split_once(' ') else {
            continue;
        };
        let Some(branch) = key
            .strip_prefix("worktrunk.state.")
            .and_then(|s| s.strip_suffix(".marker"))
        else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(value) else {
            continue; // Skip invalid JSON
        };
        let Some(marker) = parsed.get("marker").and_then(|v| v.as_str()) else {
            continue; // Skip if "marker" field is missing
        };
        let set_at = parsed.get("set_at").and_then(|v| v.as_u64()).unwrap_or(0);
        markers.push(MarkerEntry {
            branch: branch.to_string(),
            marker: marker.to_string(),
            set_at,
        });
    }

    // Sort by age (most recent first), then by branch name for ties
    markers.sort_by(|a, b| {
        b.set_at
            .cmp(&a.set_at)
            .then_with(|| a.branch.cmp(&b.branch))
    });
    markers
}
