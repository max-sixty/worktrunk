use anyhow::Context;
use color_print::cformat;
use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};
use std::fmt::Write as _;
use std::path::PathBuf;
use worktrunk::config::WorktrunkConfig;
use worktrunk::config::{find_unknown_project_keys, find_unknown_user_keys};
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;
use worktrunk::shell::{Shell, scan_for_detection_details};
use worktrunk::styling::{
    error_message, format_bash_with_gutter, format_heading, format_toml, format_with_gutter,
    hint_message, info_message, progress_message, success_message, warning_message,
};
use worktrunk::utils::get_now;

use super::configure_shell::{ConfigAction, scan_shell_configs};
use super::list::ci_status::CachedCiStatus;
use crate::cli::version_str;
use crate::display::format_relative_time_short;
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
                "See https://worktrunk.dev/hook/ for hook documentation",
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
            "For format reference, run <bright-black>wt config create --help</>; to view, run <bright-black>wt config show</>"
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

    // Run full diagnostic checks if requested (includes slow network calls)
    if full {
        show_output.push('\n');
        render_diagnostics(&mut show_output)?;
    }

    // Render runtime info at the bottom (version, binary name, shell integration status)
    show_output.push('\n');
    render_runtime_info(&mut show_output)?;

    // Display through pager
    if let Err(e) = show_help_in_pager(&show_output) {
        log::debug!("Pager invocation failed: {}", e);
        // Fall back to direct output via eprintln (matches help behavior)
        worktrunk::styling::eprintln!("{}", show_output);
    }

    Ok(())
}

/// Render runtime information (version, binary name, shell integration status)
fn render_runtime_info(out: &mut String) -> anyhow::Result<()> {
    let cmd = crate::binary_name();
    let version = version_str();
    let shell_active = output::is_shell_integration_active();

    // Version as suffix to heading (avoid lonely gutter)
    writeln!(
        out,
        "{}",
        format_heading("RUNTIME", Some(&cformat!("{cmd} {version}")))
    )?;

    // Shell integration status
    if shell_active {
        writeln!(out, "{}", info_message("Shell integration active"))?;
    } else {
        writeln!(out, "{}", warning_message("Shell integration not active"))?;
    }

    // Debug info for shell integration troubleshooting
    // Show invocation details that help diagnose issues like https://github.com/max-sixty/worktrunk/pull/387
    let invocation = crate::invocation_path();
    let is_git_subcommand = crate::is_git_subcommand();

    // Build debug lines
    let mut debug_lines = Vec::new();

    // Always show how wt was invoked
    debug_lines.push(cformat!("Invoked as: <bold>{invocation}</>"));

    // Show if running as git subcommand (affects shell integration)
    if is_git_subcommand {
        debug_lines.push("Git subcommand: yes (GIT_EXEC_PATH set)".to_string());
    }

    // Show shell integration debug info in a gutter
    let debug_text = debug_lines.join("\n");
    writeln!(out, "{}", format_with_gutter(&debug_text, None))?;

    // Show hyperlink support status (separate from shell integration)
    let hyperlinks_supported =
        worktrunk::styling::supports_hyperlinks(worktrunk::styling::Stream::Stderr);
    let status = if hyperlinks_supported {
        "active"
    } else {
        "inactive"
    };
    writeln!(
        out,
        "{}",
        info_message(cformat!("Hyperlinks: <bold>{status}</>"))
    )?;

    Ok(())
}

