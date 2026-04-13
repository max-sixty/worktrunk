//! Schema helpers for config validation.
//!
//! Uses JsonSchema to derive valid top-level keys, feeding
//! `WorktrunkConfig::is_valid_key` so unknown-key classification can tell
//! "belongs in the other config" from "truly unknown."

use schemars::SchemaGenerator;

use super::UserConfig;

/// Returns all valid top-level keys in user config, derived from the JsonSchema.
///
/// This includes keys from UserConfig and HooksConfig (flattened).
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
