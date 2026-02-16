//! Config mutation methods with file locking.
//!
//! These methods modify the UserConfig and persist changes to disk,
//! using file locking to prevent race conditions between concurrent processes.

use config::ConfigError;
use fs2::FileExt;

use crate::path::format_path_for_display;

use super::UserConfig;
use super::path::get_config_path;
use super::sections::CommitConfig;
use super::sections::CommitGenerationConfig;

/// Acquire an exclusive lock on the config file for read-modify-write operations.
///
/// Uses a `.lock` file alongside the config file to coordinate between processes.
/// The lock is released when the returned guard is dropped.
pub(crate) fn acquire_config_lock(
    config_path: &std::path::Path,
) -> Result<std::fs::File, ConfigError> {
    let lock_path = config_path.with_extension("toml.lock");

    // Create parent directory if needed
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ConfigError::Message(format!("Failed to create config directory: {e}")))?;
    }

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| ConfigError::Message(format!("Failed to open lock file: {e}")))?;

    file.lock_exclusive()
        .map_err(|e| ConfigError::Message(format!("Failed to acquire config lock: {e}")))?;

    Ok(file)
}

impl UserConfig {
    /// Execute a mutation under an exclusive file lock.
    ///
    /// Acquires lock, reloads from disk, calls the mutator, and saves if mutator returns true.
    pub(super) fn with_locked_mutation<F>(
        &mut self,
        config_path: Option<&std::path::Path>,
        mutate: F,
    ) -> Result<(), ConfigError>
    where
        F: FnOnce(&mut Self) -> bool,
    {
        let path = match config_path {
            Some(p) => p.to_path_buf(),
            None => get_config_path().ok_or_else(|| {
                ConfigError::Message(
                    "Cannot determine config directory. Set $HOME or $XDG_CONFIG_HOME".to_string(),
                )
            })?,
        };
        let _lock = acquire_config_lock(&path)?;
        self.reload_projects_from(config_path)?;

        if mutate(self) {
            self.save_impl(config_path)?;
        }
        Ok(())
    }

    /// Reload only the projects section from disk, preserving other in-memory state
    ///
    /// This replaces the in-memory projects with the authoritative disk state,
    /// while keeping other config values (worktree-path, commit-generation, etc.).
    /// Callers should reload before modifying and saving to avoid race conditions.
    fn reload_projects_from(
        &mut self,
        config_path: Option<&std::path::Path>,
    ) -> Result<(), ConfigError> {
        let path = match config_path {
            Some(p) => Some(p.to_path_buf()),
            None => get_config_path(),
        };

        let Some(path) = path else {
            return Ok(()); // No config file to reload from
        };

        if !path.exists() {
            return Ok(()); // Nothing to reload
        }

        let content = std::fs::read_to_string(&path).map_err(|e| {
            ConfigError::Message(format!(
                "Failed to read config file {}: {}",
                format_path_for_display(&path),
                e
            ))
        })?;

        let disk_config: UserConfig = toml::from_str(&content).map_err(|e| {
            ConfigError::Message(format!(
                "Failed to parse config file {}: {}",
                format_path_for_display(&path),
                e
            ))
        })?;

        // Replace in-memory projects with disk state (disk is authoritative)
        self.projects = disk_config.projects;

        Ok(())
    }

    /// Set `skip-shell-integration-prompt = true` and save.
    ///
    /// Acquires lock, reloads from disk, sets flag if not already set, and saves.
    /// Pass `None` for default config path, or `Some(path)` for testing.
    pub fn set_skip_shell_integration_prompt(
        &mut self,
        config_path: Option<&std::path::Path>,
    ) -> Result<(), ConfigError> {
        self.with_locked_mutation(config_path, |config| {
            if config.skip_shell_integration_prompt {
                return false;
            }
            config.skip_shell_integration_prompt = true;
            true
        })
    }

    /// Set `skip-commit-generation-prompt = true` and save.
    ///
    /// Acquires lock, reloads from disk, sets flag if not already set, and saves.
    /// Pass `None` for default config path, or `Some(path)` for testing.
    pub fn set_skip_commit_generation_prompt(
        &mut self,
        config_path: Option<&std::path::Path>,
    ) -> Result<(), ConfigError> {
        self.with_locked_mutation(config_path, |config| {
            if config.skip_commit_generation_prompt {
                return false;
            }
            config.skip_commit_generation_prompt = true;
            true
        })
    }

    /// Set commit generation command and save.
    ///
    /// Sets `[commit.generation] command = ...` in the user config.
    /// Acquires lock, reloads from disk, sets the command, and saves.
    /// Pass `None` for default config path, or `Some(path)` for testing.
    pub fn set_commit_generation_command(
        &mut self,
        command: String,
        config_path: Option<&std::path::Path>,
    ) -> Result<(), ConfigError> {
        self.with_locked_mutation(config_path, |config| {
            // Ensure commit config exists
            let commit_config = config
                .configs
                .commit
                .get_or_insert_with(CommitConfig::default);
            let gen_config = commit_config
                .generation
                .get_or_insert_with(CommitGenerationConfig::default);

            // Set the command
            gen_config.command = Some(command.clone());
            true
        })
    }
}
