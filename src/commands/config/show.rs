//! Config show command and rendering.
//!
//! Functions for displaying user config, project config, shell status,
//! diagnostics, and runtime info.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Context;
use color_print::cformat;
use serde::{Serialize, de::DeserializeOwned};
use worktrunk::config::{
    ProjectConfig, UserConfig, default_system_config_path, require_config_path, system_config_path,
};
use worktrunk::git::remote_ref::azure::azure_devops_extension_installed;
use worktrunk::git::{ErrorExt, ForgeKind, Repository, WorktrunkError};
use worktrunk::path::format_path_for_display;
use worktrunk::shell::{
    FileDetectionResult, Shell, ZshStartupScope, probe_zsh_compdef, scan_for_detection_details,
};
use worktrunk::shell_exec::Cmd;
use worktrunk::styling::{
    FormattedMessage, error_message, format_bash_with_gutter, format_heading, format_toml,
    format_with_gutter, hint_message, info_message, success_message, warning_message,
};

use crate::cli::{SwitchFormat, version_str};
use crate::commands::configure_shell::{ConfigAction, ConfigureResult, scan_shell_configs};
use crate::commands::list::ci_status::CiToolsStatus;
use crate::help_pager::show_help_in_pager;
use crate::llm::test_commit_generation;
use crate::output;
use crate::output::print_json;

/// Render the full report, then exit non-zero if any section is invalid.
pub fn handle_config_show(full: bool, format: SwitchFormat) -> anyhow::Result<()> {
    if format == SwitchFormat::Json {
        return handle_config_show_json();
    }
    // Build the complete output as a string
    let mut show_output = String::new();

    let repo = Repository::current().ok();

    let mut invalid = false;
    let has_system_config = if let Some(system_invalid) = render_system_config(&mut show_output)? {
        invalid |= system_invalid;
        show_output.push('\n');
        true
    } else {
        false
    };

    // Render user config
    invalid |= render_user_config(&mut show_output, repo.as_ref(), has_system_config)?;
    show_output.push('\n');

    // Render project config if in a git repository
    invalid |= render_project_config(&mut show_output, repo.as_ref())?;
    show_output.push('\n');

    let mut approvals_output = String::new();
    invalid |= render_approvals(&mut approvals_output, repo.as_ref())?;
    if !approvals_output.is_empty() {
        show_output.push_str(&approvals_output);
        show_output.push('\n');
    }

    // Render shell integration status
    render_shell_status(&mut show_output)?;

    // Render Claude Code status (only when claude CLI is available)
    if is_claude_available() {
        show_output.push('\n');
        render_claude_code_status(&mut show_output)?;
    }

    // Render Codex status (only when codex CLI is available)
    if is_codex_available() {
        show_output.push('\n');
        render_codex_status(&mut show_output)?;
    }

    // Render OpenCode status (only when opencode CLI is available)
    if is_opencode_available() {
        show_output.push('\n');
        render_opencode_status(&mut show_output)?;
    }

    // Render Pi status (only when the Pi CLI is available)
    if is_pi_available() {
        show_output.push('\n');
        render_pi_status(&mut show_output)?;
    }

    // Render Gemini status (only when gemini CLI is available)
    if is_gemini_available() {
        show_output.push('\n');
        render_gemini_status(&mut show_output)?;
    }

    // Run full diagnostic checks if requested (includes slow network calls)
    if full {
        show_output.push('\n');
        render_diagnostics(&mut show_output)?;
    }

    // Render runtime info at the bottom (version, binary name, shell integration status)
    show_output.push('\n');
    render_runtime_info(&mut show_output)?;

    // Display through pager (config show is always long-form output)
    show_help_in_pager(&show_output, true);

    if invalid {
        return Err(WorktrunkError::AlreadyDisplayed { exit_code: 1 }.into());
    }

    Ok(())
}

/// JSON retains the report on invalid input and signals failure by exit code.
fn handle_config_show_json() -> anyhow::Result<()> {
    let repo = Repository::current().ok();
    let mut invalid = false;
    let user_path = require_config_path()?;
    let user_exists = user_path.exists();
    let user_config = if user_exists {
        match read_json_config::<UserConfig>(&user_path)? {
            Some(source_config) => match UserConfig::load() {
                Ok(config) => Some(serde_json::to_value(config)?),
                Err(_) => {
                    invalid = true;
                    Some(source_config)
                }
            },
            None => {
                invalid = true;
                None
            }
        }
    } else {
        None
    };

    let (project_path, project_exists, project_config, project_identifier) =
        if let Some(repo) = repo.as_ref() {
            let on_disk = repo.project_config_path()?;
            let object_store = match &on_disk {
                Some(path) if path.exists() => None,
                _ => repo.default_branch_project_config_content(),
            };
            let object_store_exists = object_store.is_some();
            let (path, config) = match &on_disk {
                Some(path) if path.exists() => {
                    let config = read_json_config::<ProjectConfig>(path)?;
                    (on_disk.clone(), config)
                }
                _ => match object_store {
                    Some((contents, spec)) => {
                        let config = parse_json_config::<ProjectConfig>(&contents)?;
                        (Some(spec), config)
                    }
                    None => (on_disk.clone(), None),
                },
            };
            if (on_disk.as_ref().is_some_and(|path| path.exists()) || object_store_exists)
                && config.is_none()
            {
                invalid = true;
            }
            let identifier = repo.project_identifier().ok();
            let exists = on_disk.as_ref().is_some_and(|path| path.exists()) || object_store_exists;
            (path, exists, config, identifier)
        } else {
            (None, false, None, None)
        };

    let system_path = system_config_path().or_else(default_system_config_path);
    let system_exists = system_path.as_ref().is_some_and(|p| p.exists());
    let system_invalid = if let Some(path) = system_path.as_deref().filter(|_| system_exists) {
        match std::fs::read_to_string(path) {
            Ok(contents) => config_parse_error::<UserConfig>(&contents).is_some(),
            Err(_) => true,
        }
    } else {
        false
    };
    let approvals_invalid = matches!(
        approvals_diagnostic(repo.as_ref()),
        ApprovalsDiagnostic::Invalid(_)
    );

    let output = serde_json::json!({
        "user": {
            "path": user_path,
            "exists": user_exists,
            "config": user_config,
        },
        "project": {
            "path": project_path,
            // An invalid on-disk source still exists even though `config` is
            // null. The object-store fallback counts as existing too, though
            // its revision spec is not a filesystem path.
            "exists": project_exists,
            "identifier": project_identifier,
            "config": project_config,
        },
        "system": {
            "path": system_path,
            "exists": system_exists,
        },
    });
    invalid |= system_invalid
        || approvals_invalid
        || repo
            .as_ref()
            .is_some_and(|repo| validate_column_selection(repo).is_err());
    print_json(&output)?;

    if invalid {
        return Err(WorktrunkError::AlreadyDisplayed { exit_code: 1 }.into());
    }

    Ok(())
}

