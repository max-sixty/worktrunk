use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use worktrunk::path::format_path_for_display;
use worktrunk::shell::Shell;
use worktrunk::styling::{
    CYAN_BOLD, INFO_EMOJI, PROGRESS_EMOJI, SUCCESS_EMOJI, format_bash_with_gutter,
};

pub struct ConfigureResult {
    pub shell: Shell,
    pub path: PathBuf,
    pub action: ConfigAction,
    pub config_line: String,
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
            ConfigAction::WouldAdd => "Will add to",
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
    command_name: String,
) -> Result<Vec<ConfigureResult>, String> {
    // First, do a dry-run to see what would be changed
    let preview_results = scan_shell_configs(shell_filter, true, &command_name)?;

    // If nothing to do, return early
    if preview_results.is_empty() {
        return Ok(vec![]);
    }

    // Check if any changes are needed (not all are AlreadyExists)
    let needs_changes = preview_results
        .iter()
        .any(|r| !matches!(r.action, ConfigAction::AlreadyExists));

    // If nothing needs to be changed, just return the preview results
    if !needs_changes {
        return Ok(preview_results);
    }

    // Show what will be done and ask for confirmation (unless --force flag is used)
    if !skip_confirmation && !prompt_for_confirmation(&preview_results)? {
        return Err("Cancelled by user".to_string());
    }

    // User confirmed (or --force flag was used), now actually apply the changes
    scan_shell_configs(shell_filter, false, &command_name)
}

fn scan_shell_configs(
    shell_filter: Option<Shell>,
    dry_run: bool,
    command_name: &str,
) -> Result<Vec<ConfigureResult>, String> {
    let shells = if let Some(shell) = shell_filter {
        vec![shell]
    } else {
        // Try all shells in consistent order
        vec![
            Shell::Bash,
            Shell::Zsh,
            Shell::Fish,
            Shell::Nushell,
            Shell::Powershell,
            Shell::Oil,
            Shell::Elvish,
            Shell::Xonsh,
        ]
    };

    let mut results = Vec::new();
    let mut checked_paths = Vec::new();

    for shell in shells {
        let paths = shell.config_paths(command_name);

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

        // Track all checked paths for better error messages
        checked_paths.extend(paths.iter().map(|p| (shell, p.clone())));

        // Only configure if explicitly targeting this shell OR if config file/location exists
        let should_configure = shell_filter.is_some() || has_config_location;

        if should_configure {
            let path = target_path.or_else(|| paths.first());
            if let Some(path) = path {
                match configure_shell_file(
                    shell,
                    path,
                    dry_run,
                    shell_filter.is_some(),
                    command_name,
                ) {
                    Ok(Some(result)) => results.push(result),
                    Ok(None) => {} // No action needed
                    Err(e) => {
                        // For non-critical errors, we could continue with other shells
                        // but for now we'll fail fast
                        return Err(format!("Failed to configure {}: {}", shell, e));
                    }
                }
            }
        }
    }

    if results.is_empty() && shell_filter.is_none() {
        // Provide helpful error message with checked locations
        let example_paths: Vec<String> = checked_paths
            .iter()
            .take(3)
            .map(|(_, p)| p.display().to_string())
            .collect();

        return Err(format!(
            "No shell config files found in $HOME. Checked: {}, and more. Create a config file or use --shell to specify a shell.",
            example_paths.join(", ")
        ));
    }

    Ok(results)
}

fn configure_shell_file(
    shell: Shell,
    path: &Path,
    dry_run: bool,
    explicit_shell: bool,
    command_name: &str,
) -> Result<Option<ConfigureResult>, String> {
    // Get a summary of the shell integration for display
    let integration_summary = shell.integration_summary(command_name);

    // The actual line we write to the config file
    let config_content = shell.config_line(command_name);

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
            let line = line.map_err(|e| format!("Failed to read line: {}", e))?;

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
        let parent_exists = path.parent().map(|p| p.exists()).unwrap_or(false);
        if !parent_exists {
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

fn prompt_for_confirmation(results: &[ConfigureResult]) -> Result<bool, String> {
    use anstyle::Style;
    use std::io::Write;
    use worktrunk::styling::{HINT_EMOJI, eprint, eprintln};

    // CRITICAL: Flush stdout before writing to stderr to prevent stream interleaving
    // In directive mode, flushes both stdout (directives) and stderr (messages)
    // In interactive mode, flushes both stdout and stderr
    crate::output::flush_for_stderr_prompt().map_err(|e| e.to_string())?;

    // Interactive prompts go to stderr so they appear even when stdout is redirected
    eprintln!();
    eprintln!("{HINT_EMOJI} {CYAN_BOLD}Configuration changes:{CYAN_BOLD:#}");
    eprintln!();

    for result in results {
        // Skip items that are already configured
        if matches!(result.action, ConfigAction::AlreadyExists) {
            continue;
        }

        // Format with bold shell and path
        let bold = Style::new().bold();
        let shell = result.shell;
        let path = format_path_for_display(&result.path);

        eprintln!(
            "{} {} {bold}{shell}{bold:#} {bold}{path}{bold:#}",
            result.action.emoji(),
            result.action.description(),
        );

        // Show the config line that will be added with gutter
        eprint!("{}", format_bash_with_gutter(&result.config_line, ""));
    }

    eprintln!();
    let bold = Style::new().bold();
    eprint!("{HINT_EMOJI} Proceed? {bold}[y/N]{bold:#} ");
    io::stderr().flush().map_err(|e| e.to_string())?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .map_err(|e| e.to_string())?;

    let response = input.trim().to_lowercase();
    Ok(response == "y" || response == "yes")
}
