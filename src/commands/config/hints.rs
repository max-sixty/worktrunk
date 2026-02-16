//! Hint management commands.
//!
//! Commands for viewing and clearing shown hints.
//!
//! Hints are stored in VCS-local config (git config or jj repo config).

use color_print::cformat;
use worktrunk::styling::{eprintln, info_message, println, success_message};
use worktrunk::workspace::open_workspace;

/// Handle the hints get command (list shown hints)
pub fn handle_hints_get() -> anyhow::Result<()> {
    let workspace = open_workspace()?;
    let hints = workspace.list_shown_hints();

    if hints.is_empty() {
        eprintln!("{}", info_message("No hints have been shown"));
    } else {
        for hint in hints {
            println!("{hint}");
        }
    }

    Ok(())
}

/// Handle the hints clear command
pub fn handle_hints_clear(name: Option<String>) -> anyhow::Result<()> {
    let workspace = open_workspace()?;

    match name {
        Some(hint_name) => {
            let cleared = workspace.clear_hint(&hint_name)?;
            let msg = if cleared {
                success_message(cformat!("Cleared hint <bold>{hint_name}</>"))
            } else {
                info_message(cformat!("Hint <bold>{hint_name}</> was not set"))
            };
            eprintln!("{msg}");
        }
        None => {
            let cleared = workspace.clear_all_hints()?;
            let msg = if cleared == 0 {
                info_message("No hints to clear")
            } else {
                let suffix = if cleared == 1 { "" } else { "s" };
                success_message(cformat!("Cleared <bold>{cleared}</> hint{suffix}"))
            };
            eprintln!("{msg}");
        }
    }

    Ok(())
}