/// Run full diagnostic checks (CI tools, commit generation) and render to buffer
fn render_diagnostics(out: &mut String) -> anyhow::Result<()> {
    use super::list::ci_status::{CiPlatform, CiToolsStatus, get_platform_for_repo};

    writeln!(out, "{}", format_heading("DIAGNOSTICS", None))?;

    // Check CI tool based on detected platform
    let platform = Repository::current()
        .worktree_root()
        .ok()
        .and_then(|root| get_platform_for_repo(root.to_str()?));

    match platform {
        Some(CiPlatform::GitHub) => {
            let ci_tools = CiToolsStatus::detect(None);
            render_ci_tool_status(
                out,
                "gh",
                "GitHub",
                ci_tools.gh_installed,
                ci_tools.gh_authenticated,
            )?;
        }
        Some(CiPlatform::GitLab) => {
            let ci_tools = CiToolsStatus::detect(None);
            render_ci_tool_status(
                out,
                "glab",
                "GitLab",
                ci_tools.glab_installed,
                ci_tools.glab_authenticated,
            )?;
        }
        None => {
            writeln!(
                out,
                "{}",
                hint_message("CI status requires GitHub or GitLab remote")
            )?;
        }
    }

    // Test commit generation
    let config = WorktrunkConfig::load()?;
    let commit_config = &config.commit_generation;

    if !commit_config.is_configured() {
        writeln!(out, "{}", hint_message("Commit generation not configured"))?;
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

    match test_commit_generation(commit_config) {
        Ok(message) => {
            writeln!(
                out,
                "{}",
                success_message(cformat!(
                    "Commit generation working (<bold>{command_display}</>)"
                ))
            )?;
            writeln!(out, "{}", format_with_gutter(&message, None))?;
        }
        Err(e) => {
            writeln!(
                out,
                "{}",
                error_message(cformat!(
                    "Commit generation failed (<bold>{command_display}</>)"
                ))
            )?;
            writeln!(out, "{}", format_with_gutter(&e.to_string(), None))?;
        }
    }

    Ok(())
}

fn render_user_config(out: &mut String) -> anyhow::Result<()> {
    let config_path = require_user_config_path()?;

    writeln!(
        out,
        "{}",
        format_heading("USER CONFIG", Some(&format_path_for_display(&config_path)))
    )?;

    // Check if file exists
    if !config_path.exists() {
        writeln!(
            out,
            "{}",
            hint_message(cformat!(
                "Not found; to create one, run <bright-black>wt config create</>"
            ))
        )?;
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
    write!(out, "{}", format_toml(&contents))?;

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
                cformat!(
                    "<dim>{}</>",
                    format_heading("PROJECT CONFIG", Some("Not in a git repository"))
                )
            )?;
            return Ok(());
        }
    };
    let config_path = repo_root.join(".config").join("wt.toml");

    writeln!(
        out,
        "{}",
        format_heading(
            "PROJECT CONFIG",
            Some(&format_path_for_display(&config_path))
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
    write!(out, "{}", format_toml(&contents))?;

    Ok(())
}

fn render_shell_status(out: &mut String) -> anyhow::Result<()> {
    writeln!(out, "{}", format_heading("SHELL INTEGRATION", None))?;

    // Use the same detection logic as `wt config shell install`
    let cmd = crate::binary_name();
    let scan_result = match scan_shell_configs(None, true, &cmd) {
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

    // Get detection details to show matched lines inline
    let detection_results = scan_for_detection_details(&cmd).unwrap_or_default();

    let mut any_not_configured = false;
    let mut has_any_unmatched = false;

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
                // Show the matched lines directly under this status
                let detection = detection_results
                    .iter()
                    .find(|d| d.path == result.path && !d.matched_lines.is_empty());

                // Build file:line location (clickable in terminals)
                let location = if let Some(det) = detection {
                    let line_nums: Vec<_> = det
                        .matched_lines
                        .iter()
                        .map(|d| d.line_number.to_string())
                        .collect();
                    format!("{}:{}", path, line_nums.join(","))
                } else {
                    path.to_string()
                };

                writeln!(
                    out,
                    "{}",
                    info_message(cformat!(
                        "Already configured {what} for <bold>{shell}</> @ {location}"
                    ))
                )?;

                if let Some(det) = detection {
                    for detected in &det.matched_lines {
                        writeln!(out, "{}", format_bash_with_gutter(detected.content.trim()))?;
                    }
                }

                // Check if zsh has compinit enabled (required for completions)
                if matches!(shell, Shell::Zsh) && check_zsh_compinit_missing() {
                    writeln!(
                        out,
                        "{}",
                        warning_message(
                            "Completions won't work; add to ~/.zshrc before the wt line:"
                        )
                    )?;
                    writeln!(
                        out,
                        "{}",
                        format_with_gutter("autoload -Uz compinit && compinit", None)
                    )?;
                }

                // For fish, check completions file separately
                if matches!(shell, Shell::Fish)
                    && let Ok(completion_path) = shell.completion_path(&cmd)
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
                "To enable shell integration, run <bright-black>wt config shell install</>"
            ))
        )?;
    }

    // Show potential false negatives (lines containing cmd but not detected)
    for detection in &detection_results {
        if !detection.unmatched_candidates.is_empty() {
            has_any_unmatched = true;
            let path = format_path_for_display(&detection.path);
            // Build file:line location (clickable in terminals)
            let line_nums: Vec<_> = detection
                .unmatched_candidates
                .iter()
                .map(|d| d.line_number.to_string())
                .collect();
            let location = format!("{}:{}", path, line_nums.join(","));
            writeln!(
                out,
                "{}",
                warning_message(cformat!(
                    "Found <bold>{cmd}</> in <bold>{location}</> but not detected as integration:"
                ))
            )?;
            for detected in &detection.unmatched_candidates {
                writeln!(out, "{}", format_bash_with_gutter(detected.content.trim()))?;
            }
        }
    }

    // If we have unmatched candidates but no configured shells, suggest raising an issue
    let has_any_configured = scan_result
        .configured
        .iter()
        .any(|r| matches!(r.action, ConfigAction::AlreadyExists));
    if has_any_unmatched && !has_any_configured {
        let unmatched_summary: Vec<_> = detection_results
            .iter()
            .filter(|r| !r.unmatched_candidates.is_empty())
            .flat_map(|r| {
                r.unmatched_candidates
                    .iter()
                    .map(|d| d.content.trim().to_string())
            })
            .collect();
        let body = format!(
            "Shell integration not detected despite config containing `{cmd}`.\n\n\
             **Unmatched lines:**\n```\n{}\n```\n\n\
             **Expected behavior:** These lines should be detected as shell integration.",
            unmatched_summary.join("\n")
        );
        let issue_url = format!(
            "https://github.com/max-sixty/worktrunk/issues/new?title={}&body={}",
            urlencoding::encode("Shell integration detection false negative"),
            urlencoding::encode(&body)
        );

        writeln!(out)?;
        writeln!(
            out,
            "{}",
            hint_message(format!(
                "If this is shell integration, report a false negative: {issue_url}"
            ))
        )?;
    }

    Ok(())
}

