//! General utilities.

use std::borrow::Cow;
use std::io::{self, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Replace C0/C1 control characters — other than tab and newline — with a
/// visible escape so the text stays valid for plain-text and markdown sinks.
///
/// Raw subprocess output can carry control bytes, most commonly NUL from
/// `-z`/`--null` git invocations (`git config --list -z`,
/// `git for-each-ref --format=…%00…`). Left in place they ride into the
/// human-facing `trace.log` and the `diagnostic.md` bug-report bundle, where
/// the content-type sniffing behind `gh gist create` flags the file as binary
/// ("binary file not supported") and refuses to upload it.
///
/// Tab and newline are preserved — they're legitimate text structure (git
/// output is full of tabs; `trace.log` and `diagnostic.md` are multi-line).
/// NUL renders as the compact `\0` rather than `escape_default`'s `\u{0}`
/// because it's by far the most common byte here. The function is idempotent:
/// its own output carries no control bytes, so a sink can re-sanitize content
/// another sink already cleaned without doubling the escapes.
///
/// Borrows when there's nothing to escape (the common case), so the hot logging
/// path allocates only for lines that actually carry control bytes.
pub fn escape_controls(s: &str) -> Cow<'_, str> {
    if !s.chars().any(|c| c.is_control() && c != '\t' && c != '\n') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\t' | '\n' => out.push(c),
            '\0' => out.push_str(r"\0"),
            c if c.is_control() => out.extend(c.escape_default()),
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// Format a Unix timestamp as ISO 8601 UTC (e.g., "2025-01-01T00:00:00Z"),
/// or `None` when the timestamp is out of chrono's representable range.
///
/// The one ISO 8601 formatter wt has — diagnostics, logs, and schema-2
/// `wt list --format=json` all derive from it.
pub fn format_timestamp_iso8601_opt(timestamp: i64) -> Option<String> {
    chrono::DateTime::from_timestamp(timestamp, 0)
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

/// Format a Unix timestamp as ISO 8601 string (e.g., "2025-01-01T00:00:00Z").
///
/// Used for human-readable timestamps in diagnostic reports and logs.
///
/// If the timestamp is out of range for chrono's date handling, returns an
/// explicit placeholder string rather than a misleading value.
pub fn format_timestamp_iso8601(timestamp: u64) -> String {
    i64::try_from(timestamp)
        .ok()
        .and_then(format_timestamp_iso8601_opt)
        .unwrap_or_else(|| format!("invalid-timestamp({timestamp})"))
}

/// Format the current time as ISO 8601 string.
///
/// Convenience function combining `epoch_now()` and `format_timestamp_iso8601()`.
pub fn now_iso8601() -> String {
    format_timestamp_iso8601(epoch_now())
}

/// Get current Unix timestamp in seconds.
///
/// When `WORKTRUNK_TEST_EPOCH` environment variable is set (by tests), returns that
/// value instead of the actual current time. This enables deterministic test
/// snapshots.
///
/// Note: We use `WORKTRUNK_TEST_EPOCH` rather than `SOURCE_DATE_EPOCH` because the
/// latter is a build-time standard for reproducible builds, commonly set by
/// NixOS/direnv in development shells. Using it at runtime causes incorrect
/// age display. See: <https://github.com/max-sixty/worktrunk/issues/763>
///
/// All code that needs timestamps for display or storage should use this
/// function rather than `SystemTime::now()` directly.
pub fn epoch_now() -> u64 {
    std::env::var("WORKTRUNK_TEST_EPOCH")
        .ok()
        .and_then(|val| val.parse::<u64>().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before Unix epoch")
                .as_secs()
        })
}

/// Replace `path`'s contents in one step: write a sibling temp file, sync it,
/// then rename it over the target.
///
/// `fs::write` truncates in place, so a crash, a full disk, or a lost power
/// cable between the truncate and the write leaves the file empty or
/// half-written. Every file that reaches this function is one a torn write
/// breaks — rc files and shell wrappers the user's shell sources at startup,
/// `config.toml`, `approvals.toml`, another tool's `settings.json` — so it ends
/// up either entirely the old version or entirely the new one. Regenerable
/// content keeps the plain write ([`crate::cache::write_json`], the `-vv`
/// diagnostic report): a torn write there costs a recompute, not the user's
/// data.
///
/// Three details a bare `NamedTempFile::new()` + `persist()` gets wrong:
///
/// - **Symlinks.** Dotfile managers (Mackup, chezmoi, stow) symlink `~/.zshrc`
///   or `~/.config/worktrunk` into a repo or a synced folder. `fs::write`
///   follows the link and updates the real file; a rename would replace the
///   link itself with a regular file and silently detach the user's setup. The
///   target is resolved first, so the rename lands on the real file and the
///   link survives. A dangling link has nothing to resolve to, so it is
///   replaced by a regular file rather than followed to the missing path it
///   names.
/// - **Same directory.** A temp file elsewhere (`/tmp`) means `persist` crosses
///   a filesystem boundary, where it degrades to a copy — no longer atomic, and
///   an outright failure on some setups. The temp file is created beside the
///   resolved target.
/// - **Mode.** A fresh temp file is `0600`. An existing target's mode is copied
///   onto the temp file before the rename, so permissions survive the
///   replacement. A file that doesn't exist yet keeps the `0600`, narrower than
///   the `0644` a truncating write leaves under a default umask; every file
///   written here is read by its own owner. Windows has no equivalent to carry.
///
/// The target becomes a new inode, so ownership resets to the running user (a
/// root-run rewrite of someone else's file hands it to root) and any hardlink
/// to it keeps the old contents. The `atomic-write-file` crate carries owner
/// and mode across, but replaces a symlink instead of writing through it,
/// leaving the canonicalize step here either way; `atomicwrites` does neither.
///
/// The temp file needs a writable parent directory, which a truncating write
/// did not, so a read-only directory holding a writable file now fails with the
/// file left as it was — the trade Data Safety asks for.
pub fn write_atomically(path: &Path, content: &str) -> io::Result<()> {
    write_atomically_with(path, content, true)
}

/// Create `path` atomically, failing if another writer creates it first.
///
/// Use this when the caller observed that the path was absent and must not
/// overwrite content that appears before the temp file is persisted. A
/// dangling symlink already occupies the path, so this returns `AlreadyExists`
/// instead of replacing the link as [`write_atomically`] does.
pub fn write_new_atomically(path: &Path, content: &str) -> io::Result<()> {
    write_atomically_with(path, content, false)
}

fn write_atomically_with(path: &Path, content: &str, overwrite: bool) -> io::Result<()> {
    let target = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    // A bare filename's parent is `Some("")`, which no directory call accepts.
    let dir = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let mut temp = tempfile::Builder::new()
        .prefix(".wt-tmp.")
        .suffix(".tmp")
        .tempfile_in(dir)?;
    temp.write_all(content.as_bytes())?;
    temp.flush()?;

    // Before the sync, so the mode lands on disk with the contents.
    #[cfg(unix)]
    if let Ok(metadata) = std::fs::metadata(&target) {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode();
        temp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(mode))?;
    }

    temp.as_file().sync_all()?;
    if overwrite {
        temp.persist(&target).map_err(|e| e.error)?;
    } else {
        temp.persist_noclobber(&target).map_err(|e| e.error)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_atomically_creates_then_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        write_atomically(&path, "first\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first\n");

        write_atomically(&path, "second\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second\n");

        // The rename leaves no temp file behind.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn test_write_new_atomically_creates_but_never_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");

        write_new_atomically(&path, "first\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first\n");

        assert!(write_new_atomically(&path, "second\n").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first\n");

        // A failed persist removes its temp file.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    /// A file with no target to copy a mode from keeps the temp file's `0600`,
    /// narrower than the `0644` a truncating write leaves under a default
    /// umask.
    #[cfg(unix)]
    #[test]
    fn test_write_atomically_creates_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.toml");
        write_atomically(&path, "x\n").unwrap();

        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn test_epoch_now_returns_reasonable_timestamp() {
        let now = epoch_now();
        // Should be after 2020-01-01
        assert!(now > 1577836800, "epoch_now() should return current time");
    }

    #[test]
    fn test_epoch_now_respects_test_epoch() {
        // When WORKTRUNK_TEST_EPOCH is set (by test harness), epoch_now() returns it
        if let Ok(epoch) = std::env::var("WORKTRUNK_TEST_EPOCH") {
            let expected: u64 = epoch.parse().unwrap();
            assert_eq!(epoch_now(), expected);
        }
    }

    #[test]
    fn test_format_timestamp_iso8601_u64_overflow() {
        // Timestamps exceeding i64::MAX are handled by try_from
        let too_large = (i64::MAX as u64) + 1;
        let formatted = format_timestamp_iso8601(too_large);
        assert!(formatted.starts_with("invalid-timestamp("));
    }

    #[test]
    fn test_format_timestamp_iso8601_chrono_out_of_range() {
        // Timestamps within i64 but beyond chrono's range (~year 262143)
        let chrono_out_of_range: u64 = 9_000_000_000_000; // ~year 287396
        let formatted = format_timestamp_iso8601(chrono_out_of_range);
        assert!(formatted.starts_with("invalid-timestamp("));
    }

    #[test]
    fn escape_controls_borrows_clean_text() {
        // No control bytes (tab/newline aside) → borrowed, untouched.
        let out = escape_controls("plain text");
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, "plain text");
    }

    #[test]
    fn escape_controls_preserves_tab_and_newline() {
        // Tab and newline are legitimate structure and must survive verbatim
        // (git output is full of tabs; trace.log / diagnostic.md are multi-line).
        let out = escape_controls("a\tb\nc");
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, "a\tb\nc");
    }

    #[test]
    fn escape_controls_renders_nul_compactly() {
        // NUL (the `-z`/`--null` record separator) → `\0`, not `\u{0}`.
        assert_eq!(
            escape_controls("core.bare\0false\0core.filemode\0true"),
            r"core.bare\0false\0core.filemode\0true"
        );
    }

    #[test]
    fn escape_controls_escapes_other_control_bytes() {
        // ESC (0x1b) and BEL (0x07) take the general `escape_default` arm.
        assert_eq!(escape_controls("a\x1bb"), r"a\u{1b}b");
        assert_eq!(escape_controls("a\x07b"), r"a\u{7}b");
    }

    #[test]
    fn escape_controls_is_idempotent() {
        // Re-escaping already-escaped text is a no-op: the escapes carry no
        // control bytes, so diagnostic.md assembly can re-sanitize a trace.log
        // the formatter already cleaned without doubling the backslashes.
        let once = escape_controls("x\0y\x1bz").into_owned();
        assert_eq!(escape_controls(&once), once);
    }
}
