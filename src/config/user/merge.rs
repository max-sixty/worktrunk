//! Merge trait for configuration layering.
//!
//! Provides the mechanism for project-specific config to override global config,
//! where unset fields fall back to the global value.

/// Trait for merging configuration structs.
///
/// Project-specific config fields override global fields when set.
/// Fields that are `None` in the override fall back to the base value.
pub trait Merge {
    /// Merge with another config, where `other` takes precedence for set fields.
    fn merge_with(&self, other: &Self) -> Self;
}

/// Merge optional global and project configs, returning the effective config.
///
/// - Both set: merge (project takes precedence for set fields)
/// - Only project set: clone project
/// - Only global set: clone global
/// - Neither set: None
pub fn merge_optional<T: Merge + Clone>(global: Option<&T>, project: Option<&T>) -> Option<T> {
    match (global, project) {
        (Some(g), Some(p)) => Some(g.merge_with(p)),
        (None, Some(p)) => Some(p.clone()),
        (Some(g), None) => Some(g.clone()),
        (None, None) => None,
    }
}
