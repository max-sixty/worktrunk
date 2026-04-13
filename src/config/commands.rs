//! Command configuration types for hook pipelines.
//!
//! See `wt hook --help` → "Pipeline Ordering" for user-facing docs.
//! See [`HookStep`] and [`CommandConfig`] for the internal model.
//!
//! # TOML representation notes
//!
//! In primitive terms, a hook deserializes from one of three values: a
//! string, a dict, or a list of (string | dict). TOML offers multiple
//! syntaxes for each primitive — `[hook]` section vs. `hook = {...}` inline,
//! `[[hook]]` headers vs. `hook = [{...}]` inline. These are equivalent at
//! the parsed value, so the deserializer sees only the primitive shape.
//!
//! | Primitive | Example | Resulting `steps` |
//! |---|---|---|
//! | string | `hook = "cmd"` | `[Single(unnamed)]` |
//! | dict | `[hook]` + keys, or `hook = {a="...", b="..."}` | `[Concurrent(all entries)]` — always `Concurrent`, even for one entry |
//! | list | `hook = [{a="..."}, "cmd", ...]` | one step per element: string → `Single(unnamed)`; 1-key dict → `Single(named)`; multi-key dict → `Concurrent` |
//!
//! ## Dict-at-top vs. dict-in-list is asymmetric
//!
//! A top-level dict always becomes `Concurrent`. A one-entry dict inside a
//! list becomes `Single(named)` instead (see `map_to_step`). So
//! `{test="..."}` and `[{test="..."}]` have the same command set but
//! different `HookStep` variants. For pre-* hooks this is invisible
//! (everything runs serially anyway); for post-* hooks it controls
//! parallelism.
//!
//! ## `[[hook]]` header form is not a full alternative to pipeline form
//!
//! TOML array-of-tables headers only produce dict elements. A pipeline that
//! mixes anonymous strings with named dicts — e.g.
//! `hook = ["cargo setup", {build="..."}]` — cannot be rewritten as repeated
//! `[[hook]]` blocks without inventing a name for each bare-string step.
//! Naming is user-visible: it changes the step from anonymous `Single` to
//! named `Single`, which affects log file paths
//! (`.../set-vars.log` vs. a positional slot) and hook-selection filtering
//! (`wt hook post-start <name>`).

use std::collections::BTreeMap;

use indexmap::IndexMap;
use schemars::JsonSchema;
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize};

/// Represents a command with its template and optionally expanded form.
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    /// Optional name for the command (e.g., "build", "test")
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

/// A step in a hook pipeline.
///
/// The execution model depends on the hook type:
/// - **Post-* hooks**: `Single` steps run serially, `Concurrent` steps spawn in parallel.
///   The entire pipeline runs in the background as one detached process.
/// - **Pre-* hooks**: All commands run serially regardless of step type.
#[derive(Debug, Clone, PartialEq)]
pub enum HookStep {
    /// A single command (from a string in a list, or a single-entry map).
    Single(Command),
    /// Multiple commands that run concurrently (from a multi-entry map).
    Concurrent(Vec<Command>),
}

/// Configuration for commands — canonical representation.
///
/// Internally stores a pipeline of `HookStep`s. Deserializes from three TOML forms:
/// - Single string: `post-start = "npm install"`
/// - Named table: `[post-start]` with `name = "command"` entries → one Concurrent step
/// - Pipeline: `post-start = ["cmd", { a = "cmd1", b = "cmd2" }]` → serial steps
///
/// **Order preservation:** Named commands preserve TOML insertion order (IndexMap).
#[derive(Debug, Clone, PartialEq)]
pub struct CommandConfig {
    steps: Vec<HookStep>,
}

impl CommandConfig {
    /// Create a config with a single unnamed command.
    pub fn single(template: impl Into<String>) -> Self {
        Self {
            steps: vec![HookStep::Single(Command::new(None, template.into()))],
        }
    }

    /// Returns a flat iterator over all commands (for approval, completion, display).
    pub fn commands(&self) -> impl Iterator<Item = &Command> {
        self.steps.iter().flat_map(|step| match step {
            HookStep::Single(cmd) => std::slice::from_ref(cmd).iter(),
            HookStep::Concurrent(cmds) => cmds.iter(),
        })
    }

