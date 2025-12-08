use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use worktrunk::path::format_path_for_display;
use worktrunk::shell::{self, Shell};
use worktrunk::styling::{
    INFO_EMOJI, PROGRESS_EMOJI, SUCCESS_EMOJI, format_bash_with_gutter, warning_message,
};

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

    pub fn emoji(&self) -> &'static str {
        match self {
            UninstallAction::Removed => SUCCESS_EMOJI,
            UninstallAction::WouldRemove => PROGRESS_EMOJI,
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

    /// Returns the appropriate emoji for this action
    pub fn emoji(&self) -> &'static str {
        match self {
            ConfigAction::Added | ConfigAction::Created => SUCCESS_EMOJI,
            ConfigAction::AlreadyExists => INFO_EMOJI,
            ConfigAction::WouldAdd | ConfigAction::WouldCreate => PROGRESS_EMOJI,
        }
    }
}

pub fn handle_configure_shell(
    shell_filter: Option<Shell>,
    skip_confirmation: bool,
) -> Result<ScanResult, String> {
    // First, do a dry-run to see what would be changed
    let preview = scan_shell_configs(shell_filter, true)?;

    // Preview completions that would be written
    let shells: Vec<_> = preview.configured.iter().map(|r| r.shell).collect();
    let completion_preview = process_shell_completions(&shells, true)?;

    // If nothing to do, return early
    if preview.configured.is_empty() {
        return Ok(ScanResult {
            configured: preview.configured,
            completion_results: completion_preview,
            skipped: preview.skipped,
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

    // If nothing needs to be changed, just return the preview results
    if !needs_shell_changes && !needs_completion_changes {
        return Ok(ScanResult {
            configured: preview.configured,
            completion_results: completion_preview,
            skipped: preview.skipped,
        });
    }

    // Show what will be done and ask for confirmation (unless --force flag is used)
    if !skip_confirmation && !prompt_for_confirmation(&preview.configured, &completion_preview)? {
        return Err("Cancelled by user".to_string());
    }

    // User confirmed (or --force flag was used), now actually apply the changes
    let result = scan_shell_configs(shell_filter, false)?;
    let completion_results = process_shell_completions(&shells, false)?;

    // Zsh completions require compinit to be enabled. Unlike bash/fish, zsh doesn't
    // enable its completion system by default - users must explicitly call compinit.
    // We detect this and show an advisory hint to help users get completions working.
    //
    // We only show this advisory during `install`, not `init`, because:
    // - `init` outputs a script that gets eval'd - advisory would pollute that
    // - `install` is the user-facing command where hints are appropriate
    //
    // We show the advisory when:
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
            || (shell_filter.is_none() && shell::is_current_shell_zsh()));

    if should_check_compinit {
        // Probe user's zsh to check if compinit is enabled.
        // Only show advisory if we positively detect it's missing (Some(false)).
        // If detection fails (None), stay silent - we can't be sure.
        if shell::detect_zsh_compinit() == Some(false) {
            let _ = crate::output::print(warning_message(
                "Completions won't work; add to ~/.zshrc before the wt line:",
            ));
            let _ = crate::output::gutter(format_bash_with_gutter(
                "autoload -Uz compinit && compinit",
                "",
            ));
        }
    }

    Ok(ScanResult {
        configured: result.configured,
        completion_results,
        skipped: result.skipped,
    })
}

pub fn scan_shell_configs(
    shell_filter: Option<Shell>,
    dry_run: bool,
) -> Result<ScanResult, String> {
    let shells = if let Some(shell) = shell_filter {
        vec![shell]
    } else {
        // Try all supported shells in consistent order
        vec![Shell::Bash, Shell::Zsh, Shell::Fish]
    };

    let mut results = Vec::new();
    let mut skipped = Vec::new();

    for shell in shells {
        let paths = shell
            .config_paths()
            .map_err(|e| format!("Failed to get config paths for {}: {}", shell, e))?;

        // Find the first existing config file
        let target_path = paths.iter().find(|p| p.exists());

        // For Fish, also check if the parent directory (conf.d/) exists
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
                match configure_shell_file(shell, path, dry_run, shell_filter.is_some()) {
                    Ok(Some(result)) => results.push(result),
                    Ok(None) => {} // No action needed
                    Err(e) => {
                        // For non-critical errors, we could continue with other shells
                        // but for now we'll fail fast
                        return Err(format!("Failed to configure {}: {}", shell, e));
                    }
                }
            }
        } else if shell_filter.is_none() {
            // Track skipped shells (only when not explicitly filtering)
            // For Fish, we check for conf.d directory; for others, the config file
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
    })
}