fn read_json_config<C>(path: &Path) -> anyhow::Result<Option<serde_json::Value>>
where
    C: DeserializeOwned + Serialize,
{
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Ok(None);
    };
    parse_json_config::<C>(&contents)
}

fn parse_json_config<C>(contents: &str) -> anyhow::Result<Option<serde_json::Value>>
where
    C: DeserializeOwned + Serialize,
{
    let migrated = worktrunk::config::migrate_content(contents);
    toml::from_str::<C>(&migrated)
        .ok()
        .map(|config| serde_json::to_value(config).map_err(Into::into))
        .transpose()
}

// ==================== Helper Functions ====================

/// Check if Claude Code CLI is available
pub(super) fn is_claude_available() -> bool {
    // Allow tests to override detection
    if let Ok(val) = std::env::var("WORKTRUNK_TEST_CLAUDE_INSTALLED") {
        return val == "1";
    }
    which::which("claude").is_ok()
}

/// Check if Codex CLI is available
pub(super) fn is_codex_available() -> bool {
    // Allow tests to override detection
    if let Ok(val) = std::env::var("WORKTRUNK_TEST_CODEX_INSTALLED") {
        return val == "1";
    }
    which::which("codex").is_ok()
}

/// Get the home directory for Claude Code config detection
pub(super) fn home_dir() -> Option<PathBuf> {
    // Try HOME/USERPROFILE env vars first (for tests and explicit overrides),
    // then fall back to the OS lookup
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
        .or_else(worktrunk::path::home_dir)
}

/// Get the Claude Code config directory.
///
/// Honors `CLAUDE_CONFIG_DIR`, which Claude Code uses to relocate its entire
/// config tree (`settings.json`, `plugins/`, ...) away from the default
/// `~/.claude`. A leading `~/` in the value is expanded against the home
/// directory; the shell normally expands it before the variable is set, so a
/// literal `~` only reaches us when the variable is set in a non-shell context.
pub(super) fn claude_config_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR")
        && !dir.is_empty()
    {
        if let Some(rest) = dir.strip_prefix("~/") {
            return home_dir().map(|home| home.join(rest));
        }
        return Some(PathBuf::from(dir));
    }
    home_dir().map(|home| home.join(".claude"))
}

/// Check if the worktrunk plugin is installed in Claude Code
pub(super) fn is_plugin_installed() -> bool {
    let Some(config_dir) = claude_config_dir() else {
        return false;
    };

    let plugins_file = config_dir.join("plugins/installed_plugins.json");
    let Ok(content) = std::fs::read_to_string(&plugins_file) else {
        return false;
    };

    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };

    json.get("plugins")
        .and_then(|p| p.get("worktrunk@worktrunk"))
        .is_some()
}

/// Whether Claude Code's statusline runs worktrunk's.
///
/// The question is which subcommand the configured command invokes, so it's
/// asked of the adjacent tokens `list statusline` — the binary answers the
/// same whether it's `wt`, `git-wt`, or an absolute path. Matching on the
/// binary alone accepted any command that merely spelled `wt ` somewhere
/// (`newt status`), which reported a foreign statusline as worktrunk's and
/// left `install-statusline` refusing to install.
pub(super) fn is_statusline_configured() -> bool {
    let Some(config_dir) = claude_config_dir() else {
        return false;
    };

    let settings_file = config_dir.join("settings.json");
    let Ok(content) = std::fs::read_to_string(&settings_file) else {
        return false;
    };

    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };

    json.get("statusLine")
        .and_then(|s| s.get("command"))
        .and_then(|c| c.as_str())
        .is_some_and(|cmd| {
            let tokens: Vec<&str> = cmd.split_whitespace().collect();
            tokens.windows(2).any(|pair| pair == ["list", "statusline"])
        })
}

// ==================== Render Functions ====================

/// Render CLAUDE CODE section (plugin and statusline status).
/// Caller must check `is_claude_available()` first.
fn render_claude_code_status(out: &mut String) -> anyhow::Result<()> {
    writeln!(out, "{}", format_heading("CLAUDE CODE", None))?;

    // Plugin status
    let plugin_installed = is_plugin_installed();
    if plugin_installed {
        writeln!(out, "{}", success_message("Plugin installed"))?;
    } else {
        writeln!(
            out,
            "{}",
            hint_message(cformat!(
                "Plugin not installed. To install, run <underline>wt config plugins claude install</>"
            ))
        )?;
    }

    // Statusline status
    let statusline_configured = is_statusline_configured();
    if statusline_configured {
        writeln!(out, "{}", success_message("Statusline configured"))?;
    } else {
        writeln!(
            out,
            "{}",
            hint_message(cformat!(
                "Statusline not configured. To configure, run <underline>wt config plugins claude install-statusline</>"
            ))
        )?;
    }

    Ok(())
}

/// Render CODEX section (marketplace install hint).
/// Caller must check `is_codex_available()` first.
fn render_codex_status(out: &mut String) -> anyhow::Result<()> {
    writeln!(out, "{}", format_heading("CODEX", None))?;
    writeln!(out, "{}", success_message("Codex CLI available"))?;
    writeln!(
        out,
        "{}",
        hint_message(cformat!(
            "If Worktrunk is not installed in /plugins yet, run <underline>wt config plugins codex install</>"
        ))
    )?;

    Ok(())
}

/// Check if OpenCode CLI is available
fn is_opencode_available() -> bool {
    // Allow tests to override detection
    if let Ok(val) = std::env::var("WORKTRUNK_TEST_OPENCODE_INSTALLED") {
        return val == "1";
    }
    which::which("opencode").is_ok()
}

/// Check if the Pi coding agent CLI is available.
fn is_pi_available() -> bool {
    if let Ok(val) = std::env::var("WORKTRUNK_TEST_PI_INSTALLED") {
        return val == "1";
    }
    which::which("omp").is_ok()
}

/// Render OPENCODE section (plugin status).
/// Caller must check `is_opencode_available()` first.
fn render_opencode_status(out: &mut String) -> anyhow::Result<()> {
    writeln!(out, "{}", format_heading("OPENCODE", None))?;

    // Plugin status
    let plugin_installed = super::opencode::is_plugin_installed();
    let plugin_exists = super::opencode::plugin_file_exists();
    if plugin_installed {
        writeln!(out, "{}", success_message("Plugin installed"))?;
    } else if plugin_exists {
        writeln!(
            out,
            "{}",
            hint_message(cformat!(
                "Plugin outdated. To update, run <underline>wt config plugins opencode install</>"
            ))
        )?;
    } else {
        writeln!(
            out,
            "{}",
            hint_message(cformat!(
                "Plugin not installed. To install, run <underline>wt config plugins opencode install</>"
            ))
        )?;
    }

    Ok(())
}

