use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anstyle::Style;
use worktrunk::path::format_path_for_display;
use worktrunk::shell::{self, Shell};
use worktrunk::styling::{
    INFO_SYMBOL, SUCCESS_SYMBOL, eprint, eprintln, format_bash_with_gutter, format_toml,
    format_with_gutter, hint_message, prompt_message, warning_message,
};

use crate::output::prompt::{PromptResponse, prompt_yes_no_preview};
use crate::output::shell_integration::shell_extension_label;

pub struct ConfigureResult {
    pub shell: Shell,
    pub path: PathBuf,
    pub action: ConfigAction,
    pub config_line: String,
}

pub struct UninstallResult {
    pub shell: Shell,
    pub path: PathBuf,
    pub action: UninstallAction,
    /// Path that replaces this one (for deprecated location cleanup)
    pub superseded_by: Option<PathBuf>,
}

pub struct UninstallScanResult {
    pub results: Vec<UninstallResult>,
    pub completion_results: Vec<CompletionUninstallResult>,
    /// Shell extensions not found (bash/zsh show as "integration", fish as "shell extension")
    pub not_found: Vec<(Shell, PathBuf)>,
    /// Completion files not found (only fish has separate completion files)
    pub completion_not_found: Vec<(Shell, PathBuf)>,
}

pub struct CompletionUninstallResult {
    pub shell: Shell,
    pub path: PathBuf,
    pub action: UninstallAction,
}

pub struct ScanResult {
    pub configured: Vec<ConfigureResult>,
    pub completion_results: Vec<CompletionResult>,
    pub skipped: Vec<(Shell, PathBuf)>, // Shell + first path that was checked
    /// Zsh was configured but compinit is missing (completions won't work without it)
    pub zsh_needs_compinit: bool,
    /// Legacy files that were cleaned up (e.g., fish conf.d/wt.fish -> functions/wt.fish migration)
    pub legacy_cleanups: Vec<PathBuf>,
}

pub struct CompletionResult {
    pub shell: Shell,
    pub path: PathBuf,
    pub action: ConfigAction,
}

#[derive(Debug, PartialEq)]
pub enum UninstallAction {
    Removed,
    WouldRemove,
}

impl UninstallAction {
    pub fn description(&self) -> &str {
        match self {
            UninstallAction::Removed => "Removed",
            UninstallAction::WouldRemove => "Will remove",
        }
    }

    pub fn symbol(&self) -> &'static str {
        match self {
            UninstallAction::Removed => SUCCESS_SYMBOL,
            UninstallAction::WouldRemove => INFO_SYMBOL,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum ConfigAction {
    Added,
    AlreadyExists,
    Created,
    WouldAdd,
    WouldCreate,
}

impl ConfigAction {
    pub fn description(&self) -> &str {
        match self {
            ConfigAction::Added => "Added",
            ConfigAction::AlreadyExists => "Already configured",
            ConfigAction::Created => "Created",
            ConfigAction::WouldAdd => "Will add",
            ConfigAction::WouldCreate => "Will create",
        }
    }

    /// Returns the appropriate symbol for this action
    pub fn symbol(&self) -> &'static str {
        match self {
            ConfigAction::Added | ConfigAction::Created => SUCCESS_SYMBOL,
            ConfigAction::AlreadyExists => INFO_SYMBOL,
            ConfigAction::WouldAdd | ConfigAction::WouldCreate => INFO_SYMBOL,
        }
    }
}

/// Check if file content appears to be worktrunk-managed (contains our markers)
///
/// Used to identify files safe to delete during migration/uninstall.
/// Requires both the init command AND pipe to source, to avoid false positives.
fn is_worktrunk_managed_content(content: &str, cmd: &str) -> bool {
    content.contains(&format!("{cmd} config shell init")) && content.contains("| source")
}

/// Cmd-agnostic content check: matches a worktrunk-generated wrapper file
/// regardless of the binary name embedded in it.
///
/// Used by `uninstall` to recognize wrapper files installed under any binary
/// name (e.g. `wt.fish`, `git-wt.fish`, `git-wt.nu`) when scanning the wrapper
/// directories. The canonical marker is the `# worktrunk shell integration for
/// <shell>` comment at the top of every wrapper template; a regex fallback
/// catches older installs that predate the marker.
fn is_worktrunk_managed_content_any_cmd(content: &str) -> bool {
    if content.contains("# worktrunk shell integration for") {
        return true;
    }
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"\b[\w.-]+?(?:\.exe)?\s+config\s+shell\s+init\b")
            .expect("static regex is valid")
    });
    re.is_match(content) && content.contains("| source")
}

/// Clean up legacy fish conf.d file after installing to functions/
///
/// Previously, fish shell integration was installed to `~/.config/fish/conf.d/{cmd}.fish`.
/// This caused issues with Homebrew PATH setup (see issue #566). We now install to
/// `functions/{cmd}.fish` instead. This function removes the legacy file if it exists.
///
/// Returns the paths of files that were cleaned up.
fn cleanup_legacy_fish_conf_d(configured: &[ConfigureResult], cmd: &str) -> Vec<PathBuf> {
    let mut cleaned = Vec::new();

    // Clean up if fish was part of the install (regardless of whether it already existed)
    // This handles the case where user manually created functions/wt.fish but still has
    // the old conf.d/wt.fish hanging around
    let fish_targeted = configured.iter().any(|r| r.shell == Shell::Fish);

    if !fish_targeted {
        return cleaned;
    }

    // Check for legacy conf.d file
    let Ok(legacy_path) = Shell::legacy_fish_conf_d_path(cmd) else {
        return cleaned;
    };

    if !legacy_path.exists() {
        return cleaned;
    }

    // Only remove if the file contains worktrunk integration markers
    // to avoid deleting user's custom wt.fish that isn't from worktrunk
    let Ok(content) = fs::read_to_string(&legacy_path) else {
        return cleaned;
    };

    if !is_worktrunk_managed_content(&content, cmd) {
        return cleaned;
    }

    match fs::remove_file(&legacy_path) {
        Ok(()) => {
            cleaned.push(legacy_path);
        }
        Err(e) => {
            // Warn but don't fail - the new integration will still work
            eprintln!(
                "{}",
                warning_message(color_print::cformat!(
                    "Failed to remove deprecated <bold>{}</>: {e}",
                    format_path_for_display(&legacy_path)
                ))
            );
        }
    }

    cleaned
}

