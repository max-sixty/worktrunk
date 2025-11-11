use etcetera::base_strategy::{BaseStrategy, choose_base_strategy};
use std::path::PathBuf;
use worktrunk::git::{GitError, GitResultExt, Repository};
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
            config_path.display()
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
        config_path.display()
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
        config_path.display()
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
        config_path.display()
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