fn configure_shell_file(
    shell: Shell,
    path: &Path,
    dry_run: bool,
    explicit_shell: bool,
) -> Result<Option<ConfigureResult>, String> {
    // Get a summary of the shell integration for display
    let integration_summary = shell.integration_summary();

    // The actual line we write to the config file
    let config_content = shell.config_line();

    // For Fish, we write to a separate conf.d/ file
    if matches!(shell, Shell::Fish) {
        return configure_fish_file(
            shell,
            path,
            &config_content,
            dry_run,
            explicit_shell,
            &integration_summary,
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
            if line.trim() == config_content {
                return Ok(Some(ConfigureResult {
                    shell,
                    path: path.to_path_buf(),
                    action: ConfigAction::AlreadyExists,
                    config_line: integration_summary.clone(),
                }));
            }
        }

        // Line doesn't exist, add it
        if dry_run {
            return Ok(Some(ConfigureResult {
                shell,
                path: path.to_path_buf(),
                action: ConfigAction::WouldAdd,
                config_line: integration_summary.clone(),
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
        write!(file, "\n{}\n", config_content).map_err(|e| {
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
            config_line: integration_summary.clone(),
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
                    config_line: integration_summary.clone(),
                }));
            }

            // Create parent directories if they don't exist
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    format!("Failed to create directory {}: {}", parent.display(), e)
                })?;
            }

            // Write the config content
            fs::write(path, format!("{}\n", config_content)).map_err(|e| {
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
                config_line: integration_summary.clone(),
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
    integration_summary: &str,
) -> Result<Option<ConfigureResult>, String> {
    // For Fish, we write to conf.d/{cmd_prefix}.fish (separate file)

    // Check if it already exists and has our integration
    if path.exists() {
        let existing_content = fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", format_path_for_display(path), e))?;

        // Canonical detection: check if the file matches exactly what we write
        if existing_content.trim() == content {
            return Ok(Some(ConfigureResult {
                shell,
                path: path.to_path_buf(),
                action: ConfigAction::AlreadyExists,
                config_line: integration_summary.to_string(),
            }));
        }
    }

    // File doesn't exist or doesn't have our integration
    // For Fish, create if parent directory exists or if explicitly targeting this shell
    // This is different from other shells because Fish uses conf.d/ which may exist
    // even if the specific wt.fish file doesn't
    if !explicit_shell && !path.exists() {
        // Check if parent directory exists
        if !path.parent().is_some_and(|p| p.exists()) {
            return Ok(None);
        }
    }

    if dry_run {
        return Ok(Some(ConfigureResult {
            shell,
            path: path.to_path_buf(),
            action: if path.exists() {
                ConfigAction::WouldAdd
            } else {
                ConfigAction::WouldCreate
            },
            config_line: integration_summary.to_string(),
        }));
    }

    // Create parent directories if they don't exist
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory {}: {}", parent.display(), e))?;
    }

    // Write the conditional wrapper (short one-liner that calls wt init fish | source)
    fs::write(path, format!("{}\n", content)).map_err(|e| {
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
        config_line: integration_summary.to_string(),
    }))
}

fn prompt_for_confirmation(
    results: &[ConfigureResult],
    completion_results: &[CompletionResult],
) -> Result<bool, String> {
    use anstyle::Style;
    use worktrunk::styling::{eprint, eprintln};

    // CRITICAL: Flush stdout before writing to stderr to prevent stream interleaving
    // In directive mode, flushes both stdout (directives) and stderr (messages)
    // In interactive mode, flushes both stdout and stderr
    crate::output::flush_for_stderr_prompt().map_err(|e| e.to_string())?;

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

        eprintln!(
            "{} {} {what} for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}",
            result.action.emoji(),
            result.action.description(),
        );

        // Show the config line that will be added with gutter
        eprint!("{}", format_bash_with_gutter(&result.config_line, ""));
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
            result.action.emoji(),
            result.action.description(),
        );

        // Show the completion content that will be written
        eprint!("{}", format_bash_with_gutter(FISH_COMPLETION.trim(), ""));
        eprintln!(); // Blank line after
    }

    prompt_yes_no()
}

/// Prompt user for yes/no confirmation, returns true if user confirms
fn prompt_yes_no() -> Result<bool, String> {
    use anstyle::Style;
    use std::io::Write;
    use worktrunk::styling::{PROMPT_EMOJI, eprint, eprintln};

    let bold = Style::new().bold();
    eprint!("{PROMPT_EMOJI} Proceed? {bold}[y/N]{bold:#} ");
    io::stderr().flush().map_err(|e| e.to_string())?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|e| e.to_string())?;

    eprintln!();

    let response = input.trim().to_lowercase();
    Ok(response == "y" || response == "yes")
}

/// Fish completion content - finds wt in PATH, with WORKTRUNK_BIN as optional override
const FISH_COMPLETION: &str = r#"# worktrunk completions for fish
complete --keep-order --exclusive --command wt --arguments "(test -n \"\$WORKTRUNK_BIN\"; or set -l WORKTRUNK_BIN (type -P wt); COMPLETE=fish \$WORKTRUNK_BIN -- (commandline --current-process --tokenize --cut-at-cursor) (commandline --current-token))"
"#;