pub fn handle_configure_shell(
    shell_filter: Option<Shell>,
    skip_confirmation: bool,
    dry_run: bool,
    cmd: String,
) -> Result<ScanResult, String> {
    shell::validate_shell_command_name(&cmd)?;

    // First, do a dry-run to see what would be changed
    let preview = scan_shell_configs(shell_filter, true, &cmd)?;

    // Preview completions that would be written
    let shells: Vec<_> = preview.configured.iter().map(|r| r.shell).collect();
    let completion_preview = process_shell_completions(&shells, true, &cmd)?;

    // If nothing to do, return early
    if preview.configured.is_empty() {
        return Ok(ScanResult {
            configured: preview.configured,
            completion_results: completion_preview,
            skipped: preview.skipped,
            zsh_needs_compinit: false,
            legacy_cleanups: Vec::new(),
        });
    }

    // Check if any changes are needed (not all are AlreadyExists)
    let needs_shell_changes = preview
        .configured
        .iter()
        .any(|r| !matches!(r.action, ConfigAction::AlreadyExists));
    let needs_completion_changes = completion_preview
        .iter()
        .any(|r| !matches!(r.action, ConfigAction::AlreadyExists));

    // For --dry-run, show preview and return without modifying anything
    if dry_run {
        show_install_preview(&preview.configured, &completion_preview, &cmd);
        return Ok(ScanResult {
            configured: preview.configured,
            completion_results: completion_preview,
            skipped: preview.skipped,
            zsh_needs_compinit: false,
            legacy_cleanups: Vec::new(),
        });
    }

    // If nothing needs to be changed, still clean up legacy fish conf.d files
    // A user might have upgraded and have both functions/wt.fish and conf.d/wt.fish
    if !needs_shell_changes && !needs_completion_changes {
        let legacy_cleanups = cleanup_legacy_fish_conf_d(&preview.configured, &cmd);
        return Ok(ScanResult {
            configured: preview.configured,
            completion_results: completion_preview,
            skipped: preview.skipped,
            zsh_needs_compinit: false,
            legacy_cleanups,
        });
    }

    // Show what will be done and ask for confirmation (unless --yes flag is used)
    if !skip_confirmation
        && !prompt_for_install(
            &preview.configured,
            &completion_preview,
            &cmd,
            "Install shell integration?",
        )?
    {
        return Err("Cancelled by user".to_string());
    }

    // User confirmed (or --yes flag was used), now actually apply the changes
    let result = scan_shell_configs(shell_filter, false, &cmd)?;
    let completion_results = process_shell_completions(&shells, false, &cmd)?;

    // Zsh completions require compinit to be enabled. Unlike bash/fish, zsh doesn't
    // enable its completion system by default - users must explicitly call compinit.
    // We detect this and return a flag so the caller can show an appropriate advisory.
    //
    // We only check this during `install`, not `init`, because:
    // - `init` outputs a script that gets eval'd - advisory would pollute that
    // - `install` is the user-facing command where hints are appropriate
    //
    // We check when:
    // - User explicitly runs `install zsh` (they clearly want zsh integration)
    // - User runs `install` (all shells) AND their $SHELL is zsh (they use zsh daily)
    //
    // We skip if:
    // - User runs `install` but their $SHELL is bash/fish (they may be configuring
    //   zsh for occasional use; don't nag about their non-primary shell)
    // - Zsh was already configured (AlreadyExists) - they've seen this before
    let zsh_was_configured = result
        .configured
        .iter()
        .any(|r| r.shell == Shell::Zsh && !matches!(r.action, ConfigAction::AlreadyExists));
    let should_check_compinit = zsh_was_configured
        && (shell_filter == Some(Shell::Zsh)
            || (shell_filter.is_none() && shell::current_shell() == Some(Shell::Zsh)));

    // Probe user's zsh to check if compinit is enabled.
    // Only flag if we positively detect it's missing (Some(false)).
    // If detection fails (None), stay silent - we can't be sure.
    let zsh_needs_compinit = should_check_compinit && shell::detect_zsh_compinit() == Some(false);

    // Clean up legacy fish conf.d file if we just installed to functions/
    // This handles migration from the old conf.d location (issue #566)
    let legacy_cleanups = cleanup_legacy_fish_conf_d(&result.configured, &cmd);

    Ok(ScanResult {
        configured: result.configured,
        completion_results,
        skipped: result.skipped,
        zsh_needs_compinit,
        legacy_cleanups,
    })
}

/// Check if we should auto-configure PowerShell profiles.
///
/// **Non-Windows:** PowerShell Core sets PSModulePath, which we use to detect
/// PowerShell sessions. This is reliable because PowerShell must be explicitly
/// installed on these platforms.
///
/// **Windows:** We check that `SHELL` is NOT set. The `SHELL` env var is set by
/// Git Bash, MSYS2, and Cygwin, but NOT by cmd.exe or PowerShell. When `SHELL`
/// is absent on Windows, the user is likely in a Windows-native shell (cmd or
/// PowerShell), so we auto-configure both PowerShell profiles. This avoids the
/// PSModulePath false-positive issue (issue #885) while still supporting
/// PowerShell users who haven't created a profile yet.
fn should_auto_configure_powershell() -> bool {
    // Allow tests to override detection (set via Command::env() in integration tests)
    if let Ok(val) = std::env::var("WORKTRUNK_TEST_POWERSHELL_ENV") {
        return val == "1";
    }

    #[cfg(windows)]
    {
        // On Windows, SHELL is set by Git Bash/MSYS2/Cygwin but not by cmd/PowerShell.
        // If SHELL is absent, we're likely in a Windows-native shell.
        std::env::var_os("SHELL").is_none()
    }

    #[cfg(not(windows))]
    {
        // On non-Windows, PSModulePath reliably indicates PowerShell Core
        std::env::var_os("PSModulePath").is_some()
    }
}