    /// Returns true if this config uses a pipeline (list form with multiple steps).
    /// Single-step configs (string or map) return false.
    pub fn is_pipeline(&self) -> bool {
        self.steps.len() > 1
    }

    /// Returns the pipeline steps for execution.
    pub fn steps(&self) -> &[HookStep] {
        &self.steps
    }

    /// Merge two configs by appending steps (base steps first, then overlay).
    ///
    /// Used for per-project hook overrides where both global and project hooks run.
    pub fn merge_append(&self, other: &Self) -> Self {
        let mut steps = self.steps.clone();
        steps.extend(other.steps.iter().cloned());
        Self { steps }
    }
}

/// Validate that no command names contain colons (would break log spec parsing).
fn validate_no_colons<E: serde::de::Error>(map: &IndexMap<String, String>) -> Result<(), E> {
    for name in map.keys() {
        if name.contains(':') {
            return Err(serde::de::Error::custom(format!(
                "hook name '{}' cannot contain colons",
                name
            )));
        }
    }
    Ok(())
}

/// Convert an IndexMap of named commands to a HookStep.
/// Single-entry maps become `Single` (named serial step),
/// multi-entry maps become `Concurrent`.
fn map_to_step(map: IndexMap<String, String>) -> HookStep {
    if map.len() == 1 {
        let (name, template) = map.into_iter().next().unwrap();
        HookStep::Single(Command::new(Some(name), template))
    } else {
        HookStep::Concurrent(
            map.into_iter()
                .map(|(name, template)| Command::new(Some(name), template))
                .collect(),
        )
    }
}

/// Append alias commands from `additions` into `base`.
///
/// On name collision, commands are appended (base first, then additions),
/// matching how hooks merge across config layers.
pub fn append_aliases(
    base: &mut BTreeMap<String, CommandConfig>,
    additions: &BTreeMap<String, CommandConfig>,
) {
    for (k, v) in additions {
        base.entry(k.clone())
            .and_modify(|existing| *existing = existing.merge_append(v))
            .or_insert_with(|| v.clone());
    }
}

/// Accepted forms for a command, reused across error messages so the three
/// supported shapes appear in every invalid-type diagnostic.
const EXPECTING: &str = r#"a command in one of these forms:
- a string: "cargo build"
- a named table: { build = "cargo build", test = "cargo test" }
- a pipeline list: ["cargo build", { test = "cargo test" }]
run `wt hook --help` for details"#;

/// Accepted forms for an entry inside a pipeline list (sub-form of `EXPECTING`
/// — pipelines can't nest, so only the string and named-table forms are valid).
const EXPECTING_PIPELINE_ENTRY: &str =
    r#"a command string "cargo build" or a named table { build = "cargo build" }"#;

/// An entry in a pipeline list: either a string or a map of named commands.
///
/// Anonymous strings work but are intentionally undocumented — they
/// complicate the explanation without adding much over single-entry maps.
enum PipelineEntry {
    Anonymous(String),
    Named(IndexMap<String, String>),
}

impl<'de> Deserialize<'de> for PipelineEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct PipelineEntryVisitor;

        impl<'de> serde::de::Visitor<'de> for PipelineEntryVisitor {
            type Value = PipelineEntry;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str(EXPECTING_PIPELINE_ENTRY)
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(PipelineEntry::Anonymous(v.to_string()))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut entries: IndexMap<String, String> = IndexMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    let value = map.next_value::<String>()?;
                    entries.insert(key, value);
                }
                Ok(PipelineEntry::Named(entries))
            }
        }

        deserializer.deserialize_any(PipelineEntryVisitor)
    }
}

