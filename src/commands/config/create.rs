//! Config file creation.
//!
//! Commands for creating user and project configuration files.

use anyhow::Context;
use color_print::cformat;
use std::path::PathBuf;
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{hint_message, info_message, success_message};

use super::state::require_user_config_path;
use crate::output;

/// Example user configuration file content (displayed in help with values uncommented)
const USER_CONFIG_EXAMPLE: &str = include_str!("../../../dev/config.example.toml");

/// Example project configuration file content
const PROJECT_CONFIG_EXAMPLE: &str = include_str!("../../../dev/wt.example.toml");

/// Comment out all non-comment, non-empty lines for writing to disk
pub(super) fn comment_out_config(content: &str) -> String {
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
pub fn handle_config_create(project: bool) -> anyhow::Result<()> {
    if project {
        let repo = Repository::current()?;
        let config_path = repo.current_worktree().root()?.join(".config/wt.toml");
        let user_config_exists = require_user_config_path()
            .map(|p| p.exists())
            .unwrap_or(false);
        create_config_file(
            config_path,
            PROJECT_CONFIG_EXAMPLE,
            "Project config",
            &[
                "Edit this file to configure hooks for this repository",
                "See https://worktrunk.dev/hook/ for hook documentation",
            ],
            user_config_exists,
            true, // is_project
        )
    } else {
        let project_config_exists = Repository::current()
            .and_then(|repo| repo.current_worktree().root())
            .map(|root| root.join(".config/wt.toml").exists())
            .unwrap_or(false);
        create_config_file(
            require_user_config_path()?,
            USER_CONFIG_EXAMPLE,
            "User config",
            &["Edit this file to customize worktree paths and LLM settings"],
            project_config_exists,
            false, // is_project
        )
    }
}

/// Create a config file at the specified path with the given content
fn create_config_file(
    path: PathBuf,
    content: &str,
    config_type: &str,
    success_hints: &[&str],
    other_config_exists: bool,
    is_project: bool,
) -> anyhow::Result<()> {
    // Check if file already exists
    if path.exists() {
        output::print(info_message(cformat!(
            "{config_type} already exists: <bold>{}</>",
            format_path_for_display(&path)
        )))?;

        // Build hint message based on whether the other config exists
        let hint = if other_config_exists {
            // Both configs exist
            cformat!("To view both user and project configs, run <bright-black>wt config show</>")
        } else if is_project {
            // Project config exists, no user config
            cformat!(
                "To view, run <bright-black>wt config show</>. To create a user config, run <bright-black>wt config create</>"
            )
        } else {
            // User config exists, no project config
            cformat!(
                "To view, run <bright-black>wt config show</>. To create a project config, run <bright-black>wt config create --project</>"
            )
        };
        output::print(hint_message(hint))?;
        return Ok(());
    }

    // Create parent directory if it doesn't exist
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("Failed to create config directory")?;
    }

    // Write the example config with all values commented out
    let commented_config = comment_out_config(content);
    std::fs::write(&path, commented_config).context("Failed to write config file")?;

    // Success message
    output::print(success_message(cformat!(
        "Created {}: <bold>{}</>",
        config_type.to_lowercase(),
        format_path_for_display(&path)
    )))?;
    output::blank()?;
    for hint in success_hints {
        output::print(hint_message(*hint))?;
    }

    Ok(())
}
