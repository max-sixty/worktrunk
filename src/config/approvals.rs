//! Approval state management.
//!
//! Approved commands are stored in `~/.config/worktrunk/approvals.toml`, separate
//! from portable user configuration in `config.toml`. This allows dotfile management
//! of config.toml without machine-local trust state.
//!
//! File format:
//! ```toml
//! [projects."github.com/user/repo"]
//! approved-commands = [
//!     "npm install",
//!     "npm test",
//! ]
//! ```
//!
//! **Fallback**: When `approvals.toml` doesn't exist, `approved-commands` are
//! silently read from `config.toml` for backward compatibility. Once any approval
//! is saved (creating `approvals.toml`), it becomes the authoritative source.
//! Users can then remove stale `approved-commands` from `config.toml` at their
//! convenience.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use config::ConfigError;
use serde::{Deserialize, Serialize};

use crate::config::deprecation::normalize_template_vars;
use crate::path::format_path_for_display;

/// Approved commands, stored in `approvals.toml`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Approvals {
    #[serde(default)]
    projects: BTreeMap<String, ApprovedProject>,
}

/// Per-project approved commands.
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
struct ApprovedProject {
    #[serde(
        default,
        rename = "approved-commands",
        skip_serializing_if = "Vec::is_empty"
    )]
    approved_commands: Vec<String>,
}

// =========================================================================
// Path resolution
// =========================================================================

/// Get the approvals file path.
///
/// Priority:
/// 1. `WORKTRUNK_APPROVALS_PATH` environment variable
/// 2. In test builds: panic if env var not set
/// 3. Production: sibling of config.toml (`approvals.toml` in same directory)
pub fn get_approvals_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("WORKTRUNK_APPROVALS_PATH") {
        return Some(PathBuf::from(path));
    }

    #[cfg(test)]
    panic!(
        "WORKTRUNK_APPROVALS_PATH not set in test. Tests must use TestRepo which sets this \
         automatically, or set it manually to an isolated test approvals path."
    );

    #[cfg(not(test))]
    {
        super::user::get_config_path().map(|p| p.with_file_name("approvals.toml"))
    }
}

// =========================================================================
// Loading
// =========================================================================

impl Approvals {
    /// Load approvals from `approvals.toml`, with silent fallback to `config.toml`.
    ///
    /// 1. If `approvals.toml` exists → load from it (authoritative)
    /// 2. If not → silently read `approved-commands` from `config.toml`
    /// 3. If none found → return empty
    ///
    /// The fallback is silent (no file writes, no warnings). Once any approval is
    /// saved via mutation methods, `approvals.toml` is created and becomes the
    /// source of truth. Users can clean up stale `approved-commands` in config.toml
    /// at their convenience.
    pub fn load() -> Result<Self, ConfigError> {
        let Some(path) = get_approvals_path() else {
            return Ok(Self::default());
        };

        Self::load_with_fallback(&path)
    }

    /// Load approvals from a specific file path.
    fn load_from_file(path: &Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            ConfigError::Message(format!(
                "Failed to read approvals file {}: {}",
                format_path_for_display(path),
                e
            ))
        })?;
        let approvals: Self = toml::from_str(&content).map_err(|e| {
            ConfigError::Message(format!(
                "Failed to parse approvals file {}: {}",
                format_path_for_display(path),
                e
            ))
        })?;
        Ok(approvals)
    }

    /// Load approvals from a specific path (no fallback, for testing).
    #[cfg(test)]
    pub fn load_from_path(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Ok(Self::default());
        }
        Self::load_from_file(path)
    }

    /// Load approvals from an approvals file, falling back to config.toml.
    ///
    /// 1. If the approvals file exists → load from it (authoritative)
    /// 2. If not → read `approved-commands` from sibling `config.toml`
    /// 3. If neither exists → return empty
    ///
    /// The fallback uses sibling derivation (`path.with_file_name("config.toml")`)
    /// which is correct because `get_approvals_path()` derives approvals.toml as
    /// a sibling of config.toml.
    fn load_with_fallback(path: &Path) -> Result<Self, ConfigError> {
        if path.exists() {
            return Self::load_from_file(path);
        }

        let config_path = path.with_file_name("config.toml");
        if config_path.exists() {
            return Self::load_from_config_file(&config_path);
        }

        Ok(Self::default())
    }
}

// =========================================================================
// Saving
// =========================================================================

