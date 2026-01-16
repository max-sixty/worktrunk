//! Hint management commands.
//!
//! Commands for viewing and clearing shown hints.

use color_print::cformat;
use worktrunk::git::Repository;
use worktrunk::styling::{info_message, success_message};

use crate::output;

/// Handle the hints get command (list shown hints)
pub fn handle_hints_get() -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let hints = repo.list_shown_hints();

    if hints.is_empty() {
        output::print(info_message("No hints have been shown"))?;
    } else {
        for hint in hints {
            output::stdout(&hint)?;
        }
    }

    Ok(())
}

/// Handle the hints clear command
pub fn handle_hints_clear(name: Option<String>) -> anyhow::Result<()> {
    let repo = Repository::current()?;

    match name {
        Some(hint_name) => {
            let msg = if repo.clear_hint(&hint_name)? {
                success_message(cformat!("Cleared hint <bold>{hint_name}</>"))
            } else {
                info_message(cformat!("Hint <bold>{hint_name}</> was not set"))
            };
            output::print(msg)?;
        }
        None => {
            let cleared = repo.clear_all_hints()?;
            let msg = if cleared == 0 {
                info_message("No hints to clear")
            } else {
                let suffix = if cleared == 1 { "" } else { "s" };
                success_message(cformat!("Cleared <bold>{cleared}</> hint{suffix}"))
            };
            output::print(msg)?;
        }
    }

    Ok(())
}
