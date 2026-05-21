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
///
/// `pre-create`/`post-create` are appended as silent serde aliases for the
/// canonical `pre-start`/`post-start` hook keys (see `HooksConfig` in
/// `src/config/hooks.rs`). The schema only knows canonical names from
/// `#[serde(rename = ...)]`; adding the aliases here keeps the unknown-key
/// round-trip from flagging an accepted name as unknown.
pub fn valid_user_config_keys() -> Vec<String> {
    let schema = SchemaGenerator::default().into_root_schema_for::<UserConfig>();

    // Extract property names from the schema
    // The schema flattens nested structs, so all top-level keys appear in properties
    let mut keys: Vec<String> = schema
        .as_object()
        .and_then(|obj| obj.get("properties"))
        .and_then(|p| p.as_object())
        .map(|props| props.keys().cloned().collect())
        .unwrap_or_default();
    keys.push("pre-create".to_string());
    keys.push("post-create".to_string());
    keys
}
