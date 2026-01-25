//! Command configuration types for project hooks
//!
//! Handles parsing and representation of commands that run during various phases
//! of worktree and merge operations.

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};

/// Represents a command with its template and optionally expanded form
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    /// Optional name for the command (e.g., "build", "test", or auto-numbered "1", "2")
    pub name: Option<String>,
    /// Template string that may contain variables like {{ branch }}, {{ worktree }}
    pub template: String,
    /// Expanded command with variables substituted (same as template if not expanded yet)
    pub expanded: String,
}

impl Command {
    /// Create a new command from a template (not yet expanded)
    pub fn new(name: Option<String>, template: String) -> Self {
        Self {
            name,
            expanded: template.clone(),
            template,
        }
    }

    /// Create a command with both template and expanded forms
    pub fn with_expansion(name: Option<String>, template: String, expanded: String) -> Self {
        Self {
            name,
            template,
            expanded,
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

    /// Merge two configs by appending commands (base commands first, then overlay).
    ///
    /// Used for per-project hook overrides where both global and project hooks run.
    pub fn merge_append(&self, other: &Self) -> Self {
        let mut commands = self.commands.clone();
        commands.extend(other.commands.iter().cloned());
        Self { commands }
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
                vec![Command::new(None, cmd)]
            }
            CommandConfigToml::Named(map) => {
                // IndexMap preserves insertion order from TOML
                map.into_iter()
                    .map(|(name, template)| Command::new(Some(name), template))
                    .collect()
            }
        };
        Ok(CommandConfig { commands })
    }
}

// JsonSchema for CommandConfig - describes the two TOML formats
impl JsonSchema for CommandConfig {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "CommandConfig".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        // CommandConfig accepts either a string or an object with string values
        // We just need this for schema generation, not validation
        schemars::json_schema!({
            "oneOf": [
                { "type": "string" },
                { "type": "object", "additionalProperties": { "type": "string" } }
            ]
        })
    }
}

// Serialize back to most appropriate format
impl Serialize for CommandConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
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
        let cmd = Command::new(Some("build".to_string()), "cargo build".to_string());
        assert_eq!(cmd.name, Some("build".to_string()));
        assert_eq!(cmd.template, "cargo build");
        assert_eq!(cmd.expanded, "cargo build"); // Same as template when not expanded
    }

    #[test]
    fn test_command_new_unnamed() {
        let cmd = Command::new(None, "npm install".to_string());
        assert_eq!(cmd.name, None);
        assert_eq!(cmd.template, "npm install");
        assert_eq!(cmd.expanded, "npm install");
    }

    #[test]
    fn test_command_with_expansion() {
        let cmd = Command::with_expansion(
            Some("test".to_string()),
            "cargo test --package {{ repo }}".to_string(),
            "cargo test --package myrepo".to_string(),
        );
        assert_eq!(cmd.name, Some("test".to_string()));
        assert_eq!(cmd.template, "cargo test --package {{ repo }}");
        assert_eq!(cmd.expanded, "cargo test --package myrepo");
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
                commands: vec![Command::new(None, "npm install".to_string())],
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
                    Command::new(Some("build".to_string()), "cargo build".to_string()),
                    Command::new(Some("test".to_string()), "cargo test".to_string()),
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
            commands: vec![Command::new(None, "echo hello".to_string())],
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
                Command::new(Some("a".to_string()), "echo a".to_string()),
                Command::new(Some("b".to_string()), "echo b".to_string()),
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
                Command::new(None, "cmd1".to_string()),
                Command::new(None, "cmd2".to_string()),
            ],
        };

        let cmds = config.commands();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].template, "cmd1");
        assert_eq!(cmds[1].template, "cmd2");
    }

    #[test]
    fn test_command_config_equality() {
        let config1 = CommandConfig {
            commands: vec![Command::new(None, "test".to_string())],
        };
        let config2 = CommandConfig {
            commands: vec![Command::new(None, "test".to_string())],
        };
        assert_eq!(config1, config2);
    }

    #[test]
    fn test_command_config_merge_append() {
        let base = CommandConfig {
            commands: vec![
                Command::new(None, "echo base1".to_string()),
                Command::new(Some("named".to_string()), "echo base2".to_string()),
            ],
        };
        let overlay = CommandConfig {
            commands: vec![Command::new(None, "echo overlay".to_string())],
        };

        let merged = base.merge_append(&overlay);
        assert_eq!(merged.commands.len(), 3);
        assert_eq!(merged.commands[0].template, "echo base1");
        assert_eq!(merged.commands[1].template, "echo base2");
        assert_eq!(merged.commands[2].template, "echo overlay");
    }

    #[test]
    fn test_command_config_merge_append_empty_base() {
        let base = CommandConfig { commands: vec![] };
        let overlay = CommandConfig {
            commands: vec![Command::new(None, "echo overlay".to_string())],
        };

        let merged = base.merge_append(&overlay);
        assert_eq!(merged.commands.len(), 1);
        assert_eq!(merged.commands[0].template, "echo overlay");
    }

    #[test]
    fn test_command_config_merge_append_empty_overlay() {
        let base = CommandConfig {
            commands: vec![Command::new(None, "echo base".to_string())],
        };
        let overlay = CommandConfig { commands: vec![] };

        let merged = base.merge_append(&overlay);
        assert_eq!(merged.commands.len(), 1);
        assert_eq!(merged.commands[0].template, "echo base");
    }
}
