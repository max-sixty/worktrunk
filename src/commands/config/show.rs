//! Config show command and rendering.
//!
//! Functions for displaying user config, project config, shell status,
//! diagnostics, and runtime info.

use std::fmt::Write as _;
use std::path::PathBuf;

use anyhow::Context;
use color_print::cformat;
use worktrunk::config::{
    ProjectConfig, WorktrunkConfig, find_unknown_project_keys, find_unknown_user_keys,
};
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;
use worktrunk::shell::{Shell, scan_for_detection_details};
use worktrunk::styling::{
    error_message, format_bash_with_gutter, format_heading, format_toml, format_with_gutter,
    hint_message, info_message, success_message, warning_message,
};

use super::state::require_user_config_path;
use crate::cli::version_str;
use crate::commands::configure_shell::{ConfigAction, scan_shell_configs};
use crate::help_pager::show_help_in_pager;
use crate::llm::test_commit_generation;
use crate::output;

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

    // Display through pager (config show is always long-form output)
    if let Err(e) = show_help_in_pager(&show_output, true) {
        log::debug!("Pager invocation failed: {}", e);
        // Fall back to direct output via eprintln (matches help behavior)
        worktrunk::styling::eprintln!("{}", show_output);
    }

    Ok(())
}

// ==================== Helper Functions ====================