/// Render PI section (plugin status).
/// Caller must check `is_pi_available()` first.
fn render_pi_status(out: &mut String) -> anyhow::Result<()> {
    writeln!(out, "{}", format_heading("PI", None))?;

    if super::pi::is_plugin_installed() {
        writeln!(out, "{}", success_message("Plugin installed"))?;
    } else if super::pi::plugin_file_exists() {
        writeln!(
            out,
            "{}",
            hint_message(cformat!(
                "Plugin outdated. To update, run <underline>wt config plugins pi install</>"
            ))
        )?;
    } else {
        writeln!(
            out,
            "{}",
            hint_message(cformat!(
                "Plugin not installed. To install, run <underline>wt config plugins pi install</>"
            ))
        )?;
    }

    Ok(())
}

/// Check if Gemini CLI is available
fn is_gemini_available() -> bool {
    // Allow tests to override detection
    if let Ok(val) = std::env::var("WORKTRUNK_TEST_GEMINI_INSTALLED") {
        return val == "1";
    }
    which::which("gemini").is_ok()
}

/// Check if the worktrunk extension is installed in Gemini CLI.
///
/// `gemini extensions install` clones the extension into
/// `~/.gemini/extensions/<name>/`, so a worktrunk install leaves a
/// `gemini-extension.json` whose `name` is `worktrunk` at that path.
fn is_gemini_extension_installed() -> bool {
    let Some(home) = home_dir() else {
        return false;
    };

    let manifest = home.join(".gemini/extensions/worktrunk/gemini-extension.json");
    let Ok(content) = std::fs::read_to_string(&manifest) else {
        return false;
    };

    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };

    json.get("name").and_then(|n| n.as_str()) == Some("worktrunk")
}

/// Render GEMINI CLI section (extension status).
/// Caller must check `is_gemini_available()` first.
fn render_gemini_status(out: &mut String) -> anyhow::Result<()> {
    writeln!(out, "{}", format_heading("GEMINI CLI", None))?;

    if is_gemini_extension_installed() {
        writeln!(out, "{}", success_message("Extension installed"))?;
    } else {
        writeln!(
            out,
            "{}",
            hint_message(cformat!(
                "Extension not installed. To install, run <underline>gemini extensions install https://github.com/max-sixty/worktrunk</>"
            ))
        )?;
    }

    Ok(())
}

/// Render OTHER section (version, hyperlinks)
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
    if let Ok(git_version) = worktrunk::git::git_version() {
        writeln!(
            out,
            "{}",
            info_message(cformat!("git: <bold>{git_version}</>"))
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
    writeln!(out, "{}", format_heading("DIAGNOSTICS", None))?;

    // Check the CI tool for this repo's platform (configured forge platform,
    // else remote URL).
    let repo = Repository::current()?;
    match repo.ci_platform(None) {
        Some(ForgeKind::GitHub) => {
            let ci_tools = CiToolsStatus::detect(None);
            render_ci_tool_status(
                out,
                "gh",
                "GitHub",
                ci_tools.gh_installed,
                ci_tools.gh_authenticated,
            )?;
        }
        Some(ForgeKind::GitLab) => {
            let ci_tools = CiToolsStatus::detect(None);
            render_ci_tool_status(
                out,
                "glab",
                "GitLab",
                ci_tools.glab_installed,
                ci_tools.glab_authenticated,
            )?;
        }
        Some(ForgeKind::Gitea) => {
            let ci_tools = CiToolsStatus::detect(None);
            render_ci_tool_status(
                out,
                "tea",
                "Gitea",
                ci_tools.tea_installed,
                ci_tools.tea_authenticated,
            )?;
        }
        Some(ForgeKind::AzureDevOps) => {
            let ci_tools = CiToolsStatus::detect(None);
            render_ci_tool_status(
                out,
                "az",
                "Azure DevOps",
                ci_tools.az_installed,
                ci_tools.az_authenticated,
            )?;
            // The whole `az repos` command group ships in the azure-devops
            // extension, so an `az` without it reports no CI status however
            // well it's authenticated — and only the user can install it.
            if ci_tools.az_installed && !azure_devops_extension_installed(repo.repo_path()?) {
                writeln!(
                    out,
                    "{}",
                    warning_message(cformat!(
                        "<bold>azure-devops</> extension not installed; run <bold>az extension add --name azure-devops</>"
                    ))
                )?;
            }
        }
        None => {
            writeln!(
                out,
                "{}",
                hint_message("CI status requires GitHub, GitLab, Gitea, or Azure DevOps remote")
            )?;
        }
    }

    // Check for newer version on GitHub
    render_version_check(out)?;

    // Test commit generation - use effective config for current project
    let config = UserConfig::load().context("Failed to load config")?;
    let project_id = repo.project_identifier().ok();
    let commit_config = config.commit_generation(project_id.as_deref());

    if !commit_config.is_configured() {
        writeln!(out, "{}", hint_message("Commit generation not configured"))?;
    } else {
        // `is_configured()` guarantees `command` is `Some` and non-empty here;
        // `unwrap_or_default()` avoids a panic-prone `unwrap()` in this
        // `Result`-returning function (the default is unreachable).
        let command_display = commit_config.command.clone().unwrap_or_default();

        match test_commit_generation(&commit_config) {
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
                // Use the typed diagnostic block (with hint, gutter, etc.)
                // when present; otherwise fall back to the short Display label.
                let body = e.render_diagnostic().unwrap_or_else(|| e.to_string());
                writeln!(out, "{}", format_with_gutter(&body, None))?;
            }
        }
    }

    Ok(())
}

/// Render system config when present, returning whether it is invalid.
fn render_system_config(out: &mut String) -> anyhow::Result<Option<bool>> {
    let Some(system_path) = system_config_path() else {
        return Ok(None);
    };

    writeln!(
        out,
        "{}",
        format_heading(
            "SYSTEM CONFIG",
            Some(&format!("@ {}", format_path_for_display(&system_path)))
        )
    )?;

    let contents = match std::fs::read_to_string(&system_path) {
        Ok(contents) => contents,
        Err(err) => {
            render_config_read_error(out, &err)?;
            return Ok(Some(true));
        }
    };

    if contents.trim().is_empty() {
        writeln!(out, "{}", hint_message("Empty file (no system defaults)"))?;
        return Ok(Some(false));
    }

    // Validate config (syntax + schema) and warn if invalid
    let mut invalid = false;
    if let Some(e) = config_parse_error::<UserConfig>(&contents) {
        invalid = true;
        writeln!(out, "{}", error_message("Invalid config"))?;
        writeln!(out, "{}", format_with_gutter(&e.to_string(), None))?;
    } else {
        out.push_str(&warn_unknown_keys::<UserConfig>(&contents));
    }

    // Display TOML with syntax highlighting
    writeln!(out, "{}", format_toml(&contents))?;

    Ok(Some(invalid))
}

