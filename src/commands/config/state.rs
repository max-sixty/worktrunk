//! State management commands.
//!
//! Commands for getting, setting, and clearing stored state like default branch,
//! previous branch, CI status, markers, and logs.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use color_print::cformat;
use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};
use worktrunk::git::Repository;
use worktrunk::path::{format_path_for_display, sanitize_for_filename};
use worktrunk::styling::{
    eprintln, format_heading, format_with_gutter, info_message, println, success_message,
    warning_message,
};
use worktrunk::workspace::{Workspace, open_workspace};

use crate::cli::OutputFormat;
use crate::commands::process::HookLog;
use crate::commands::require_git_workspace;
use worktrunk::utils::get_now;

use super::super::list::ci_status::{CachedCiStatus, CiBranchName};
use crate::display::format_relative_time_short;
use crate::help_pager::show_help_in_pager;

// ==================== Path Helpers ====================

/// Core logic for determining user config path from env var values
pub(super) fn resolve_user_config_path(
    xdg_config_home: Option<&str>,
    home: Option<&str>,
) -> Option<PathBuf> {
    // Respect XDG_CONFIG_HOME environment variable (Linux)
    if let Some(xdg_config) = xdg_config_home {
        let config_path = PathBuf::from(xdg_config);
        return Some(config_path.join("worktrunk").join("config.toml"));
    }

    // Respect HOME environment variable (fallback)
    if let Some(home) = home {
        let home_path = PathBuf::from(home);
        return Some(
            home_path
                .join(".config")
                .join("worktrunk")
                .join("config.toml"),
        );
    }

    None
}

pub(super) fn get_user_config_path() -> Option<PathBuf> {
    // Try env vars first, then fall back to etcetera
    resolve_user_config_path(
        std::env::var("XDG_CONFIG_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    )
    .or_else(|| {
        let strategy = choose_base_strategy().ok()?;
        Some(strategy.config_dir().join("worktrunk").join("config.toml"))
    })
}

pub fn require_user_config_path() -> anyhow::Result<PathBuf> {
    get_user_config_path().ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot determine config directory. Set $HOME or $XDG_CONFIG_HOME environment variable"
        )
    })
}

// ==================== Helpers ====================

/// Resolve the current branch/workspace name from an explicit option or workspace detection.
fn resolve_branch_name(
    workspace: &dyn Workspace,
    branch: Option<String>,
) -> anyhow::Result<String> {
    match branch {
        Some(b) => Ok(b),
        None => {
            let cwd = std::env::current_dir()?;
            workspace
                .current_name(&cwd)?
                .ok_or_else(|| anyhow::anyhow!("Cannot determine current branch/workspace name"))
        }
    }
}

// ==================== Log Management ====================

/// Clear all log files from the wt-logs directory
fn clear_logs(log_dir: &Path) -> anyhow::Result<usize> {
    if !log_dir.exists() {
        return Ok(0);
    }

    let mut cleared = 0;
    for entry in std::fs::read_dir(log_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|ext| ext == "log") {
            std::fs::remove_file(&path)?;
            cleared += 1;
        }
    }

    // Remove the directory if empty
    if std::fs::read_dir(log_dir)?.next().is_none() {
        let _ = std::fs::remove_dir(log_dir);
    }

    Ok(cleared)
}

