//! Command configuration types for project hooks
//!
//! Handles parsing and representation of commands that run during various phases
//! of worktree and merge operations.

use crate::git::HookType;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Phase in which a command executes (alias to the canonical hook type)
pub type CommandPhase = HookType;

/// Represents a command with its template and optionally expanded form
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    /// Optional name for the command (e.g., "build", "test", or auto-numbered "1", "2")
    pub name: Option<String>,
    /// Template string that may contain variables like {{ branch }}, {{ worktree }}
    pub template: String,
    /// Expanded command with variables substituted (same as template if not expanded yet)
    pub expanded: String,
    /// Phase in which this command executes
    pub phase: CommandPhase,
}

impl Command {
    /// Create a new command from a template (not yet expanded)
    pub fn new(name: Option<String>, template: String, phase: CommandPhase) -> Self {
        Self {
            name,
            expanded: template.clone(),
            template,
            phase,
        }
    }

    /// Create a command with both template and expanded forms
    pub fn with_expansion(
        name: Option<String>,
        template: String,
        expanded: String,
        phase: CommandPhase,
    ) -> Self {
        Self {
            name,
            template,
            expanded,
            phase,
        }
    }
}

/// Configuration for commands - canonical representation
///
/// Internally stores commands as `Vec<Command>` for uniform processing.
/// Deserializes from three TOML formats:
/// - Single string: `post-create-command = "npm install"`
/// - Array: `post-create-command = ["npm install", "npm test"]`
/// - Named table: `[post-create-command]` followed by `install = "npm install"`
///
/// **Order preservation:** Named commands preserve TOML insertion order (requires
/// `preserve_order` feature on toml crate and IndexMap for deserialization). This
/// allows users to control execution order explicitly.
///
/// This canonical form eliminates branching at call sites - code just iterates over commands.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandConfig {
    commands: Vec<Command>,
}

impl CommandConfig {
    /// Returns the commands as a slice
    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    /// Returns commands with the specified phase
    pub fn commands_with_phase(&self, phase: CommandPhase) -> Vec<Command> {
        self.commands
            .iter()
            .map(|cmd| Command {
                phase,
                ..cmd.clone()
            })
            .collect()
    }
}

// Custom deserialization to handle 3 TOML formats
impl<'de> Deserialize<'de> for CommandConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum CommandConfigToml {
            Single(String),
            Multiple(Vec<String>),
            Named(IndexMap<String, String>),
        }

        let toml = CommandConfigToml::deserialize(deserializer)?;
        let commands = match toml {
            CommandConfigToml::Single(cmd) => {
                // Phase will be set later when commands are collected
                vec![Command::new(None, cmd, CommandPhase::PostCreate)]
            }
            CommandConfigToml::Multiple(cmds) => cmds
                .into_iter()
                .map(|template| Command::new(None, template, CommandPhase::PostCreate))
                .collect(),
            CommandConfigToml::Named(map) => {
                // IndexMap preserves insertion order from TOML
                map.into_iter()
                    .map(|(name, template)| {
                        Command::new(Some(name), template, CommandPhase::PostCreate)
                    })
                    .collect()
            }
        };
        Ok(CommandConfig { commands })
    }
}

// Serialize back to most appropriate format
impl Serialize for CommandConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;

        // If single unnamed command, serialize as string
        if self.commands.len() == 1 && self.commands[0].name.is_none() {
            return self.commands[0].template.serialize(serializer);
        }

        // If all commands are unnamed or numbered 1,2,3..., serialize as array
        let all_numbered = self
            .commands
            .iter()
            .enumerate()
            .all(|(i, cmd)| cmd.name.as_ref().is_none_or(|n| n == &(i + 1).to_string()));

        if all_numbered {
            let templates: Vec<_> = self.commands.iter().map(|cmd| &cmd.template).collect();
            return templates.serialize(serializer);
        }

        // Otherwise serialize as named map
        // At this point, all commands must have names (from Named TOML format)
        let mut map = serializer.serialize_map(Some(self.commands.len()))?;
        for cmd in &self.commands {
            let key = cmd.name.as_ref().unwrap();
            map.serialize_entry(key, &cmd.template)?;
        }
        map.end()
    }
}
