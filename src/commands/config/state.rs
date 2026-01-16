//! State management commands.
//!
//! Commands for getting, setting, and clearing stored state like default branch,
//! previous branch, CI status, markers, and logs.

use std::fmt::Write as _;
use std::path::PathBuf;

use color_print::cformat;
use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    format_heading, format_with_gutter, info_message, success_message, warning_message,
};
use worktrunk::utils::get_now;

use super::super::list::ci_status::CachedCiStatus;
use crate::display::format_relative_time_short;
use crate::help_pager::show_help_in_pager;
use crate::output;

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

// ==================== Log Management ====================

/// Clear all log files from the wt-logs directory
fn clear_logs(repo: &Repository) -> anyhow::Result<usize> {
    let log_dir = repo.wt_logs_dir();

    if !log_dir.exists() {
        return Ok(0);
    }

    let mut cleared = 0;
    for entry in std::fs::read_dir(&log_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() && path.extension().is_some_and(|ext| ext == "log") {
            std::fs::remove_file(&path)?;
            cleared += 1;
        }
    }

    // Remove the directory if empty
    if std::fs::read_dir(&log_dir)?.next().is_none() {
        let _ = std::fs::remove_dir(&log_dir);
    }

    Ok(cleared)
}

/// Render the LOG FILES section (heading + table or "(none)") into the output buffer
pub(super) fn render_log_files(out: &mut String, repo: &Repository) -> anyhow::Result<()> {
    let log_dir = repo.wt_logs_dir();
    let log_dir_display = format_path_for_display(&log_dir);

    writeln!(
        out,
        "{}",
        format_heading("LOG FILES", Some(&format!("@ {log_dir_display}")))
    )?;

    if !log_dir.exists() {
        writeln!(out, "{}", format_with_gutter("(none)", None))?;
        return Ok(());
    }

    let mut entries: Vec<_> = std::fs::read_dir(&log_dir)?
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

// ==================== State Get/Set/Clear Commands ====================

/// Handle the state get command
pub fn handle_state_get(key: &str, branch: Option<String>) -> anyhow::Result<()> {
    use super::super::list::ci_status::PrStatus;

    let repo = Repository::current()?;

    match key {
        "default-branch" => {
            let branch_name = repo.default_branch().ok_or_else(|| {
                anyhow::anyhow!(cformat!(
                    "Cannot determine default branch. To configure, run <bold>wt config state default-branch set BRANCH</>"
                ))
            })?;
            output::stdout(branch_name)?;
        }
        "previous-branch" => match repo.get_switch_previous() {
            Some(prev) => output::stdout(prev)?,
            None => output::stdout("")?,
        },
        "marker" => {
            let branch_name = match branch {
                Some(b) => b,
                None => repo.require_current_branch("get marker for current branch")?,
            };
            match repo.branch_keyed_marker(&branch_name) {
                Some(marker) => output::stdout(marker)?,
                None => output::stdout("")?,
            }
        }
        "ci-status" => {
            let branch_name = match branch {
                Some(b) => b,
                None => repo.require_current_branch("get ci-status for current branch")?,
            };

            // Get the HEAD commit for this branch
            let head = repo
                .run_command(&["rev-parse", &branch_name])
                .map(|s| s.trim().to_string())
                .unwrap_or_default();

            if head.is_empty() {
                return Err(worktrunk::git::GitError::InvalidReference {
                    reference: branch_name,
                }
                .into());
            }

            let has_upstream = repo.upstream_branch(&branch_name).ok().flatten().is_some();
            let ci_status = PrStatus::detect(&repo, &branch_name, &head, has_upstream)
                .map_or(super::super::list::ci_status::CiStatus::NoCI, |s| {
                    s.ci_status
                });
            let status_str: &'static str = ci_status.into();
            output::stdout(status_str)?;
        }
        // TODO: Consider simplifying to just print the path and let users run `ls -al` themselves
        "logs" => {
            let mut out = String::new();
            render_log_files(&mut out, &repo)?;

            // Display through pager (fall back to stderr if pager unavailable)
            if show_help_in_pager(&out, true).is_err() {
                worktrunk::styling::eprintln!("{}", out);
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
            if !repo.local_branch_exists(&value)? {
                output::print(warning_message(cformat!(
                    "Branch <bold>{value}</> does not exist locally"
                )))?;
            }
            repo.set_default_branch(&value)?;
            output::print(success_message(cformat!(
                "Set default branch to <bold>{value}</>"
            )))?;
        }
        "previous-branch" => {
            repo.record_switch_previous(Some(&value))?;
            output::print(success_message(cformat!(
                "Set previous branch to <bold>{value}</>"
            )))?;
        }
        "marker" => {
            let branch_name = match branch {
                Some(b) => b,
                None => repo.require_current_branch("set marker for current branch")?,
            };

            // Store as JSON with timestamp
            let now = get_now();
            let json = serde_json::json!({
                "marker": value,
                "set_at": now
            });

            let config_key = format!("worktrunk.state.{branch_name}.marker");
            repo.run_command(&["config", &config_key, &json.to_string()])?;

            output::print(success_message(cformat!(
                "Set marker for <bold>{branch_name}</> to <bold>{value}</>"
            )))?;
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
                output::print(success_message("Cleared default branch cache"))?;
            } else {
                output::print(info_message("No default branch cache to clear"))?;
            }
        }
        "previous-branch" => {
            if repo
                .run_command(&["config", "--unset", "worktrunk.history"])
                .is_ok()
            {
                output::print(success_message("Cleared previous branch"))?;
            } else {
                output::print(info_message("No previous branch to clear"))?;
            }
        }
        "ci-status" => {
            if all {
                let cleared = CachedCiStatus::clear_all(&repo);
                if cleared == 0 {
                    output::print(info_message("No CI cache entries to clear"))?;
                } else {
                    output::print(success_message(cformat!(
                        "Cleared <bold>{cleared}</> CI cache entr{}",
                        if cleared == 1 { "y" } else { "ies" }
                    )))?;
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
                    output::print(success_message(cformat!(
                        "Cleared CI cache for <bold>{branch_name}</>"
                    )))?;
                } else {
                    output::print(info_message(cformat!(
                        "No CI cache for <bold>{branch_name}</>"
                    )))?;
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
                    output::print(info_message("No markers to clear"))?;
                } else {
                    output::print(success_message(cformat!(
                        "Cleared <bold>{cleared_count}</> marker{}",
                        if cleared_count == 1 { "" } else { "s" }
                    )))?;
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
                    output::print(success_message(cformat!(
                        "Cleared marker for <bold>{branch_name}</>"
                    )))?;
                } else {
                    output::print(info_message(cformat!(
                        "No marker set for <bold>{branch_name}</>"
                    )))?;
                }
            }
        }
        "logs" => {
            let cleared = clear_logs(&repo)?;
            if cleared == 0 {
                output::print(info_message("No logs to clear"))?;
            } else {
                output::print(success_message(cformat!(
                    "Cleared <bold>{cleared}</> log file{}",
                    if cleared == 1 { "" } else { "s" }
                )))?;
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
        cleared_any = true;
    }

    // Clear previous branch
    if repo
        .run_command(&["config", "--unset", "worktrunk.history"])
        .is_ok()
    {
        cleared_any = true;
    }

    // Clear all markers
    let markers_output = repo
        .run_command(&["config", "--get-regexp", r"^worktrunk\.state\..+\.marker$"])
        .unwrap_or_default();
    for line in markers_output.lines() {
        if let Some(config_key) = line.split_whitespace().next() {
            let _ = repo.run_command(&["config", "--unset", config_key]);
            cleared_any = true;
        }
    }

    // Clear all CI status cache
    let ci_cleared = CachedCiStatus::clear_all(&repo);
    if ci_cleared > 0 {
        cleared_any = true;
    }

    // Clear all logs
    let logs_cleared = clear_logs(&repo)?;
    if logs_cleared > 0 {
        cleared_any = true;
    }

    // Clear all hints
    let hints_cleared = repo.clear_all_hints()?;
    if hints_cleared > 0 {
        cleared_any = true;
    }

    if cleared_any {
        output::print(success_message("Cleared all stored state"))?;
    } else {
        output::print(info_message("No stored state to clear"))?;
    }

    Ok(())
}

// ==================== State Show Commands ====================

/// Handle the state get command (shows all state)
pub fn handle_state_show(format: crate::cli::OutputFormat) -> anyhow::Result<()> {
    use crate::cli::OutputFormat;

    let repo = Repository::current()?;

    match format {
        OutputFormat::Json => handle_state_show_json(&repo),
        OutputFormat::Table => handle_state_show_table(&repo),
    }
}

/// Output state as JSON
fn handle_state_show_json(repo: &Repository) -> anyhow::Result<()> {
    // Get default branch
    let default_branch = repo.default_branch();

    // Get previous branch
    let previous_branch = repo.get_switch_previous();

    // Get markers
    let markers: Vec<serde_json::Value> = get_all_markers(repo)
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

    // Get log files
    let log_dir = repo.wt_logs_dir();
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

    // Get hints
    let hints = repo.list_shown_hints();

    let output = serde_json::json!({
        "default_branch": default_branch,
        "previous_branch": previous_branch,
        "markers": markers,
        "ci_status": ci_status,
        "logs": logs,
        "hints": hints
    });

    output::stdout(serde_json::to_string_pretty(&output)?)?;
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
    match repo.get_switch_previous() {
        Some(prev) => writeln!(out, "{}", format_with_gutter(&prev, None))?,
        None => writeln!(out, "{}", format_with_gutter("(none)", None))?,
    }
    writeln!(out)?;

    // Show branch markers
    writeln!(out, "{}", format_heading("BRANCH MARKERS", None))?;
    let markers = get_all_markers(repo);
    if markers.is_empty() {
        writeln!(out, "{}", format_with_gutter("(none)", None))?;
    } else {
        let mut table = String::from("| Branch | Marker | Age |\n");
        table.push_str("|--------|--------|-----|\n");
        for entry in markers {
            let age = format_relative_time_short(entry.set_at as i64);
            table.push_str(&format!(
                "| {} | {} | {} |\n",
                entry.branch, entry.marker, age
            ));
        }
        let rendered = crate::md_help::render_markdown_table(&table);
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
    render_log_files(&mut out, repo)?;

    // Display through pager (fall back to stderr if pager unavailable)
    if let Err(e) = show_help_in_pager(&out, true) {
        log::debug!("Pager invocation failed: {}", e);
        // Fall back to direct output via eprintln (matches help behavior)
        worktrunk::styling::eprintln!("{}", out);
    }

    Ok(())
}

// ==================== Marker Helpers ====================

/// Marker entry with branch, text, and timestamp
pub(super) struct MarkerEntry {
    pub branch: String,
    pub marker: String,
    pub set_at: u64,
}

/// Get all branch markers from git config with timestamps
pub(super) fn get_all_markers(repo: &Repository) -> Vec<MarkerEntry> {
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