pub fn scan_shell_configs(
    shell_filter: Option<Shell>,
    dry_run: bool,
    cmd: &str,
) -> Result<ScanResult, String> {
    shell::validate_shell_command_name(cmd)?;

    // Iterate every supported shell. Shells the user doesn't have are filtered
    // out of the Skipped output by `is_installed()` below, matching how
    // bash/zsh/fish/nushell are handled.
    let default_shells = Shell::all();

    // Detect whether the user is *running in* PowerShell or Nushell right now.
    // This unlocks `allow_create` so we'll write a profile/autoload file even
    // when none exists — needed because PowerShell users may not have a profile
    // (issue #885) and Nushell's vendor/autoload was introduced in 0.96.0.
    // - PowerShell (non-Windows): PSModulePath set
    // - PowerShell (Windows): SHELL absent (Git Bash/MSYS2/Cygwin set it)
    // - Nushell: `nu` on PATH
    let in_powershell_env = should_auto_configure_powershell();
    let nushell_available = Shell::Nushell.is_installed();

    let shells = shell_filter.map_or(default_shells, |shell| vec![shell]);

    let mut results = Vec::new();
    let mut skipped = Vec::new();

    for shell in shells {
        let paths = shell
            .config_paths(cmd)
            .map_err(|e| format!("Failed to get config paths for {shell}: {e}"))?;

        // Find the first existing config file
        let target_path = paths.iter().find(|p| p.exists());

        // For Fish/Nushell, also check if any candidate's parent directory exists
        // since we create the file there rather than modifying an existing one
        let has_config_location = if shell.is_wrapper_based() {
            paths.iter().any(|p| p.parent().is_some_and(|d| d.exists())) || target_path.is_some()
        } else {
            target_path.is_some()
        };

        // Auto-configure shells when we detect them on the system, even if their
        // config directory doesn't exist yet:
        // - PowerShell: profile may not exist (issue #885)
        // - Nushell: vendor/autoload/ may not exist (introduced in nushell v0.96.0)
        let in_detected_shell = (matches!(shell, Shell::PowerShell) && in_powershell_env)
            || (matches!(shell, Shell::Nushell) && nushell_available);

        // Only configure if explicitly targeting this shell OR if config file/location exists
        // OR if we detected we're running in this shell's environment
        let should_configure = shell_filter.is_some() || has_config_location || in_detected_shell;

        // Allow creating the config file if explicitly targeting this shell,
        // or if we detected we're in this shell's environment
        let allow_create = shell_filter.is_some() || in_detected_shell;

        if should_configure {
            let path = target_path.or_else(|| paths.first());
            if let Some(path) = path {
                match configure_shell_file(shell, path, dry_run, allow_create, cmd) {
                    Ok(Some(result)) => results.push(result),
                    Ok(None) => {} // No action needed
                    Err(e) => {
                        // For non-critical errors, we could continue with other shells
                        // but for now we'll fail fast
                        return Err(format!("Failed to configure {shell}: {e}"));
                    }
                }
            }
        } else if shell_filter.is_none() && shell.is_installed() {
            // Track skipped shells (only when not explicitly filtering, and only
            // when the shell binary is on PATH — otherwise the user almost
            // certainly doesn't use this shell and the entry is just clutter).
            // For Fish/Nushell, we check for parent directory; for others, the config file
            let skipped_path = if shell.is_wrapper_based() {
                paths
                    .first()
                    .and_then(|p| p.parent())
                    .map(|p| p.to_path_buf())
            } else {
                paths.first().cloned()
            };
            if let Some(path) = skipped_path {
                skipped.push((shell, path));
            }
        }
    }

    Ok(ScanResult {
        configured: results,
        completion_results: Vec::new(), // Completions handled separately in handle_configure_shell
        skipped,
        zsh_needs_compinit: false,   // Caller handles compinit detection
        legacy_cleanups: Vec::new(), // Caller handles legacy cleanup
    })
}

fn configure_shell_file(
    shell: Shell,
    path: &Path,
    dry_run: bool,
    allow_create: bool,
    cmd: &str,
) -> Result<Option<ConfigureResult>, String> {
    // The line we write to the config file (also used for display)
    let config_line = shell.config_line(cmd);

    // For Fish and Nushell, we write the full wrapper to a file that gets autoloaded.
    // This allows updates to worktrunk to automatically provide the latest wrapper logic
    // without requiring reinstall.
    if shell.is_wrapper_based() {
        let init = shell::ShellInit::with_prefix(shell, cmd.to_string());
        let wrapper = if matches!(shell, Shell::Fish) {
            init.generate_fish_wrapper()
                .map_err(|e| format!("Failed to generate fish wrapper: {e}"))?
        } else {
            init.generate()
                .map_err(|e| format!("Failed to generate nushell wrapper: {e}"))?
        };
        return configure_wrapper_file(shell, path, &wrapper, dry_run, allow_create, &config_line);
    }

    // For other shells, check if file exists
    if path.exists() {
        // Read the file and check if our integration already exists
        let file = fs::File::open(path)
            .map_err(|e| format!("Failed to read {}: {}", format_path_for_display(path), e))?;

        let reader = BufReader::new(file);

        // Check for the canonical line and older/manual forms for this shell.
        for line in reader.lines() {
            let line = line.map_err(|e| {
                format!(
                    "Failed to read line from {}: {}",
                    format_path_for_display(path),
                    e
                )
            })?;

            if is_install_shell_integration_line(&line, shell, cmd) {
                return Ok(Some(ConfigureResult {
                    shell,
                    path: path.to_path_buf(),
                    action: ConfigAction::AlreadyExists,
                    config_line: config_line.clone(),
                }));
            }
        }

        // Line doesn't exist, add it
        if dry_run {
            return Ok(Some(ConfigureResult {
                shell,
                path: path.to_path_buf(),
                action: ConfigAction::WouldAdd,
                config_line: config_line.clone(),
            }));
        }

        // Append the line with proper spacing
        let mut file = OpenOptions::new().append(true).open(path).map_err(|e| {
            format!(
                "Failed to open {} for writing: {}",
                format_path_for_display(path),
                e
            )
        })?;

        // Add blank line before config, then the config line with its own newline
        write!(file, "\n{}\n", config_line).map_err(|e| {
            format!(
                "Failed to write to {}: {}",
                format_path_for_display(path),
                e
            )
        })?;

        Ok(Some(ConfigureResult {
            shell,
            path: path.to_path_buf(),
            action: ConfigAction::Added,
            config_line: config_line.clone(),
        }))
    } else {
        // File doesn't exist
        // Only create if allowed (explicitly targeting this shell or detected environment)
        if allow_create {
            if dry_run {
                return Ok(Some(ConfigureResult {
                    shell,
                    path: path.to_path_buf(),
                    action: ConfigAction::WouldCreate,
                    config_line: config_line.clone(),
                }));
            }

            // Create parent directories if they don't exist
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    format!(
                        "Failed to create directory {}: {}",
                        format_path_for_display(parent),
                        e
                    )
                })?;
            }

            // Write the config content
            fs::write(path, format!("{}\n", config_line)).map_err(|e| {
                format!(
                    "Failed to write to {}: {}",
                    format_path_for_display(path),
                    e
                )
            })?;

            Ok(Some(ConfigureResult {
                shell,
                path: path.to_path_buf(),
                action: ConfigAction::Created,
                config_line: config_line.clone(),
            }))
        } else {
            // Don't create config files for shells the user might not use
            Ok(None)
        }
    }
}

