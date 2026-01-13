use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use worktrunk::path::format_path_for_display;
use worktrunk::shell::{self, Shell};
use worktrunk::styling::{
    INFO_SYMBOL, PROMPT_SYMBOL, SUCCESS_SYMBOL, format_bash_with_gutter, format_with_gutter,
    warning_message,
};

use crate::output;

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
fn is_worktrunk_managed_content(content: &str, cmd: &str) -> bool {
    content.contains(&format!("{cmd} config shell init"))
}

/// Clean up legacy fish conf.d file after installing to functions/
///
/// Previously, fish shell integration was installed to `~/.config/fish/conf.d/{cmd}.fish`.
/// This caused issues with Homebrew PATH setup (see issue #566). We now install to
/// `functions/{cmd}.fish` instead. This function removes the legacy file if it exists.
///
/// Returns the paths of files that were cleaned up.
fn cleanup_legacy_fish_conf_d(configured: &[ConfigureResult], cmd: &str) -> Vec<PathBuf> {
    use worktrunk::path::format_path_for_display;
    use worktrunk::styling::warning_message;

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
            let _ = crate::output::print(warning_message(format!(
                "Failed to remove deprecated {}: {e}",
                format_path_for_display(&legacy_path)
            )));
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

pub fn scan_shell_configs(
    shell_filter: Option<Shell>,
    dry_run: bool,
    cmd: &str,
) -> Result<ScanResult, String> {
    #[cfg(windows)]
    let default_shells = vec![Shell::Bash, Shell::Zsh, Shell::Fish, Shell::PowerShell];
    #[cfg(not(windows))]
    let default_shells = vec![Shell::Bash, Shell::Zsh, Shell::Fish];

    let shells = shell_filter.map_or(default_shells, |shell| vec![shell]);

    let mut results = Vec::new();
    let mut skipped = Vec::new();

    for shell in shells {
        let paths = shell
            .config_paths(cmd)
            .map_err(|e| format!("Failed to get config paths for {shell}: {e}"))?;

        // Find the first existing config file
        let target_path = paths.iter().find(|p| p.exists());

        // For Fish, also check if the parent directory (functions/) exists
        // since we create the file there rather than modifying an existing one
        let has_config_location = if matches!(shell, Shell::Fish) {
            paths
                .first()
                .and_then(|p| p.parent())
                .map(|p| p.exists())
                .unwrap_or(false)
                || target_path.is_some()
        } else {
            target_path.is_some()
        };

        // Only configure if explicitly targeting this shell OR if config file/location exists
        let should_configure = shell_filter.is_some() || has_config_location;

        if should_configure {
            let path = target_path.or_else(|| paths.first());
            if let Some(path) = path {
                match configure_shell_file(shell, path, dry_run, shell_filter.is_some(), cmd) {
                    Ok(Some(result)) => results.push(result),
                    Ok(None) => {} // No action needed
                    Err(e) => {
                        // For non-critical errors, we could continue with other shells
                        // but for now we'll fail fast
                        return Err(format!("Failed to configure {shell}: {e}"));
                    }
                }
            }
        } else if shell_filter.is_none() {
            // Track skipped shells (only when not explicitly filtering)
            // For Fish, we check for functions/ directory; for others, the config file
            let skipped_path = if matches!(shell, Shell::Fish) {
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

    if results.is_empty() && shell_filter.is_none() && skipped.is_empty() {
        // No shells checked at all (shouldn't happen normally)
        return Err("No shell config files found".to_string());
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
    explicit_shell: bool,
    cmd: &str,
) -> Result<Option<ConfigureResult>, String> {
    // The line we write to the config file (also used for display)
    let config_line = shell.config_line(cmd);

    // For Fish, we write a minimal wrapper to functions/{cmd}.fish that sources the
    // full function from the binary. This allows updates to worktrunk to automatically
    // provide the latest wrapper logic without requiring reinstall.
    if matches!(shell, Shell::Fish) {
        let init = shell::ShellInit::with_prefix(shell, cmd.to_string());
        let fish_wrapper = init
            .generate_fish_wrapper()
            .map_err(|e| format!("Failed to generate fish wrapper: {e}"))?;
        return configure_fish_file(
            shell,
            path,
            &fish_wrapper,
            dry_run,
            explicit_shell,
            &config_line,
        );
    }

    // For other shells, check if file exists
    if path.exists() {
        // Read the file and check if our integration already exists
        let file = fs::File::open(path)
            .map_err(|e| format!("Failed to read {}: {}", format_path_for_display(path), e))?;

        let reader = BufReader::new(file);

        // Check for the exact conditional wrapper we would write
        for line in reader.lines() {
            let line = line.map_err(|e| {
                format!(
                    "Failed to read line from {}: {}",
                    format_path_for_display(path),
                    e
                )
            })?;

            // Canonical detection: check if the line matches exactly what we write
            if line.trim() == config_line {
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
        // Only create if explicitly targeting this shell
        if explicit_shell {
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
                    format!("Failed to create directory {}: {}", parent.display(), e)
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

fn configure_fish_file(
    shell: Shell,
    path: &Path,
    content: &str,
    dry_run: bool,
    explicit_shell: bool,
    config_line: &str,
) -> Result<Option<ConfigureResult>, String> {
    // For Fish, we write a minimal wrapper to functions/{cmd}.fish that sources
    // the full function from `{cmd} config shell init fish` at runtime.
    // Fish autoloads these files on first invocation of the command.

    // Check if it already exists and has our integration
    // Use .ok() for read errors - treat as "not configured" rather than failing
    if let Some(existing_content) = path
        .exists()
        .then(|| fs::read_to_string(path).ok())
        .flatten()
    {
        // Canonical detection: check if the file matches exactly what we write
        // Trim both sides to handle trailing newlines consistently across platforms
        if existing_content.trim() == content.trim() {
            return Ok(Some(ConfigureResult {
                shell,
                path: path.to_path_buf(),
                action: ConfigAction::AlreadyExists,
                config_line: config_line.to_string(),
            }));
        }
    }

    // File doesn't exist or doesn't have our integration
    // For Fish, create if parent directory exists or if explicitly targeting this shell
    // This is different from other shells because Fish uses functions/ which may exist
    // even if the specific wt.fish file doesn't
    if !explicit_shell && !path.exists() {
        // Check if parent directory exists
        if !path.parent().is_some_and(|p| p.exists()) {
            return Ok(None);
        }
    }

    if dry_run {
        // Fish always writes the complete file (doesn't append), so always WouldCreate
        return Ok(Some(ConfigureResult {
            shell,
            path: path.to_path_buf(),
            action: ConfigAction::WouldCreate,
            config_line: config_line.to_string(),
        }));
    }

    // Create parent directories if they don't exist
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory {}: {e}", parent.display()))?;
    }

    // Write the complete fish function file
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
    use anstyle::Style;

    let bold = Style::new().bold();

    // Show shell extension changes
    for result in results {
        // Skip items that are already configured
        if matches!(result.action, ConfigAction::AlreadyExists) {
            continue;
        }

        let shell = result.shell;
        let path = format_path_for_display(&result.path);
        // Bash/Zsh: inline completions; Fish: separate completion file
        let what = if matches!(shell, Shell::Fish) {
            "shell extension"
        } else {
            "shell extension & completions"
        };

        let _ = output::print(format!(
            "{} {} {what} for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}",
            result.action.symbol(),
            result.action.description(),
        ));

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
        let _ = output::print(format_bash_with_gutter(&content));
        let _ = output::blank(); // Blank line after each shell block
    }

    // Show completion changes (only fish has separate completion files)
    for result in completion_results {
        if matches!(result.action, ConfigAction::AlreadyExists) {
            continue;
        }

        let shell = result.shell;
        let path = format_path_for_display(&result.path);

        let _ = output::print(format!(
            "{} {} completions for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}",
            result.action.symbol(),
            result.action.description(),
        ));

        // Show the completion content that will be written
        let fish_completion = fish_completion_content(cmd);
        let _ = output::print(format_bash_with_gutter(fish_completion.trim()));
        let _ = output::blank(); // Blank line after
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
    use anstyle::Style;

    let bold = Style::new().bold();

    for result in results {
        let shell = result.shell;
        let path = format_path_for_display(&result.path);

        // Deprecated files get a different message format
        if let Some(canonical) = &result.superseded_by {
            let canonical_path = format_path_for_display(canonical);
            let _ = output::print(format!(
                "{INFO_SYMBOL} {} {bold}{path}{bold:#} (deprecated; now using {bold}{canonical_path}{bold:#})",
                result.action.description(),
            ));
        } else {
            // Bash/Zsh: inline completions; Fish: separate completion file
            let what = if matches!(shell, Shell::Fish) {
                "shell extension"
            } else {
                "shell extension & completions"
            };

            let _ = output::print(format!(
                "{} {} {what} for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}",
                result.action.symbol(),
                result.action.description(),
            ));
        }
    }

    for result in completion_results {
        let shell = result.shell;
        let path = format_path_for_display(&result.path);

        let _ = output::print(format!(
            "{} {} completions for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}",
            result.action.symbol(),
            result.action.description(),
        ));
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
    use std::io::Write;

    // Flush before interactive prompt
    crate::output::flush().map_err(|e| e.to_string())?;

    loop {
        eprint!(
            "{}",
            color_print::cformat!("{} {} <bold>[y/N/?]</> ", PROMPT_SYMBOL, prompt_text)
        );
        io::stderr().flush().map_err(|e| e.to_string())?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|e| e.to_string())?;

        let response = input.trim().to_lowercase();
        match response.as_str() {
            "y" | "yes" => {
                eprintln!();
                return Ok(true);
            }
            "?" => {
                eprintln!();
                show_install_preview(results, completion_results, cmd);
                // Loop back to prompt again
            }
            _ => {
                // Empty, "n", "no", or anything else is decline
                eprintln!();
                return Ok(false);
            }
        }
    }
}

/// Prompt user for yes/no confirmation (simple [y/N] prompt)
fn prompt_yes_no() -> Result<bool, String> {
    use anstyle::Style;
    use std::io::Write;
    use worktrunk::styling::eprint;

    let bold = Style::new().bold();
    eprint!("{PROMPT_SYMBOL} Proceed? {bold}[y/N]{bold:#} ");
    io::stderr().flush().map_err(|e| e.to_string())?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|e| e.to_string())?;

    crate::output::blank().map_err(|e| e.to_string())?;

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
        // Use .ok() for read errors - treat as "not configured" rather than failing
        if let Some(existing) = completion_path
            .exists()
            .then(|| fs::read_to_string(&completion_path).ok())
            .flatten()
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
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory {}: {e}", parent.display()))?;
        }

        // Write the completion file
        fs::write(&completion_path, &fish_completion)
            .map_err(|e| format!("Failed to write {}: {e}", completion_path.display()))?;

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
    cmd: &str,
) -> Result<UninstallScanResult, String> {
    // First, do a dry-run to see what would be changed
    let preview = scan_for_uninstall(shell_filter, true, cmd)?;

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
    scan_for_uninstall(shell_filter, false, cmd)
}

fn scan_for_uninstall(
    shell_filter: Option<Shell>,
    dry_run: bool,
    cmd: &str,
) -> Result<UninstallScanResult, String> {
    #[cfg(windows)]
    let default_shells = vec![Shell::Bash, Shell::Zsh, Shell::Fish, Shell::PowerShell];
    #[cfg(not(windows))]
    let default_shells = vec![Shell::Bash, Shell::Zsh, Shell::Fish];

    let shells = shell_filter.map_or(default_shells, |shell| vec![shell]);

    let mut results = Vec::new();
    let mut not_found = Vec::new();

    for &shell in &shells {
        let paths = shell
            .config_paths(cmd)
            .map_err(|e| format!("Failed to get config paths for {shell}: {e}"))?;

        // For Fish, delete entire {cmd}.fish file (check both canonical and legacy locations)
        if matches!(shell, Shell::Fish) {
            let mut found_any = false;

            // Check canonical location (functions/)
            // Only remove if it contains worktrunk markers to avoid deleting user's custom file
            if let Some(fish_path) = paths.first()
                && fish_path.exists()
            {
                let is_worktrunk_managed = fs::read_to_string(fish_path)
                    .map(|content| is_worktrunk_managed_content(&content, cmd))
                    .unwrap_or(false);

                if is_worktrunk_managed {
                    found_any = true;
                    if dry_run {
                        results.push(UninstallResult {
                            shell,
                            path: fish_path.clone(),
                            action: UninstallAction::WouldRemove,
                            superseded_by: None,
                        });
                    } else {
                        fs::remove_file(fish_path).map_err(|e| {
                            format!(
                                "Failed to remove {}: {}",
                                format_path_for_display(fish_path),
                                e
                            )
                        })?;
                        results.push(UninstallResult {
                            shell,
                            path: fish_path.clone(),
                            action: UninstallAction::Removed,
                            superseded_by: None,
                        });
                    }
                }
            }

            // Also check legacy location (conf.d/) - issue #566
            // Only remove if it contains worktrunk markers to avoid deleting user's custom file
            let canonical_path = paths.first().cloned();
            if let Ok(legacy_path) = Shell::legacy_fish_conf_d_path(cmd)
                && legacy_path.exists()
            {
                let is_worktrunk_managed = fs::read_to_string(&legacy_path)
                    .map(|content| is_worktrunk_managed_content(&content, cmd))
                    .unwrap_or(false);

                if is_worktrunk_managed {
                    found_any = true;
                    if dry_run {
                        results.push(UninstallResult {
                            shell,
                            path: legacy_path.clone(),
                            action: UninstallAction::WouldRemove,
                            superseded_by: canonical_path.clone(),
                        });
                    } else {
                        fs::remove_file(&legacy_path).map_err(|e| {
                            format!("Failed to remove {}: {e}", legacy_path.display())
                        })?;
                        results.push(UninstallResult {
                            shell,
                            path: legacy_path,
                            action: UninstallAction::Removed,
                            superseded_by: canonical_path,
                        });
                    }
                }
            }

            if !found_any && let Some(fish_path) = paths.first() {
                not_found.push((shell, fish_path.clone()));
            }
            continue;
        }

        // For Bash/Zsh, scan config files
        let mut found = false;

        for path in &paths {
            if !path.exists() {
                continue;
            }

            match uninstall_from_file(shell, path, dry_run, cmd) {
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

    // Fish has a separate completion file that needs to be removed
    let mut completion_results = Vec::new();
    let mut completion_not_found = Vec::new();

    for &shell in &shells {
        if shell != Shell::Fish {
            continue;
        }

        let completion_path = shell
            .completion_path(cmd)
            .map_err(|e| format!("Failed to get completion path for {}: {}", shell, e))?;

        if completion_path.exists() {
            if dry_run {
                completion_results.push(CompletionUninstallResult {
                    shell,
                    path: completion_path,
                    action: UninstallAction::WouldRemove,
                });
            } else {
                fs::remove_file(&completion_path).map_err(|e| {
                    format!("Failed to remove {}: {}", completion_path.display(), e)
                })?;
                completion_results.push(CompletionUninstallResult {
                    shell,
                    path: completion_path,
                    action: UninstallAction::Removed,
                });
            }
        } else {
            completion_not_found.push((shell, completion_path));
        }
    }

    Ok(UninstallScanResult {
        results,
        completion_results,
        not_found,
        completion_not_found,
    })
}

fn uninstall_from_file(
    shell: Shell,
    path: &Path,
    dry_run: bool,
    cmd: &str,
) -> Result<Option<UninstallResult>, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", format_path_for_display(path), e))?;

    let lines: Vec<&str> = content.lines().collect();
    let integration_lines: Vec<(usize, &str)> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| shell::is_shell_integration_line(line, cmd))
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
    let mut indices_to_remove: std::collections::HashSet<usize> =
        integration_lines.iter().map(|(i, _)| *i).collect();
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
    use anstyle::Style;

    crate::output::flush().map_err(|e| e.to_string())?;

    for result in results {
        let bold = Style::new().bold();
        let shell = result.shell;
        let path = format_path_for_display(&result.path);
        // Bash/Zsh: inline completions; Fish: separate completion file
        let what = if matches!(shell, Shell::Fish) {
            "shell extension"
        } else {
            "shell extension & completions"
        };

        crate::output::print(format!(
            "{} {} {what} for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}",
            result.action.symbol(),
            result.action.description(),
        ))
        .map_err(|e| e.to_string())?;
    }

    for result in completion_results {
        let bold = Style::new().bold();
        let shell = result.shell;
        let path = format_path_for_display(&result.path);

        crate::output::print(format!(
            "{} {} completions for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}",
            result.action.symbol(),
            result.action.description(),
        ))
        .map_err(|e| e.to_string())?;
    }

    prompt_yes_no()
}

/// Show samples of all output message types
pub fn handle_show_theme() -> Result<(), String> {
    use color_print::cformat;
    use worktrunk::styling::{
        error_message, hint_message, info_message, progress_message, success_message,
    };

    // Progress
    crate::output::print(progress_message(cformat!(
        "Rebasing <bold>feature</> onto <bold>main</>..."
    )))
    .map_err(|e| e.to_string())?;

    // Success
    crate::output::print(success_message(cformat!(
        "Created worktree for <bold>feature</> @ <bold>/path/to/worktree</>"
    )))
    .map_err(|e| e.to_string())?;

    // Error
    crate::output::print(error_message(cformat!("Branch <bold>feature</> not found")))
        .map_err(|e| e.to_string())?;

    // Warning
    crate::output::print(warning_message(cformat!(
        "Branch <bold>feature</> has uncommitted changes"
    )))
    .map_err(|e| e.to_string())?;

    // Hint
    crate::output::print(hint_message(cformat!(
        "To rebase onto main, run <bright-black>wt merge</>"
    )))
    .map_err(|e| e.to_string())?;

    // Info
    crate::output::print(info_message(cformat!("Showing <bold>5</> worktrees")))
        .map_err(|e| e.to_string())?;

    crate::output::blank().map_err(|e| e.to_string())?;

    // Gutter - quoted content
    crate::output::print(info_message("Gutter formatting (quoted content):"))
        .map_err(|e| e.to_string())?;
    crate::output::print(format_with_gutter(
        "[commit-generation]\ncommand = \"llm --model claude\"",
        None,
    ))
    .map_err(|e| e.to_string())?;

    crate::output::blank().map_err(|e| e.to_string())?;

    // Gutter - bash code
    crate::output::print(info_message("Gutter formatting (shell code):"))
        .map_err(|e| e.to_string())?;
    crate::output::print(format_bash_with_gutter(
        "eval \"$(wt config shell init bash)\"",
    ))
    .map_err(|e| e.to_string())?;

    crate::output::blank().map_err(|e| e.to_string())?;

    // Prompt
    crate::output::print(info_message("Prompt formatting:")).map_err(|e| e.to_string())?;
    crate::output::print(format!("{PROMPT_SYMBOL} Proceed? [y/N] ")).map_err(|e| e.to_string())?;

    Ok(())
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
}
