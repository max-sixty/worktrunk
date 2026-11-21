//! JSON Schema documents for the user and project configuration files.
//!
//! `test_docs_are_in_sync` commits these to `docs/public/schema/`, published
//! at the `$id` each document carries, so editors with TOML schema support can
//! validate and autocomplete config files.
//!
//! Generated under schemars' *deserialize* contract, which marks a field
//! required only when serde cannot fill it in. The serialize contract would
//! mark defaulted fields required because the serializer always emits them,
//! rejecting configs that omit optional settings. The documents are strict
//! about unknown keys so an editor flags typos instead of silently ignoring
//! them. That includes the deprecated `pre-create`/`post-create` hook
//! aliases: serde still accepts them, but schemars only sees the canonical
//! names, so an editor flags them for migration.

use schemars::JsonSchema;
use serde_json::Value;

/// JSON Schema for `~/.config/worktrunk/config.toml`.
pub fn user_config_schema_document() -> Value {
    schema_document::<super::UserConfig>(
        "https://worktrunk.dev/schema/user-config.json",
        "Worktrunk user config",
        "Schema for ~/.config/worktrunk/config.toml.",
    )
}

/// JSON Schema for `.config/wt.toml`.
pub fn project_config_schema_document() -> Value {
    schema_document::<super::ProjectConfig>(
        "https://worktrunk.dev/schema/project-config.json",
        "Worktrunk project config",
        "Schema for .config/wt.toml.",
    )
}

fn schema_document<T: JsonSchema>(id: &str, title: &str, description: &str) -> Value {
    let settings = schemars::generate::SchemaSettings::default();
    let schema = schemars::SchemaGenerator::new(settings).into_root_schema_for::<T>();

    let mut value = serde_json::to_value(&schema).expect("schema serializes");
    let object = value.as_object_mut().expect("schema is an object");
    object.insert("$id".to_string(), id.into());
    object.insert("title".to_string(), title.into());
    object.insert("description".to_string(), description.into());

    finalize(&mut value);
    // Keep the published schema stable regardless of JSON map storage order.
    value.sort_all_objects();
    value
}

/// Walk every subschema, closing object schemas to unknown keys. Flattened
/// hook fields appear at the root and inside a `[projects."…"]` entry, so the
/// walk is generic.
///
/// Only objects that declare named `properties` without their own
/// `additionalProperties` are closed: map-valued schemas (`projects`,
/// `aliases`, `custom-columns`) and `CommandConfig`'s table form already carry
/// one.
fn finalize(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if object.contains_key("properties") && !object.contains_key("additionalProperties") {
                object.insert("additionalProperties".to_string(), false.into());
            }
            object.values_mut().for_each(finalize);
        }
        Value::Array(items) => items.iter_mut().for_each(finalize),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ProjectConfig, UserConfig};
    use serde::de::DeserializeOwned;

    /// Schema errors for `toml`, after checking the loader accepts it as `T`,
    /// so every fixture is a config Worktrunk actually reads.
    fn schema_errors<T: DeserializeOwned>(schema: &Value, toml: &str) -> Vec<String> {
        toml::from_str::<T>(toml).expect("fixture deserializes");
        let instance: Value = toml::from_str(toml).expect("fixture is TOML");
        jsonschema::validator_for(schema)
            .expect("schema compiles")
            .iter_errors(&instance)
            .map(|error| format!("{}: {error}", error.instance_path()))
            .collect()
    }

    fn rejects(schema: &Value, toml: &str) -> bool {
        let instance: Value = toml::from_str(toml).expect("fixture is TOML");
        !jsonschema::is_valid(schema, &instance)
    }

    #[test]
    fn user_schema_accepts_representative_config() {
        let config = r#"
worktree-path = "../{{ repo }}.{{ branch | sanitize }}"
pre-start = "npm ci"
post-start = ["npm run dev"]
skip-shell-integration-prompt = true

[pre-merge]
test = "npm test"
lint = "npm run lint"

[list]
full = true
json-schema = 2
columns = ["branch", "ci"]

[list.custom-columns.Ticket]
template = "{{ vars.ticket }}"

[commit.generation]
command = "llm -m test"

[aliases]
deploy = "make deploy"

[projects."github.com/acme/repo"]
worktree-path = ".worktrees/{{ branch | sanitize }}"
post-start = "echo project"
"#;
        let errors = schema_errors::<UserConfig>(&user_config_schema_document(), config);
        assert!(errors.is_empty(), "config rejected: {errors:#?}");
    }

    #[test]
    fn project_schema_accepts_representative_config() {
        let config = r#"
pre-start = "npm ci"
post-start = "npm run dev"
pre-merge = ["npm test", "npm run lint"]

[list]
url = "http://localhost:{{ branch | hash_port }}"

[forge]
platform = "github"
hostname = "github.example.com"

[commit.generation]
template-append = "Use conventional commits"

[step.copy-ignored]
exclude = [".cache/"]

[aliases]
deploy = "make deploy"
"#;
        let errors = schema_errors::<ProjectConfig>(&project_config_schema_document(), config);
        assert!(errors.is_empty(), "config rejected: {errors:#?}");
    }

    #[test]
    fn schemas_reject_misspelled_keys() {
        let user = user_config_schema_document();
        assert!(rejects(&user, r#"worktree-pth = "x""#));
        assert!(rejects(&user, "[list]\nful = true"));
        assert!(rejects(
            &user,
            "[projects.\"github.com/acme/repo\"]\npre-starts = \"x\""
        ));

        let project = project_config_schema_document();
        assert!(rejects(&project, r#"pre-starts = "x""#));
        assert!(rejects(&project, "[forge]\nplatfrom = \"github\""));
        // User-only settings don't belong in a project file.
        assert!(rejects(&project, "[projects.\"github.com/acme/repo\"]"));

        // Deprecated hook aliases load, but the schema steers editors to the
        // canonical names.
        assert!(rejects(&user, r#"pre-create = "x""#));
        assert!(rejects(&project, r#"post-create = "x""#));
    }
}
