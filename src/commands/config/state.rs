//! State management commands.
//!
//! Commands for getting, setting, and clearing stored state like default branch,
//! previous branch, CI status, markers, and logs.
//!
//! # Log layout invariant
//!
//! Inside `wt_logs_dir()`, top-level *files* are shared logs (`commands.jsonl*`,
//! `verbose.log`, `diagnostic.md`) and top-level *directories* are per-branch log
//! trees (`{branch}/{source|internal}/{hook-type}/{name}.log`). Categorization
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
use worktrunk::path::{format_path_for_display, sanitize_for_filename};
use worktrunk::styling::{
    eprintln, format_heading, format_with_gutter, info_message, println, success_message,
    warning_message,
};

use crate::cli::{OutputFormat, SwitchFormat};
use crate::commands::process::HookLog;
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

/// Check if a top-level file is a diagnostic file (`verbose.log` or `diagnostic.md`).
///
/// These are created by `-vv` and live directly under `wt_logs_dir()`.
fn is_diagnostic_file(name: &str) -> bool {
    name == "verbose.log" || name == "diagnostic.md"
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
/// 1. **Top-level files**: `commands.jsonl*`, `verbose.log`, `diagnostic.md`.
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
}

impl LogRow {
    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "file": self.display_name,
            "path": self.path,
            "size": self.size,
            "modified_at": self.modified_at,
        })
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

// ==================== Logs Get Command ====================

/// Handle the logs get command
///
/// When `hook` is None, lists all log files.
/// When `hook` is Some, returns the path to the specific log file for that hook.
///
/// # Hook spec format
///
/// - `source:hook-type:name` for hook commands (e.g., `user:post-start:server`)
/// - `internal:op` for internal operations (e.g., `internal:remove`)
pub fn handle_logs_get(
    hook: Option<String>,
    branch: Option<String>,
    format: SwitchFormat,
) -> anyhow::Result<()> {
    let repo = Repository::current()?;

    match hook {
        None if format == SwitchFormat::Json => {
            let (command_log, hook_output, diagnostic) = partition_log_files_json(&repo)?;
            let output = serde_json::json!({
                "command_log": command_log,
                "hook_output": hook_output,
                "diagnostic": diagnostic,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        None => {
            // No hook specified, show all log files
            let mut out = String::new();
            render_all_log_sections(&mut out, &repo)?;

            // Display through pager (fall back to stderr if pager unavailable)
            if show_help_in_pager(&out, true).is_err() {
                eprintln!("{}", out);
            }
        }
        Some(hook_spec) => {
            // Get the branch name
            let branch = match branch {
                Some(b) => b,
                None => repo.require_current_branch("get log for current branch")?,
            };

            let log_dir = repo.wt_logs_dir();

            // Parse the hook spec using HookLog
            let hook_log = HookLog::parse(&hook_spec).map_err(|e| anyhow::anyhow!("{}", e))?;

            // Check log directory exists
            if !log_dir.exists() {
                anyhow::bail!(
                    "No log directory exists. Run a background hook first to create logs."
                );
            }

            // Get the expected log path
            let log_path = hook_log.path(&log_dir, &branch);

            if log_path.exists() {
                // Use to_slash_lossy() so Windows paths use forward slashes, consistent
                // with the relative paths in `logs list` output.
                let path_str = log_path.to_slash_lossy().into_owned();
                if format == SwitchFormat::Json {
                    let output = serde_json::json!({ "path": path_str });
                    println!("{}", serde_json::to_string_pretty(&output)?);
                } else {
                    // Output just the path to stdout for easy piping.
                    println!("{}", path_str);
                }
                return Ok(());
            }

            // No match found — show the expected relative path and list any
            // logs that do exist for this branch.
            let expected_relative = log_path
                .strip_prefix(&log_dir)
                .unwrap_or(&log_path)
                .to_slash_lossy()
                .into_owned();
            let branch_dir = log_dir.join(sanitize_for_filename(&branch));
            let available: Vec<String> = if branch_dir.exists() {
                // Walk only the branch subtree, not every branch.
                let mut entries = Vec::new();
                walk_branch_dir(&log_dir, &branch_dir, &mut entries)?;
                entries.into_iter().map(|e| e.relative_display).collect()
            } else {
                Vec::new()
            };

            if available.is_empty() {
                anyhow::bail!(cformat!(
                    "No log files for branch <bold>{}</>. Run a background hook first.",
                    branch
                ));
            } else {
                let details = format!(
                    "Expected: {}\nAvailable: {}",
                    expected_relative,
                    available.join(", ")
                );
                return Err(anyhow::anyhow!(details).context(cformat!(
                    "No log file matches <bold>{}</> for branch <bold>{}</>",
                    hook_log.to_spec(),
                    branch
                )));
            }
        }
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
                    .run_command(&["config", "--get", &config_key])
                    .ok()
                    .map(|s| s.trim().to_string())
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
            repo.run_command(&["config", &config_key, &json.to_string()])?;

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
            if repo
                .run_command(&["config", "--unset", "worktrunk.history"])
                .is_ok()
            {
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
                if repo
                    .run_command(&["config", "--unset", &config_key])
                    .is_ok()
                {
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
                        repo.run_command(&["config", "--unset", config_key])?;
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
                if repo
                    .run_command(&["config", "--unset", &config_key])
                    .is_ok()
                {
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
    if repo
        .run_command(&["config", "--unset", "worktrunk.history"])
        .is_ok()
    {
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
            let _ = repo.run_command(&["config", "--unset", config_key]);
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

    let output = serde_json::json!({
        "default_branch": default_branch,
        "previous_branch": previous_branch,
        "markers": markers,
        "ci_status": ci_status,
        "vars": vars_data,
        "command_log": command_log,
        "hook_output": hook_output,
        "diagnostic": diagnostic,
        "hints": hints
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

    // Display through pager (fall back to stderr if pager unavailable)
    if let Err(e) = show_help_in_pager(&out, true) {
        log::debug!("Pager invocation failed: {}", e);
        // Fall back to direct output via eprintln (matches help behavior)
        eprintln!("{}", out);
    }

    Ok(())
}

// ==================== Vars Operations ====================

/// Validate a vars key name: letters, digits, hyphens, underscores only.
fn validate_vars_key(key: &str) -> anyhow::Result<()> {
    if key.is_empty() {
        anyhow::bail!("Key cannot be empty");
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        anyhow::bail!(
            "Invalid key {key:?}: keys must contain only letters, digits, hyphens, and underscores"
        );
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
    repo.run_command(&["config", &config_key, value])?;

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
                let _ = repo.run_command(&["config", "--unset", &config_key]);
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
        if repo
            .run_command(&["config", "--unset", &config_key])
            .is_ok()
        {
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
            let _ = repo.run_command(&["config", "--unset", &config_key]);
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
