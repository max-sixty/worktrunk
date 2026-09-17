//! Config mutation methods with file locking.
//!
//! These methods modify the UserConfig and write the changed value to disk,
//! using file locking to prevent race conditions between concurrent processes.

use fs2::FileExt;

use crate::config::ConfigError;

use crate::path::format_path_for_display;
use crate::styling::{eprint, eprintln, warning_message};
use color_print::cformat;

use super::UserConfig;
use super::persistence::{ConfigEdit, ConfigFile, Edited};
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
            .map_err(|e| ConfigError(format!("Failed to create config directory: {e}")))?;
    }

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| ConfigError(format!("Failed to open lock file: {e}")))?;

    file.lock_exclusive()
        .map_err(|e| ConfigError(format!("Failed to acquire config lock: {e}")))?;

    Ok(file)
}

impl UserConfig {
    /// Execute a mutation under an exclusive file lock.
    ///
    /// Acquires the lock and reads the config file. The mutator runs on the
    /// file's config, returning the value it changed or `None` when the file
    /// already has it, and that value alone is written into the file (see
    /// [`ConfigFile::edited`]). The mutator also runs on `self`, which is not
    /// replaced by the file's config: it carries system config, environment
    /// variables, and `--config-set` too, which the rest of the command still
    /// reads.
    pub(super) fn with_locked_mutation<'a, F>(
        &mut self,
        config_path: &std::path::Path,
        mutate: F,
    ) -> Result<(), ConfigError>
    where
        F: Fn(&mut Self) -> Option<ConfigEdit<'a>>,
    {
        let _lock = acquire_config_lock(config_path)?;
        let file = ConfigFile::read(config_path)?;
        let mut changed = file.config.clone();
        let edit = mutate(&mut changed);
        mutate(self);

        let Some(edit) = edit else {
            return Ok(());
        };
        let edited = file.edited(&edit, &changed)?;
        crate::config::ensure_config_parses(edited.content())?;
        crate::utils::write_atomically(config_path, edited.content()).map_err(|e| {
            ConfigError(format!(
                "Failed to write config file {}: {}",
                format_path_for_display(config_path),
                e
            ))
        })?;

        // Reported only once the file is written, and only for the fallback:
        // `wt config update` is otherwise what materializes migrations.
        if let Edited::Migrated { changes, .. } = &edited {
            eprintln!(
                "{}",
                warning_message(cformat!(
                    "Migrated deprecated settings @ <bold>{}</> — the setting being written can't be read beside them",
                    format_path_for_display(config_path)
                ))
            );
            eprint!(
                "{}",
                crate::config::deprecation::format_applied_lines(changes)
            );
        }
        Ok(())
    }

    /// Set `skip-shell-integration-prompt = true` in the config file.
    ///
    /// Under the lock, writes the flag unless the file already has it.
    pub fn set_skip_shell_integration_prompt(
        &mut self,
        config_path: &std::path::Path,
    ) -> Result<(), ConfigError> {
        self.with_locked_mutation(config_path, |config| {
            if config.skip_shell_integration_prompt {
                return None;
            }
            config.skip_shell_integration_prompt = true;
            Some(ConfigEdit {
                tables: vec![],
                key: "skip-shell-integration-prompt",
                value: true.into(),
            })
        })
    }

    /// Set `skip-commit-generation-prompt = true` in the config file.
    ///
    /// Under the lock, writes the flag unless the file already has it.
    pub fn set_skip_commit_generation_prompt(
        &mut self,
        config_path: &std::path::Path,
    ) -> Result<(), ConfigError> {
        self.with_locked_mutation(config_path, |config| {
            if config.skip_commit_generation_prompt {
                return None;
            }
            config.skip_commit_generation_prompt = true;
            Some(ConfigEdit {
                tables: vec![],
                key: "skip-commit-generation-prompt",
                value: true.into(),
            })
        })
    }

    /// Set `worktree-path` for a specific project in the config file.
    ///
    /// Under the lock, writes the path unless the file already has it, creating
    /// the project entry if it doesn't exist.
    pub fn set_project_worktree_path(
        &mut self,
        project: &str,
        worktree_path: String,
        config_path: &std::path::Path,
    ) -> Result<(), ConfigError> {
        self.with_locked_mutation(config_path, |config| {
            let entry = config.projects.entry(project.to_string()).or_default();
            if entry.worktree_path.as_ref() == Some(&worktree_path) {
                return None;
            }
            entry.worktree_path = Some(worktree_path.clone());
            Some(ConfigEdit {
                tables: vec!["projects", project],
                key: "worktree-path",
                value: worktree_path.as_str().into(),
            })
        })
    }

    /// Set `[commit.generation] command` in the config file.
    ///
    /// Under the lock, writes the command unless the file already has it.
    pub fn set_commit_generation_command(
        &mut self,
        command: String,
        config_path: &std::path::Path,
    ) -> Result<(), ConfigError> {
        self.with_locked_mutation(config_path, |config| {
            let gen_config = config
                .commit
                .generation
                .get_or_insert_with(CommitGenerationConfig::default);

            if gen_config.command.as_ref() == Some(&command) {
                return None;
            }
            gen_config.command = Some(command.clone());
            Some(ConfigEdit {
                tables: vec!["commit", "generation"],
                key: "command",
                value: command.as_str().into(),
            })
        })
    }
}