/// Process shell completions - either preview or write based on dry_run flag
///
/// Note: Bash and Zsh use inline lazy completions in the init script.
/// Fish uses a separate completion file at ~/.config/fish/completions/wt.fish
/// that finds wt in PATH (with WORKTRUNK_BIN as optional override) to bypass the shell wrapper.
pub fn process_shell_completions(
    shells: &[Shell],
    dry_run: bool,
) -> Result<Vec<CompletionResult>, String> {
    let mut results = Vec::new();

    for &shell in shells {
        // Only fish has a separate completion file
        if shell != Shell::Fish {
            continue;
        }

        let completion_path = shell
            .completion_path()
            .map_err(|e| format!("Failed to get completion path for {}: {}", shell, e))?;

        // Check if completions already exist with correct content
        if completion_path.exists() {
            let existing = fs::read_to_string(&completion_path)
                .map_err(|e| format!("Failed to read {}: {}", completion_path.display(), e))?;
            if existing == FISH_COMPLETION {
                results.push(CompletionResult {
                    shell,
                    path: completion_path,
                    action: ConfigAction::AlreadyExists,
                });
                continue;
            }
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
                .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
        }

        // Write the completion file
        fs::write(&completion_path, FISH_COMPLETION)
            .map_err(|e| format!("Failed to write {}: {}", completion_path.display(), e))?;

        results.push(CompletionResult {
            shell,
            path: completion_path,
            action: ConfigAction::Created,
        });
    }

    Ok(results)
}

// Pattern detection for shell integration
fn has_integration_pattern(content: &str) -> bool {
    let lower = content.to_lowercase();
    lower.contains("wt init") || lower.contains("wt config shell init")
}

fn is_integration_line(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.starts_with('#')
        && has_integration_pattern(trimmed)
        && (trimmed.contains("eval") || trimmed.contains("source") || trimmed.contains("if "))
}

pub fn handle_unconfigure_shell(
    shell_filter: Option<Shell>,
    skip_confirmation: bool,
) -> Result<UninstallScanResult, String> {
    // First, do a dry-run to see what would be changed
    let preview = scan_for_uninstall(shell_filter, true)?;

    // If nothing to do, return early
    if preview.results.is_empty() && preview.completion_results.is_empty() {
        return Ok(preview);
    }

    // Show what will be done and ask for confirmation (unless --force flag is used)
    if !skip_confirmation
        && !prompt_for_uninstall_confirmation(&preview.results, &preview.completion_results)?
    {
        return Err("Cancelled by user".to_string());
    }

    // User confirmed (or --force flag was used), now actually apply the changes
    scan_for_uninstall(shell_filter, false)
}

fn scan_for_uninstall(
    shell_filter: Option<Shell>,
    dry_run: bool,
) -> Result<UninstallScanResult, String> {
    let shells = if let Some(shell) = shell_filter {
        vec![shell]
    } else {
        vec![Shell::Bash, Shell::Zsh, Shell::Fish]
    };

    let mut results = Vec::new();
    let mut not_found = Vec::new();

    for &shell in &shells {
        let paths = shell
            .config_paths()
            .map_err(|e| format!("Failed to get config paths for {}: {}", shell, e))?;

        // For Fish, check for wt.fish specifically (delete entire file)
        if matches!(shell, Shell::Fish) {
            if let Some(fish_path) = paths.first() {
                if fish_path.exists() {
                    if dry_run {
                        results.push(UninstallResult {
                            shell,
                            path: fish_path.clone(),
                            action: UninstallAction::WouldRemove,
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
                        });
                    }
                } else {
                    not_found.push((shell, fish_path.clone()));
                }
            }
            continue;
        }

        // For Bash/Zsh, scan config files
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

    // Fish has a separate completion file that needs to be removed
    let mut completion_results = Vec::new();
    let mut completion_not_found = Vec::new();

    for &shell in &shells {
        if shell != Shell::Fish {
            continue;
        }

        let completion_path = shell
            .completion_path()
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
) -> Result<Option<UninstallResult>, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", format_path_for_display(path), e))?;

    let lines: Vec<&str> = content.lines().collect();
    let integration_lines: Vec<(usize, &str)> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| is_integration_line(line))
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
        }));
    }

    // Remove matching lines
    let indices_to_remove: std::collections::HashSet<usize> =
        integration_lines.iter().map(|(i, _)| *i).collect();
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
    }))
}

fn prompt_for_uninstall_confirmation(
    results: &[UninstallResult],
    completion_results: &[CompletionUninstallResult],
) -> Result<bool, String> {
    use anstyle::Style;
    use worktrunk::styling::eprintln;

    crate::output::flush_for_stderr_prompt().map_err(|e| e.to_string())?;

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

        eprintln!(
            "{} {} {what} for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}",
            result.action.emoji(),
            result.action.description(),
        );
    }

    for result in completion_results {
        let bold = Style::new().bold();
        let shell = result.shell;
        let path = format_path_for_display(&result.path);

        eprintln!(
            "{} {} completions for {bold}{shell}{bold:#} @ {bold}{path}{bold:#}",
            result.action.emoji(),
            result.action.description(),
        );
    }

    prompt_yes_no()
}
