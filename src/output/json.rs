//! JSON serialization and partial mutation results.
//!
//! Inner operations return typed facts alongside an original failure when work
//! has already completed. Command handlers serialize their final result before
//! returning the error to main for diagnostics and native exit handling.
//! CommandFailure deliberately is not an Error: an ordinary `?` into anyhow
//! must not silently discard completed work.

use anyhow::Context;
use worktrunk::styling::println;

pub(crate) type CommandResult<T> = Result<T, CommandFailure<T>>;

#[derive(Debug)]
pub(crate) struct CommandFailure<T> {
    // Partial facts can be large; keep the error small without boxing successful results.
    pub(crate) result: Option<Box<T>>,
    pub(crate) error: anyhow::Error,
}

impl<T> CommandFailure<T> {
    pub(crate) fn with_result(error: impl Into<anyhow::Error>, result: T) -> Self {
        Self {
            result: Some(Box::new(result)),
            error: error.into(),
        }
    }

    pub(crate) fn map_result<U>(self, map: impl FnOnce(T) -> U) -> CommandFailure<U> {
        CommandFailure {
            result: self.result.map(|result| Box::new(map(*result))),
            error: self.error,
        }
    }
}

impl<T, E: Into<anyhow::Error>> From<E> for CommandFailure<T> {
    fn from(error: E) -> Self {
        Self {
            result: None,
            error: error.into(),
        }
    }
}

/// Finish an outer command, emitting its available result in JSON mode.
pub(crate) fn finish_output<T: serde::Serialize>(
    outcome: CommandResult<T>,
    json: bool,
) -> anyhow::Result<()> {
    let (result, error) = match outcome {
        Ok(result) => (Some(result), None),
        Err(failure) => (failure.result.map(|result| *result), Some(failure.error)),
    };
    if json && let Some(result) = result {
        print_json(&result)?;
    }
    match error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

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