fn render_ci_tool_status(
    out: &mut String,
    tool: &str,
    platform: &str,
    installed: bool,
    authenticated: bool,
) -> anyhow::Result<()> {
    if installed {
        if authenticated {
            writeln!(
                out,
                "{}",
                success_message(cformat!("<bold>{tool}</> installed & authenticated"))
            )?;
        } else {
            writeln!(
                out,
                "{}",
                warning_message(cformat!(
                    "<bold>{tool}</> installed but not authenticated; run <bold>{tool} auth login</>"
                ))
            )?;
        }
    } else {
        writeln!(
            out,
            "{}",
            hint_message(cformat!(
                "<bold>{tool}</> not found ({platform} CI status unavailable)"
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
    use std::process::Command;
    use worktrunk::shell_exec::run;

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
    let mut cmd = Command::new("zsh");
    cmd.args(["--no-globalrcs", "-ic", "(( $+functions[compdef] ))"]);
    // Suppress zsh's "insecure directories" warning from compinit.
    // See detailed rationale in shell::detect_zsh_compinit().
    cmd.env("ZSH_DISABLE_COMPFIX", "true");
    let Ok(output) = run(&mut cmd, None) else {
        return false; // Can't determine, don't warn
    };

    // compdef NOT found = need to warn
    !output.status.success()
}

/// Core logic for determining user config path from env var values
fn resolve_user_config_path(xdg_config_home: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
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

fn get_user_config_path() -> Option<PathBuf> {
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

fn require_user_config_path() -> anyhow::Result<PathBuf> {
    get_user_config_path().ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot determine config directory. Set $HOME or $XDG_CONFIG_HOME environment variable"
        )
    })
}

/// Clear all log files from the wt-logs directory
fn clear_logs(repo: &Repository) -> anyhow::Result<usize> {
    let log_dir = repo.wt_logs_dir()?;

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
fn render_log_files(out: &mut String, repo: &Repository) -> anyhow::Result<()> {
    let log_dir = repo.wt_logs_dir()?;
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

/// Handle the state get command
pub fn handle_state_get(key: &str, refresh: bool, branch: Option<String>) -> anyhow::Result<()> {
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
            crate::output::stdout(branch_name)?;
        }
        "previous-branch" => match repo.get_switch_previous() {
            Some(prev) => crate::output::stdout(prev)?,
            None => crate::output::stdout("")?,
        },
        "marker" => {
            let branch_name = match branch {
                Some(b) => b,
                None => repo.require_current_branch("get marker for current branch")?,
            };
            match repo.branch_keyed_marker(&branch_name) {
                Some(marker) => crate::output::stdout(marker)?,
                None => crate::output::stdout("")?,
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
                let config_key = format!("worktrunk.state.{branch_name}.ci-status");
                let _ = repo.run_command(&["config", "--unset", &config_key]);
            }

            let has_upstream = repo.upstream_branch(&branch_name).ok().flatten().is_some();
            let ci_status = PrStatus::detect(&branch_name, &head, repo_root, has_upstream)
                .map_or(super::list::ci_status::CiStatus::NoCI, |s| s.ci_status);
            let status_str: &'static str = ci_status.into();
            crate::output::stdout(status_str)?;
        }
        // TODO: Consider simplifying to just print the path and let users run `ls -al` themselves
        "logs" => {
            let mut out = String::new();
            render_log_files(&mut out, &repo)?;

            // Display through pager (fall back to stderr if pager unavailable)
            if show_help_in_pager(&out).is_err() {
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
    let repo = Repository::current();

    match key {
        "default-branch" => {
            repo.set_default_branch(&value)?;
            crate::output::print(success_message(cformat!(
                "Set default branch to <bold>{value}</>"
            )))?;
        }
        "previous-branch" => {
            repo.record_switch_previous(Some(&value))?;
            crate::output::print(success_message(cformat!(
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

            crate::output::print(success_message(cformat!(
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
    let repo = Repository::current();

    match key {
        "default-branch" => {
            if repo.clear_default_branch_cache()? {
                crate::output::print(success_message("Cleared default branch cache"))?;
            } else {
                crate::output::print(info_message("No default branch cache to clear"))?;
            }
        }
        "previous-branch" => {
            if repo
                .run_command(&["config", "--unset", "worktrunk.history"])
                .is_ok()
            {
                crate::output::print(success_message("Cleared previous branch"))?;
            } else {
                crate::output::print(info_message("No previous branch to clear"))?;
            }
        }
        "ci-status" => {
            if all {
                let cleared = CachedCiStatus::clear_all(&repo);
                if cleared == 0 {
                    crate::output::print(info_message("No CI cache entries to clear"))?;
                } else {
                    crate::output::print(success_message(cformat!(
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
                    crate::output::print(success_message(cformat!(
                        "Cleared CI cache for <bold>{branch_name}</>"
                    )))?;
                } else {
                    crate::output::print(info_message(cformat!(
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
                    crate::output::print(info_message("No markers to clear"))?;
                } else {
                    crate::output::print(success_message(cformat!(
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
                    crate::output::print(success_message(cformat!(
                        "Cleared marker for <bold>{branch_name}</>"
                    )))?;
                } else {
                    crate::output::print(info_message(cformat!(
                        "No marker set for <bold>{branch_name}</>"
                    )))?;
                }
            }
        }
        "logs" => {
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
    let repo = Repository::current();
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

    if cleared_any {
        crate::output::print(success_message("Cleared all stored state"))?;
    } else {
        crate::output::print(info_message("No stored state to clear"))?;
    }

    Ok(())
}

/// Handle the state get command (shows all state)
pub fn handle_state_show(format: crate::cli::OutputFormat) -> anyhow::Result<()> {
    use crate::cli::OutputFormat;

    let repo = Repository::current();

    match format {
        OutputFormat::Json => handle_state_show_json(&repo),
        OutputFormat::Table => handle_state_show_table(&repo),
    }
}

/// Output state as JSON
fn handle_state_show_json(repo: &Repository) -> anyhow::Result<()> {
    // Get default branch
    let default_branch = repo.default_branch().ok();

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
    let log_dir = repo.wt_logs_dir()?;
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

    let output = serde_json::json!({
        "default_branch": default_branch,
        "previous_branch": previous_branch,
        "markers": markers,
        "ci_status": ci_status,
        "logs": logs
    });

    crate::output::stdout(serde_json::to_string_pretty(&output)?)?;
    Ok(())
}

/// Output state as human-readable table
fn handle_state_show_table(repo: &Repository) -> anyhow::Result<()> {
    // Build complete output as a string
    let mut out = String::new();

    // Show default branch cache
    writeln!(out, "{}", format_heading("DEFAULT BRANCH", None))?;
    match repo.default_branch() {
        Ok(branch) => writeln!(out, "{}", format_with_gutter(&branch, None))?,
        Err(_) => writeln!(out, "{}", format_with_gutter("(not cached)", None))?,
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

    // Show log files
    render_log_files(&mut out, repo)?;

    // Display through pager (fall back to stderr if pager unavailable)
    if let Err(e) = show_help_in_pager(&out) {
        log::debug!("Pager invocation failed: {}", e);
        // Fall back to direct output via eprintln (matches help behavior)
        worktrunk::styling::eprintln!("{}", out);
    }

    Ok(())
}

/// Marker entry with branch, text, and timestamp
struct MarkerEntry {
    branch: String,
    marker: String,
    set_at: u64,
}

/// Get all branch markers from git config with timestamps
fn get_all_markers(repo: &Repository) -> Vec<MarkerEntry> {
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

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== comment_out_config tests ====================

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

    #[test]
    fn test_comment_out_config_empty_input() {
        assert_eq!(comment_out_config(""), "");
    }

    #[test]
    fn test_comment_out_config_only_empty_lines() {
        let input = "\n\n\n";
        let expected = "\n\n\n";
        assert_eq!(comment_out_config(input), expected);
    }

    #[test]
    fn test_comment_out_config_only_comments() {
        let input = "# comment 1\n# comment 2\n";
        let expected = "# comment 1\n# comment 2\n";
        assert_eq!(comment_out_config(input), expected);
    }

    #[test]
    fn test_comment_out_config_mixed_content() {
        let input =
            "# Header comment\n\n[section]\nkey = \"value\"\n\n# Another comment\nkey2 = true\n";
        let expected = "# Header comment\n\n# [section]\n# key = \"value\"\n\n# Another comment\n# key2 = true\n";
        assert_eq!(comment_out_config(input), expected);
    }

    #[test]
    fn test_comment_out_config_inline_table() {
        let input = "point = { x = 1, y = 2 }\n";
        let expected = "# point = { x = 1, y = 2 }\n";
        assert_eq!(comment_out_config(input), expected);
    }

    #[test]
    fn test_comment_out_config_multiline_array() {
        let input = "args = [\n  \"--flag\",\n  \"value\"\n]\n";
        let expected = "# args = [\n#   \"--flag\",\n#   \"value\"\n# ]\n";
        assert_eq!(comment_out_config(input), expected);
    }

    #[test]
    fn test_comment_out_config_whitespace_only_line() {
        // Lines with only whitespace are not empty - they should NOT be commented
        // Actually, let's check what the current behavior is:
        // The function checks `!line.is_empty()` - a line with spaces is not empty
        let input = "key = 1\n   \nkey2 = 2\n";
        let expected = "# key = 1\n#    \n# key2 = 2\n";
        assert_eq!(comment_out_config(input), expected);
    }

    // ==================== warn_unknown_keys tests ====================

    #[test]
    fn test_warn_unknown_keys_empty() {
        let mut out = String::new();
        warn_unknown_keys(&mut out, &[]).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn test_warn_unknown_keys_single() {
        let mut out = String::new();
        warn_unknown_keys(&mut out, &["unknown-key".to_string()]).unwrap();
        assert!(out.contains("unknown-key"));
        assert!(out.contains("Unknown key"));
    }

    #[test]
    fn test_warn_unknown_keys_multiple() {
        let mut out = String::new();
        warn_unknown_keys(&mut out, &["key1".to_string(), "key2".to_string()]).unwrap();
        assert!(out.contains("key1"));
        assert!(out.contains("key2"));
        // Should have two separate warning lines
        assert_eq!(out.matches("Unknown key").count(), 2);
    }

    // ==================== render_ci_tool_status tests ====================

    #[test]
    fn test_render_ci_tool_status_installed_authenticated() {
        let mut out = String::new();
        render_ci_tool_status(&mut out, "gh", "GitHub", true, true).unwrap();
        assert!(out.contains("gh"));
        assert!(out.contains("installed"));
        assert!(out.contains("authenticated"));
    }

    #[test]
    fn test_render_ci_tool_status_installed_not_authenticated() {
        let mut out = String::new();
        render_ci_tool_status(&mut out, "gh", "GitHub", true, false).unwrap();
        assert!(out.contains("gh"));
        assert!(out.contains("installed"));
        assert!(out.contains("not authenticated"));
        assert!(out.contains("gh auth login"));
    }

    #[test]
    fn test_render_ci_tool_status_not_installed() {
        let mut out = String::new();
        render_ci_tool_status(&mut out, "glab", "GitLab", false, false).unwrap();
        assert!(out.contains("glab"));
        assert!(out.contains("not found"));
        assert!(out.contains("GitLab"));
        assert!(out.contains("CI status unavailable"));
    }

    #[test]
    fn test_render_ci_tool_status_glab() {
        let mut out = String::new();
        render_ci_tool_status(&mut out, "glab", "GitLab", true, true).unwrap();
        assert!(out.contains("glab"));
        assert!(out.contains("installed"));
        assert!(out.contains("authenticated"));
    }

    // ==================== resolve_user_config_path tests ====================

    #[test]
    fn test_resolve_user_config_path_xdg_takes_priority() {
        let path = resolve_user_config_path(Some("/custom/xdg"), Some("/home/user"));
        assert_eq!(
            path,
            Some(PathBuf::from("/custom/xdg/worktrunk/config.toml"))
        );
    }

    #[test]
    fn test_resolve_user_config_path_home_fallback() {
        let path = resolve_user_config_path(None, Some("/home/testuser"));
        assert_eq!(
            path,
            Some(PathBuf::from(
                "/home/testuser/.config/worktrunk/config.toml"
            ))
        );
    }

    #[test]
    fn test_resolve_user_config_path_none_when_no_env() {
        let path = resolve_user_config_path(None, None);
        assert_eq!(path, None);
    }

    // ==================== get_user_config_path tests ====================

    #[test]
    fn test_get_user_config_path_returns_some() {
        // In a normal environment, get_user_config_path should return Some
        // (either from env vars or etcetera fallback)
        let path = get_user_config_path();
        assert!(path.is_some());
        let path = path.unwrap();
        assert!(path.ends_with("worktrunk/config.toml"));
    }

    // ==================== require_user_config_path tests ====================

    #[test]
    fn test_require_user_config_path_returns_ok() {
        // In a normal environment, require_user_config_path should succeed
        let result = require_user_config_path();
        assert!(result.is_ok());
        let path = result.unwrap();
        assert!(path.ends_with("worktrunk/config.toml"));
    }
}
