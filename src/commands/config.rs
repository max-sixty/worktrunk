use anyhow::Context;
use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};
use std::path::PathBuf;
use worktrunk::git::{Repository, detached_head};
use worktrunk::path::format_path_for_display;
use worktrunk::shell::Shell;
use worktrunk::styling::{
    AnstyleStyle, CYAN, GREEN, GREEN_BOLD, HINT, HINT_EMOJI, INFO_EMOJI, SUCCESS_EMOJI,
    format_toml, print, println,
};

use super::configure_shell::{ConfigAction, process_shell_completions, scan_shell_configs};

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
    let config_path = get_global_config_path()
        .ok_or_else(|| anyhow::anyhow!("Could not determine global config path"))?;

    // Check if file already exists
    if config_path.exists() {
        let bold = AnstyleStyle::new().bold();
        println!(
            "{INFO_EMOJI} Global config already exists: {bold}{}{bold:#}",
            format_path_for_display(&config_path)
        );
        println!();
        println!("{HINT_EMOJI} {HINT}Use 'wt config show' to view existing configuration{HINT:#}");
        println!(
            "{HINT_EMOJI} {HINT}Use 'wt config create --help' for config format reference{HINT:#}"
        );
        return Ok(());
    }

    // Create parent directory if it doesn't exist
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("Failed to create config directory: {}", e))?;
    }

    // Write the example config with all values commented out
    let commented_config = comment_out_config(CONFIG_EXAMPLE);
    std::fs::write(&config_path, commented_config).context("Failed to write config file")?;

    // Success message
    let green_bold = GREEN.bold();
    println!(
        "{SUCCESS_EMOJI} {GREEN}Created config file: {green_bold}{}{green_bold:#}",
        format_path_for_display(&config_path)
    );
    println!();
    println!(
        "{HINT_EMOJI} {HINT}Edit this file to customize worktree paths and LLM settings{HINT:#}"
    );

    Ok(())
}

/// Handle the config show command
pub fn handle_config_show() -> anyhow::Result<()> {
    // Display global config
    display_global_config()?;
    println!();

    // Display project config if in a git repository
    display_project_config()?;
    println!();

    // Display shell integration status
    display_shell_status()?;

    Ok(())
}

fn display_global_config() -> anyhow::Result<()> {
    let bold = AnstyleStyle::new().bold();

    // Get config path
    let config_path = get_global_config_path()
        .ok_or_else(|| anyhow::anyhow!("Could not determine global config path"))?;

    println!(
        "{INFO_EMOJI} Global Config: {bold}{}{bold:#}",
        format_path_for_display(&config_path)
    );

    // Check if file exists
    if !config_path.exists() {
        println!("{HINT_EMOJI} {HINT}Not found (using defaults){HINT:#}");
        println!("{HINT_EMOJI} {HINT}Run 'wt config create' to create a config file{HINT:#}");
        println!();
        let default_config =
            "# Default configuration:\nworktree-path = \"../{{ main_worktree }}.{{ branch }}\"";
        print!("{}", format_toml(default_config, ""));
        return Ok(());
    }

    // Read and display the file contents
    let contents = std::fs::read_to_string(&config_path).context("Failed to read config file")?;

    if contents.trim().is_empty() {
        println!("{HINT_EMOJI} {HINT}Empty file (using defaults){HINT:#}");
        return Ok(());
    }

    // Display TOML with syntax highlighting (gutter at column 0)
    print!("{}", format_toml(&contents, ""));

    Ok(())
}

fn display_project_config() -> anyhow::Result<()> {
    let bold = AnstyleStyle::new().bold();
    let dim = AnstyleStyle::new().dimmed();

    // Try to get current repository root
    let repo = Repository::current();
    let repo_root = match repo.worktree_root() {
        Ok(root) => root,
        Err(_) => {
            println!("{INFO_EMOJI} {dim}Project Config: Not in a git repository{dim:#}");
            return Ok(());
        }
    };
    let config_path = repo_root.join(".config").join("wt.toml");

    println!(
        "{INFO_EMOJI} Project Config: {bold}{}{bold:#}",
        format_path_for_display(&config_path)
    );

    // Check if file exists
    if !config_path.exists() {
        println!("{HINT_EMOJI} {HINT}Not found{HINT:#}");
        return Ok(());
    }

    // Read and display the file contents
    let contents = std::fs::read_to_string(&config_path).context("Failed to read config file")?;

    if contents.trim().is_empty() {
        println!("{HINT_EMOJI} {HINT}Empty file{HINT:#}");
        return Ok(());
    }

    // Display TOML with syntax highlighting (gutter at column 0)
    print!("{}", format_toml(&contents, ""));

    Ok(())
}

