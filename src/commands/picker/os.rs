//! OS integration for the picker's row shortcuts: copy a branch name to the
//! system clipboard (`alt-y`) and open a row's PR/MR URL in the browser
//! (`alt-o`).
//!
//! Both run on a background thread off skim's event loop, and neither can write
//! to the picker frame (skim owns the terminal), so the caller logs a failure
//! rather than surfacing it — see `PickerCollector`'s copy/open verbs.

use anyhow::Context;

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
/// `xdg-open` on Linux). Fire-and-forget: the opener detaches, so this returns
/// as soon as it's launched.
pub(super) fn open_url(url: &str) -> anyhow::Result<()> {
    open::that(url).with_context(|| format!("Failed to open {url}"))
}
