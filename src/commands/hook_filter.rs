//! Hook filter types for command filtering by source and name.

use worktrunk::config::{CommandConfig, HookSource};

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

    /// Check if this filter matches a command from the given source.
    pub(crate) fn matches_command(&self, source: HookSource, name: Option<&str>) -> bool {
        self.matches_source(source)
            && (self.name.is_empty() || name.is_some_and(|n| n == self.name))
    }
}

/// Select commands before template parsing, preparation, or execution.
/// Shared by manual-hook argument routing and pipeline preparation.
pub(crate) fn selected_hook_configs(
    user_config: Option<&CommandConfig>,
    project_config: Option<&CommandConfig>,
    name_filters: &[String],
) -> Vec<(HookSource, CommandConfig)> {
    let filters: Vec<_> = name_filters
        .iter()
        .map(|name| ParsedFilter::parse(name))
        .collect();
    [
        (HookSource::User, user_config),
        (HookSource::Project, project_config),
    ]
    .into_iter()
    .filter_map(|(source, config)| {
        let config = config?.filtered(|command| {
            filters.is_empty()
                || filters
                    .iter()
                    .any(|filter| filter.matches_command(source, command.name.as_deref()))
        });
        let has_commands = config.commands().next().is_some();
        has_commands.then_some((source, config))
    })
    .collect()
}
