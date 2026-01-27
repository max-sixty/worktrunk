//! Schema helpers for config validation.
//!
//! Uses JsonSchema to derive valid keys for unknown key detection.

use schemars::SchemaGenerator;

use super::UserConfig;

/// Returns all valid top-level keys in user config, derived from the JsonSchema.
///
/// This includes keys from UserConfig, OverridableConfig (flattened), and HooksConfig (flattened).
/// Public for use by the `WorktrunkConfig` trait implementation.
pub fn valid_user_config_keys() -> Vec<String> {
    let schema = SchemaGenerator::default().into_root_schema_for::<UserConfig>();

    // Extract property names from the schema
    // The schema flattens nested structs, so all top-level keys appear in properties
    schema
        .as_object()
        .and_then(|obj| obj.get("properties"))
        .and_then(|p| p.as_object())
        .map(|props| props.keys().cloned().collect())
        .unwrap_or_default()
}

/// Find unknown keys in user config TOML content.
///
/// Returns a map of unrecognized top-level keys (with their values) that will be ignored.
/// Compares against the known valid keys derived from the JsonSchema rather than using
/// serde flatten catchall (which doesn't work reliably with nested flattens).
/// The values are included to allow checking if keys belong in the other config type.
pub fn find_unknown_keys(contents: &str) -> std::collections::HashMap<String, toml::Value> {
    let Ok(table) = contents.parse::<toml::Table>() else {
        return std::collections::HashMap::new();
    };

    let valid_keys = valid_user_config_keys();

    table
        .into_iter()
        .filter(|(key, _)| !valid_keys.contains(key))
        .collect()
}
