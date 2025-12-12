use anyhow::Context;
use color_print::cformat;
use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};
use std::fmt::Write as _;
use std::path::PathBuf;
use worktrunk::config::WorktrunkConfig;
use worktrunk::config::{find_unknown_project_keys, find_unknown_user_keys};
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;
use worktrunk::shell::Shell;
use worktrunk::styling::{
    error_message, format_toml, format_with_gutter, hint_message, info_message, progress_message,
    success_message, warning_message,
};

use super::configure_shell::{ConfigAction, scan_shell_configs};
use super::list::ci_status::CachedCiStatus;
use crate::help_pager::show_help_in_pager;
use crate::llm::test_commit_generation;
use crate::output;

/// Example user configuration file content (displayed in help with values uncommented)
const USER_CONFIG_EXAMPLE: &str = include_str!("../../dev/config.example.toml");

/// Example project configuration file content
const PROJECT_CONFIG_EXAMPLE: &str = include_str!("../../dev/wt.example.toml");

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
pub fn handle_config_create(project: bool) -> anyhow::Result<()> {
    if project {
        let repo = Repository::current();
        let config_path = repo.worktree_root()?.join(".config/wt.toml");
        create_config_file(
            config_path,
            PROJECT_CONFIG_EXAMPLE,
            "Project config",
            &[
                "Edit this file to configure hooks for this repository",
                "See https://worktrunk.dev/hooks/ for hook documentation",
            ],
        )
    } else {
        create_config_file(
            require_user_config_path()?,
            USER_CONFIG_EXAMPLE,
            "User config",
            &["Edit this file to customize worktree paths and LLM settings"],
        )
    }
}

/// Create a config file at the specified path with the given content
fn create_config_file(
    path: PathBuf,
    content: &str,
    config_type: &str,
    success_hints: &[&str],
) -> anyhow::Result<()> {
    // Check if file already exists
    if path.exists() {
        output::print(info_message(cformat!(
            "{config_type} already exists: <bold>{}</>",
            format_path_for_display(&path)
        )))?;
        output::blank()?;
        output::print(hint_message(cformat!(
            "Use <bright-black>wt config show</> to view, or <bright-black>wt config create --help</> for format reference"
        )))?;
        return Ok(());
    }

    // Create parent directory if it doesn't exist
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create config directory")?;
    }

    // Write the example config with all values commented out
    let commented_config = comment_out_config(content);
    std::fs::write(&path, commented_config).context("Failed to write config file")?;

    // Success message
    output::print(success_message(cformat!(
        "Created {}: <bold>{}</>",
        config_type.to_lowercase(),
        format_path_for_display(&path)
    )))?;
    output::blank()?;
    for hint in success_hints {
        output::print(hint_message(*hint))?;
    }

    Ok(())
}

/// Handle the config show command
pub fn handle_config_show(full: bool) -> anyhow::Result<()> {
    // Build the complete output as a string
    let mut show_output = String::new();

    // Render user config
    render_user_config(&mut show_output)?;
    show_output.push('\n');

    // Render project config if in a git repository
    render_project_config(&mut show_output)?;
    show_output.push('\n');

    // Render shell integration status
    render_shell_status(&mut show_output)?;
    show_output.push('\n');

    // Render binaries status
    render_binaries_status(&mut show_output)?;

    // Display through pager (only if not in full mode, since full adds interactive output)
    if full {
        worktrunk::styling::eprintln!("{}", show_output);
    } else if let Err(e) = show_help_in_pager(&show_output) {
        log::debug!("Pager invocation failed: {}", e);
        // Fall back to direct output via eprintln (matches help behavior)
        worktrunk::styling::eprintln!("{}", show_output);
    }

    // Run full diagnostic checks if requested
    if full {
        run_full_checks()?;
    }

    Ok(())
}