fn display_shell_status() -> anyhow::Result<()> {
    use worktrunk::styling::{WARNING, WARNING_EMOJI, format_with_gutter};

    let bold = AnstyleStyle::new().bold();
    let dim = AnstyleStyle::new().dimmed();

    // Use the same detection logic as `wt config shell install`
    let scan_result = match scan_shell_configs(None, true) {
        Ok(r) => r,
        Err(e) => {
            println!("{HINT_EMOJI} {HINT}Could not determine shell status: {e}{HINT:#}");
            return Ok(());
        }
    };

    // Get completion status for fish (bash/zsh completions are inline in init script)
    let shells: Vec<_> = scan_result.configured.iter().map(|r| r.shell).collect();
    let completion_results = process_shell_completions(&shells, true).unwrap_or_default();

    let mut any_configured = false;
    let mut any_not_configured = false;

    // Show configured and not-configured shells (matching `config shell install` format exactly)
    for result in &scan_result.configured {
        let shell = result.shell;
        let path = format_path_for_display(&result.path);
        // Match the format from `wt config shell install`
        let what = if matches!(shell, Shell::Bash | Shell::Zsh) {
            "shell extension & completions"
        } else {
            "shell extension"
        };

        match result.action {
            ConfigAction::AlreadyExists => {
                any_configured = true;
                println!(
                    "{INFO_EMOJI} Already configured {what} for {bold}{shell}{bold:#} @ {path}"
                );

                // Check if zsh has compinit enabled (required for completions)
                if matches!(shell, Shell::Zsh) && check_zsh_compinit_missing() {
                    println!(
                        "{WARNING_EMOJI} {WARNING}Completions won't work; add to ~/.zshrc before the wt line:{WARNING:#}"
                    );
                    print!(
                        "{}",
                        format_with_gutter("autoload -Uz compinit && compinit", "", None)
                    );
                }
            }
            ConfigAction::WouldAdd | ConfigAction::WouldCreate => {
                any_not_configured = true;
                println!(
                    "{HINT_EMOJI} {HINT}Not configured {what} for {bold}{shell}{bold:#} @ {path}{HINT:#}"
                );
            }
            _ => {} // Added/Created won't appear in dry_run mode
        }
    }

    // Show fish completions separately (fish has separate completion files)
    for result in &completion_results {
        let shell = result.shell;
        let path = format_path_for_display(&result.path);

        match result.action {
            ConfigAction::AlreadyExists => {
                any_configured = true;
                println!(
                    "{INFO_EMOJI} Already configured completions for {bold}{shell}{bold:#} @ {path}"
                );
            }
            ConfigAction::WouldAdd | ConfigAction::WouldCreate => {
                any_not_configured = true;
                println!(
                    "{HINT_EMOJI} {HINT}Not configured completions for {bold}{shell}{bold:#} @ {path}{HINT:#}"
                );
            }
            _ => {}
        }
    }

    // Show skipped (not installed) shells
    for (shell, path) in &scan_result.skipped {
        let path = format_path_for_display(path);
        println!("{dim}⚪ Skipped {shell}; {path} not found{dim:#}");
    }

    // Summary hint
    if any_not_configured && !any_configured {
        println!();
        println!(
            "{HINT_EMOJI} {HINT}Run 'wt config shell install' to enable shell integration{HINT:#}"
        );
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

fn get_global_config_path() -> Option<PathBuf> {
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

/// Handle the config refresh-cache command
pub fn handle_config_refresh_cache() -> anyhow::Result<()> {
    let repo = Repository::current();

    // Display progress message
    crate::output::progress(format!(
        "{CYAN}Querying remote for default branch...{CYAN:#}"
    ))?;

    // Refresh the cache (this will make a network call)
    let branch = repo.refresh_default_branch()?;

    // Display success message
    crate::output::success(format!(
        "{GREEN}Cache refreshed: {GREEN_BOLD}{branch}{GREEN_BOLD:#}{GREEN:#}"
    ))?;

    Ok(())
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
        None => repo
            .current_branch()?
            .ok_or_else(|| anyhow::anyhow!("{}", detached_head()))?,
    };

    let config_key = format!("worktrunk.status.{}", branch_name);
    repo.run_command(&["config", &config_key, &value])?;

    let branch_bold = GREEN.bold();
    crate::output::success(format!(
        "{GREEN}Set status for {branch_bold}{branch_name}{branch_bold:#}{GREEN} to {GREEN_BOLD}{value}{GREEN_BOLD:#}{GREEN:#}"
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
            crate::output::success(format!(
                "{GREEN}Cleared {GREEN_BOLD}{cleared_count}{GREEN_BOLD:#}{GREEN} status{}{GREEN:#}",
                if cleared_count == 1 { "" } else { "es" }
            ))?;
        }
    } else {
        // Clear specific branch status
        let branch_name = if target.is_empty() {
            repo.current_branch()?
                .ok_or_else(|| anyhow::anyhow!("{}", detached_head()))?
        } else {
            target
        };

        let config_key = format!("worktrunk.status.{}", branch_name);
        repo.run_command(&["config", "--unset", &config_key])
            .context("Failed to unset status (may not be set)")?;

        let branch_bold = GREEN.bold();
        crate::output::success(format!(
            "{GREEN}Cleared status for {branch_bold}{branch_name}{branch_bold:#}{GREEN:#}"
        ))?;
    }

    Ok(())
}
