//! Hook filter types for command filtering by source and name.
//!
//! These types are shared between `hooks.rs` (command preparation/execution)
//! and `command_approval.rs` (approval flow). Re-exported from `hooks.rs`
//! for backward compatibility.

/// Distinguishes between user hooks and project hooks for command preparation.
///
/// Approval for project hooks is handled at the gate (command entry point),
/// not during hook execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::Display)]
#[strum(serialize_all = "kebab-case")]
pub enum HookSource {
    /// User hooks from ~/.config/worktrunk/config.toml (no approval required)
    User,
    /// Project hooks from .config/wt.toml (approval handled at gate)
    Project,
}

/// A parsed name filter, optionally scoped to a specific source.
///
/// Supports formats:
/// - `"foo"` - matches commands named "foo" from any source
/// - `"user:foo"` - matches only user's command named "foo"
/// - `"project:foo"` - matches only project's command named "foo"
/// - `"user:"` or `"project:"` - matches all commands from that source
pub struct ParsedFilter<'a> {
    pub source: Option<HookSource>,
    pub name: &'a str,
}

impl<'a> ParsedFilter<'a> {
    pub fn parse(filter: &'a str) -> Self {
        if let Some(name) = filter.strip_prefix("user:") {
            Self {
                source: Some(HookSource::User),
                name,
            }
        } else if let Some(name) = filter.strip_prefix("project:") {
            Self {
                source: Some(HookSource::Project),
                name,
            }
        } else {
            Self {
                source: None,
                name: filter,
            }
        }
    }

    /// Check if this filter matches the given source.
    pub(crate) fn matches_source(&self, source: HookSource) -> bool {
        self.source.is_none() || self.source == Some(source)
    }
}