impl Approvals {
    /// Save approvals to a specific file path.
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ConfigError::Message(format!("Failed to create approvals directory: {e}"))
            })?;
        }

        let mut doc = toml_edit::DocumentMut::new();

        if !self.projects.is_empty() {
            let mut projects_table = toml_edit::Table::new();
            projects_table.set_implicit(true);

            for (project_id, project_config) in &self.projects {
                if project_config.approved_commands.is_empty() {
                    continue;
                }
                let mut project_table = toml_edit::Table::new();
                let commands = format_multiline_array(project_config.approved_commands.iter());
                project_table["approved-commands"] = toml_edit::value(commands);
                projects_table[project_id] = toml_edit::Item::Table(project_table);
            }

            doc["projects"] = toml_edit::Item::Table(projects_table);
        }

        let output = doc.to_string();
        // If all projects were empty, write empty file
        let output = if output.trim().is_empty() {
            String::new()
        } else {
            output
        };

        std::fs::write(path, output)
            .map_err(|e| ConfigError::Message(format!("Failed to write approvals file: {e}")))?;

        Ok(())
    }
}

// =========================================================================
// Queries
// =========================================================================

impl Approvals {
    /// Check if a command is approved for the given project.
    ///
    /// Normalizes template variables before comparing, so approvals match
    /// regardless of whether they were saved with deprecated variable names.
    pub fn is_command_approved(&self, project: &str, command: &str) -> bool {
        let normalized_command = normalize_template_vars(command);
        self.projects
            .get(project)
            .map(|p| {
                p.approved_commands
                    .iter()
                    .any(|c| normalize_template_vars(c) == normalized_command)
            })
            .unwrap_or(false)
    }

    /// Iterate over projects and their approved commands.
    pub fn projects(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.projects
            .iter()
            .map(|(id, p)| (id.as_str(), p.approved_commands.as_slice()))
    }
}

// =========================================================================
// Mutations (with file locking)
// =========================================================================

impl Approvals {
    /// Execute a mutation under an exclusive file lock.
    ///
    /// Acquires lock, reloads from disk, calls the mutator, and saves if mutator returns true.
    fn with_locked_mutation<F>(
        &mut self,
        approvals_path: Option<&Path>,
        mutate: F,
    ) -> Result<(), ConfigError>
    where
        F: FnOnce(&mut Self) -> bool,
    {
        let path = match approvals_path {
            Some(p) => p.to_path_buf(),
            None => get_approvals_path().ok_or_else(|| {
                ConfigError::Message(
                    "Cannot determine approvals path. Set $HOME or $XDG_CONFIG_HOME".to_string(),
                )
            })?,
        };
        let _lock = super::user::mutation::acquire_config_lock(&path)?;
        self.reload_from(&path)?;

        if mutate(self) {
            self.save_to(&path)?;
        }
        Ok(())
    }

    /// Reload approvals from disk (under lock), with config.toml fallback.
    fn reload_from(&mut self, path: &Path) -> Result<(), ConfigError> {
        let fresh = Self::load_with_fallback(path)?;
        self.projects = fresh.projects;
        Ok(())
    }

    /// Extract approved-commands from a specific config file.
    fn load_from_config_file(config_path: &Path) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(config_path).map_err(|e| {
            ConfigError::Message(format!(
                "Failed to read config file {}: {}",
                format_path_for_display(config_path),
                e
            ))
        })?;

        let config: super::UserConfig = toml::from_str(&content).map_err(|e| {
            ConfigError::Message(format!(
                "Failed to parse config file {}: {}",
                format_path_for_display(config_path),
                e
            ))
        })?;

        let mut approvals = Approvals::default();
        for (project_id, project_config) in &config.projects {
            if !project_config.approved_commands.is_empty() {
                approvals.projects.insert(
                    project_id.clone(),
                    ApprovedProject {
                        approved_commands: project_config.approved_commands.clone(),
                    },
                );
            }
        }