/// Run full diagnostic checks (commit generation test)
fn run_full_checks() -> anyhow::Result<()> {
    output::print(info_message("Running diagnostic checks..."))?;
    output::blank()?;

    // Test commit generation
    let config = WorktrunkConfig::load()?;
    let commit_config = &config.commit_generation;

    if !commit_config.is_configured() {
        output::print(warning_message("Commit generation is not configured"))?;
        output::print(hint_message(cformat!(
            "Add <bright-black>[commit-generation]</> section to enable LLM commit messages"
        )))?;
        return Ok(());
    }

    let command_display = format!(
        "{}{}",
        commit_config.command.as_ref().unwrap(),
        if commit_config.args.is_empty() {
            String::new()
        } else {
            format!(" {}", commit_config.args.join(" "))
        }
    );

    output::print(progress_message(cformat!(
        "Testing commit generation with <bold>{command_display}</>"
    )))?;

    match test_commit_generation(commit_config) {
        Ok(message) => {
            output::print(success_message("Commit generation working"))?;
            output::blank()?;
            output::print(info_message("Sample generated message:"))?;
            output::gutter(format_with_gutter(&message, "", None))?;
        }
        Err(e) => {
            output::print(error_message("Commit generation failed"))?;
            output::gutter(format_with_gutter(&e.to_string(), "", None))?;
            output::blank()?;
            output::print(hint_message(
                "Check that the command is installed and API keys are configured",
            ))?;
        }
    }

    Ok(())
}

fn render_user_config(out: &mut String) -> anyhow::Result<()> {
    let config_path = require_user_config_path()?;

    writeln!(
        out,
        "{}",
        cformat!(
            "<cyan>USER CONFIG</>  {}",
            format_path_for_display(&config_path)
        )
    )?;

    // Check if file exists
    if !config_path.exists() {
        writeln!(
            out,
            "{}",
            hint_message(cformat!(
                "Not found (using defaults); <bright-black>wt config create</> creates one"
            ))
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
        writeln!(out, "{}", hint_message("Empty file (using defaults)"))?;
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
            warning_message(cformat!("Unknown key <bold>{key}</> will be ignored"))
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
                cformat!("<cyan><dim>PROJECT CONFIG</>  Not in a git repository</>")
            )?;
            return Ok(());
        }
    };
    let config_path = repo_root.join(".config").join("wt.toml");

    writeln!(
        out,
        "{}",
        cformat!(
            "<cyan>PROJECT CONFIG</>  {}",
            format_path_for_display(&config_path)
        )
    )?;

    // Check if file exists
    if !config_path.exists() {
        writeln!(out, "{}", hint_message("Not found"))?;
        return Ok(());
    }

    // Read and display the file contents
    let contents = std::fs::read_to_string(&config_path).context("Failed to read config file")?;

    if contents.trim().is_empty() {
        writeln!(out, "{}", hint_message("Empty file"))?;
        return Ok(());
    }

    // Check for unknown keys and warn
    warn_unknown_keys(out, &find_unknown_project_keys(&contents))?;

    // Display TOML with syntax highlighting (gutter at column 0)
    write!(out, "{}", format_toml(&contents, ""))?;

    Ok(())
}