/// Render the USER CONFIG section. Returns true if the config is invalid.
fn render_user_config(
    out: &mut String,
    repo: Option<&Repository>,
    has_system_config: bool,
) -> anyhow::Result<bool> {
    let config_path = require_config_path()?;

    writeln!(
        out,
        "{}",
        format_heading(
            "USER CONFIG",
            Some(&format!("@ {}", format_path_for_display(&config_path)))
        )
    )?;

    // Check if file exists
    if !config_path.exists() {
        writeln!(
            out,
            "{}",
            hint_message(cformat!(
                "Not found; to create one, run <underline>wt config create</>"
            ))
        )?;
        // A `[list] columns` selection can still arrive from the system layer,
        // the environment, or `--config-set`, so the check runs either way.
        return render_column_selection(out, repo);
    }

    let contents = match std::fs::read_to_string(&config_path) {
        Ok(contents) => contents,
        Err(err) => {
            render_config_read_error(out, &err)?;
            render_column_selection(out, repo)?;
            return Ok(true);
        }
    };

    // Check for deprecations with emit_inline_warnings=false (silent mode)
    // User config is global, not tied to any repository
    // Deprecated patterns supersede the TOML dump below (their diff covers
    // the file); a pending-default pin is additive, so the dump stays. An
    // empty file still gets the pending-pin details — `wt config update`
    // would rewrite it — just no dump.
    let mut invalid = false;
    let mut details_shown = false;
    let skip_dump = match worktrunk::config::check_and_migrate(
        &config_path,
        &contents,
        true,
        worktrunk::config::ConfigFileKind::User,
        None,
        false, // silent mode - we'll format the output ourselves
    ) {
        Ok(result) => {
            if let Some(info) = result.info {
                out.push_str(&worktrunk::config::format_deprecation_details(
                    &info, &contents,
                ));
                details_shown = true;
                info.has_deprecated_patterns()
            } else {
                false
            }
        }
        Err(err) => {
            invalid = true;
            writeln!(out, "{}", error_message(err.to_string()))?;
            false
        }
    };

    if contents.trim().is_empty() {
        writeln!(out, "{}", hint_message("Empty file (using defaults)"))?;
        return Ok(invalid | render_column_selection(out, repo)?);
    }

    // Validate config (syntax + schema) and warn if invalid
    if let Some(e) = config_parse_error::<UserConfig>(&contents) {
        // Use gutter for error details to avoid markup interpretation of user content
        invalid = true;
        writeln!(out, "{}", error_message("Invalid config"))?;
        writeln!(out, "{}", format_with_gutter(&e.to_string(), None))?;
    } else {
        out.push_str(&warn_unknown_keys::<UserConfig>(&contents));
    }

    // Display TOML with syntax highlighting (gutter at column 0).
    // Skip when deprecations were shown — the proposed diff already covers it.
    if !skip_dump {
        if details_shown {
            // Pending-pin details above end in their diff; separate phases.
            out.push('\n');
        }
        writeln!(out, "{}", format_toml(&contents))?;
    }

    if !has_system_config {
        render_system_config_hint(out)?;
    }

    Ok(invalid | render_column_selection(out, repo)?)
}

fn render_system_config_hint(out: &mut String) -> anyhow::Result<()> {
    if let Some(path) = default_system_config_path() {
        writeln!(
            out,
            "{}",
            hint_message(cformat!(
                "Optional system config not found @ <dim>{}</>",
                format_path_for_display(&path)
            ))
        )?;
    }
    Ok(())
}

fn render_config_read_error(out: &mut String, err: &std::io::Error) -> anyhow::Result<()> {
    writeln!(out, "{}", error_message("Cannot read config"))?;
    writeln!(out, "{}", format_with_gutter(&err.to_string(), None))?;
    Ok(())
}

/// Report list-column settings that `wt list` would reject.
fn render_column_selection(out: &mut String, repo: Option<&Repository>) -> anyhow::Result<bool> {
    let Some(repo) = repo else {
        return Ok(false);
    };
    if let Err(e) = validate_column_selection(repo) {
        writeln!(out, "{}", error_message(e.to_string()))?;
        return Ok(true);
    }
    Ok(false)
}

/// Validate the resolved list-column config exactly as `wt list` does.
fn validate_column_selection(repo: &Repository) -> anyhow::Result<()> {
    let config = repo.config();
    let custom = crate::commands::list::custom_columns::resolve_custom_columns(
        &config.list.custom_columns,
        repo,
    )?;
    let custom_names: Vec<&str> = custom.iter().map(|c| c.name.as_str()).collect();
    crate::commands::list::columns::parse_selected_columns(&config.list.columns, &custom_names)?;
    Ok(())
}

fn config_parse_error<C: DeserializeOwned>(contents: &str) -> Option<toml::de::Error> {
    toml::from_str::<C>(contents).err()
}

/// Format warnings for unknown config keys in `raw_contents`.
///
/// Generic over `C`, the config type. Classification is shared with the
/// load-time warning path via
/// [`collect_unknown_warnings`](worktrunk::config::collect_unknown_warnings);
/// only the message wording differs.
pub(super) fn warn_unknown_keys<C: worktrunk::config::WorktrunkConfig>(
    raw_contents: &str,
) -> String {
    let mut out = String::new();
    for warning in worktrunk::config::collect_unknown_warnings::<C>(raw_contents) {
        let _ = writeln!(out, "{}", warning_message(format_show_warning(&warning)));
    }
    out
}

fn format_show_warning(warning: &worktrunk::config::UnknownWarning) -> String {
    use worktrunk::config::UnknownWarning;
    match warning {
        UnknownWarning::TopLevelUnknown { key } => {
            cformat!("Unknown key <bold>{key}</> will be ignored")
        }
        UnknownWarning::TopLevelWrongConfig {
            key,
            other_description,
        } => worktrunk::config::with_scope_note(
            cformat!("Key <bold>{key}</> belongs in {other_description} (will be ignored)"),
            other_description,
            key,
        ),
        UnknownWarning::TopLevelDeprecatedWrongConfig {
            key,
            other_description,
            canonical_display,
        } => cformat!("Key <bold>{key}</> belongs in {other_description} as {canonical_display}"),
        UnknownWarning::NestedWrongConfig {
            path,
            other_description,
        } => worktrunk::config::with_scope_note(
            cformat!("Key <bold>{path}</> belongs in {other_description} (will be ignored)"),
            other_description,
            path,
        ),
        UnknownWarning::NestedUnknown { path } => {
            cformat!("Unknown key <bold>{path}</> will be ignored")
        }
    }
}

