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
/// Deserializes from two TOML formats:
/// - Single string: `post-create = "npm install"`
/// - Named table: `[post-create]` followed by `install = "npm install"`
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

// Custom deserialization to handle 2 TOML formats
impl<'de> Deserialize<'de> for CommandConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum CommandConfigToml {
            Single(String),
            Named(IndexMap<String, String>),
        }

        let toml = CommandConfigToml::deserialize(deserializer)?;
        let commands = match toml {
            CommandConfigToml::Single(cmd) => {
                // Phase will be set later when commands are collected
                vec![Command::new(None, cmd, CommandPhase::PostCreate)]
            }
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

        // Serialize as named map (all commands from Named format have names)
        let mut map = serializer.serialize_map(Some(self.commands.len()))?;
        for cmd in &self.commands {
            let key = cmd.name.as_ref().unwrap();
            map.serialize_entry(key, &cmd.template)?;
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================================
    // Command Tests
    // ============================================================================

    #[test]
    fn test_command_new() {
        let cmd = Command::new(
            Some("build".to_string()),
            "cargo build".to_string(),
            HookType::PreMerge,
        );
        assert_eq!(cmd.name, Some("build".to_string()));
        assert_eq!(cmd.template, "cargo build");
        assert_eq!(cmd.expanded, "cargo build"); // Same as template when not expanded
        assert_eq!(cmd.phase, HookType::PreMerge);
    }

    #[test]
    fn test_command_new_unnamed() {
        let cmd = Command::new(None, "npm install".to_string(), HookType::PostCreate);
        assert_eq!(cmd.name, None);
        assert_eq!(cmd.template, "npm install");
        assert_eq!(cmd.expanded, "npm install");
        assert_eq!(cmd.phase, HookType::PostCreate);
    }

    #[test]
    fn test_command_with_expansion() {
        let cmd = Command::with_expansion(
            Some("test".to_string()),
            "cargo test --package {{ repo }}".to_string(),
            "cargo test --package myrepo".to_string(),
            HookType::PreMerge,
        );
        assert_eq!(cmd.name, Some("test".to_string()));
        assert_eq!(cmd.template, "cargo test --package {{ repo }}");
        assert_eq!(cmd.expanded, "cargo test --package myrepo");
        assert_eq!(cmd.phase, HookType::PreMerge);
    }

    #[test]
    fn test_command_clone() {
        let cmd = Command::new(
            Some("build".to_string()),
            "make".to_string(),
            HookType::PreMerge,
        );
        let cloned = cmd.clone();
        assert_eq!(cmd, cloned);
    }

    // ============================================================================
    // CommandConfig Deserialization Tests
    // ============================================================================

    #[test]
    fn test_deserialize_single_string() {
        let toml_str = r#"command = "npm install""#;

        #[derive(Deserialize)]
        struct Wrapper {
            command: CommandConfig,
        }

        let wrapper: Wrapper = toml::from_str(toml_str).unwrap();
        let commands = wrapper.command.commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].name, None);
        assert_eq!(commands[0].template, "npm install");
    }

    #[test]
    fn test_deserialize_named_table() {
        let toml_str = r#"
[command]
build = "cargo build"
test = "cargo test"
"#;

        #[derive(Deserialize)]
        struct Wrapper {
            command: CommandConfig,
        }

        let wrapper: Wrapper = toml::from_str(toml_str).unwrap();
        let commands = wrapper.command.commands();
        assert_eq!(commands.len(), 2);
        assert!(commands.iter().any(|c| c.name == Some("build".to_string())));
        assert!(commands.iter().any(|c| c.name == Some("test".to_string())));
    }

    #[test]
    fn test_deserialize_preserves_order() {
        // Order should match TOML insertion order
        let toml_str = r#"
[command]
first = "echo 1"
second = "echo 2"
third = "echo 3"
"#;

        #[derive(Deserialize)]
        struct Wrapper {
            command: CommandConfig,
        }

        let wrapper: Wrapper = toml::from_str(toml_str).unwrap();
        let commands = wrapper.command.commands();
        assert_eq!(commands.len(), 3);
        // IndexMap preserves insertion order
        assert_eq!(commands[0].name, Some("first".to_string()));
        assert_eq!(commands[1].name, Some("second".to_string()));
        assert_eq!(commands[2].name, Some("third".to_string()));
    }

    // ============================================================================
    // CommandConfig Serialization Tests
    // ============================================================================

    #[test]
    fn test_serialize_single_unnamed() {
        // A single unnamed command should serialize as a string (when wrapped in a struct)
        #[derive(Serialize)]
        struct Wrapper {
            cmd: CommandConfig,
        }

        let wrapper = Wrapper {
            cmd: CommandConfig {
                commands: vec![Command::new(
                    None,
                    "npm install".to_string(),
                    HookType::PostCreate,
                )],
            },
        };

        let serialized = toml::to_string(&wrapper).unwrap();
        assert!(serialized.contains("cmd = \"npm install\""));
    }

    #[test]
    fn test_serialize_named_commands() {
        // Multiple named commands should serialize as a table
        #[derive(Serialize)]
        struct Wrapper {
            cmd: CommandConfig,
        }

        let wrapper = Wrapper {
            cmd: CommandConfig {
                commands: vec![
                    Command::new(
                        Some("build".to_string()),
                        "cargo build".to_string(),
                        HookType::PostCreate,
                    ),
                    Command::new(
                        Some("test".to_string()),
                        "cargo test".to_string(),
                        HookType::PostCreate,
                    ),
                ],
            },
        };

        let serialized = toml::to_string(&wrapper).unwrap();
        assert!(serialized.contains("build"));
        assert!(serialized.contains("cargo build"));
        assert!(serialized.contains("test"));
        assert!(serialized.contains("cargo test"));
    }

    #[test]
    fn test_serialize_deserialize_roundtrip_single() {
        let config = CommandConfig {
            commands: vec![Command::new(
                None,
                "echo hello".to_string(),
                HookType::PostCreate,
            )],
        };

        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            cmd: CommandConfig,
        }

        let wrapper = Wrapper { cmd: config };
        let serialized = toml::to_string(&wrapper).unwrap();
        let deserialized: Wrapper = toml::from_str(&serialized).unwrap();

        assert_eq!(deserialized.cmd.commands().len(), 1);
        assert_eq!(deserialized.cmd.commands()[0].template, "echo hello");
    }

    #[test]
    fn test_serialize_deserialize_roundtrip_named() {
        let config = CommandConfig {
            commands: vec![
                Command::new(
                    Some("a".to_string()),
                    "echo a".to_string(),
                    HookType::PostCreate,
                ),
                Command::new(
                    Some("b".to_string()),
                    "echo b".to_string(),
                    HookType::PostCreate,
                ),
            ],
        };

        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            cmd: CommandConfig,
        }

        let wrapper = Wrapper { cmd: config };
        let serialized = toml::to_string(&wrapper).unwrap();
        let deserialized: Wrapper = toml::from_str(&serialized).unwrap();

        assert_eq!(deserialized.cmd.commands().len(), 2);
    }

    // ============================================================================
    // CommandConfig Methods Tests
    // ============================================================================

    #[test]
    fn test_commands_returns_slice() {
        let config = CommandConfig {
            commands: vec![
                Command::new(None, "cmd1".to_string(), HookType::PostCreate),
                Command::new(None, "cmd2".to_string(), HookType::PostCreate),
            ],
        };

        let cmds = config.commands();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].template, "cmd1");
        assert_eq!(cmds[1].template, "cmd2");
    }

    #[test]
    fn test_commands_with_phase() {
        let config = CommandConfig {
            commands: vec![
                Command::new(
                    Some("build".to_string()),
                    "cargo build".to_string(),
                    HookType::PostCreate,
                ),
                Command::new(
                    Some("test".to_string()),
                    "cargo test".to_string(),
                    HookType::PostCreate,
                ),
            ],
        };

        let cmds = config.commands_with_phase(HookType::PreMerge);

        // All returned commands should have the new phase
        assert_eq!(cmds.len(), 2);
        assert!(cmds.iter().all(|c| c.phase == HookType::PreMerge));
        // But templates and names should be preserved
        assert_eq!(cmds[0].name, Some("build".to_string()));
        assert_eq!(cmds[0].template, "cargo build");
    }

    #[test]
    fn test_command_config_equality() {
        let config1 = CommandConfig {
            commands: vec![Command::new(None, "test".to_string(), HookType::PostCreate)],
        };
        let config2 = CommandConfig {
            commands: vec![Command::new(None, "test".to_string(), HookType::PostCreate)],
        };
        assert_eq!(config1, config2);
    }
}
