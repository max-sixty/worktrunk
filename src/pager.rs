use worktrunk::git::Repository;

/// Parse a pager value, treating empty strings and "cat" as "no pager".
pub(crate) fn parse_pager_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty() && trimmed != "cat").then(|| trimmed.to_string())
}

/// Read `core.pager` from git config, returning None if unset or invalid.
pub(crate) fn git_config_pager() -> Option<String> {
    let repo = Repository::current().ok()?;
    repo.run_command(&["config", "--get", "core.pager"])
        .ok()
        .and_then(|output| parse_pager_value(&output))
}