/// Render the PROJECT CONFIG section. Returns true if the config is invalid.
fn render_project_config(out: &mut String, repo: Option<&Repository>) -> anyhow::Result<bool> {
    // Try to get current repository root
    let repo = match repo {
        Some(repo) => repo,
        None => {
            writeln!(
                out,
                "{}",
                cformat!(
                    "<dim>{}</>",
                    format_heading("PROJECT CONFIG", Some("Not in a git repository"))
                )
            )?;
            return Ok(false);
        }
    };
    fn write_heading_and_identifier(
        out: &mut String,
        repo: &Repository,
        source: &str,
    ) -> anyhow::Result<()> {
        writeln!(out, "{}", format_heading("PROJECT CONFIG", Some(source)))?;
        if let Ok(project_id) = repo.project_identifier() {
            let line = info_message(cformat!("Identifier: <bold>{project_id}</>"));
            writeln!(out, "{line}")?;
        }
        Ok(())
    }

    // Match ProjectConfig::load's on-disk then object-store source order.
    let on_disk = repo.project_config_path()?;
    let (config_path, contents) = match &on_disk {
        Some(path) if path.exists() => {
            let source = format!("@ {}", format_path_for_display(path));
            write_heading_and_identifier(out, repo, &source)?;
            let contents = match std::fs::read_to_string(path) {
                Ok(contents) => contents,
                Err(err) => {
                    render_config_read_error(out, &err)?;
                    return Ok(true);
                }
            };
            (path.clone(), contents)
        }
        _ => match repo.default_branch_project_config_content() {
            Some((object_store_contents, spec)) => {
                let source = format!("@ {} (from object store)", spec.to_string_lossy());
                write_heading_and_identifier(out, repo, &source)?;
                (spec, object_store_contents)
            }
            None => {
                let Some(path) = on_disk else {
                    let heading = format_heading("PROJECT CONFIG", Some("No project config"));
                    writeln!(out, "{}", cformat!("<dim>{}</>", heading))?;
                    return Ok(false);
                };
                let source = format!("@ {}", format_path_for_display(&path));
                write_heading_and_identifier(out, repo, &source)?;
                writeln!(out, "{}", hint_message("Not found"))?;
                return Ok(false);
            }
        },
    };

    if contents.trim().is_empty() {
        writeln!(out, "{}", hint_message("Empty file"))?;
        return Ok(false);
    }

    // Only the main worktree can offer an in-place project-config migration.
    let is_main_worktree = !repo.current_worktree().is_linked().unwrap_or(true);
    let mut details_shown = false;
    let mut invalid = false;
    let skip_dump = match worktrunk::config::check_and_migrate(
        &config_path,
        &contents,
        is_main_worktree,
        worktrunk::config::ConfigFileKind::Project,
        Some(repo),
        false, // silent mode - we'll format the output ourselves
    ) {
        Ok(result) => {
            if let Some(info) = result.info {
                out.push_str(&worktrunk::config::format_deprecation_details(
                    &info, &contents,
                ));
                details_shown = true;
                info.has_deprecated_patterns()
            } else {
                false
            }
        }
        Err(err) => {
            invalid = true;
            writeln!(out, "{}", error_message(err.to_string()))?;
            false
        }
    };

    // Validate config (syntax + schema) and warn if invalid
    if let Some(e) = config_parse_error::<ProjectConfig>(&contents) {
        // Use gutter for error details to avoid markup interpretation of user content
        invalid = true;
        writeln!(out, "{}", error_message("Invalid config"))?;
        writeln!(out, "{}", format_with_gutter(&e.to_string(), None))?;
    } else {
        out.push_str(&warn_unknown_keys::<ProjectConfig>(&contents));
    }

    // Display TOML with syntax highlighting (gutter at column 0).
    // Skip when deprecations were shown — the proposed diff already covers it.
    if !skip_dump {
        if details_shown {
            // Pending-pin details above end in their diff; separate phases.
            out.push('\n');
        }
        writeln!(out, "{}", format_toml(&contents))?;
    }

    Ok(invalid)
}

/// Report an invalid approvals file or project commands awaiting approval.
fn render_approvals(out: &mut String, repo: Option<&Repository>) -> anyhow::Result<bool> {
    match approvals_diagnostic(repo) {
        ApprovalsDiagnostic::Valid => Ok(false),
        ApprovalsDiagnostic::Invalid(err) => {
            render_approvals_heading(out)?;
            writeln!(out, "{}", error_message("Invalid approvals"))?;
            writeln!(out, "{}", format_with_gutter(&err, None))?;
            Ok(true)
        }
        ApprovalsDiagnostic::Pending(pending) => {
            render_approvals_heading(out)?;
            let plural = if pending == 1 { "command" } else { "commands" };
            let status = info_message(format!("{pending} project {plural} awaiting approval"));
            writeln!(out, "{status}")?;
            let hint = hint_message(cformat!(
                "To review, run <underline>wt config approvals list</>"
            ));
            writeln!(out, "{hint}")?;
            Ok(false)
        }
    }
}

fn render_approvals_heading(out: &mut String) -> anyhow::Result<()> {
    let source = worktrunk::config::approvals_path()
        .map(|path| format!("@ {}", format_path_for_display(&path)));
    writeln!(out, "{}", format_heading("APPROVALS", source.as_deref()))?;
    Ok(())
}

enum ApprovalsDiagnostic {
    Valid,
    Invalid(String),
    Pending(usize),
}

fn approvals_diagnostic(repo: Option<&Repository>) -> ApprovalsDiagnostic {
    let approvals_file_exists = worktrunk::config::approvals_path()
        .as_ref()
        .is_some_and(|path| path.exists());
    let approvals = match worktrunk::config::Approvals::load() {
        Ok(approvals) => approvals,
        Err(err) if approvals_file_exists => {
            return ApprovalsDiagnostic::Invalid(err.to_string());
        }
        // With no approvals file, `Approvals::load` falls back to the user
        // config. Its error belongs to that section, which already reports the
        // same broken source and marks the whole diagnostic invalid.
        Err(_) => return ApprovalsDiagnostic::Valid,
    };
    let Some(repo) = repo else {
        return ApprovalsDiagnostic::Valid;
    };
    let Ok(Some(project_config)) = repo.load_project_config() else {
        return ApprovalsDiagnostic::Valid;
    };
    let commands = super::approvals::collect_approvable_commands(&project_config);
    if commands.is_empty() {
        return ApprovalsDiagnostic::Valid;
    }
    let Ok(project_id) = repo.project_identifier() else {
        return ApprovalsDiagnostic::Valid;
    };
    let pending = commands
        .into_iter()
        .filter(|cmd| !approvals.is_command_approved(&project_id, &cmd.command.template))
        .count();
    if pending == 0 {
        ApprovalsDiagnostic::Valid
    } else {
        ApprovalsDiagnostic::Pending(pending)
    }
}

