//! OS integration for the picker's row shortcuts: copy a branch name to the
//! system clipboard (`alt-y`) and open a row's PR/MR URL in the browser
//! (`alt-o`).
//!
//! Both run on a background thread off skim's event loop, and neither can write
//! to the picker frame (skim owns the terminal), so the caller logs a failure
//! rather than surfacing it — see `PickerCollector`'s copy/open verbs.

use anyhow::Context;
use std::io;
use std::process::Stdio;

use worktrunk::trace::CommandTrace;

/// Copy `text` to the system clipboard.
///
/// macOS, Windows, and Wayland persist the copy after `wt` exits. On Linux/X11
/// the selection is served by `arboard` only while this process holds it, so a
/// copy survives past exit only if a clipboard manager captured it — the same
/// caveat as `xclip`/`xsel` without `-loops`. The picker is short-lived, so the
/// copy is meant to be pasted promptly regardless.
pub(super) fn copy_to_clipboard(text: &str) -> anyhow::Result<()> {
    let mut clipboard = arboard::Clipboard::new().context("Failed to open the system clipboard")?;
    clipboard
        .set_text(text.to_owned())
        .context("Failed to copy to the system clipboard")
}

/// Open `url` in the user's default browser via the OS opener (`open` on macOS,
/// `xdg-open` on Linux). Wait for the launcher, which normally detaches the
/// browser. Retry another launcher only when spawning or waiting fails.
pub(super) fn open_url(url: &str) -> anyhow::Result<()> {
    let launch = || {
        let mut last_error = None;
        for mut command in open::commands(url) {
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let mut trace = CommandTrace::new(None, &format!("{command:?}"));
            match worktrunk::shell_exec::spawn(&mut command).and_then(|mut child| child.wait()) {
                Ok(status) => {
                    trace.complete(status.success());
                    return if status.success() {
                        Ok(())
                    } else {
                        Err(io::Error::other(format!(
                            "Launcher {command:?} failed with {status:?}"
                        )))
                    };
                }
                Err(error) => {
                    trace.fail(&error);
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| io::Error::other("No browser launcher available")))
    };
    launch().with_context(|| format!("Failed to open {url}"))
}