// Custom deserialization to handle 3 TOML formats with format-specific errors.
//
// Using a visitor (instead of `#[serde(untagged)]`) means errors describe which
// form failed and point to the offending value — an untagged enum can only
// report "data did not match any variant" at the start of the value.
impl<'de> Deserialize<'de> for CommandConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct CommandConfigVisitor;

        impl<'de> serde::de::Visitor<'de> for CommandConfigVisitor {
            type Value = CommandConfig;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str(EXPECTING)
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(CommandConfig {
                    steps: vec![HookStep::Single(Command::new(None, v.to_string()))],
                })
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let mut steps = Vec::new();
                while let Some(entry) = seq.next_element::<PipelineEntry>()? {
                    match entry {
                        PipelineEntry::Anonymous(cmd) => {
                            steps.push(HookStep::Single(Command::new(None, cmd)));
                        }
                        PipelineEntry::Named(map) => {
                            if map.is_empty() {
                                continue;
                            }
                            validate_no_colons(&map)?;
                            steps.push(map_to_step(map));
                        }
                    }
                }
                Ok(CommandConfig { steps })
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut entries: IndexMap<String, String> = IndexMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    let value = map.next_value::<String>()?;
                    entries.insert(key, value);
                }
                validate_no_colons(&entries)?;
                let commands: Vec<Command> = entries
                    .into_iter()
                    .map(|(name, template)| Command::new(Some(name), template))
                    .collect();
                Ok(CommandConfig {
                    steps: vec![HookStep::Concurrent(commands)],
                })
            }
        }

        deserializer.deserialize_any(CommandConfigVisitor)
    }
}

// JsonSchema for CommandConfig
impl JsonSchema for CommandConfig {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "CommandConfig".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "oneOf": [
                { "type": "string" },
                {
                    "type": "object",
                    "additionalProperties": { "type": "string" }
                },
                {
                    "type": "array",
                    "items": {
                        "oneOf": [
                            { "type": "string" },
                            {
                                "type": "object",
                                "additionalProperties": { "type": "string" }
                            }
                        ]
                    }
                }
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
        // Single unnamed command → string
        if self.steps.len() == 1
            && let HookStep::Single(cmd) = &self.steps[0]
            && cmd.name.is_none()
        {
            return cmd.template.serialize(serializer);
        }

        // Single concurrent step (all named) → named table
        if self.steps.len() == 1
            && let HookStep::Concurrent(cmds) = &self.steps[0]
        {
            return serialize_commands_as_map(cmds, serializer);
        }

        // Pipeline → array of mixed strings and tables
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(self.steps.len()))?;
        for step in &self.steps {
            match step {
                HookStep::Single(cmd) => {
                    if let Some(name) = &cmd.name {
                        let mut map = IndexMap::new();
                        map.insert(name.as_str(), cmd.template.as_str());
                        seq.serialize_element(&map)?;
                    } else {
                        seq.serialize_element(&cmd.template)?;
                    }
                }
                HookStep::Concurrent(cmds) => {
                    let mut map = IndexMap::new();
                    let mut unnamed_counter = 0u32;
                    for c in cmds {
                        let key = match &c.name {
                            Some(name) => name.as_str().to_string(),
                            None => {
                                unnamed_counter += 1;
                                format!("_{unnamed_counter}")
                            }
                        };
                        map.insert(key, c.template.as_str());
                    }
                    seq.serialize_element(&map)?;
                }
            }
        }
        seq.end()
    }
}

