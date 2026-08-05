//! Machine-readable output for commands with a `--format=json` mode.

use anyhow::Context;

/// Serialize a JSON answer to stdout (pretty, one trailing newline).
pub(crate) fn print_json<T: serde::Serialize>(value: &T) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(value).context("Failed to serialize to JSON")?;
    println!("{}", json);
    Ok(())
}