        Ok(approvals)
    }

    /// Add an approved command and save.
    pub fn approve_command(
        &mut self,
        project: String,
        command: String,
        approvals_path: Option<&Path>,
    ) -> Result<(), ConfigError> {
        self.with_locked_mutation(approvals_path, |approvals| {
            if approvals.is_command_approved(&project, &command) {
                return false;
            }
            approvals
                .projects
                .entry(project)
                .or_default()
                .approved_commands
                .push(command);
            true
        })
    }

    /// Add multiple approved commands in a single locked operation.
    pub fn approve_commands(
        &mut self,
        project: String,
        commands: Vec<String>,
        approvals_path: Option<&Path>,
    ) -> Result<(), ConfigError> {
        self.with_locked_mutation(approvals_path, |approvals| {
            let entry = approvals.projects.entry(project).or_default();
            let mut changed = false;
            for command in commands {
                let normalized = normalize_template_vars(&command);
                if !entry
                    .approved_commands
                    .iter()
                    .any(|c| normalize_template_vars(c) == normalized)
                {
                    entry.approved_commands.push(command);
                    changed = true;
                }
            }
            changed
        })
    }

    /// Remove all approvals for a project and save.
    pub fn revoke_project(
        &mut self,
        project: &str,
        approvals_path: Option<&Path>,
    ) -> Result<(), ConfigError> {
        let project = project.to_string();
        self.with_locked_mutation(approvals_path, |approvals| {
            let Some(project_config) = approvals.projects.get_mut(&project) else {
                return false;
            };
            if project_config.approved_commands.is_empty() {
                return false;
            }
            approvals.projects.remove(&project);
            true
        })
    }

    /// Clear all approvals for all projects and save.
    pub fn clear_all(&mut self, approvals_path: Option<&Path>) -> Result<(), ConfigError> {
        self.with_locked_mutation(approvals_path, |approvals| {
            if approvals.projects.is_empty() {
                return false;
            }
            approvals.projects.clear();
            true
        })
    }
}

// =========================================================================
// TOML formatting helpers
// =========================================================================