fn render_shell_status(out: &mut String) -> anyhow::Result<()> {
    writeln!(out, "{}", cformat!("<cyan>SHELL INTEGRATION</>"))?;

    // Use the same detection logic as `wt config shell install`
    let scan_result = match scan_shell_configs(None, true, "wt") {
        Ok(r) => r,
        Err(e) => {
            writeln!(
                out,
                "{}",
                hint_message(format!("Could not determine shell status: {e}"))
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
                    info_message(cformat!(
                        "Already configured {what} for <bold>{shell}</> @ {path}"
                    ))
                )?;

                // Check if zsh has compinit enabled (required for completions)
                if matches!(shell, Shell::Zsh) && check_zsh_compinit_missing() {
                    writeln!(
                        out,
                        "{}",
                        warning_message(
                            "Completions won't work; add to ~/.zshrc before the wt line:"
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
                            info_message(cformat!(
                                "Already configured completions for <bold>{shell}</> @ {completion_display}"
                            ))
                        )?;
                    } else {
                        any_not_configured = true;
                        writeln!(
                            out,
                            "{}",
                            hint_message(format!(
                                "Not configured completions for {shell} @ {completion_display}"
                            ))
                        )?;
                    }
                }
            }
            ConfigAction::WouldAdd | ConfigAction::WouldCreate => {
                any_not_configured = true;
                writeln!(
                    out,
                    "{}",
                    hint_message(format!("Not configured {what} for {shell} @ {path}"))
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
            info_message(cformat!("<dim>Skipped {shell}; {path} not found</>"))
        )?;
    }

    // Summary hint
    if any_not_configured {
        writeln!(out)?;
        writeln!(
            out,
            "{}",
            hint_message(cformat!(
                "<bright-black>wt config shell install</> enables shell integration"
            ))
        )?;
    }

    Ok(())
}

fn render_binaries_status(out: &mut String) -> anyhow::Result<()> {
    use super::list::ci_status::CiToolsStatus;

    writeln!(out, "{}", cformat!("<cyan>BINARIES</>"))?;

    let ci_tools = CiToolsStatus::detect();

    // Check gh (GitHub CLI)
    if ci_tools.gh_installed {
        if ci_tools.gh_authenticated {
            writeln!(
                out,
                "{}",
                info_message(cformat!("<bold>gh</> installed & authenticated"))
            )?;
        } else {
            writeln!(
                out,
                "{}",
                warning_message(cformat!(
                    "<bold>gh</> installed but not authenticated; run <bright-black>gh auth login</>"
                ))
            )?;
        }
    } else {
        writeln!(
            out,
            "{}",
            hint_message(cformat!(
                "<bold>gh</> not found (GitHub CI status unavailable)"
            ))
        )?;
    }

    // Check glab (GitLab CLI)
    if ci_tools.glab_installed {
        if ci_tools.glab_authenticated {
            writeln!(
                out,
                "{}",
                info_message(cformat!("<bold>glab</> installed & authenticated"))
            )?;
        } else {
            writeln!(
                out,
                "{}",
                warning_message(cformat!(
                    "<bold>glab</> installed but not authenticated; run <bright-black>glab auth login</>"
                ))
            )?;
        }
    } else {
        writeln!(
            out,
            "{}",
            hint_message(cformat!(
                "<bold>glab</> not found (GitLab CI status unavailable)"
            ))
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
    if std::env::var("WORKTRUNK_TEST_COMPINIT_CONFIGURED").is_ok() {
        return false; // Assume compinit is configured
    }

    // Force compinit to be missing (for tests that expect the warning)
    if std::env::var("WORKTRUNK_TEST_COMPINIT_MISSING").is_ok() {
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

/// Handle the var get command
pub fn handle_var_get(key: &str, refresh: bool, branch: Option<String>) -> anyhow::Result<()> {
    use super::list::ci_status::PrStatus;

    let repo = Repository::current();

    match key {
        "default-branch" => {
            let branch_name = if refresh {
                crate::output::print(progress_message("Querying remote for default branch..."))?;
                repo.refresh_default_branch()?
            } else {
                repo.default_branch()?
            };
            crate::output::data(branch_name)?;
        }
        "marker" => {
            let branch_name = match branch {
                Some(b) => b,
                None => repo.require_current_branch("get marker for current branch")?,
            };
            match repo.branch_keyed_marker(&branch_name) {
                Some(marker) => crate::output::data(marker)?,
                None => crate::output::data("")?,
            }
        }
        "ci-status" => {
            let branch_name = match branch {
                Some(b) => b,
                None => repo.require_current_branch("get ci-status for current branch")?,
            };

            let repo_root = repo.worktree_root()?;

            // Get the HEAD commit for this branch
            let head = repo
                .run_command(&["rev-parse", &branch_name])
                .map(|s| s.trim().to_string())
                .unwrap_or_default();

            if head.is_empty() {
                anyhow::bail!("Branch '{branch_name}' not found");
            }

            if refresh {
                crate::output::print(progress_message("Fetching CI status..."))?;
                // Clear cache to force refresh
                let config_key = format!(
                    "worktrunk.ci.{}",
                    super::list::ci_status::CachedCiStatus::escape_branch(&branch_name)
                );
                let _ = repo.run_command(&["config", "--unset", &config_key]);
            }

            let has_upstream = repo.upstream_branch(&branch_name).ok().flatten().is_some();
            match PrStatus::detect(&branch_name, &head, &repo_root, has_upstream) {
                Some(status) => {
                    // Output the CI status as a simple string for piping
                    let status_str = match status.ci_status {
                        super::list::ci_status::CiStatus::Passed => "passed",
                        super::list::ci_status::CiStatus::Running => "running",
                        super::list::ci_status::CiStatus::Failed => "failed",
                        super::list::ci_status::CiStatus::Conflicts => "conflicts",
                        super::list::ci_status::CiStatus::NoCI => "noci",
                        super::list::ci_status::CiStatus::Error => "error",
                    };
                    crate::output::data(status_str)?;
                }
                None => {
                    crate::output::data("noci")?;
                }
            }
        }
        _ => anyhow::bail!(
            "Unknown variable: {key}. Valid variables: default-branch, marker, ci-status"
        ),
    }

    Ok(())
}

/// Handle the var set command
pub fn handle_var_set(key: &str, value: String, branch: Option<String>) -> anyhow::Result<()> {
    let repo = Repository::current();

    match key {
        "marker" => {
            // TODO: Worktree-specific marker (worktrunk.marker with --worktree flag) would allow
            // different markers per worktree, but requires extensions.worktreeConfig which adds
            // complexity. Our intended workflow is one branch per worktree, so branch-keyed marker
            // is sufficient for now.

            let branch_name = match branch {
                Some(b) => b,
                None => repo.require_current_branch("set marker for current branch")?,
            };

            let config_key = format!("worktrunk.marker.{}", branch_name);
            repo.run_command(&["config", &config_key, &value])?;

            crate::output::print(success_message(cformat!(
                "Set marker for <bold>{branch_name}</> to <bold>{value}</>"
            )))?;
        }
        _ => anyhow::bail!("Unknown variable: {key}. Valid variables: marker"),
    }

    Ok(())
}

/// Handle the var clear command
pub fn handle_var_clear(key: &str, branch: Option<String>, all: bool) -> anyhow::Result<()> {
    let repo = Repository::current();

    match key {
        "marker" => {
            if all {
                // Clear all branch-keyed markers
                // Note: git config --get-regexp exits with code 1 when no matches found,
                // so we treat errors the same as empty output (both mean "no markers")
                let output = repo
                    .run_command(&["config", "--get-regexp", "^worktrunk\\.marker\\."])
                    .unwrap_or_default();

                let mut cleared_count = 0;
                for line in output.lines() {
                    if let Some(config_key) = line.split_whitespace().next() {
                        repo.run_command(&["config", "--unset", config_key])?;
                        cleared_count += 1;
                    }
                }

                if cleared_count == 0 {
                    crate::output::print(info_message("No markers to clear"))?;
                } else {
                    crate::output::print(success_message(cformat!(
                        "Cleared <bold>{cleared_count}</> marker{}",
                        if cleared_count == 1 { "" } else { "s" }
                    )))?;
                }
            } else {
                // Clear specific branch marker
                let branch_name = match branch {
                    Some(b) => b,
                    None => repo.require_current_branch("clear marker for current branch")?,
                };

                let config_key = format!("worktrunk.marker.{}", branch_name);
                repo.run_command(&["config", "--unset", &config_key])
                    .context("Failed to clear marker (may not be set)")?;

                crate::output::print(success_message(cformat!(
                    "Cleared marker for <bold>{branch_name}</>"
                )))?;
            }
        }
        _ => anyhow::bail!("Unknown variable: {key}. Valid variables: marker"),
    }

    Ok(())
}

/// Handle the cache show command
pub fn handle_cache_show() -> anyhow::Result<()> {
    let repo = Repository::current();

    // Show default branch cache (value from git config, so use gutter)
    crate::output::print(info_message("Default branch cache:"))?;
    match repo.default_branch() {
        Ok(branch) => crate::output::gutter(format_with_gutter(&branch, "", None))?,
        Err(_) => crate::output::gutter(format_with_gutter("(not cached)", "", None))?,
    }
    crate::output::blank()?;

    // Show CI status cache
    crate::output::print(info_message("CI status cache:"))?;

    let entries = CachedCiStatus::list_all(&repo);
    if entries.is_empty() {
        crate::output::gutter(format_with_gutter("(empty)", "", None))?;
        return Ok(());
    }

    // Respect SOURCE_DATE_EPOCH for reproducible test output
    let now_secs = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|val| val.parse::<u64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        });

    // Build markdown table
    let mut table = String::from("| Branch | Status | Age | Head |\n");
    table.push_str("|--------|--------|-----|------|\n");
    for (branch, cached) in entries {
        let status = match &cached.status {
            Some(pr_status) => serde_json::to_string(&pr_status.ci_status)
                .map(|s| s.trim_matches('"').to_string())
                .unwrap_or_else(|_| "unknown".to_string()),
            None => "none".to_string(),
        };
        let age = now_secs.saturating_sub(cached.checked_at);
        let head: String = cached.head.chars().take(8).collect();

        table.push_str(&format!("| {branch} | {status} | {age}s | {head} |\n"));
    }

    let rendered = crate::md_help::render_markdown_table(&table);
    crate::output::table(rendered.trim_end())?;

    Ok(())
}

/// Handle the cache clear command
pub fn handle_cache_clear(cache_type: Option<String>) -> anyhow::Result<()> {
    let repo = Repository::current();

    match cache_type.as_deref() {
        Some("ci") => {
            let cleared = CachedCiStatus::clear_all(&repo);
            if cleared == 0 {
                crate::output::print(info_message("No CI cache entries to clear"))?;
            } else {
                crate::output::print(success_message(cformat!(
                    "Cleared <bold>{cleared}</> CI cache entr{}",
                    if cleared == 1 { "y" } else { "ies" }
                )))?;
            }
        }
        Some("default-branch") => {
            if repo
                .run_command(&["config", "--unset", "worktrunk.defaultBranch"])
                .is_ok()
            {
                crate::output::print(success_message("Cleared default branch cache"))?;
            } else {
                crate::output::print(info_message("No default branch cache to clear"))?;
            }
        }
        Some("logs") => {
            let cleared = clear_logs(&repo)?;
            if cleared == 0 {
                crate::output::print(info_message("No logs to clear"))?;
            } else {
                crate::output::print(success_message(cformat!(
                    "Cleared <bold>{cleared}</> log file{}",
                    if cleared == 1 { "" } else { "s" }
                )))?;
            }
        }
        None => {
            let cleared_default = repo
                .run_command(&["config", "--unset", "worktrunk.defaultBranch"])
                .is_ok();
            let cleared_ci = CachedCiStatus::clear_all(&repo) > 0;
            let cleared_logs = clear_logs(&repo)? > 0;

            if cleared_default || cleared_ci || cleared_logs {
                crate::output::print(success_message("Cleared all caches"))?;
            } else {
                crate::output::print(info_message("No caches to clear"))?;
            }
        }
        Some(unknown) => {
            anyhow::bail!("Unknown cache type: {unknown}. Valid types: ci, default-branch, logs");
        }
    }

    Ok(())
}

/// Clear all log files from the wt-logs directory
fn clear_logs(repo: &Repository) -> anyhow::Result<usize> {
    let git_common_dir = repo.git_common_dir()?;
    let log_dir = git_common_dir.join("wt-logs");

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

/// Handle the cache refresh command (refreshes default branch)
pub fn handle_cache_refresh() -> anyhow::Result<()> {
    let repo = Repository::current();

    crate::output::print(progress_message("Querying remote for default branch..."))?;

    let branch = repo.refresh_default_branch()?;

    crate::output::print(success_message(cformat!(
        "Cache refreshed: <bold>{branch}</>"
    )))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_comment_out_config_basic() {
        let input = "key = \"value\"\n";
        let expected = "# key = \"value\"\n";
        assert_eq!(comment_out_config(input), expected);
    }

    #[test]
    fn test_comment_out_config_preserves_existing_comments() {
        let input = "# This is a comment\nkey = \"value\"\n";
        let expected = "# This is a comment\n# key = \"value\"\n";
        assert_eq!(comment_out_config(input), expected);
    }

    #[test]
    fn test_comment_out_config_preserves_empty_lines() {
        let input = "key1 = \"value\"\n\nkey2 = \"value\"\n";
        let expected = "# key1 = \"value\"\n\n# key2 = \"value\"\n";
        assert_eq!(comment_out_config(input), expected);
    }

    #[test]
    fn test_comment_out_config_preserves_trailing_newline() {
        let with_newline = "key = \"value\"\n";
        let without_newline = "key = \"value\"";

        assert!(comment_out_config(with_newline).ends_with('\n'));
        assert!(!comment_out_config(without_newline).ends_with('\n'));
    }

    #[test]
    fn test_comment_out_config_section_headers() {
        let input = "[hooks]\ncommand = \"npm test\"\n";
        let expected = "# [hooks]\n# command = \"npm test\"\n";
        assert_eq!(comment_out_config(input), expected);
    }
}