fn is_install_shell_integration_line(line: &str, shell: Shell, cmd: &str) -> bool {
    shell::is_shell_integration_line(line, cmd)
        && line
            .to_ascii_lowercase()
            .contains(&format!("config shell init {shell}"))
}

/// Extract non-comment, non-blank lines from fish source for comparison.
///
/// This lets us detect existing installations even when comment text has changed
/// between versions (e.g. updated documentation URLs).
fn fish_code_lines(source: &str) -> Vec<&str> {
    source
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

fn configure_wrapper_file(
    shell: Shell,
    path: &Path,
    content: &str,
    dry_run: bool,
    allow_create: bool,
    config_line: &str,
) -> Result<Option<ConfigureResult>, String> {
    // For Fish and Nushell, we write the full wrapper to a file that gets autoloaded.
    // - Fish: functions/{cmd}.fish is autoloaded on first invocation
    // - Nushell: vendor/autoload/{cmd}.nu is autoloaded automatically at startup

    // Check if it already exists and has our integration
    // Read errors (including not-found) fall through to "not configured"
    if let Ok(existing_content) = fs::read_to_string(path) {
        // Compare only non-comment lines so that comment changes (e.g. updated
        // URLs) don't cause existing installations to appear unconfigured.
        if fish_code_lines(&existing_content) == fish_code_lines(content) {
            return Ok(Some(ConfigureResult {
                shell,
                path: path.to_path_buf(),
                action: ConfigAction::AlreadyExists,
                config_line: config_line.to_string(),
            }));
        }
    }

    // File doesn't exist or doesn't have our integration
    // For Fish/Nushell, create if parent directory exists or if explicitly allowed
    // This is different from other shells because these use autoload directories
    // which may exist even if the specific wrapper file doesn't
    if !allow_create && !path.exists() {
        // Check if parent directory exists
        if !path.parent().is_some_and(|p| p.exists()) {
            return Ok(None);
        }
    }

    if dry_run {
        // Fish/Nushell write the complete file - use WouldAdd if file exists, WouldCreate if new
        let action = if path.exists() {
            ConfigAction::WouldAdd
        } else {
            ConfigAction::WouldCreate
        };
        return Ok(Some(ConfigureResult {
            shell,
            path: path.to_path_buf(),
            action,
            config_line: config_line.to_string(),
        }));
    }

    // Create parent directories if they don't exist
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Failed to create directory {}: {e}",
                format_path_for_display(parent)
            )
        })?;
    }

    // Write the complete wrapper file
    fs::write(path, format!("{}\n", content))
        .map_err(|e| format!("Failed to write {}: {e}", format_path_for_display(path)))?;

    Ok(Some(ConfigureResult {
        shell,
        path: path.to_path_buf(),
        action: ConfigAction::Created,
        config_line: config_line.to_string(),
    }))
}

/// Display what will be installed (shell extensions and completions)
///
/// Shows the config lines that will be added without prompting.
/// Used both for install preview and when user types `?` at prompt.
///
/// Note: I/O errors are intentionally ignored - preview is best-effort
/// and shouldn't block the prompt flow.
pub fn show_install_preview(
    results: &[ConfigureResult],
    completion_results: &[CompletionResult],
    cmd: &str,
) {
    let bold = Style::new().bold();

    // Show shell extension changes
    for result in results {
        // Skip items that are already configured
        if matches!(result.action, ConfigAction::AlreadyExists) {
            continue;
        }

        let shell = result.shell;
        let path = format_path_for_display(&result.path);
        let what = shell_extension_label(shell);

        eprintln!(
            "{} {} {what} for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}",
            result.action.symbol(),
            result.action.description(),
        );

        // Show the config content that will be added with gutter
        // Fish: show the wrapper (it's a complete file that sources the full function)
        // Other shells: show the one-liner that gets appended
        let content = if matches!(shell, Shell::Fish) {
            shell::ShellInit::with_prefix(shell, cmd.to_string())
                .generate_fish_wrapper()
                .unwrap_or_else(|_| result.config_line.clone())
        } else {
            result.config_line.clone()
        };
        eprintln!("{}", format_bash_with_gutter(&content));

        if matches!(shell, Shell::Nushell) {
            eprintln!("{}", hint_message("Nushell support is experimental"));
        }

        eprintln!(); // Blank line after each shell block
    }

    // Show completion changes (only fish has separate completion files)
    for result in completion_results {
        if matches!(result.action, ConfigAction::AlreadyExists) {
            continue;
        }

        let shell = result.shell;
        let path = format_path_for_display(&result.path);

        eprintln!(
            "{} {} completions for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}",
            result.action.symbol(),
            result.action.description(),
        );

        // Show the completion content that will be written
        let fish_completion = fish_completion_content(cmd);
        eprintln!("{}", format_bash_with_gutter(fish_completion.trim()));
        eprintln!(); // Blank line after
    }
}

