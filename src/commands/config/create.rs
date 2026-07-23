//! Config file creation.
//!
//! Commands for creating user and project configuration files.

use anyhow::Context;
use color_print::cformat;
use std::path::PathBuf;
use worktrunk::config::{ConfigFileKind, require_config_path};
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{eprintln, hint_message, info_message, success_message};

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
        let config_path = repo.project_config_path()?.ok_or_else(|| {
            if repo.is_bare().unwrap_or(false) {
                anyhow::anyhow!(
                    "Bare repository has no linked worktrees yet.\n\
                     Run `wt switch <branch>` to create a worktree first, then run `wt config create --project` from inside it."
                )
            } else {
                anyhow::anyhow!("Cannot determine project config location — no worktree found")
            }
        })?;
        let user_config_exists = require_config_path().map(|p| p.exists()).unwrap_or(false);
        create_config_file(
            config_path,
            PROJECT_CONFIG_EXAMPLE,
            ConfigFileKind::Project,
            &[
                "Edit this file to configure hooks for this repository",
                "See https://worktrunk.dev/hook/ for hook documentation",
            ],
            user_config_exists,
        )
    } else {
        let project_config_exists = Repository::current()
            .and_then(|repo| repo.project_config_path())
            .ok()
            .and_then(|opt| opt)
            .map(|path| path.exists())
            .unwrap_or(false);
        create_config_file(
            require_config_path()?,
            USER_CONFIG_EXAMPLE,
            ConfigFileKind::User,
            &["Edit this file to customize worktree paths and LLM settings"],
            project_config_exists,
        )
    }
}

/// Create a config file at the specified path with the given content
fn create_config_file(
    path: PathBuf,
    content: &str,
    kind: ConfigFileKind,
    success_hints: &[&str],
    other_config_exists: bool,
) -> anyhow::Result<()> {
    // Check if file already exists
    if path.exists() {
        eprintln!(
            "{}",
            info_message(cformat!(
                "{} already exists: <bold>{}</>",
                kind.label(),
                format_path_for_display(&path)
            ))
        );

        // Build hint message based on whether the other config exists
        let hint = if other_config_exists {
            // Both configs exist
            cformat!("To view both user and project configs, run <underline>wt config show</>")
        } else if kind == ConfigFileKind::Project {
            // Project config exists, no user config
            cformat!(
                "To view, run <underline>wt config show</>. To create a user config, run <underline>wt config create</>"
            )
        } else {
            // User config exists, no project config
            cformat!(
                "To view, run <underline>wt config show</>. To create a project config, run <underline>wt config create --project</>"
            )
        };
        eprintln!("{}", hint_message(hint));
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
    eprintln!(
        "{}",
        success_message(cformat!(
            "Created {}: <bold>{}</>",
            kind.label().to_lowercase(),
            format_path_for_display(&path)
        ))
    );
    for hint in success_hints {
        eprintln!("{}", hint_message(*hint));
    }

    Ok(())
}