/// Emit the "fish integration found in deprecated location" notice plus the
/// hint pointing at the canonical path. Used wherever fish integration lives
/// at the legacy `~/.config/fish/conf.d/` location (deprecated since #566).
fn render_fish_legacy_migration(
    out: &mut String,
    legacy_fish_conf_d: Option<&Path>,
    cmd: &str,
) -> anyhow::Result<()> {
    let legacy_path = legacy_fish_conf_d
        .map(format_path_for_display)
        .unwrap_or_default();
    let canonical_path = Shell::Fish
        .config_paths(cmd)
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
            "To migrate to <underline>{canonical_path}</>, run <underline>{cmd} config shell install fish</>"
        ))
    )?;
    Ok(())
}

/// Zsh-only: warn when compinit isn't enabled, since the integration
/// installs completions but they won't load without compinit.
fn render_zsh_compinit_warning(out: &mut String) -> anyhow::Result<()> {
    if probe_zsh_compdef(ZshStartupScope::UserOnly) != Some(false) {
        return Ok(());
    }
    writeln!(
        out,
        "{}",
        warning_message("Completions won't work; add to ~/.zshrc before the wt line:")
    )?;
    writeln!(
        out,
        "{}",
        format_with_gutter("autoload -Uz compinit && compinit", None)
    )?;
    Ok(())
}

/// Fish-only: report whether the separate completions file is in place.
/// Doesn't flip `any_not_configured` — missing fish completions have a
/// shell-specific remediation hint rather than the generic "To configure"
/// summary.
fn render_fish_completion_status(out: &mut String, cmd: &str) -> anyhow::Result<()> {
    let Ok(completion_path) = Shell::Fish.completion_path(cmd) else {
        return Ok(());
    };
    let completion_display = format_path_for_display(&completion_path);
    let shell = Shell::Fish;
    if completion_path.exists() {
        writeln!(
            out,
            "{}",
            info_message(cformat!(
                "<bold>{shell}</>: Already configured completions @ {completion_display}"
            ))
        )?;
    } else {
        let warning = warning_message(cformat!(
            "<bold>{shell}</>: Completions not configured @ <bold>{completion_display}</>"
        ));
        let hint = hint_message(cformat!(
            "To configure completions, run <underline>{cmd} config shell install {shell}</>"
        ));
        writeln!(out, "{warning}\n{hint}")?;
    }
    Ok(())
}

/// When the integration is configured but the wrapper isn't loaded in the
/// running shell, suggest a verify command. Only fires for the user's
/// current shell — `type wt` (or `Get-Command wt`) probes the running
/// shell, so a hint targeted at a different shell would mislead.
fn render_verify_wrapper_hint(
    out: &mut String,
    shell: Shell,
    cmd: &str,
    shell_active: bool,
) -> anyhow::Result<()> {
    if shell_active || Some(shell) != worktrunk::shell::current_shell() {
        return Ok(());
    }
    let verify_cmd = match shell {
        Shell::PowerShell => format!("Get-Command {cmd}"),
        _ => format!("type {cmd}"),
    };
    let hint = hint_message(cformat!(
        "To verify wrapper loaded: <underline>{verify_cmd}</>"
    ));
    writeln!(out, "{hint}")?;
    Ok(())
}

/// Render the `AlreadyExists` row plus any per-shell follow-ups (matched
/// lines, .exe warning, zsh compinit, fish completions, verify hint).
fn render_already_configured(
    out: &mut String,
    result: &ConfigureResult,
    detection_results: &[FileDetectionResult],
    cmd: &str,
    shell_active: bool,
) -> anyhow::Result<()> {
    let shell = result.shell;
    let path = format_path_for_display(&result.path);
    let what = crate::output::shell_integration::shell_extension_label(shell);

    let detection = detection_results
        .iter()
        .find(|d| d.path == result.path && !d.matched_lines.is_empty());

    // Build file:line location (clickable in terminals - use first line only)
    let location = match detection.and_then(|d| d.matched_lines.first()) {
        Some(first_line) => format!("{}:{}", path, first_line.line_number),
        None => path.to_string(),
    };

    writeln!(
        out,
        "{}",
        info_message(cformat!(
            "<bold>{shell}</>: Already configured {what} @ {location}"
        ))
    )?;

    if let Some(det) = detection {
        for detected in &det.matched_lines {
            writeln!(out, "{}", format_bash_with_gutter(detected.content.trim()))?;
        }
        // Warn when the matched lines use .exe — installs the function as
        // wt.exe, but aliases still need to point at wt.
        if det.matched_lines.iter().any(|m| m.content.contains(".exe")) {
            writeln!(
                out,
                "{}",
                hint_message(cformat!(
                    "Creates shell function <bold>{cmd}</>. Aliases should use <underline>{cmd}</>, not <underline>{cmd}.exe</>"
                ))
            )?;
        }
    }

    match shell {
        Shell::Zsh => render_zsh_compinit_warning(out)?,
        Shell::Fish => render_fish_completion_status(out, cmd)?,
        _ => {}
    }

    render_verify_wrapper_hint(out, shell, cmd, shell_active)?;
    Ok(())
}