/// Display what will be uninstalled (shell extensions and completions)
///
/// Shows the files that will be modified without prompting.
/// Used for --dry-run mode.
///
/// Note: I/O errors are intentionally ignored - preview is best-effort
/// and shouldn't block the flow.
pub fn show_uninstall_preview(
    results: &[UninstallResult],
    completion_results: &[CompletionUninstallResult],
) {
    let bold = Style::new().bold();

    for result in results {
        let shell = result.shell;
        let path = format_path_for_display(&result.path);

        // Deprecated files get a different message format
        if let Some(canonical) = &result.superseded_by {
            let canonical_path = format_path_for_display(canonical);
            eprintln!(
                "{INFO_SYMBOL} {} {bold}{path}{bold:#} (deprecated; now using {bold}{canonical_path}{bold:#})",
                result.action.description(),
            );
        } else {
            let what = shell_extension_label(shell);

            eprintln!(
                "{} {} {what} for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}",
                result.action.symbol(),
                result.action.description(),
            );
        }
    }

    for result in completion_results {
        let shell = result.shell;
        let path = format_path_for_display(&result.path);

        eprintln!(
            "{} {} completions for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}",
            result.action.symbol(),
            result.action.description(),
        );
    }
}

/// Prompt for install with [y/N/?] options
///
/// - `y` or `yes`: Accept and return true
/// - `n`, `no`, or empty: Decline and return false
/// - `?`: Show preview (via show_install_preview) and re-prompt
pub fn prompt_for_install(
    results: &[ConfigureResult],
    completion_results: &[CompletionResult],
    cmd: &str,
    prompt_text: &str,
) -> Result<bool, String> {
    let response = prompt_yes_no_preview(prompt_text, || {
        show_install_preview(results, completion_results, cmd);
    })
    .map_err(|e| e.to_string())?;

    Ok(response == PromptResponse::Accepted)
}

/// Prompt user for yes/no confirmation (simple [y/N] prompt)
fn prompt_yes_no() -> Result<bool, String> {
    // Blank line before prompt for visual separation
    eprintln!();
    eprint!(
        "{} ",
        prompt_message(color_print::cformat!("Proceed? <bold>[y/N]</>"))
    );
    io::stderr().flush().map_err(|e| e.to_string())?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|e| e.to_string())?;

    let response = input.trim().to_lowercase();
    Ok(response == "y" || response == "yes")
}

/// Fish completion content - finds command in PATH, with WORKTRUNK_BIN as optional override
fn fish_completion_content(cmd: &str) -> String {
    format!(
        r#"# worktrunk completions for fish
complete --keep-order --exclusive --command {cmd} --arguments "(test -n \"\$WORKTRUNK_BIN\"; or set -l WORKTRUNK_BIN (type -P {cmd} 2>/dev/null); and COMPLETE=fish \$WORKTRUNK_BIN -- (commandline --current-process --tokenize --cut-at-cursor) (commandline --current-token))"
"#
    )
}

/// Process shell completions - either preview or write based on dry_run flag
///
/// Note: Bash and Zsh use inline lazy completions in the init script.
/// Fish uses a separate completion file at ~/.config/fish/completions/{cmd}.fish
/// that finds the command in PATH (with WORKTRUNK_BIN as optional override) to bypass the shell wrapper.
pub fn process_shell_completions(
    shells: &[Shell],
    dry_run: bool,
    cmd: &str,
) -> Result<Vec<CompletionResult>, String> {
    shell::validate_shell_command_name(cmd)?;

    let mut results = Vec::new();
    let fish_completion = fish_completion_content(cmd);

    for &shell in shells {
        // Only fish has a separate completion file
        if shell != Shell::Fish {
            continue;
        }

        let completion_path = shell
            .completion_path(cmd)
            .map_err(|e| format!("Failed to get completion path for {shell}: {e}"))?;

        // Check if completions already exist with correct content
        // Read errors (including not-found) fall through to "not configured"
        if let Ok(existing) = fs::read_to_string(&completion_path)
            && existing == fish_completion
        {
            results.push(CompletionResult {
                shell,
                path: completion_path,
                action: ConfigAction::AlreadyExists,
            });
            continue;
        }

        if dry_run {
            let action = if completion_path.exists() {
                ConfigAction::WouldAdd
            } else {
                ConfigAction::WouldCreate
            };
            results.push(CompletionResult {
                shell,
                path: completion_path,
                action,
            });
            continue;
        }

        // Create parent directory if needed
        if let Some(parent) = completion_path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "Failed to create directory {}: {e}",
                    format_path_for_display(parent)
                )
            })?;
        }

        // Write the completion file
        fs::write(&completion_path, &fish_completion).map_err(|e| {
            format!(
                "Failed to write {}: {e}",
                format_path_for_display(&completion_path)
            )
        })?;

        results.push(CompletionResult {
            shell,
            path: completion_path,
            action: ConfigAction::Created,
        });
    }

    Ok(results)
}

pub fn handle_unconfigure_shell(
    shell_filter: Option<Shell>,
    skip_confirmation: bool,
    dry_run: bool,
) -> Result<UninstallScanResult, String> {
    // First, do a dry-run to see what would be changed
    let preview = scan_for_uninstall(shell_filter, true)?;

    // If nothing to do, return early
    if preview.results.is_empty() && preview.completion_results.is_empty() {
        return Ok(preview);
    }

    // For --dry-run, show preview and return without prompting or applying
    if dry_run {
        show_uninstall_preview(&preview.results, &preview.completion_results);
        return Ok(preview);
    }

    // Show what will be done and ask for confirmation (unless --yes flag is used)
    if !skip_confirmation
        && !prompt_for_uninstall_confirmation(&preview.results, &preview.completion_results)?
    {
        return Err("Cancelled by user".to_string());
    }

    // User confirmed (or --yes flag was used), now actually apply the changes
    scan_for_uninstall(shell_filter, false)
}