fn serialize_commands_as_map<S>(cmds: &[Command], serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    let mut map = serializer.serialize_map(Some(cmds.len()))?;
    let mut unnamed_counter = 0u32;
    for cmd in cmds {
        let key = match &cmd.name {
            Some(name) => name.clone(),
            None => {
                unnamed_counter += 1;
                format!("_{unnamed_counter}")
            }
        };
        map.serialize_entry(&key, &cmd.template)?;
    }
    map.end()
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use super::*;

    // ============================================================================
    // Deserialization Tests
    // ============================================================================

    #[test]
    fn test_deserialize_single_string() {
        let toml_str = r#"command = "npm install""#;

        #[derive(Deserialize)]
        struct Wrapper {
            command: CommandConfig,
        }

        let wrapper: Wrapper = toml::from_str(toml_str).unwrap();
        let commands: Vec<_> = wrapper.command.commands().collect();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].name, None);
        assert_eq!(commands[0].template, "npm install");

        // Single string → one Single step
        assert_eq!(wrapper.command.steps().len(), 1);
        assert!(matches!(&wrapper.command.steps()[0], HookStep::Single(_)));
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
        let commands: Vec<_> = wrapper.command.commands().collect();
        assert_eq!(commands.len(), 2);
        assert!(commands.iter().any(|c| c.name == Some("build".to_string())));
        assert!(commands.iter().any(|c| c.name == Some("test".to_string())));

        // Named table → one Concurrent step
        assert_eq!(wrapper.command.steps().len(), 1);
        assert!(matches!(
            &wrapper.command.steps()[0],
            HookStep::Concurrent(cmds) if cmds.len() == 2
        ));
    }

    #[test]
    fn test_deserialize_preserves_order() {
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
        let commands: Vec<_> = wrapper.command.commands().collect();
        assert_eq!(commands.len(), 3);
        assert_eq!(commands[0].name, Some("first".to_string()));
        assert_eq!(commands[1].name, Some("second".to_string()));
        assert_eq!(commands[2].name, Some("third".to_string()));
    }

    #[test]
    fn test_deserialize_rejects_colons_in_name() {
        let toml_str = r#"
[command]
"my:server" = "npm start"
"#;

        #[derive(Debug, Deserialize)]
        struct Wrapper {
            #[serde(rename = "command")]
            _command: CommandConfig,
        }

        let result: Result<Wrapper, _> = toml::from_str(toml_str);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("cannot contain colons"),
            "Expected colon rejection error: {}",
            err
        );
    }

    // ============================================================================
    // Pipeline Deserialization Tests
    // ============================================================================

    #[test]
    fn test_deserialize_pipeline_strings() {
        let toml_str = r#"command = ["npm install", "npm run build"]"#;

        #[derive(Deserialize)]
        struct Wrapper {
            command: CommandConfig,
        }

        let wrapper: Wrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(wrapper.command.steps().len(), 2);
        assert!(
            matches!(&wrapper.command.steps()[0], HookStep::Single(c) if c.template == "npm install")
        );
        assert!(
            matches!(&wrapper.command.steps()[1], HookStep::Single(c) if c.template == "npm run build")
        );
    }

    #[test]
    fn test_deserialize_pipeline_mixed() {
        let toml_str = r#"command = [
    "npm install",
    { build = "npm run build", lint = "npm run lint" }
]"#;

        #[derive(Deserialize)]
        struct Wrapper {
            command: CommandConfig,
        }

        let wrapper: Wrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(wrapper.command.steps().len(), 2);
        assert!(matches!(&wrapper.command.steps()[0], HookStep::Single(c) if c.name.is_none()));
        assert!(matches!(
            &wrapper.command.steps()[1],
            HookStep::Concurrent(cmds) if cmds.len() == 2
        ));

        let commands: Vec<_> = wrapper.command.commands().collect();
        assert_eq!(commands.len(), 3);
    }

    #[test]
    fn test_deserialize_pipeline_named_single() {
        // Single-entry map in list → named serial step
        let toml_str = r#"command = [
    { install = "npm install" },
    { build = "npm run build", lint = "npm run lint" }
]"#;

        #[derive(Deserialize)]
        struct Wrapper {
            command: CommandConfig,
        }

        let wrapper: Wrapper = toml::from_str(toml_str).unwrap();
        assert_eq!(wrapper.command.steps().len(), 2);

        // First step: named single
        if let HookStep::Single(cmd) = &wrapper.command.steps()[0] {
            assert_eq!(cmd.name.as_deref(), Some("install"));
            assert_eq!(cmd.template, "npm install");
        } else {
            panic!("Expected Single step");
        }

        // Second step: concurrent group
        assert!(matches!(
            &wrapper.command.steps()[1],
            HookStep::Concurrent(cmds) if cmds.len() == 2
        ));
    }

    #[test]
    fn test_deserialize_pipeline_rejects_colons() {
        let toml_str = r#"command = [{ "my:hook" = "npm start" }]"#;

        #[derive(Debug, Deserialize)]
        struct Wrapper {
            #[serde(rename = "command")]
            _command: CommandConfig,
        }

        let result: Result<Wrapper, _> = toml::from_str(toml_str);
        assert!(result.is_err());
    }

    // ============================================================================
    // Error Message Tests
    //
    // These lock in the format-aware error messages. The generic serde error
    // "data did not match any variant of untagged enum" is not useful — users
    // need to know which forms are accepted and which value is invalid.
    // ============================================================================

    #[derive(Debug, Deserialize)]
    struct CommandWrapper {
        #[serde(rename = "command")]
        _command: CommandConfig,
    }

    fn deserialize_err(toml_str: &str) -> String {
        toml::from_str::<CommandWrapper>(toml_str)
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn test_error_lists_accepted_forms_at_top_level() {
        // Wrong type at the top level → error must list all three accepted forms
        // so the user knows what to write instead.
        assert_snapshot!(deserialize_err("command = 42"), @r#"
        TOML parse error at line 1, column 11
          |
        1 | command = 42
          |           ^^
        invalid type: integer `42`, expected a command in one of these forms:
        - a string: "cargo build"
        - a named table: { build = "cargo build", test = "cargo test" }
        - a pipeline list: ["cargo build", { test = "cargo test" }]
        run `wt hook --help` for details
        "#);
    }

    #[test]
    fn test_error_identifies_non_string_value_in_named_table() {
        // Non-string value inside a named table → error should point at the
        // specific value, not report a generic "no variant matched".
        assert_snapshot!(
            deserialize_err(
                r#"[command]
build = "cargo build"
broken = 42
"#,
            ),
            @r#"
        TOML parse error at line 3, column 10
          |
        3 | broken = 42
          |          ^^
        invalid type: integer `42`, expected a string
        "#
        );
    }

    #[test]
    fn test_error_describes_pipeline_entry_forms_for_wrong_type() {
        // Wrong type as a pipeline entry → error must list the two accepted
        // entry forms (string or named table). Pipelines can't nest, so the
        // top-level "pipeline list" form isn't repeated here.
        assert_snapshot!(deserialize_err("command = [42]"), @r#"
        TOML parse error at line 1, column 12
          |
        1 | command = [42]
          |            ^^
        invalid type: integer `42`, expected a command string "cargo build" or a named table { build = "cargo build" }
        "#);
    }

    #[test]
    fn test_error_identifies_non_string_value_in_pipeline_map() {
        // Non-string value inside a pipeline map → error should point at the
        // specific value. This is the case that prompted the improvement:
        // previously produced "data did not match any variant of untagged enum
        // CommandConfigToml" with no indication of which value was invalid.
        assert_snapshot!(
            deserialize_err(
                r#"command = [
    { build = "cargo build", ignore_exit = true }
]"#,
            ),
            @r#"
        TOML parse error at line 2, column 44
          |
        2 |     { build = "cargo build", ignore_exit = true }
          |                                            ^^^^
        invalid type: boolean `true`, expected a string
        "#
        );
    }

    // ============================================================================
    // Serialization Tests
    // ============================================================================

    #[test]
    fn test_serialize_single_unnamed() {
        #[derive(Serialize)]
        struct Wrapper {
            cmd: CommandConfig,
        }

        let wrapper = Wrapper {
            cmd: CommandConfig {
                steps: vec![HookStep::Single(Command::new(
                    None,
                    "npm install".to_string(),
                ))],
            },
        };

        assert_snapshot!(toml::to_string(&wrapper).unwrap(), @r#"cmd = "npm install""#);
    }

    #[test]
    fn test_serialize_concurrent() {
        #[derive(Serialize)]
        struct Wrapper {
            cmd: CommandConfig,
        }

        let wrapper = Wrapper {
            cmd: CommandConfig {
                steps: vec![HookStep::Concurrent(vec![
                    Command::new(Some("build".to_string()), "cargo build".to_string()),
                    Command::new(Some("test".to_string()), "cargo test".to_string()),
                ])],
            },
        };

        assert_snapshot!(toml::to_string(&wrapper).unwrap(), @r#"
        [cmd]
        build = "cargo build"
        test = "cargo test"
        "#);
    }

    #[test]
    fn test_serialize_pipeline() {
        #[derive(Serialize)]
        struct Wrapper {
            cmd: CommandConfig,
        }

        let wrapper = Wrapper {
            cmd: CommandConfig {
                steps: vec![
                    HookStep::Single(Command::new(None, "npm install".to_string())),
                    HookStep::Concurrent(vec![
                        Command::new(Some("build".to_string()), "npm run build".to_string()),
                        Command::new(Some("lint".to_string()), "npm run lint".to_string()),
                    ]),
                ],
            },
        };

        assert_snapshot!(toml::to_string(&wrapper).unwrap(), @r#"cmd = ["npm install", { build = "npm run build", lint = "npm run lint" }]"#);
    }

    #[test]
    fn test_serialize_deserialize_roundtrip_single() {
        let config = CommandConfig {
            steps: vec![HookStep::Single(Command::new(
                None,
                "echo hello".to_string(),
            ))],
        };

        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            cmd: CommandConfig,
        }

        let wrapper = Wrapper { cmd: config };
        let serialized = toml::to_string(&wrapper).unwrap();
        let deserialized: Wrapper = toml::from_str(&serialized).unwrap();

        assert_eq!(deserialized.cmd.commands().count(), 1);
        assert_eq!(
            deserialized.cmd.commands().next().unwrap().template,
            "echo hello"
        );
    }

    #[test]
    fn test_serialize_deserialize_roundtrip_named() {
        let config = CommandConfig {
            steps: vec![HookStep::Concurrent(vec![
                Command::new(Some("a".to_string()), "echo a".to_string()),
                Command::new(Some("b".to_string()), "echo b".to_string()),
            ])],
        };

        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            cmd: CommandConfig,
        }

        let wrapper = Wrapper { cmd: config };
        let serialized = toml::to_string(&wrapper).unwrap();
        let deserialized: Wrapper = toml::from_str(&serialized).unwrap();

        assert_eq!(deserialized.cmd.commands().count(), 2);
    }

    // ============================================================================
    // Commands() Flattening Tests
    // ============================================================================

    #[test]
    fn test_commands_flattens_pipeline() {
        let config = CommandConfig {
            steps: vec![
                HookStep::Single(Command::new(None, "cmd1".to_string())),
                HookStep::Concurrent(vec![
                    Command::new(Some("a".to_string()), "cmd2".to_string()),
                    Command::new(Some("b".to_string()), "cmd3".to_string()),
                ]),
                HookStep::Single(Command::new(None, "cmd4".to_string())),
            ],
        };

        let cmds: Vec<_> = config.commands().collect();
        assert_eq!(cmds.len(), 4);
        assert_eq!(cmds[0].template, "cmd1");
        assert_eq!(cmds[1].template, "cmd2");
        assert_eq!(cmds[2].template, "cmd3");
        assert_eq!(cmds[3].template, "cmd4");
    }

    // ============================================================================
    // Merge Tests
    // ============================================================================

    #[test]
    fn test_merge_append_steps() {
        let base = CommandConfig {
            steps: vec![HookStep::Single(Command::new(None, "step1".to_string()))],
        };
        let overlay = CommandConfig {
            steps: vec![HookStep::Concurrent(vec![
                Command::new(Some("a".to_string()), "step2a".to_string()),
                Command::new(Some("b".to_string()), "step2b".to_string()),
            ])],
        };

        let merged = base.merge_append(&overlay);
        assert_eq!(merged.steps.len(), 2);
        assert!(matches!(&merged.steps[0], HookStep::Single(_)));
        assert!(matches!(&merged.steps[1], HookStep::Concurrent(_)));
    }

    // ============================================================================
    // Backward Compatibility
    // ============================================================================

    #[test]
    fn test_serialize_mixed_named_unnamed_succeeds() {
        #[derive(Serialize)]
        struct Wrapper {
            cmd: CommandConfig,
        }

        // Simulate merge of unnamed global + named project hooks
        let global = CommandConfig {
            steps: vec![HookStep::Single(Command::new(
                None,
                "npm install".to_string(),
            ))],
        };
        let per_project = CommandConfig {
            steps: vec![HookStep::Concurrent(vec![Command::new(
                Some("setup".to_string()),
                "echo setup".to_string(),
            )])],
        };

        let merged = global.merge_append(&per_project);
        assert_eq!(merged.steps.len(), 2);

        // Pipeline serialization
        let wrapper = Wrapper { cmd: merged };
        assert_snapshot!(toml::to_string(&wrapper).unwrap(), @r#"cmd = ["npm install", { setup = "echo setup" }]"#);
    }
}