/// Format a string array as multiline TOML for readability.
fn format_multiline_array<'a>(items: impl Iterator<Item = &'a String>) -> toml_edit::Array {
    let mut array: toml_edit::Array = items.collect();
    for item in array.iter_mut() {
        item.decor_mut().set_prefix("\n    ");
    }
    array.set_trailing("\n");
    array.set_trailing_comma(true);
    array
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_dir() -> (TempDir, PathBuf) {
        let temp_dir = TempDir::new().unwrap();
        let approvals_path = temp_dir.path().join("approvals.toml");
        (temp_dir, approvals_path)
    }

    #[test]
    fn test_empty_approvals() {
        let approvals = Approvals::default();
        assert!(!approvals.is_command_approved("any/project", "any command"));
    }

    #[test]
    fn test_approve_and_check() {
        let (_temp_dir, path) = test_dir();

        let mut approvals = Approvals::default();
        approvals
            .approve_command(
                "github.com/user/repo".to_string(),
                "npm install".to_string(),
                Some(&path),
            )
            .unwrap();

        assert!(approvals.is_command_approved("github.com/user/repo", "npm install"));
        assert!(!approvals.is_command_approved("github.com/user/repo", "npm test"));
        assert!(!approvals.is_command_approved("github.com/other/repo", "npm install"));
    }

    #[test]
    fn test_approve_duplicate_is_noop() {
        let (_temp_dir, path) = test_dir();

        let mut approvals = Approvals::default();
        approvals
            .approve_command(
                "github.com/user/repo".to_string(),
                "npm install".to_string(),
                Some(&path),
            )
            .unwrap();
        approvals
            .approve_command(
                "github.com/user/repo".to_string(),
                "npm install".to_string(),
                Some(&path),
            )
            .unwrap();

        let count = approvals
            .projects
            .get("github.com/user/repo")
            .unwrap()
            .approved_commands
            .len();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_approve_commands_batch() {
        let (_temp_dir, path) = test_dir();

        let mut approvals = Approvals::default();
        approvals
            .approve_commands(
                "github.com/user/repo".to_string(),
                vec!["npm install".to_string(), "npm test".to_string()],
                Some(&path),
            )
            .unwrap();

        assert!(approvals.is_command_approved("github.com/user/repo", "npm install"));
        assert!(approvals.is_command_approved("github.com/user/repo", "npm test"));
    }

    #[test]
    fn test_revoke_project() {
        let (_temp_dir, path) = test_dir();

        let mut approvals = Approvals::default();
        approvals
            .approve_commands(
                "github.com/user/repo1".to_string(),
                vec!["npm install".to_string(), "npm test".to_string()],
                Some(&path),
            )
            .unwrap();
        approvals
            .approve_command(
                "github.com/user/repo2".to_string(),
                "cargo build".to_string(),
                Some(&path),
            )
            .unwrap();

        approvals
            .revoke_project("github.com/user/repo1", Some(&path))
            .unwrap();
        assert!(!approvals.projects.contains_key("github.com/user/repo1"));
        assert!(approvals.projects.contains_key("github.com/user/repo2"));
    }

    #[test]
    fn test_clear_all() {
        let (_temp_dir, path) = test_dir();

        let mut approvals = Approvals::default();
        approvals
            .approve_command(
                "github.com/user/repo1".to_string(),
                "npm install".to_string(),
                Some(&path),
            )
            .unwrap();
        approvals
            .approve_command(
                "github.com/user/repo2".to_string(),
                "cargo build".to_string(),
                Some(&path),
            )
            .unwrap();

        approvals.clear_all(Some(&path)).unwrap();
        assert!(approvals.projects.is_empty());
    }

    #[test]
    fn test_save_and_load() {
        let (_temp_dir, path) = test_dir();

        let mut approvals = Approvals::default();
        approvals
            .approve_commands(
                "github.com/user/repo".to_string(),
                vec!["npm install".to_string(), "npm test".to_string()],
                Some(&path),
            )
            .unwrap();

        // Load from disk
        let loaded = Approvals::load_from_path(&path).unwrap();
        assert!(loaded.is_command_approved("github.com/user/repo", "npm install"));
        assert!(loaded.is_command_approved("github.com/user/repo", "npm test"));
    }

    #[test]
    fn test_save_format() {
        let (_temp_dir, path) = test_dir();

        let mut approvals = Approvals::default();
        approvals
            .approve_commands(
                "github.com/user/repo".to_string(),
                vec!["npm install".to_string(), "npm test".to_string()],
                Some(&path),
            )
            .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        insta::assert_snapshot!(content, @r#"
        [projects."github.com/user/repo"]
        approved-commands = [
            "npm install",
            "npm test",
        ]
        "#);
    }

    #[test]
    fn test_normalized_approval_matching() {
        let (_temp_dir, path) = test_dir();

        let mut approvals = Approvals::default();
        // Approve with deprecated variable name
        approvals
            .approve_command(
                "project".to_string(),
                "echo {{ repo_root }}".to_string(),
                Some(&path),
            )
            .unwrap();

        // Should match with canonical variable name
        assert!(approvals.is_command_approved("project", "echo {{ repo_path }}"));
    }

    #[test]
    fn test_concurrent_approve_preserves_all() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let (_temp_dir, path) = test_dir();

        let num_threads = 10;
        let barrier = Arc::new(Barrier::new(num_threads));
        let config_path = Arc::new(path);

        let handles: Vec<_> = (0..num_threads)
            .map(|i| {
                let barrier = Arc::clone(&barrier);
                let config_path = Arc::clone(&config_path);
                thread::spawn(move || {
                    let mut approvals = Approvals::default();
                    barrier.wait();
                    approvals
                        .approve_command(
                            "github.com/user/repo".to_string(),
                            format!("command_{i}"),
                            Some(&config_path),
                        )
                        .unwrap();
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        let content = std::fs::read_to_string(&*config_path).unwrap();
        for i in 0..num_threads {
            assert!(
                content.contains(&format!("command_{i}")),
                "command_{i} should be preserved"
            );
        }
    }

    /// `load_from_config_file` extracts approved-commands from a config.toml file.
    #[test]
    fn test_load_from_config_file() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        std::fs::write(
            &config_path,
            r#"
[projects."github.com/user/repo"]
approved-commands = ["npm install", "npm test"]

[projects."github.com/other/repo"]
approved-commands = ["cargo build"]
"#,
        )
        .unwrap();

        let approvals = Approvals::load_from_config_file(&config_path).unwrap();
        assert!(approvals.is_command_approved("github.com/user/repo", "npm install"));
        assert!(approvals.is_command_approved("github.com/user/repo", "npm test"));
        assert!(approvals.is_command_approved("github.com/other/repo", "cargo build"));
    }

    /// When `approvals.toml` doesn't exist but `config.toml` is a sibling with
    /// `approved-commands`, the first mutation reads the fallback and preserves
    /// existing commands alongside the new one.
    #[test]
    fn test_mutation_picks_up_config_toml_fallback() {
        let temp_dir = TempDir::new().unwrap();
        let approvals_path = temp_dir.path().join("approvals.toml");
        let config_path = temp_dir.path().join("config.toml");

        // Write config.toml with existing approved-commands (no approvals.toml yet)
        std::fs::write(
            &config_path,
            r#"
[projects."github.com/user/repo"]
approved-commands = ["npm install"]
"#,
        )
        .unwrap();

        // Approve a new command — reload_from should pick up config.toml fallback
        let mut approvals = Approvals::default();
        approvals
            .approve_command(
                "github.com/user/repo".to_string(),
                "npm test".to_string(),
                Some(&approvals_path),
            )
            .unwrap();

        // Both the fallback command and the new one should be present
        assert!(approvals.is_command_approved("github.com/user/repo", "npm install"));
        assert!(approvals.is_command_approved("github.com/user/repo", "npm test"));

        // approvals.toml should now exist with both commands
        let content = std::fs::read_to_string(&approvals_path).unwrap();
        assert!(content.contains("npm install"));
        assert!(content.contains("npm test"));
    }

    #[test]
    fn test_load_from_path_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("nonexistent.toml");
        let approvals = Approvals::load_from_path(&path).unwrap();
        assert!(approvals.projects.is_empty());
    }

    #[test]
    fn test_load_from_file_invalid_toml() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("approvals.toml");
        std::fs::write(&path, "this is { not valid toml").unwrap();
        let err = Approvals::load_from_file(&path).unwrap_err();
        assert!(
            err.to_string().contains("Failed to parse approvals file"),
            "Expected parse error, got: {}",
            err
        );
    }

    #[test]
    fn test_load_from_config_file_invalid_toml() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");
        std::fs::write(&config_path, "not { valid toml here").unwrap();
        let err = Approvals::load_from_config_file(&config_path).unwrap_err();
        assert!(
            err.to_string().contains("Failed to parse config file"),
            "Expected parse error, got: {}",
            err
        );
    }

    #[test]
    fn test_revoke_project_nonexistent() {
        let (_temp_dir, path) = test_dir();
        let mut approvals = Approvals::default();
        approvals
            .approve_command("project-a".to_string(), "cmd1".to_string(), Some(&path))
            .unwrap();
        // Revoke a project that doesn't exist — should be a no-op
        approvals
            .revoke_project("nonexistent", Some(&path))
            .unwrap();
        assert!(approvals.is_command_approved("project-a", "cmd1"));
    }

    #[test]
    fn test_clear_all_when_empty() {
        let (_temp_dir, path) = test_dir();
        let mut approvals = Approvals::default();
        // Clear when there's nothing — should be a no-op
        approvals.clear_all(Some(&path)).unwrap();
        assert!(approvals.projects.is_empty());
    }

    #[test]
    fn test_save_skips_empty_project() {
        let (_temp_dir, path) = test_dir();
        let mut approvals = Approvals::default();
        // Manually insert a project with empty commands
        approvals.projects.insert(
            "empty-project".to_string(),
            super::ApprovedProject {
                approved_commands: vec![],
            },
        );
        approvals.projects.insert(
            "real-project".to_string(),
            super::ApprovedProject {
                approved_commands: vec!["cmd1".to_string()],
            },
        );
        // Call save_to directly so the empty project reaches the save logic
        approvals.save_to(&path).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(!content.contains("empty-project"));
        assert!(content.contains("real-project"));
    }

    #[test]
    fn test_revoke_project_with_empty_commands() {
        let (_temp_dir, path) = test_dir();
        let mut approvals = Approvals::default();
        approvals
            .approve_command("project-a".to_string(), "cmd1".to_string(), Some(&path))
            .unwrap();
        // Manually clear the commands (without removing the project entry)
        approvals
            .projects
            .get_mut("project-a")
            .unwrap()
            .approved_commands
            .clear();
        // Write this state to disk
        approvals.save_to(&path).unwrap();
        // Now revoke_project should find the project but see empty commands → no-op
        approvals.revoke_project("project-a", Some(&path)).unwrap();
    }

    #[test]
    fn test_projects_accessor() {
        let (_temp_dir, path) = test_dir();

        let mut approvals = Approvals::default();
        approvals
            .approve_command("project1".to_string(), "cmd1".to_string(), Some(&path))
            .unwrap();
        approvals
            .approve_command("project2".to_string(), "cmd2".to_string(), Some(&path))
            .unwrap();

        let projects: Vec<_> = approvals.projects().collect();
        assert_eq!(projects.len(), 2);
    }
}