/// Render the `WouldAdd`/`WouldCreate` arm. Returns whether the row
/// should count toward `any_not_configured` (the trailing "To configure"
/// summary is suppressed for outdated wrappers and dotfile-managed setups).
fn render_would_add_or_create(
    out: &mut String,
    result: &ConfigureResult,
    legacy_fish_conf_d: Option<&Path>,
    legacy_fish_has_integration: bool,
    cmd: &str,
    shell_active: bool,
) -> anyhow::Result<bool> {
    let shell = result.shell;
    let path = format_path_for_display(&result.path);
    let what = crate::output::shell_integration::shell_extension_label(shell);

    // Fish: prefer migration hint when the legacy conf.d location has
    // working integration — silencing the "Not configured" row.
    if matches!(shell, Shell::Fish) && legacy_fish_has_integration {
        render_fish_legacy_migration(out, legacy_fish_conf_d, cmd)?;
        return Ok(false);
    }

    // Wrapper-based shells with WouldAdd: file exists but content drifted
    // (e.g. outdated wrapper). The per-shell "To update" hint covers it,
    // so the generic "To configure" summary stays silent.
    if shell.is_wrapper_based() && matches!(result.action, ConfigAction::WouldAdd) {
        let warning = warning_message(cformat!(
            "<bold>{shell}</>: Outdated shell extension @ <bold>{path}</>"
        ));
        let hint = hint_message(cformat!(
            "To update, run <underline>{cmd} config shell install {shell}</>"
        ));
        writeln!(out, "{warning}\n{hint}")?;
        return Ok(false);
    }

    // Integration is loaded at runtime even though no rc file matched —
    // common with dotfile managers (stow/chezmoi) that source from
    // unknown locations.
    if shell_active && Some(shell) == worktrunk::shell::current_shell() {
        writeln!(
            out,
            "{}",
            info_message(cformat!(
                "<bold>{shell}</>: Configured {what} (not found in {path})"
            ))
        )?;
        return Ok(false);
    }

    writeln!(
        out,
        "{}",
        info_message(cformat!("<bold>{shell}</>: Not configured {what}"))
    )?;
    Ok(true)
}