/// Remove a config file with a context-rich error message.
fn remove_config_file(path: &std::path::Path) -> Result<(), String> {
    fs::remove_file(path)
        .map_err(|e| format!("Failed to remove {}: {e}", format_path_for_display(path)))
}

/// Uninstall scans the shell-owned directories and removes every worktrunk-managed
/// file or line, regardless of the binary name it was installed under.
///
/// For Fish/Nushell wrapper files (one file per binary name), this lists the
/// wrapper directories and admits any file whose content matches
/// `is_worktrunk_managed_content_any_cmd`. For Bash/Zsh/PowerShell (line-based),
/// it scans the rc/profile files and uses `is_shell_integration_line_for_uninstall_any_cmd`.
/// No `--cmd` is needed because the marker is the content, not the file name.
fn scan_for_uninstall(
    shell_filter: Option<Shell>,
    dry_run: bool,
) -> Result<UninstallScanResult, String> {
    // For uninstall, scan every shell (Shell::all includes PowerShell) to clean
    // up any existing profiles.
    let default_shells = Shell::all();

    let shells = shell_filter.map_or(default_shells, |shell| vec![shell]);

    let home =
        shell::home_dir_required().map_err(|e| format!("Cannot determine home directory: {e}"))?;

    let mut results = Vec::new();
    let mut not_found = Vec::new();
    // Wrapper file names found (e.g. `wt.fish`, `git-wt.fish`); the matching
    // completion files in `completions/` share these names.
    let mut fish_wrapper_names: HashSet<String> = HashSet::new();

    for &shell in &shells {
        match shell {
            Shell::Fish => {
                let functions_dir = home.join(".config").join("fish").join("functions");
                let confd_dir = home.join(".config").join("fish").join("conf.d");

                let canonical = scan_fish_wrappers(&functions_dir)?;
                let legacy = scan_fish_wrappers(&confd_dir)?;
                let found_any = !canonical.is_empty() || !legacy.is_empty();

                for path in &canonical {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        fish_wrapper_names.insert(name.to_string());
                    }
                    let action = if dry_run {
                        UninstallAction::WouldRemove
                    } else {
                        remove_config_file(path)?;
                        UninstallAction::Removed
                    };
                    results.push(UninstallResult {
                        shell,
                        path: path.clone(),
                        action,
                        superseded_by: None,
                    });
                }

                for path in &legacy {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        fish_wrapper_names.insert(name.to_string());
                    }
                    let superseded_by = path.file_name().map(|n| functions_dir.join(n));
                    let action = if dry_run {
                        UninstallAction::WouldRemove
                    } else {
                        remove_config_file(path)?;
                        UninstallAction::Removed
                    };
                    results.push(UninstallResult {
                        shell,
                        path: path.clone(),
                        action,
                        superseded_by,
                    });
                }

                if !found_any {
                    not_found.push((shell, functions_dir));
                }
            }

            Shell::Nushell => {
                let mut found_any = false;
                let candidates = shell::nushell_config_candidates(&home);
                for config_dir in &candidates {
                    let autoload_dir = config_dir.join("vendor").join("autoload");
                    let nu_files = scan_nushell_wrappers(&autoload_dir)?;
                    for path in &nu_files {
                        found_any = true;
                        let action = if dry_run {
                            UninstallAction::WouldRemove
                        } else {
                            remove_config_file(path)?;
                            UninstallAction::Removed
                        };
                        results.push(UninstallResult {
                            shell,
                            path: path.clone(),
                            action,
                            superseded_by: None,
                        });
                    }
                }
                if !found_any {
                    // Report the first candidate's autoload dir as the expected location
                    if let Some(first) = candidates.first() {
                        not_found.push((shell, first.join("vendor").join("autoload")));
                    }
                }
            }

            Shell::Bash | Shell::Zsh | Shell::PowerShell => {
                let paths = line_based_config_paths(shell, &home);
                let mut found = false;

                for path in &paths {
                    if !path.exists() {
                        continue;
                    }

                    match uninstall_from_file(shell, path, dry_run) {
                        Ok(Some(result)) => {
                            results.push(result);
                            found = true;
                            break; // Only process first matching file per shell
                        }
                        Ok(None) => {} // No integration found in this file
                        Err(e) => return Err(e),
                    }
                }

                if !found && let Some(first_path) = paths.first() {
                    not_found.push((shell, first_path.clone()));
                }
            }
        }
    }

    // Fish completion files share names with the wrappers; clean up the same set.
    let mut completion_results = Vec::new();
    let mut completion_not_found = Vec::new();
    if shells.contains(&Shell::Fish) {
        let completions_dir = home.join(".config").join("fish").join("completions");
        let mut completion_found_any = false;
        for name in &fish_wrapper_names {
            let comp_path = completions_dir.join(name);
            if comp_path.exists() {
                completion_found_any = true;
                let action = if dry_run {
                    UninstallAction::WouldRemove
                } else {
                    remove_config_file(&comp_path)?;
                    UninstallAction::Removed
                };
                completion_results.push(CompletionUninstallResult {
                    shell: Shell::Fish,
                    path: comp_path,
                    action,
                });
            }
        }
        if !completion_found_any {
            completion_not_found.push((Shell::Fish, completions_dir));
        }
    }

    Ok(UninstallScanResult {
        results,
        completion_results,
        not_found,
        completion_not_found,
    })
}

