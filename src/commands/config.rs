use anyhow::Context;
use color_print::cformat;
use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};
use std::fmt::Write as _;
use std::path::PathBuf;
use worktrunk::config::{find_unknown_project_keys, find_unknown_user_keys};
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;
use worktrunk::shell::Shell;
use worktrunk::styling::{HINT_EMOJI, INFO_EMOJI, WARNING_EMOJI, format_toml, format_with_gutter};

use super::configure_shell::{ConfigAction, scan_shell_configs};
use super::list::ci_status::CachedCiStatus;
use crate::help_pager::show_help_in_pager;
use crate::output;

/// Example configuration file content (displayed in help with values uncommented)
const CONFIG_EXAMPLE: &str = include_str!("../../dev/config.example.toml");

/// Comment out all non-comment, non-empty lines for writing to disk
fn comment_out_config(content: &str) -> String {
    let has_trailing_newline = content.ends_with('\n');
    let result = content
        .lines()
        .map(|line| {
            // Comment out non-empty lines that aren't already comments
            if !line.is_empty() && !line.starts_with('#') {
                format!("# {}", line)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    if has_trailing_newline {
        format!("{}\n", result)
    } else {
        result
    }
}

/// Handle the config create command
pub fn handle_config_create() -> anyhow::Result<()> {
    let config_path = require_user_config_path()?;

    // Check if file already exists
    if config_path.exists() {
        output::info(cformat!(
            "User config already exists: <bold>{}</>",
            format_path_for_display(&config_path)
        ))?;
        output::blank()?;
        output::print(cformat!(
            "{HINT_EMOJI} <dim>Use </>wt config show<dim> to view existing configuration</>"
        ))?;
        output::print(cformat!(
            "{HINT_EMOJI} <dim>Use </>wt config create --help<dim> for config format reference</>"
        ))?;
        return Ok(());
    }

    // Create parent directory if it doesn't exist
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create config directory")?;
    }

    // Write the example config with all values commented out
    let commented_config = comment_out_config(CONFIG_EXAMPLE);
    std::fs::write(&config_path, commented_config).context("Failed to write config file")?;

    // Success message
    output::success(cformat!(
        "Created config file: <bold>{}</>",
        format_path_for_display(&config_path)
    ))?;
    output::blank()?;
    output::hint("Edit this file to customize worktree paths and LLM settings")?;

    Ok(())
}

/// Handle the config show command
pub fn handle_config_show() -> anyhow::Result<()> {
    // Build the complete output as a string
    let mut output = String::new();

    // Render user config
    render_user_config(&mut output)?;
    output.push('\n');

    // Render project config if in a git repository
    render_project_config(&mut output)?;
    output.push('\n');

    // Render shell integration status
    render_shell_status(&mut output)?;

    // Display through pager
    if let Err(e) = show_help_in_pager(&output) {
        log::debug!("Pager invocation failed: {}", e);
        // Fall back to direct output via eprintln (matches help behavior)
        worktrunk::styling::eprintln!("{}", output);
    }

    Ok(())
}

fn render_user_config(out: &mut String) -> anyhow::Result<()> {
    let config_path = require_user_config_path()?;

    writeln!(
        out,
        "{}",
        cformat!(
            "{INFO_EMOJI} User Config: <bold>{}</>",
            format_path_for_display(&config_path)
        )
    )?;

    // Check if file exists
    if !config_path.exists() {
        writeln!(
            out,
            "{}",
            cformat!("{HINT_EMOJI} <dim>Not found (using defaults)</>")
        )?;
        writeln!(
            out,
            "{}",
            cformat!("{HINT_EMOJI} <dim>Run </>wt config create<dim> to create a config file</>")
        )?;
        writeln!(out)?;
        let default_config =
            "# Default configuration:\nworktree-path = \"../{{ main_worktree }}.{{ branch }}\"";
        write!(out, "{}", format_toml(default_config, ""))?;
        return Ok(());
    }

    // Read and display the file contents
    let contents = std::fs::read_to_string(&config_path).context("Failed to read config file")?;

    if contents.trim().is_empty() {
        writeln!(
            out,
            "{}",
            cformat!("{HINT_EMOJI} <dim>Empty file (using defaults)</>")
        )?;
        return Ok(());
    }

    // Check for unknown keys and warn
    warn_unknown_keys(out, &find_unknown_user_keys(&contents))?;

    // Display TOML with syntax highlighting (gutter at column 0)
    write!(out, "{}", format_toml(&contents, ""))?;

    Ok(())
}

/// Write warnings for any unknown config keys
fn warn_unknown_keys(out: &mut String, unknown_keys: &[String]) -> anyhow::Result<()> {
    for key in unknown_keys {
        writeln!(
            out,
            "{}",
            cformat!("{WARNING_EMOJI} <yellow>Unknown key <bold>{key}</> will be ignored</>")
        )?;
    }
    Ok(())
}

fn render_project_config(out: &mut String) -> anyhow::Result<()> {
    // Try to get current repository root
    let repo = Repository::current();
    let repo_root = match repo.worktree_root() {
        Ok(root) => root,
        Err(_) => {
            writeln!(
                out,
                "{}",
                cformat!("{INFO_EMOJI} <dim>Project Config: Not in a git repository</>")
            )?;
            return Ok(());
        }
    };
    let config_path = repo_root.join(".config").join("wt.toml");

    writeln!(
        out,
        "{}",
        cformat!(
            "{INFO_EMOJI} Project Config: <bold>{}</>",
            format_path_for_display(&config_path)
        )
    )?;

    // Check if file exists
    if !config_path.exists() {
        writeln!(out, "{}", cformat!("{HINT_EMOJI} <dim>Not found</>"))?;
        return Ok(());
    }

    // Read and display the file contents
    let contents = std::fs::read_to_string(&config_path).context("Failed to read config file")?;

    if contents.trim().is_empty() {
        writeln!(out, "{}", cformat!("{HINT_EMOJI} <dim>Empty file</>"))?;
        return Ok(());
    }

    // Check for unknown keys and warn
    warn_unknown_keys(out, &find_unknown_project_keys(&contents))?;

    // Display TOML with syntax highlighting (gutter at column 0)
    write!(out, "{}", format_toml(&contents, ""))?;

    Ok(())
}

fn render_shell_status(out: &mut String) -> anyhow::Result<()> {
    // Use the same detection logic as `wt config shell install`
    let scan_result = match scan_shell_configs(None, true) {
        Ok(r) => r,
        Err(e) => {
            writeln!(
                out,
                "{}",
                cformat!("{HINT_EMOJI} <dim>Could not determine shell status: {e}</>")
            )?;
            return Ok(());
        }
    };

    let mut any_not_configured = false;

    // Show configured and not-configured shells (matching `config shell install` format exactly)
    // Bash/Zsh: inline completions, show "shell extension & completions"
    // Fish: separate completion file, show "shell extension" for conf.d and "completions" for completions/
    for result in &scan_result.configured {
        let shell = result.shell;
        let path = format_path_for_display(&result.path);
        // Fish has separate completion file; bash/zsh have inline completions
        let what = if matches!(shell, Shell::Fish) {
            "shell extension"
        } else {
            "shell extension & completions"
        };

        match result.action {
            ConfigAction::AlreadyExists => {
                writeln!(
                    out,
                    "{}",
                    cformat!(
                        "{INFO_EMOJI} Already configured {what} for <bold>{shell}</> @ {path}"
                    )
                )?;

                // Check if zsh has compinit enabled (required for completions)
                if matches!(shell, Shell::Zsh) && check_zsh_compinit_missing() {
                    writeln!(
                        out,
                        "{}",
                        cformat!(
                            "{WARNING_EMOJI} <yellow>Completions won't work; add to ~/.zshrc before the wt line:</>"
                        )
                    )?;
                    write!(
                        out,
                        "{}",
                        format_with_gutter("autoload -Uz compinit && compinit", "", None,)
                    )?;
                }

                // For fish, check completions file separately
                if matches!(shell, Shell::Fish)
                    && let Ok(completion_path) = shell.completion_path()
                {
                    let completion_display = format_path_for_display(&completion_path);
                    if completion_path.exists() {
                        writeln!(
                            out,
                            "{}",
                            cformat!(
                                "{INFO_EMOJI} Already configured completions for <bold>{shell}</> @ {completion_display}"
                            )
                        )?;
                    } else {
                        any_not_configured = true;
                        writeln!(
                            out,
                            "{}",
                            cformat!(
                                "{HINT_EMOJI} <dim>Not configured completions for {shell} @ {completion_display}</>"
                            )
                        )?;
                    }
                }
            }
            ConfigAction::WouldAdd | ConfigAction::WouldCreate => {
                any_not_configured = true;
                writeln!(
                    out,
                    "{}",
                    cformat!("{HINT_EMOJI} <dim>Not configured {what} for {shell} @ {path}</>")
                )?;
            }
            _ => {} // Added/Created won't appear in dry_run mode
        }
    }

    // Show skipped (not installed) shells
    for (shell, path) in &scan_result.skipped {
        let path = format_path_for_display(path);
        writeln!(
            out,
            "{}",
            cformat!("<dim>⚪ Skipped {shell}; {path} not found</>")
        )?;
    }

    // Summary hint
    if any_not_configured {
        writeln!(out)?;
        writeln!(
            out,
            "{}",
            cformat!(
                "{HINT_EMOJI} <dim>Run </>wt config shell install<dim> to enable shell integration</>"
            )
        )?;
    }

    Ok(())
}

/// Check if zsh has compinit enabled by spawning an interactive shell
///
/// Returns true if compinit is NOT enabled (i.e., user needs to add it).
/// Returns false if compinit is enabled or we can't determine (fail-safe: don't warn).
fn check_zsh_compinit_missing() -> bool {
    use std::process::{Command, Stdio};

    // Allow tests to bypass this check since zsh subprocess behavior varies across CI envs
    if std::env::var("WT_ASSUME_COMPINIT").is_ok() {
        return false; // Assume compinit is configured
    }

    // Force compinit to be missing (for tests that expect the warning)
    if std::env::var("WT_ASSUME_NO_COMPINIT").is_ok() {
        return true; // Force warning to appear
    }

    // Probe zsh to check if compdef function exists (indicates compinit has run)
    // Use --no-globalrcs to skip system files (like /etc/zshrc on macOS which enables compinit)
    // This ensures we're checking the USER's configuration, not system defaults
    // Suppress stderr to avoid noise like "can't change option: zle"
    // The (( ... )) arithmetic returns exit 0 if true (compdef exists), 1 if false
    let status = Command::new("zsh")
        .args(["--no-globalrcs", "-ic", "(( $+functions[compdef] ))"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .ok();

    match status {
        Some(s) => !s.success(), // compdef NOT found = need to warn
        None => false,           // Can't determine, don't warn
    }
}

fn get_user_config_path() -> Option<PathBuf> {
    // Respect XDG_CONFIG_HOME environment variable for testing (Linux)
    if let Ok(xdg_config) = std::env::var("XDG_CONFIG_HOME") {
        let config_path = PathBuf::from(xdg_config);
        return Some(config_path.join("worktrunk").join("config.toml"));
    }

    // Respect HOME environment variable for testing (fallback)
    if let Ok(home) = std::env::var("HOME") {
        let home_path = PathBuf::from(home);
        return Some(
            home_path
                .join(".config")
                .join("worktrunk")
                .join("config.toml"),
        );
    }

    let strategy = choose_base_strategy().ok()?;
    Some(strategy.config_dir().join("worktrunk").join("config.toml"))
}

fn require_user_config_path() -> anyhow::Result<PathBuf> {
    get_user_config_path().ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot determine config directory. Set $HOME or $XDG_CONFIG_HOME environment variable"
        )
    })
}

/// Handle the config status set command
pub fn handle_config_status_set(value: String, branch: Option<String>) -> anyhow::Result<()> {
    let repo = Repository::current();

    // TODO: Worktree-specific status (worktrunk.status with --worktree flag) would allow
    // different statuses per worktree, but requires extensions.worktreeConfig which adds
    // complexity. Our intended workflow is one branch per worktree, so branch-keyed status
    // is sufficient for now.

    let branch_name = match branch {
        Some(b) => b,
        None => repo.require_current_branch("set status for current branch")?,
    };

    let config_key = format!("worktrunk.status.{}", branch_name);
    repo.run_command(&["config", &config_key, &value])?;

    crate::output::success(cformat!(
        "Set status for <bold>{branch_name}</> to <bold>{value}</>"
    ))?;

    Ok(())
}

/// Handle the config status unset command
pub fn handle_config_status_unset(target: String) -> anyhow::Result<()> {
    let repo = Repository::current();

    if target == "*" {
        // Clear all branch-keyed statuses
        let output = repo
            .run_command(&["config", "--get-regexp", "^worktrunk\\.status\\."])
            .unwrap_or_default();

        let mut cleared_count = 0;
        for line in output.lines() {
            if let Some(key) = line.split_whitespace().next() {
                repo.run_command(&["config", "--unset", key])?;
                cleared_count += 1;
            }
        }

        if cleared_count == 0 {
            crate::output::info("No statuses to clear")?;
        } else {
            crate::output::success(cformat!(
                "Cleared <bold>{cleared_count}</> status{}",
                if cleared_count == 1 { "" } else { "es" }
            ))?;
        }
    } else {
        // Clear specific branch status
        let branch_name = if target.is_empty() {
            repo.require_current_branch("clear status for current branch")?
        } else {
            target
        };

        let config_key = format!("worktrunk.status.{}", branch_name);
        repo.run_command(&["config", "--unset", &config_key])
            .context("Failed to unset status (may not be set)")?;

        crate::output::success(cformat!("Cleared status for <bold>{branch_name}</>"))?;
    }

    Ok(())
}

/// Handle the cache show command
pub fn handle_cache_show() -> anyhow::Result<()> {
    let repo = Repository::current();

    // Show default branch cache
    crate::output::info("Default branch cache:")?;
    match repo.default_branch() {
        Ok(branch) => crate::output::data(format!("  {branch}"))?,
        Err(_) => crate::output::data("  (not cached)")?,
    }
    crate::output::blank()?;

    // Show CI status cache
    crate::output::info("CI status cache:")?;

    let entries = CachedCiStatus::list_all(&repo);
    if entries.is_empty() {
        crate::output::data("  (no CI cache entries)")?;
        return Ok(());
    }

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    for (branch, cached) in entries {
        let status = serde_json::to_string(&cached.status.ci_status)
            .map(|s| s.trim_matches('"').to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        let age = now_secs.saturating_sub(cached.checked_at);
        let head: String = cached.head.chars().take(8).collect();

        crate::output::data(format!("  {branch}: {status} (age: {age}s, head: {head})"))?;
    }

    Ok(())
}

/// Handle the cache clear command
pub fn handle_cache_clear(cache_type: Option<String>) -> anyhow::Result<()> {
    let repo = Repository::current();

    match cache_type.as_deref() {
        Some("ci") => {
            let cleared = CachedCiStatus::clear_all(&repo);
            if cleared == 0 {
                crate::output::info("No CI cache entries to clear")?;
            } else {
                crate::output::success(cformat!(
                    "Cleared <bold>{cleared}</> CI cache entr{}",
                    if cleared == 1 { "y" } else { "ies" }
                ))?;
            }
        }
        Some("default-branch") => {
            if repo
                .run_command(&["config", "--unset", "worktrunk.defaultBranch"])
                .is_ok()
            {
                crate::output::success("Cleared default branch cache")?;
            } else {
                crate::output::info("No default branch cache to clear")?;
            }
        }
        None => {
            let cleared_default = repo
                .run_command(&["config", "--unset", "worktrunk.defaultBranch"])
                .is_ok();
            let cleared_ci = CachedCiStatus::clear_all(&repo) > 0;

            if cleared_default || cleared_ci {
                crate::output::success("Cleared all caches")?;
            } else {
                crate::output::info("No caches to clear")?;
            }
        }
        Some(unknown) => {
            anyhow::bail!("Unknown cache type: {unknown}. Valid types: ci, default-branch");
        }
    }

    Ok(())
}

/// Handle the cache refresh command (refreshes default branch)
pub fn handle_cache_refresh() -> anyhow::Result<()> {
    let repo = Repository::current();

    crate::output::progress("Querying remote for default branch...")?;

    let branch = repo.refresh_default_branch()?;

    crate::output::success(cformat!("Cache refreshed: <bold>{branch}</>"))?;

    Ok(())
}