/// Check if Claude Code CLI is available
fn is_claude_available() -> bool {
    use worktrunk::shell_exec::Cmd;

    Cmd::new("claude")
        .arg("--version")
        .run()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if the worktrunk plugin is installed in Claude Code
fn is_plugin_installed() -> bool {
    // Try HOME/USERPROFILE env vars first (for tests and explicit overrides), then fall back to dirs
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
        .or_else(dirs::home_dir);

    let Some(home) = home else {
        return false;
    };

    let plugins_file = home.join(".claude/plugins/installed_plugins.json");
    let Ok(content) = std::fs::read_to_string(&plugins_file) else {
        return false;
    };

    // Look for "worktrunk@worktrunk" in the plugins object
    content.contains("\"worktrunk@worktrunk\"")
}

/// Get the git version string (e.g., "2.47.1")
fn get_git_version() -> Option<String> {
    use worktrunk::shell_exec::Cmd;

    let output = Cmd::new("git").arg("--version").run().ok()?;
    if !output.status.success() {
        return None;
    }

    // Parse "git version 2.47.1" -> "2.47.1"
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .trim()
        .strip_prefix("git version ")
        .map(|s| s.to_string())
}

/// Check if zsh has compinit enabled by spawning an interactive shell
///
/// Returns true if compinit is NOT enabled (i.e., user needs to add it).
/// Returns false if compinit is enabled or we can't determine (fail-safe: don't warn).
fn check_zsh_compinit_missing() -> bool {
    use worktrunk::shell_exec::Cmd;

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
    // Suppress zsh's "insecure directories" warning from compinit.
    // See detailed rationale in shell::detect_zsh_compinit().
    let Ok(output) = Cmd::new("zsh")
        .args(["--no-globalrcs", "-ic", "(( $+functions[compdef] ))"])
        .env("ZSH_DISABLE_COMPFIX", "true")
        .run()
    else {
        return false; // Can't determine, don't warn
    };

    // compdef NOT found = need to warn
    !output.status.success()
}

// ==================== Render Functions ====================

/// Render OTHER section (version, Claude plugin, hyperlinks)
fn render_runtime_info(out: &mut String) -> anyhow::Result<()> {
    let cmd = crate::binary_name();
    let version = version_str();

    writeln!(out, "{}", format_heading("OTHER", None))?;

    // Version info
    writeln!(
        out,
        "{}",
        info_message(cformat!("{cmd}: <bold>{version}</>"))
    )?;
    if let Some(git_version) = get_git_version() {
        writeln!(
            out,
            "{}",
            info_message(cformat!("git: <bold>{git_version}</>"))
        )?;
    }

    // Claude Code plugin status
    let plugin_installed = is_plugin_installed();
    let claude_available = is_claude_available();

    if plugin_installed {
        writeln!(out, "{}", success_message("Claude Code plugin installed"))?;
    } else if claude_available {
        writeln!(
            out,
            "{}",
            hint_message("Claude Code plugin not installed. To install, run:")
        )?;
        let install_commands = "claude plugin marketplace add max-sixty/worktrunk\nclaude plugin install worktrunk@worktrunk";
        writeln!(out, "{}", format_bash_with_gutter(install_commands))?;
    } else {
        writeln!(
            out,
            "{}",
            hint_message(cformat!(
                "Claude Code plugin not installed (<bold>claude</> not found)"
            ))
        )?;
    }

    // Show hyperlink support status
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
    use crate::commands::list::ci_status::{CiPlatform, CiToolsStatus, get_platform_for_repo};

    writeln!(out, "{}", format_heading("DIAGNOSTICS", None))?;

    // Check CI tool based on detected platform (with config override support)
    let repo = Repository::current()?;
    let project_config = repo.load_project_config().ok().flatten();
    let platform_override = project_config.as_ref().and_then(|c| c.ci_platform());
    let platform = get_platform_for_repo(&repo, platform_override);

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

    // Validate config (syntax + schema) and warn if invalid
    if let Err(e) = toml::from_str::<WorktrunkConfig>(&contents) {
        // Use gutter for error details to avoid markup interpretation of user content
        writeln!(out, "{}", error_message("Invalid config"))?;
        writeln!(out, "{}", format_with_gutter(&e.to_string(), None))?;
    } else {
        // Only check for unknown keys if config is valid
        warn_unknown_keys(out, &find_unknown_user_keys(&contents))?;
    }

    // Display TOML with syntax highlighting (gutter at column 0)
    write!(out, "{}", format_toml(&contents))?;

    Ok(())
}

/// Write warnings for any unknown config keys
pub(super) fn warn_unknown_keys(out: &mut String, unknown_keys: &[String]) -> anyhow::Result<()> {
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
    let repo_root = match Repository::current().and_then(|repo| repo.current_worktree().root()) {
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

    // Validate config (syntax + schema) and warn if invalid
    if let Err(e) = toml::from_str::<ProjectConfig>(&contents) {
        // Use gutter for error details to avoid markup interpretation of user content
        writeln!(out, "{}", error_message("Invalid config"))?;
        writeln!(out, "{}", format_with_gutter(&e.to_string(), None))?;
    } else {
        // Only check for unknown keys if config is valid
        warn_unknown_keys(out, &find_unknown_project_keys(&contents))?;
    }

    // Display TOML with syntax highlighting (gutter at column 0)
    write!(out, "{}", format_toml(&contents))?;

    Ok(())
}

fn render_shell_status(out: &mut String) -> anyhow::Result<()> {
    writeln!(out, "{}", format_heading("SHELL INTEGRATION", None))?;

    // Shell integration runtime status (moved from RUNTIME section)
    let shell_active = output::is_shell_integration_active();
    if shell_active {
        writeln!(out, "{}", info_message("Shell integration active"))?;
    } else {
        writeln!(out, "{}", warning_message("Shell integration not active"))?;
        // Show invocation details to help diagnose
        let invocation = crate::invocation_path();
        let is_git_subcommand = crate::is_git_subcommand();
        let mut debug_lines = vec![cformat!("Invoked as: <bold>{invocation}</>")];

        // Show actual binary path if different from invocation (helps detect wrong wt in PATH)
        if let Ok(exe_path) = std::env::current_exe() {
            let exe_display = format_path_for_display(&exe_path);
            // Only show if meaningfully different (not just ./ prefix differences)
            let invocation_canonical = std::fs::canonicalize(&invocation).ok();
            let exe_canonical = std::fs::canonicalize(&exe_path).ok();
            if invocation_canonical != exe_canonical {
                debug_lines.push(cformat!("Running from: <bold>{exe_display}</>"));
            }
        }

        // Show $SHELL to help diagnose rc file sourcing issues
        if let Ok(shell_env) = std::env::var("SHELL") {
            debug_lines.push(cformat!("$SHELL: <bold>{shell_env}</>"));
        }

        if is_git_subcommand {
            debug_lines.push("Git subcommand: yes (GIT_EXEC_PATH set)".to_string());
        }
        writeln!(out, "{}", format_with_gutter(&debug_lines.join("\n"), None))?;
    }
    writeln!(out)?;

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

    // Check for legacy fish conf.d path (deprecated location from before #566)
    // We need this early to handle the case where fish shows "Not configured" at the
    // new location but has valid integration at the legacy location.
    let legacy_fish_conf_d = Shell::legacy_fish_conf_d_path(&cmd).ok();
    let legacy_fish_has_integration = legacy_fish_conf_d.as_ref().is_some_and(|legacy_path| {
        detection_results
            .iter()
            .any(|d| d.path == *legacy_path && !d.matched_lines.is_empty())
    });

    let mut any_not_configured = false;
    let mut has_any_unmatched = false;

    // Show configured and not-configured shells (matching `config shell install` format exactly)
    // Bash/Zsh: inline completions, show "shell extension & completions"
    // Fish: separate completion file, show "shell extension" for functions/ and "completions" for completions/
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

                // Build file:line location (clickable in terminals - use first line only)
                let location = if let Some(det) = detection {
                    if let Some(first_line) = det.matched_lines.first() {
                        format!("{}:{}", path, first_line.line_number)
                    } else {
                        path.to_string()
                    }
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

                    // Check if any matched lines use .exe suffix and warn about function name
                    let uses_exe = det.matched_lines.iter().any(|m| m.content.contains(".exe"));
                    if uses_exe {
                        writeln!(
                            out,
                            "{}",
                            hint_message(cformat!(
                                "Creates shell function <bold>{cmd}</>. Aliases should use <bright-black>{cmd}</>, not <bright-black>{cmd}.exe</>"
                            ))
                        )?;
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
                            hint_message(format!("Not configured completions for {shell}"))
                        )?;
                    }
                }
            }
            ConfigAction::WouldAdd | ConfigAction::WouldCreate => {
                // For fish, check if we have valid integration at the legacy conf.d location
                if matches!(shell, Shell::Fish) && legacy_fish_has_integration {
                    // Show migration hint instead of "Not configured"
                    let legacy_path = legacy_fish_conf_d
                        .as_ref()
                        .map(|p| format_path_for_display(p))
                        .unwrap_or_default();
                    writeln!(
                        out,
                        "{}",
                        info_message(cformat!(
                            "Fish integration found in deprecated location @ <bold>{legacy_path}</>"
                        ))
                    )?;
                    // Get canonical path for the migration hint
                    let canonical_path = Shell::Fish
                        .config_paths(&cmd)
                        .ok()
                        .and_then(|p| p.into_iter().next())
                        .map(|p| format_path_for_display(&p))
                        .unwrap_or_else(|| "~/.config/fish/functions/".to_string());
                    writeln!(
                        out,
                        "{}",
                        hint_message(cformat!(
                            "To migrate to <bright-black>{canonical_path}</>, run <bright-black>{cmd} config shell install fish</>"
                        ))
                    )?;
                } else {
                    any_not_configured = true;
                    writeln!(
                        out,
                        "{}",
                        hint_message(format!("Not configured {what} for {shell}"))
                    )?;
                }
            }
            _ => {} // Added/Created won't appear in dry_run mode
        }
    }

    // Show skipped (not installed) shells
    // For fish with legacy integration, show migration hint instead of "skipped"
    for (shell, path) in &scan_result.skipped {
        if matches!(shell, Shell::Fish) && legacy_fish_has_integration {
            // Show migration hint for legacy fish location
            let legacy_path = legacy_fish_conf_d
                .as_ref()
                .map(|p| format_path_for_display(p))
                .unwrap_or_default();
            let canonical_path = Shell::Fish
                .config_paths(&cmd)
                .ok()
                .and_then(|p| p.into_iter().next())
                .map(|p| format_path_for_display(&p))
                .unwrap_or_else(|| "~/.config/fish/functions/".to_string());
            writeln!(
                out,
                "{}",
                info_message(cformat!(
                    "Fish integration found in deprecated location @ <bold>{legacy_path}</>"
                ))
            )?;
            writeln!(
                out,
                "{}",
                hint_message(cformat!(
                    "To migrate to <bright-black>{canonical_path}</>, run <bright-black>{cmd} config shell install fish</>"
                ))
            )?;
            continue;
        }
        let path = format_path_for_display(path);
        writeln!(
            out,
            "{}",
            info_message(cformat!("<dim>Skipped {shell}; {path} not found</>"))
        )?;
    }

    // Summary hint when shells need configuration
    if any_not_configured {
        writeln!(out)?;
        writeln!(
            out,
            "{}",
            hint_message(cformat!(
                "To configure, run <bright-black>{cmd} config shell install</>"
            ))
        )?;
    }

    // Show potential false negatives (lines containing cmd but not detected)
    // Skip files that have valid integration detected (matched_lines) - those are fine,
    // and the other lines containing cmd are just part of the integration script.
    for detection in &detection_results {
        if !detection.unmatched_candidates.is_empty() && detection.matched_lines.is_empty() {
            has_any_unmatched = true;
            let path = format_path_for_display(&detection.path);

            // Build file:line location (clickable in terminals - use first line only)
            let location = if let Some(first) = detection.unmatched_candidates.first() {
                format!("{}:{}", path, first.line_number)
            } else {
                path.to_string()
            };
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

            // If any unmatched lines contain .exe, explain the function name issue
            let uses_exe = detection
                .unmatched_candidates
                .iter()
                .any(|m| m.content.contains(".exe"));
            if uses_exe {
                writeln!(
                    out,
                    "{}",
                    hint_message(cformat!(
                        "Note: <bold>{cmd}.exe</> creates shell function <bold>{cmd}</>. \
                         Aliases should use <bright-black>{cmd}</>, not <bright-black>{cmd}.exe</>"
                    ))
                )?;
            }
        }
    }

    // Show aliases that bypass shell integration (Issue #348)
    for detection in &detection_results {
        for alias in &detection.bypass_aliases {
            let path = format_path_for_display(&detection.path);
            let location = format!("{}:{}", path, alias.line_number);
            writeln!(
                out,
                "{}",
                warning_message(cformat!(
                    "Alias <bold>{}</> bypasses shell integration — won't auto-cd",
                    alias.alias_name
                ))
            )?;
            writeln!(out, "{}", format_bash_with_gutter(alias.content.trim()))?;
            writeln!(
                out,
                "{}",
                hint_message(cformat!(
                    "Change to <bright-black>alias {}=\"{cmd}\"</> @ {location}",
                    alias.alias_name
                ))
            )?;
        }
    }

    // Check if any shell has config already (eval line present)
    let has_any_configured = scan_result
        .configured
        .iter()
        .any(|r| matches!(r.action, ConfigAction::AlreadyExists));

    // If we have unmatched candidates but no configured shells, suggest raising an issue
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

        // Quote a short version of the unmatched content in the hint
        let quoted = if unmatched_summary.len() == 1 {
            format!("`{}`", unmatched_summary[0])
        } else {
            format!(
                "`{}` (and {} more)",
                unmatched_summary[0],
                unmatched_summary.len() - 1
            )
        };
        writeln!(
            out,
            "{}",
            hint_message(format!(
                "If {quoted} is shell integration, report a false negative: {issue_url}"
            ))
        )?;
    }

    Ok(())
}

pub(super) fn render_ci_tool_status(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_git_version_returns_version() {
        // In a normal environment with git installed, should return a version
        let version = get_git_version();
        assert!(version.is_some());
        let version = version.unwrap();
        // Version should look like a semver (e.g., "2.47.1")
        assert!(version.chars().next().unwrap().is_ascii_digit());
        assert!(version.contains('.'));
    }
}
