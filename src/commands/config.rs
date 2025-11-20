use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};
use std::path::PathBuf;
use worktrunk::git::{GitError, GitResultExt, Repository};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    AnstyleStyle, CYAN, GREEN, GREEN_BOLD, HINT, HINT_EMOJI, INFO_EMOJI, SUCCESS_EMOJI,
    format_toml, print, println,
};

/// Example configuration file content
const CONFIG_EXAMPLE: &str = include_str!("../../config.example.toml");

/// Handle the config init command
pub fn handle_config_init() -> Result<(), GitError> {
    let config_path = get_global_config_path().ok_or_else(|| {
        GitError::CommandFailed("Could not determine global config path".to_string())
    })?;

    // Check if file already exists
    if config_path.exists() {
        let bold = AnstyleStyle::new().bold();
        println!(
            "{INFO_EMOJI} Global config already exists: {bold}{}{bold:#}",
            format_path_for_display(&config_path)
        );
        println!();
        println!("{HINT_EMOJI} {HINT}Use 'wt config list' to view existing configuration{HINT:#}");
        return Ok(());
    }

    // Create parent directory if it doesn't exist
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            GitError::CommandFailed(format!("Failed to create config directory: {}", e))
        })?;
    }

    // Write the example config
    std::fs::write(&config_path, CONFIG_EXAMPLE).git_context("Failed to write config file")?;

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

/// Handle the config list command
pub fn handle_config_list() -> Result<(), GitError> {
    // Display global config
    display_global_config()?;
    println!();

    // Display project config if in a git repository
    display_project_config()?;

    Ok(())
}

fn display_global_config() -> Result<(), GitError> {
    let bold = AnstyleStyle::new().bold();

    // Get config path
    let config_path = get_global_config_path().ok_or_else(|| {
        GitError::CommandFailed("Could not determine global config path".to_string())
    })?;

    println!(
        "{INFO_EMOJI} Global Config: {bold}{}{bold:#}",
        format_path_for_display(&config_path)
    );

    // Check if file exists
    if !config_path.exists() {
        println!("{HINT_EMOJI} {HINT}Not found (using defaults){HINT:#}");
        println!("{HINT_EMOJI} {HINT}Run 'wt config init' to create a config file{HINT:#}");
        println!();
        let default_config =
            "# Default configuration:\nworktree-path = \"../{{ main_worktree }}.{{ branch }}\"";
        print!("{}", format_toml(default_config, ""));
        return Ok(());
    }

    // Read and display the file contents
    let contents =
        std::fs::read_to_string(&config_path).git_context("Failed to read config file")?;

    if contents.trim().is_empty() {
        println!("{HINT_EMOJI} {HINT}Empty file (using defaults){HINT:#}");
        return Ok(());
    }

    // Display TOML with syntax highlighting (gutter at column 0)
    print!("{}", format_toml(&contents, ""));

    Ok(())
}

fn display_project_config() -> Result<(), GitError> {
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
    let contents =
        std::fs::read_to_string(&config_path).git_context("Failed to read config file")?;

    if contents.trim().is_empty() {
        println!("{HINT_EMOJI} {HINT}Empty file{HINT:#}");
        return Ok(());
    }

    // Display TOML with syntax highlighting (gutter at column 0)
    print!("{}", format_toml(&contents, ""));

    Ok(())
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
pub fn handle_config_refresh_cache() -> Result<(), GitError> {
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
pub fn handle_config_status_set(value: String, branch: Option<String>) -> Result<(), GitError> {
    let repo = Repository::current();

    // TODO: Worktree-specific status (worktrunk.status with --worktree flag) would allow
    // different statuses per worktree, but requires extensions.worktreeConfig which adds
    // complexity. Our intended workflow is one branch per worktree, so branch-keyed status
    // is sufficient for now.

    let branch_name = match branch {
        Some(b) => b,
        None => repo.current_branch()?.ok_or_else(|| {
            GitError::CommandFailed("Not on a branch (detached HEAD)".to_string())
        })?,
    };

    let config_key = format!("worktrunk.status.{}", branch_name);
    repo.run_command(&["config", &config_key, &value])?;

    let branch_bold = GREEN.bold();
    crate::output::success(format!(
        "{GREEN}Set status for {branch_bold}{branch_name}{branch_bold:#} to {GREEN_BOLD}{value}{GREEN_BOLD:#}{GREEN:#}"
    ))?;

    Ok(())
}

/// Handle the config status unset command
pub fn handle_config_status_unset(target: String) -> Result<(), GitError> {
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
                "{GREEN}Cleared {GREEN_BOLD}{cleared_count}{GREEN_BOLD:#} status{}{GREEN:#}",
                if cleared_count == 1 { "" } else { "es" }
            ))?;
        }
    } else {
        // Clear specific branch status
        let branch_name = if target.is_empty() {
            repo.current_branch()?.ok_or_else(|| {
                GitError::CommandFailed("Not on a branch (detached HEAD)".to_string())
            })?
        } else {
            target
        };

        let config_key = format!("worktrunk.status.{}", branch_name);
        repo.run_command(&["config", "--unset", &config_key])
            .git_context("Failed to unset status (may not be set)")?;

        let branch_bold = GREEN.bold();
        crate::output::success(format!(
            "{GREEN}Cleared status for {branch_bold}{branch_name}{branch_bold:#}{GREEN:#}"
        ))?;
    }

    Ok(())
}