fn render_shell_status(out: &mut String) -> anyhow::Result<()> {
    writeln!(out, "{}", format_heading("SHELL INTEGRATION", None))?;

    // Use the same detection logic as `wt config shell install`. Hoisted above the
    // active/inactive warning so the warning text can distinguish "installed but
    // not loaded" (▲ not active) from "never installed" (▲ not configured).
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

    // "Configured somewhere" means any rc file already has the init line, any
    // wrapper-based shell has its wrapper file (WouldAdd on a wrapper means
    // the file exists but content differs — i.e. outdated, still installed),
    // or fish has integration at the legacy conf.d path.
    let any_configured_somewhere = scan_result.configured.iter().any(|r| {
        matches!(r.action, ConfigAction::AlreadyExists)
            || (r.shell.is_wrapper_based() && matches!(r.action, ConfigAction::WouldAdd))
    }) || legacy_fish_has_integration;

    // Shell integration runtime status (moved from RUNTIME section)
    let shell_active = output::is_shell_integration_active();
    // When the user has no working integration anywhere, the install hint goes
    // directly under the warning so cause and remedy stay together. The trailing
    // hint at the bottom of the section is suppressed in this case.
    let hint_under_warning = !shell_active && !any_configured_somewhere;
    if shell_active {
        writeln!(out, "{}", info_message("Shell integration active"))?;
    } else {
        let warning_text = if any_configured_somewhere {
            "Shell integration not active"
        } else {
            "Shell integration not configured"
        };
        writeln!(out, "{}", warning_message(warning_text))?;
        if hint_under_warning {
            let hint = hint_message(cformat!(
                "To configure, run <underline>{cmd} config shell install</>"
            ));
            writeln!(out, "{hint}")?;
        }
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

        // Show the shell actually running wt — the process tree names the
        // interactive shell even when $SHELL points at a different login shell
        let ancestor = worktrunk::shell::ancestor_shell();
        if let Some(ancestor) = ancestor {
            debug_lines.push(cformat!(
                "Detected shell: <bold>{}</> (process tree)",
                ancestor.name
            ));
        }
        // Show $SHELL to help diagnose rc file sourcing issues
        let shell_env = std::env::var("SHELL").ok().filter(|s| !s.is_empty());
        if let Some(shell_env) = &shell_env {
            debug_lines.push(cformat!("$SHELL: <bold>{shell_env}</>"));
        } else if ancestor.is_none()
            && let Some(detected) = worktrunk::shell::current_shell()
        {
            debug_lines.push(cformat!(
                "Detected shell: <bold>{detected}</> (via PSModulePath)"
            ));
        }

        if is_git_subcommand {
            debug_lines.push("Git subcommand: yes (GIT_EXEC_PATH set)".to_string());
        }
        writeln!(out, "{}", format_with_gutter(&debug_lines.join("\n"), None))?;
    }

    // Blank line separates the active/not-active status from the per-shell list.
    // Only emit when there's a list to render — otherwise the trailing hint
    // ("To configure, run …") would float behind a stray blank, breaking the
    // hint-attaches-to-subject formatting rule.
    if !scan_result.configured.is_empty() || !scan_result.skipped.is_empty() {
        writeln!(out)?;
    }

    let mut any_not_configured = false;
    let mut has_any_unmatched = false;

    // Show configured and not-configured shells (matching `config shell install` format exactly)
    // Fish ships completions as a separate file: "shell extension" for functions/ and "completions" for completions/
    // Every other supported shell wires completions inline with the extension, so they show "shell extension & completions"
    for result in &scan_result.configured {
        match result.action {
            ConfigAction::AlreadyExists => {
                render_already_configured(out, result, &detection_results, &cmd, shell_active)?;
            }
            ConfigAction::WouldAdd | ConfigAction::WouldCreate
                if render_would_add_or_create(
                    out,
                    result,
                    legacy_fish_conf_d.as_deref(),
                    legacy_fish_has_integration,
                    &cmd,
                    shell_active,
                )? =>
            {
                any_not_configured = true;
            }
            _ => {} // Added/Created won't appear in dry_run mode
        }
    }

    // Show skipped (not installed) shells
    // For fish with legacy integration, show migration hint instead of "skipped"
    for (shell, path) in &scan_result.skipped {
        if matches!(shell, Shell::Fish) && legacy_fish_has_integration {
            // Show migration hint for legacy fish location
            render_fish_legacy_migration(out, legacy_fish_conf_d.as_deref(), &cmd)?;
            continue;
        }
        let path = format_path_for_display(path);
        writeln!(
            out,
            "{}",
            info_message(cformat!(
                "<bold>{shell}</>: <dim>Skipped; {path} not found</>"
            ))
        )?;
    }

    // Summary hint pointing at `wt config shell install`. The fresh-user and
    // nothing-configured-anywhere cases are already covered by the hint emitted
    // directly under the warning, so this trailing emit is suppressed when
    // `hint_under_warning` fired. It still fires for the partial-config case
    // (some shells configured, some not) and for fish-legacy (working
    // integration via the legacy path, other shells unconfigured).
    let nothing_configured_yet = !shell_active && scan_result.configured.is_empty();
    if !hint_under_warning && (any_not_configured || nothing_configured_yet) {
        writeln!(
            out,
            "{}",
            hint_message(cformat!(
                "To configure, run <underline>{cmd} config shell install</>"
            ))
        )?;
    }

    // Show potential false negatives (lines containing cmd but not detected)
    // Skip files that have valid integration detected (matched_lines) - those are fine,
    // and the other lines containing cmd are just part of the integration script.
    // Also skip files already confirmed as integration by scan_shell_configs (e.g., Nushell/Fish
    // wrapper files that ARE the integration, not config files that source it).
    let confirmed_paths: HashSet<&Path> = scan_result
        .configured
        .iter()
        .filter(|r| {
            // For wrapper-based shells, the file at the path IS the integration — any
            // action means it was recognized. For eval-based shells, only AlreadyExists
            // means the config line was found.
            r.shell.is_wrapper_based() || matches!(r.action, ConfigAction::AlreadyExists)
        })
        .map(|r| r.path.as_path())
        .collect();
    for detection in &detection_results {
        if !detection.unmatched_candidates.is_empty()
            && detection.matched_lines.is_empty()
            && !confirmed_paths.contains(detection.path.as_path())
        {
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
                         Aliases should use <underline>{cmd}</>, not <underline>{cmd}.exe</>"
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
                    "Change to <underline>alias {}=\"{cmd}\"</> @ {location}",
                    alias.alias_name
                ))
            )?;
        }
    }

    // Whenever there are unmatched candidates, offer the false-negative report link.
    // A real detector miss in one config file is worth reporting even if another shell
    // is correctly detected — the `confirmed_paths` filter already excludes wrapper files,
    // and the narrow phrasing ("meant to load") steers non-integration lines (plain aliases)
    // away from reporting themselves as bugs.
    if has_any_unmatched {
        let unmatched_summary: Vec<_> = detection_results
            .iter()
            .filter(|r| {
                !r.unmatched_candidates.is_empty()
                    && r.matched_lines.is_empty()
                    && !confirmed_paths.contains(r.path.as_path())
            })
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
                "If {quoted} is meant to load Worktrunk shell integration, report a false negative: {issue_url}"
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
            // The auth-setup command differs by CLI: `gh`/`glab` use
            // `<tool> auth login`, `az` uses `az login`, `tea` uses `tea login add`.
            let auth_command = match tool {
                "az" => format!("{tool} login"),
                "tea" => format!("{tool} login add"),
                _ => format!("{tool} auth login"),
            };
            writeln!(
                out,
                "{}",
                warning_message(cformat!(
                    "<bold>{tool}</> installed but not authenticated; run <bold>{auth_command}</>"
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

/// Format the version-check line given the latest release version.
///
/// Pure over `latest` so both arms are unit-testable without injecting a
/// version through the environment; `render_version_check` supplies the value
/// from `fetch_latest_version`.
fn format_version_status(latest: &str) -> FormattedMessage {
    let current = crate::cli::version_str();
    if is_newer_version(latest, env!("CARGO_PKG_VERSION")) {
        info_message(cformat!(
            "Update available: <bold>{latest}</> (current: {current})"
        ))
    } else {
        info_message(cformat!("Up to date (<bold>{current}</>)"))
    }
}

/// Render version update check (fetches from GitHub)
fn render_version_check(out: &mut String) -> anyhow::Result<()> {
    match fetch_latest_version() {
        Ok(latest) => writeln!(out, "{}", format_version_status(&latest))?,
        Err(e) => {
            tracing::debug!(error = %e, "Version check failed: {e}");
            writeln!(out, "{}", hint_message("Version check unavailable"))?;
        }
    }
    Ok(())
}

/// Fetch the latest release version from GitHub
fn fetch_latest_version() -> anyhow::Result<String> {
    // Held for the whole lookup; the sub-startup-delay paths (test injection,
    // fast failures, response parsing) return before it ever renders.
    let _watchdog = worktrunk::progress::Watchdog::start("the version check", None);

    // Allow tests to inject a version without network access.
    // Set to "error" to simulate a fetch failure.
    if let Ok(version) = std::env::var("WORKTRUNK_TEST_LATEST_VERSION") {
        if version == "error" {
            anyhow::bail!("simulated fetch failure");
        }
        return Ok(version);
    }

    let user_agent = format!(
        "worktrunk/{} (https://worktrunk.dev)",
        env!("CARGO_PKG_VERSION")
    );
    // The watchdog (above) supplies "still waiting" feedback, so the fetch needn't
    // be cut off at an aggressive 5s. But the watchdog is TTY-gated — a
    // non-interactive run (CI, scripts, redirected stderr) gets no feedback — so a
    // hard ceiling still has to exist or such a run could hang silently:
    // --connect-timeout fails fast when offline, and a generous --max-time bounds a
    // connected-but-stalled host without cutting off a slow-but-progressing fetch.
    let output = {
        Cmd::new("curl")
            .args([
                "--silent",
                "--fail",
                "--connect-timeout",
                "10",
                "--max-time",
                "60",
                "--header",
                &format!("User-Agent: {user_agent}"),
                "https://api.github.com/repos/max-sixty/worktrunk/releases/latest",
            ])
            .run()?
    };

    if !output.status.success() {
        anyhow::bail!("GitHub API request failed");
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let tag = json["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing tag_name in response"))?;

    // Strip leading 'v' prefix (e.g., "v0.23.2" -> "0.23.2")
    Ok(tag.strip_prefix('v').unwrap_or(tag).to_string())
}

/// Compare two semver version strings (e.g., "0.24.0" > "0.23.2")
fn is_newer_version(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Option<(u32, u32, u32)> {
        let mut parts = s.splitn(3, '.');
        Some((
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
            parts.next()?.parse().ok()?,
        ))
    };
    match (parse(latest), parse(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_newer_version() {
        // Newer versions
        assert!(is_newer_version("0.24.0", "0.23.2"));
        assert!(is_newer_version("1.0.0", "0.99.99"));
        assert!(is_newer_version("0.23.3", "0.23.2"));
        assert!(is_newer_version("0.23.2", "0.23.1"));

        // Same version
        assert!(!is_newer_version("0.23.2", "0.23.2"));

        // Older versions
        assert!(!is_newer_version("0.23.1", "0.23.2"));
        assert!(!is_newer_version("0.22.0", "0.23.2"));

        // Invalid input
        assert!(!is_newer_version("invalid", "0.23.2"));
        assert!(!is_newer_version("0.23.2", "invalid"));
    }

    #[test]
    fn test_format_version_status() {
        // A version far above the current crate version is "newer".
        let update = format_version_status("999.0.0").to_string();
        assert!(
            update.contains("Update available"),
            "expected update message, got: {update}"
        );

        // The current crate version is not newer than itself.
        let up_to_date = format_version_status(env!("CARGO_PKG_VERSION")).to_string();
        assert!(
            up_to_date.contains("Up to date"),
            "expected up-to-date message, got: {up_to_date}"
        );
    }
}
