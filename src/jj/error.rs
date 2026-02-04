//! Jujutsu error types with styled output.
//!
//! These error types implement Display with ANSI styling for terminal output.

use std::path::PathBuf;

use color_print::cformat;

use crate::path::format_path_for_display;

/// Errors specific to jj operations.
///
/// All variants format with ANSI colors via Display.
#[derive(Debug)]
pub enum JjError {
    /// jj command not found
    JjNotFound,

    /// Not inside a jj repository
    NotInRepository {
        path: PathBuf,
    },

    /// Workspace doesn't exist
    WorkspaceNotFound {
        name: String,
    },

    /// Workspace already exists
    WorkspaceAlreadyExists {
        name: String,
    },

    /// Workspace path is occupied by another workspace
    WorkspacePathOccupied {
        name: String,
        path: PathBuf,
        occupant: Option<String>,
    },

    /// Workspace directory is missing (workspace exists in jj but directory is gone)
    WorkspaceMissing {
        name: String,
    },

    /// Bookmark doesn't exist
    BookmarkNotFound {
        bookmark: String,
        show_create_hint: bool,
    },

    /// Bookmark already exists
    BookmarkAlreadyExists {
        bookmark: String,
    },

    /// No workspace found for the given bookmark
    NoWorkspaceForBookmark {
        bookmark: String,
    },

    /// Not in a workspace (e.g., in the repo root without a workspace)
    NotInWorkspace {
        action: Option<String>,
    },

    /// Failed to create workspace
    WorkspaceCreationFailed {
        name: String,
        error: String,
    },

    /// Generic jj error
    Other {
        message: String,
    },
}

impl std::fmt::Display for JjError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::JjNotFound => {
                write!(f, "{}", cformat!("<red><bold>error:</></> jj command not found\n\nInstall Jujutsu from https://martinvonz.github.io/jj/"))
            }

            Self::NotInRepository { path } => {
                let path_display = format_path_for_display(path);
                write!(
                    f,
                    "{}",
                    cformat!(
                        "<red><bold>error:</></> not a jj repository: <bold>{path_display}</>"
                    )
                )
            }

            Self::WorkspaceNotFound { name } => {
                write!(
                    f,
                    "{}",
                    cformat!("<red><bold>error:</></> workspace not found: <bold>{name}</>")
                )
            }

            Self::WorkspaceAlreadyExists { name } => {
                write!(
                    f,
                    "{}",
                    cformat!("<red><bold>error:</></> workspace already exists: <bold>{name}</>")
                )
            }

            Self::WorkspacePathOccupied {
                name,
                path,
                occupant,
            } => {
                let path_display = format_path_for_display(path);
                let occupant_info = occupant
                    .as_ref()
                    .map(|o| cformat!(" (workspace <bold>{o}</>)"))
                    .unwrap_or_default();
                write!(
                    f,
                    "{}",
                    cformat!(
                        "<red><bold>error:</></> cannot create workspace <bold>{name}</> at <bold>{path_display}</>\n\
                         Path is already a workspace{occupant_info}"
                    )
                )
            }

            Self::WorkspaceMissing { name } => {
                write!(
                    f,
                    "{}",
                    cformat!(
                        "<red><bold>error:</></> workspace <bold>{name}</> directory is missing\n\n\
                         The workspace is tracked by jj but the directory was deleted.\n\
                         Run <bright-black>jj workspace forget {name}</> to remove the stale entry."
                    )
                )
            }

            Self::BookmarkNotFound {
                bookmark,
                show_create_hint,
            } => {
                let hint = if *show_create_hint {
                    cformat!(
                        "\n\nTo create a new bookmark and workspace, use <bright-black>wt switch --create {bookmark}</>"
                    )
                } else {
                    String::new()
                };
                write!(
                    f,
                    "{}",
                    cformat!(
                        "<red><bold>error:</></> bookmark not found: <bold>{bookmark}</>{hint}"
                    )
                )
            }

            Self::BookmarkAlreadyExists { bookmark } => {
                write!(
                    f,
                    "{}",
                    cformat!("<red><bold>error:</></> bookmark already exists: <bold>{bookmark}</>")
                )
            }

            Self::NoWorkspaceForBookmark { bookmark } => {
                write!(
                    f,
                    "{}",
                    cformat!(
                        "<red><bold>error:</></> no workspace found for bookmark <bold>{bookmark}</>\n\n\
                         Use <bright-black>wt switch {bookmark}</> to create a workspace for this bookmark."
                    )
                )
            }

            Self::NotInWorkspace { action } => {
                let action_info = action
                    .as_ref()
                    .map(|a| format!(" to {a}"))
                    .unwrap_or_default();
                write!(
                    f,
                    "{}",
                    cformat!(
                        "<red><bold>error:</></> not in a workspace{action_info}\n\n\
                         Run this command from within a jj workspace."
                    )
                )
            }

            Self::WorkspaceCreationFailed { name, error } => {
                write!(
                    f,
                    "{}",
                    cformat!(
                        "<red><bold>error:</></> failed to create workspace <bold>{name}</>\n\n{error}"
                    )
                )
            }

            Self::Other { message } => {
                write!(f, "{}", cformat!("<red><bold>error:</></> {message}"))
            }
        }
    }
}

impl std::error::Error for JjError {}