/// Render the LOG FILES section (heading + table or "(none)") into the output buffer
pub(super) fn render_log_files(out: &mut String, log_dir: &Path) -> anyhow::Result<()> {
    let log_dir_display = format_path_for_display(log_dir);

    writeln!(
        out,
        "{}",
        format_heading("LOG FILES", Some(&format!("@ {log_dir_display}")))
    )?;

    if !log_dir.exists() {
        writeln!(out, "{}", format_with_gutter("(none)", None))?;
        return Ok(());
    }

    let mut entries: Vec<_> = std::fs::read_dir(log_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file() && e.path().extension().is_some_and(|ext| ext == "log"))
        .collect();

    if entries.is_empty() {
        writeln!(out, "{}", format_with_gutter("(none)", None))?;
        return Ok(());
    }

    // Sort by modification time (newest first), then by name for stability
    entries.sort_by(|a, b| {
        let a_time = a.metadata().and_then(|m| m.modified()).ok();
        let b_time = b.metadata().and_then(|m| m.modified()).ok();
        b_time
            .cmp(&a_time)
            .then_with(|| a.file_name().cmp(&b.file_name()))
    });

    // Build table
    let mut table = String::from("| File | Size | Age |\n");
    table.push_str("|------|------|-----|\n");

    for entry in entries {
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let meta = entry.metadata().ok();

        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let size_str = if size < 1024 {
            format!("{size}B")
        } else {
            format!("{}K", size / 1024)
        };

        let age = meta
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| format_relative_time_short(d.as_secs() as i64))
            .unwrap_or_else(|| "?".to_string());

        table.push_str(&format!("| {name} | {size_str} | {age} |\n"));
    }

    let rendered = crate::md_help::render_markdown_table(&table);
    write!(out, "{}", rendered.trim_end())?;

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
pub fn handle_logs_get(hook: Option<String>, branch: Option<String>) -> anyhow::Result<()> {
    let workspace = open_workspace()?;
    let log_dir = workspace.wt_logs_dir();

    match hook {
        None => {
            // No hook specified, show all log files (existing behavior)
            let mut out = String::new();
            render_log_files(&mut out, &log_dir)?;

            // Display through pager (fall back to stderr if pager unavailable)
            if show_help_in_pager(&out, true).is_err() {
                eprintln!("{}", out);
            }
        }
        Some(hook_spec) => {
            let branch = resolve_branch_name(&*workspace, branch)?;

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
                // Output just the path to stdout for easy piping
                println!("{}", log_path.display());
                return Ok(());
            }

            // No match found - show expected filename and available files
            let expected_filename = hook_log.filename(&branch);
            let safe_branch = sanitize_for_filename(&branch);
            let mut available = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&log_dir) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with(&format!("{}-", safe_branch)) && name.ends_with(".log") {
                        available.push(name);
                    }
                }
            }

            if available.is_empty() {
                anyhow::bail!(cformat!(
                    "No log files for branch <bold>{}</>. Run a background hook first.",
                    branch
                ));
            } else {
                let available_list = available.join(", ");
                let details = format!(
                    "Expected: {}\nAvailable: {}",
                    expected_filename, available_list
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
pub fn handle_state_get(key: &str, branch: Option<String>) -> anyhow::Result<()> {
    use super::super::list::ci_status::PrStatus;

    let workspace = open_workspace()?;

    match key {
        "default-branch" => {
            let branch_name = workspace.default_branch_name().ok_or_else(|| {
                anyhow::anyhow!(cformat!(
                    "Cannot determine default branch. To configure, run <bold>wt config state default-branch set BRANCH</>"
                ))
            })?;
            println!("{branch_name}");
        }
        "previous-branch" => match workspace.switch_previous() {
            Some(prev) => println!("{prev}"),
            None => println!(""),
        },
        "marker" => {
            let branch_name = resolve_branch_name(&*workspace, branch)?;
            match workspace.branch_marker(&branch_name) {
                Some(marker) => println!("{marker}"),
                None => println!(""),
            }
        }
        "ci-status" => {
            let repo = require_git_workspace(&*workspace, "config state get ci-status")?;
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
                }
                .into());
            }

            let ci_branch = CiBranchName::from_branch_ref(&branch_name, is_remote, repo);
            let ci_status = PrStatus::detect(repo, &ci_branch, &head)
                .map_or(super::super::list::ci_status::CiStatus::NoCI, |s| {
                    s.ci_status
                });
            let status_str: &'static str = ci_status.into();
            println!("{status_str}");
        }
        // TODO: Consider simplifying to just print the path and let users run `ls -al` themselves
        "logs" => {
            let log_dir = workspace.wt_logs_dir();
            let mut out = String::new();
            render_log_files(&mut out, &log_dir)?;

            // Display through pager (fall back to stderr if pager unavailable)
            if show_help_in_pager(&out, true).is_err() {
                eprintln!("{}", out);
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
    let workspace = open_workspace()?;

    match key {
        "default-branch" => {
            // Warn if the branch doesn't exist locally (git-only check)
            if let Some(repo) = workspace.as_any().downcast_ref::<Repository>()
                && !repo.branch(&value).exists_locally()?
            {
                eprintln!(
                    "{}",
                    warning_message(cformat!("Branch <bold>{value}</> does not exist locally"))
                );
            }
            workspace.set_default_branch(&value)?;
            eprintln!(
                "{}",
                success_message(cformat!("Set default branch to <bold>{value}</>"))
            );
        }
        "previous-branch" => {
            workspace.set_switch_previous(Some(&value))?;
            eprintln!(
                "{}",
                success_message(cformat!("Set previous branch to <bold>{value}</>"))
            );
        }
        "marker" => {
            let branch_name = resolve_branch_name(&*workspace, branch)?;
            workspace.set_branch_marker(&branch_name, &value, get_now())?;
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
    let workspace = open_workspace()?;

    match key {
        "default-branch" => {
            if workspace.clear_default_branch()? {
                eprintln!("{}", success_message("Cleared default branch cache"));
            } else {
                eprintln!("{}", info_message("No default branch cache to clear"));
            }
        }
        "previous-branch" => {
            if workspace.clear_switch_previous()? {
                eprintln!("{}", success_message("Cleared previous branch"));
            } else {
                eprintln!("{}", info_message("No previous branch to clear"));
            }
        }
        "ci-status" => {
            let repo = require_git_workspace(&*workspace, "config state ci-status clear")?;
            if all {
                let cleared = CachedCiStatus::clear_all(repo);
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
                let cleared_count = workspace.clear_all_markers();
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
                let branch_name = resolve_branch_name(&*workspace, branch)?;
                if workspace.clear_branch_marker(&branch_name) {
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
            let log_dir = workspace.wt_logs_dir();
            let cleared = clear_logs(&log_dir)?;
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
    let workspace = open_workspace()?;
    let mut cleared_any = false;

    // Clear previous branch (trait method — works for both git and jj)
    if workspace.clear_switch_previous()? {
        cleared_any = true;
    }

    // Clear logs (works for both git and jj)
    let log_dir = workspace.wt_logs_dir();
    let logs_cleared = clear_logs(&log_dir)?;
    if logs_cleared > 0 {
        cleared_any = true;
    }

    // Clear default branch cache (works for both git and jj)
    if matches!(workspace.clear_default_branch(), Ok(true)) {
        cleared_any = true;
    }

    // Clear all markers (trait method — works for both git and jj)
    if workspace.clear_all_markers() > 0 {
        cleared_any = true;
    }

    // Clear all hints (trait method — works for both git and jj)
    if workspace.clear_all_hints()? > 0 {
        cleared_any = true;
    }

    // Clear CI status cache (git-only)
    if let Some(repo) = workspace.as_any().downcast_ref::<Repository>()
        && CachedCiStatus::clear_all(repo) > 0
    {
        cleared_any = true;
    }

    if cleared_any {
        eprintln!("{}", success_message("Cleared all stored state"));
    } else {
        eprintln!("{}", info_message("No stored state to clear"));
    }

    Ok(())
}

// ==================== State Show Commands ====================

/// Handle the state get command (shows all state)
pub fn handle_state_show(format: OutputFormat) -> anyhow::Result<()> {
    let workspace = open_workspace()?;
    let repo = workspace.as_any().downcast_ref::<Repository>();

    match format {
        OutputFormat::Json => handle_state_show_json(&*workspace, repo),
        OutputFormat::Table | OutputFormat::ClaudeCode => {
            handle_state_show_table(&*workspace, repo)
        }
    }
}

/// Output state as JSON
fn handle_state_show_json(
    workspace: &dyn Workspace,
    repo: Option<&Repository>,
) -> anyhow::Result<()> {
    let default_branch = workspace.default_branch_name();
    let previous_branch = workspace.switch_previous();

    let markers: Vec<serde_json::Value> = workspace
        .list_all_markers()
        .into_iter()
        .map(|(branch, marker, set_at)| {
            serde_json::json!({
                "branch": branch,
                "marker": marker,
                "set_at": if set_at > 0 { Some(set_at) } else { None }
            })
        })
        .collect();

    let ci_status: Vec<serde_json::Value> = repo
        .map(|r| {
            let mut ci_entries = CachedCiStatus::list_all(r);
            ci_entries.sort_by(|a, b| {
                b.1.checked_at
                    .cmp(&a.1.checked_at)
                    .then_with(|| a.0.cmp(&b.0))
            });
            ci_entries
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
                .collect()
        })
        .unwrap_or_default();

    // Log files
    let log_dir = workspace.wt_logs_dir();
    let logs: Vec<serde_json::Value> = if log_dir.exists() {
        let mut entries: Vec<_> = std::fs::read_dir(&log_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file() && e.path().extension().is_some_and(|ext| ext == "log"))
            .collect();

        entries.sort_by(|a, b| {
            let a_time = a.metadata().and_then(|m| m.modified()).ok();
            let b_time = b.metadata().and_then(|m| m.modified()).ok();
            b_time.cmp(&a_time)
        });

        entries
            .into_iter()
            .map(|entry| {
                let path = entry.path();
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let meta = entry.metadata().ok();
                let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                let modified = meta
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs());

                serde_json::json!({
                    "file": name,
                    "size": size,
                    "modified_at": modified
                })
            })
            .collect()
    } else {
        vec![]
    };

    let hints = workspace.list_shown_hints();

    let output = serde_json::json!({
        "default_branch": default_branch,
        "previous_branch": previous_branch,
        "markers": markers,
        "ci_status": ci_status,
        "logs": logs,
        "hints": hints
    });

    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

/// Output state as human-readable table
fn handle_state_show_table(
    workspace: &dyn Workspace,
    repo: Option<&Repository>,
) -> anyhow::Result<()> {
    // Build complete output as a string
    let mut out = String::new();

    // Show default branch cache (trait method)
    writeln!(out, "{}", format_heading("DEFAULT BRANCH", None))?;
    match workspace.default_branch_name() {
        Some(branch) => writeln!(out, "{}", format_with_gutter(&branch, None))?,
        None => writeln!(out, "{}", format_with_gutter("(not available)", None))?,
    }
    writeln!(out)?;

    // Show previous branch (trait method)
    writeln!(out, "{}", format_heading("PREVIOUS BRANCH", None))?;
    match workspace.switch_previous() {
        Some(prev) => writeln!(out, "{}", format_with_gutter(&prev, None))?,
        None => writeln!(out, "{}", format_with_gutter("(none)", None))?,
    }
    writeln!(out)?;

    // Show branch markers (trait method)
    {
        let markers = workspace.list_all_markers();
        writeln!(out, "{}", format_heading("BRANCH MARKERS", None))?;
        if markers.is_empty() {
            writeln!(out, "{}", format_with_gutter("(none)", None))?;
        } else {
            let mut table = String::from("| Branch | Marker | Age |\n");
            table.push_str("|--------|--------|-----|\n");
            for (branch, marker, set_at) in markers {
                let age = format_relative_time_short(set_at as i64);
                table.push_str(&format!("| {branch} | {marker} | {age} |\n"));
            }
            let rendered = crate::md_help::render_markdown_table(&table);
            writeln!(out, "{}", rendered.trim_end())?;
        }
        writeln!(out)?;
    }

    // Show CI status cache (git-only)
    if let Some(repo) = repo {
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
            // Build markdown table
            let mut table = String::from("| Branch | Status | Age | Head |\n");
            table.push_str("|--------|--------|-----|------|\n");
            for (branch, cached) in entries {
                let status = match &cached.status {
                    Some(pr_status) => {
                        let status: &'static str = pr_status.ci_status.into();
                        status.to_string()
                    }
                    None => "none".to_string(),
                };
                let age = format_relative_time_short(cached.checked_at as i64);
                let head: String = cached.head.chars().take(8).collect();

                table.push_str(&format!("| {branch} | {status} | {age} | {head} |\n"));
            }

            let rendered = crate::md_help::render_markdown_table(&table);
            writeln!(out, "{}", rendered.trim_end())?;
        }
        writeln!(out)?;
    }

    // Show hints (trait method)
    {
        let hints = workspace.list_shown_hints();
        writeln!(out, "{}", format_heading("HINTS", None))?;
        if hints.is_empty() {
            writeln!(out, "{}", format_with_gutter("(none)", None))?;
        } else {
            for hint in hints {
                writeln!(out, "{}", format_with_gutter(&hint, None))?;
            }
        }
        writeln!(out)?;
    }

    // Show log files (trait method)
    let log_dir = workspace.wt_logs_dir();
    render_log_files(&mut out, &log_dir)?;

    // Display through pager (fall back to stderr if pager unavailable)
    if let Err(e) = show_help_in_pager(&out, true) {
        log::debug!("Pager invocation failed: {}", e);
        // Fall back to direct output via eprintln (matches help behavior)
        eprintln!("{}", out);
    }

    Ok(())
}