/// Compute line-based config paths (Bash/Zsh/PowerShell) inline so uninstall
/// doesn't need a cmd to drive `Shell::config_paths` (which uses cmd only for
/// wrapper-shell file names).
fn line_based_config_paths(shell: Shell, home: &Path) -> Vec<PathBuf> {
    match shell {
        Shell::Bash => vec![home.join(".bashrc")],
        Shell::Zsh => {
            let zdotdir = std::env::var("ZDOTDIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| home.to_path_buf());
            vec![zdotdir.join(".zshrc")]
        }
        Shell::PowerShell => shell::powershell_profile_paths(home),
        Shell::Fish | Shell::Nushell => Vec::new(),
    }
}

/// List `*.fish` files in `dir` whose content matches a worktrunk wrapper
/// (regardless of binary name). Returns empty if `dir` does not exist.
fn scan_fish_wrappers(dir: &Path) -> Result<Vec<PathBuf>, String> {
    scan_wrapper_directory(dir, "fish")
}

/// List `*.nu` files in `dir` whose content matches a worktrunk wrapper.
fn scan_nushell_wrappers(dir: &Path) -> Result<Vec<PathBuf>, String> {
    scan_wrapper_directory(dir, "nu")
}

fn scan_wrapper_directory(dir: &Path, extension: &str) -> Result<Vec<PathBuf>, String> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(dir)
        .map_err(|e| format!("Failed to read {}: {e}", format_path_for_display(dir)))?;
    let mut out = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|e| format!("Failed to read {}: {e}", format_path_for_display(dir)))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some(extension) {
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue, // unreadable file: skip, don't fail the whole uninstall
        };
        if is_worktrunk_managed_content_any_cmd(&content) {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn uninstall_from_file(
    shell: Shell,
    path: &Path,
    dry_run: bool,
) -> Result<Option<UninstallResult>, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", format_path_for_display(path), e))?;

    let lines: Vec<&str> = content.lines().collect();
    let integration_lines: Vec<(usize, &str)> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| shell::is_shell_integration_line_for_uninstall_any_cmd(line))
        .map(|(i, line)| (i, *line))
        .collect();

    if integration_lines.is_empty() {
        return Ok(None);
    }

    if dry_run {
        return Ok(Some(UninstallResult {
            shell,
            path: path.to_path_buf(),
            action: UninstallAction::WouldRemove,
            superseded_by: None,
        }));
    }

    // Remove matching lines and any immediately preceding blank line
    // (install adds "\n{line}\n", so we remove both the blank and the integration line)
    let mut indices_to_remove: HashSet<usize> = integration_lines.iter().map(|(i, _)| *i).collect();
    for &(i, _) in &integration_lines {
        if i > 0 && lines[i - 1].trim().is_empty() {
            indices_to_remove.insert(i - 1);
        }
    }
    let new_lines: Vec<&str> = lines
        .iter()
        .enumerate()
        .filter(|(i, _)| !indices_to_remove.contains(i))
        .map(|(_, line)| *line)
        .collect();

    let new_content = new_lines.join("\n");
    // Preserve trailing newline if original had one
    let new_content = if content.ends_with('\n') {
        format!("{}\n", new_content)
    } else {
        new_content
    };

    fs::write(path, new_content)
        .map_err(|e| format!("Failed to write {}: {}", format_path_for_display(path), e))?;

    Ok(Some(UninstallResult {
        shell,
        path: path.to_path_buf(),
        action: UninstallAction::Removed,
        superseded_by: None,
    }))
}

fn prompt_for_uninstall_confirmation(
    results: &[UninstallResult],
    completion_results: &[CompletionUninstallResult],
) -> Result<bool, String> {
    for result in results {
        let bold = Style::new().bold();
        let shell = result.shell;
        let path = format_path_for_display(&result.path);
        let what = shell_extension_label(shell);

        eprintln!(
            "{} {} {what} for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}",
            result.action.symbol(),
            result.action.description(),
        );
    }

    for result in completion_results {
        let bold = Style::new().bold();
        let shell = result.shell;
        let path = format_path_for_display(&result.path);

        eprintln!(
            "{} {} completions for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}",
            result.action.symbol(),
            result.action.description(),
        );
    }

    prompt_yes_no()
}

