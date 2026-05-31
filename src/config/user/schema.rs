//! Schema helpers for config validation.
//!
//! Uses JsonSchema to derive valid top-level keys, feeding
//! `WorktrunkConfig::is_valid_key` so unknown-key classification can tell
//! "belongs in the other config" from "truly unknown."

use super::UserConfig;

/// All valid top-level keys in user config (UserConfig + flattened HooksConfig),
/// derived from the JsonSchema via `config::schema_top_level_keys`.
/// Public for use by the `WorktrunkConfig` trait implementation.
pub fn valid_user_config_keys() -> Vec<String> {
    crate::config::schema_top_level_keys::<UserConfig>()
}
