//! Machine-readable output for commands with a `--format=json` mode.

use anyhow::Context;
use worktrunk::styling::println;

/// Serialize a JSON answer to stdout (pretty, one trailing newline).
///
/// Prints through anstream's `println!`, which drops a `BrokenPipe` write
/// error where std's panics — a consumer that stops reading (`wt … | head`)
/// must not fail the command.
pub(crate) fn print_json<T: serde::Serialize>(value: &T) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(value).context("Failed to serialize to JSON")?;
    println!("{}", json);
    Ok(())
}