/// Show samples of all output message types
pub fn handle_show_theme() {
    use color_print::cformat;
    use worktrunk::styling::{
        error_message, hint_message, info_message, progress_message, success_message,
    };

    // Progress
    eprintln!(
        "{}",
        progress_message(cformat!("Rebasing <bold>feature</> onto <bold>main</>..."))
    );

    // Success
    eprintln!(
        "{}",
        success_message(cformat!(
            "Created worktree for <bold>feature</> @ <bold>/path/to/worktree</>"
        ))
    );

    // Error
    eprintln!(
        "{}",
        error_message(cformat!("Branch <bold>feature</> not found"))
    );

    // Warning
    eprintln!(
        "{}",
        warning_message(cformat!("Branch <bold>feature</> has uncommitted changes"))
    );

    // Hint
    eprintln!(
        "{}",
        hint_message(cformat!("To rebase onto main, run <underline>wt merge</>"))
    );

    // Info
    eprintln!("{}", info_message(cformat!("Showing <bold>5</> worktrees")));

    eprintln!();

    // Gutter - error details (plain text, no syntax highlighting)
    eprintln!("{}", info_message("Gutter formatting (error details):"));
    eprintln!(
        "{}",
        format_with_gutter("expected `=`, found newline at line 3 column 1", None,)
    );

    eprintln!();

    // Gutter - TOML config (syntax highlighted)
    eprintln!("{}", info_message("Gutter formatting (config):"));
    eprintln!(
        "{}",
        format_toml("[commit.generation]\ncommand = \"llm --model claude\"")
    );

    eprintln!();

    // Gutter - bash code (short, long wrapping, multi-line string, multi-line command, and template)
    eprintln!("{}", info_message("Gutter formatting (shell code):"));
    eprintln!(
        "{}",
        format_bash_with_gutter(
            "eval \"$(wt config shell init bash)\"\necho 'This is a long command that will wrap to the next line when the terminal is narrow enough to require wrapping.'\necho 'hello\nworld'\ncargo build --release &&\ncargo test\ncp {{ repo_root }}/target {{ worktree }}/target"
        )
    );

    eprintln!();

    // Prompt
    eprintln!("{}", info_message("Prompt formatting:"));
    eprintln!("{} ", prompt_message("Proceed? [y/N]"));

    eprintln!();

    // Color palette — each color rendered in itself
    eprintln!("{}", info_message("Color palette:"));
    use anstyle::{AnsiColor, Color};
    let fg = |c: AnsiColor| Some(Color::Ansi(c));
    let palette: &[(&str, Style)] = &[
        ("red", Style::new().fg_color(fg(AnsiColor::Red))),
        ("green", Style::new().fg_color(fg(AnsiColor::Green))),
        ("yellow", Style::new().fg_color(fg(AnsiColor::Yellow))),
        ("blue", Style::new().fg_color(fg(AnsiColor::Blue))),
        ("cyan", Style::new().fg_color(fg(AnsiColor::Cyan))),
        ("bold", Style::new().bold()),
        ("dim", Style::new().dimmed()),
        ("bold red", Style::new().fg_color(fg(AnsiColor::Red)).bold()),
        (
            "bold green",
            Style::new().fg_color(fg(AnsiColor::Green)).bold(),
        ),
        (
            "bold yellow",
            Style::new().fg_color(fg(AnsiColor::Yellow)).bold(),
        ),
        (
            "bold cyan",
            Style::new().fg_color(fg(AnsiColor::Cyan)).bold(),
        ),
        (
            "dim bright-black",
            Style::new().fg_color(fg(AnsiColor::BrightBlack)).dimmed(),
        ),
        (
            "dim blue",
            Style::new().fg_color(fg(AnsiColor::Blue)).dimmed(),
        ),
        (
            "dim green",
            Style::new().fg_color(fg(AnsiColor::Green)).dimmed(),
        ),
        (
            "dim cyan",
            Style::new().fg_color(fg(AnsiColor::Cyan)).dimmed(),
        ),
        (
            "dim magenta",
            Style::new().fg_color(fg(AnsiColor::Magenta)).dimmed(),
        ),
        (
            "dim yellow",
            Style::new().fg_color(fg(AnsiColor::Yellow)).dimmed(),
        ),
    ];

    let palette_text: String = palette
        .iter()
        .map(|(name, style)| format!("{style}{name}{style:#}"))
        .collect::<Vec<_>>()
        .join("\n");
    eprintln!("{}", format_with_gutter(&palette_text, None));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uninstall_action_description() {
        assert_eq!(UninstallAction::Removed.description(), "Removed");
        assert_eq!(UninstallAction::WouldRemove.description(), "Will remove");
    }

    #[test]
    fn test_uninstall_action_emoji() {
        assert_eq!(UninstallAction::Removed.symbol(), SUCCESS_SYMBOL);
        assert_eq!(UninstallAction::WouldRemove.symbol(), INFO_SYMBOL);
    }

    #[test]
    fn test_config_action_description() {
        assert_eq!(ConfigAction::Added.description(), "Added");
        assert_eq!(
            ConfigAction::AlreadyExists.description(),
            "Already configured"
        );
        assert_eq!(ConfigAction::Created.description(), "Created");
        assert_eq!(ConfigAction::WouldAdd.description(), "Will add");
        assert_eq!(ConfigAction::WouldCreate.description(), "Will create");
    }

    #[test]
    fn test_config_action_emoji() {
        assert_eq!(ConfigAction::Added.symbol(), SUCCESS_SYMBOL);
        assert_eq!(ConfigAction::Created.symbol(), SUCCESS_SYMBOL);
        assert_eq!(ConfigAction::AlreadyExists.symbol(), INFO_SYMBOL);
        assert_eq!(ConfigAction::WouldAdd.symbol(), INFO_SYMBOL);
        assert_eq!(ConfigAction::WouldCreate.symbol(), INFO_SYMBOL);
    }

    #[test]
    fn test_is_shell_integration_line() {
        // Valid integration lines for "wt"
        assert!(shell::is_shell_integration_line(
            "eval \"$(wt config shell init bash)\"",
            "wt"
        ));
        assert!(shell::is_shell_integration_line(
            "  eval \"$(wt config shell init zsh)\"  ",
            "wt"
        ));
        assert!(shell::is_shell_integration_line(
            "if command -v wt; then eval \"$(wt config shell init bash)\"; fi",
            "wt"
        ));
        assert!(shell::is_shell_integration_line(
            "source <(wt config shell init fish)",
            "wt"
        ));

        // Valid integration lines for "git-wt"
        assert!(shell::is_shell_integration_line(
            "eval \"$(git-wt config shell init bash)\"",
            "git-wt"
        ));
        assert!(!shell::is_shell_integration_line(
            "eval \"$(wt config shell init bash)\"",
            "git-wt"
        ));

        // Not integration lines (comments)
        assert!(!shell::is_shell_integration_line(
            "# eval \"$(wt config shell init bash)\"",
            "wt"
        ));

        // Not integration lines (no eval/source/if)
        assert!(!shell::is_shell_integration_line(
            "wt config shell init bash",
            "wt"
        ));
        assert!(!shell::is_shell_integration_line(
            "echo wt config shell init bash",
            "wt"
        ));
    }

    #[test]
    fn test_fish_completion_content() {
        insta::assert_snapshot!(fish_completion_content("wt"));
    }

    #[test]
    fn test_fish_completion_content_custom_cmd() {
        insta::assert_snapshot!(fish_completion_content("myapp"));
    }

    // Note: should_auto_configure_powershell() is tested via WORKTRUNK_TEST_POWERSHELL_ENV
    // override in tests/integration_tests/configure_shell.rs.

    #[test]
    fn test_fish_code_lines_strips_comments_and_blanks() {
        let source = "# comment\n\nfunction wt\n    command wt $argv\nend\n";
        assert_eq!(
            fish_code_lines(source),
            vec!["function wt", "command wt $argv", "end"]
        );
    }

    #[test]
    fn test_fish_code_lines_matches_despite_different_comments() {
        let old = "# Docs: https://worktrunk.dev/docs/shell-integration\nfunction wt\n    command wt $argv\nend";
        let new = "# Docs: https://worktrunk.dev/config/#shell-integration\nfunction wt\n    command wt $argv\nend";
        assert_eq!(fish_code_lines(old), fish_code_lines(new));
    }
}
