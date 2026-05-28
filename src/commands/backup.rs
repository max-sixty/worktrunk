//! Atomic no-overwrite backup of a path blocking a `--clobber` move.
//!
//! Both `wt switch --clobber` and `wt step relocate --clobber` need to move a
//! stale or blocking path aside before they can take its place. They share this
//! helper so the two paths behave identically: the backup name and the atomic
//! no-overwrite rename are defined once.
//!
//! # Why an atomic rename
//!
//! The backup name is only second-resolution (`.bak.<YYYYmmdd-HHMMSS>`), so an
//! `exists()` check followed by a rename would race: another process could
//! create that path in the gap. [`renamore::rename_exclusive`] closes the gap
//! — an atomic no-overwrite rename (`renameat2(RENAME_NOREPLACE)` on Linux,
//! `renamex_np(RENAME_EXCL)` on macOS, `MoveFileExW` on Windows) that fails
//! closed rather than overwriting an existing backup. `std::fs::rename`
//! silently replaces an existing file or empty directory and cannot be used
//! here. A name collision is not fatal: the move counts up (`…-2`, `…-3`, …)
//! until it lands on a free name.

use std::path::{Path, PathBuf};

use worktrunk::path::format_path_for_display;

/// Generate a backup path for the given path with a timestamp suffix.
///
/// For paths with extensions: `file.txt` → `file.txt.bak.TIMESTAMP`
/// For paths without extensions: `foo` → `foo.bak.TIMESTAMP`
///
/// Returns an error for unusual paths without a file name (e.g., `/` or `..`).
fn generate_backup_path(path: &Path, suffix: &str) -> anyhow::Result<PathBuf> {
    let file_name = path.file_name().ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot generate backup path for {}",
            format_path_for_display(path)
        )
    })?;

    if path.extension().is_none() {
        // Path has no extension (e.g., /repo/feature)
        Ok(path.with_file_name(format!("{}.bak.{suffix}", file_name.to_string_lossy())))
    } else {
        // Path has an extension (e.g., /repo.feature or /file.txt)
        Ok(path.with_extension(format!(
            "{}.bak.{suffix}",
            path.extension()
                .map(|e| e.to_string_lossy().to_string())
                .unwrap_or_default()
        )))
    }
}

/// Move `blocking_path` aside to a `.bak.<base_suffix>` sibling.
///
/// If that name is already taken — a same-second clobber, or a path that raced
/// in after planning — it counts up (`…-2`, `…-3`, …) until it finds a free
/// name. Every attempt is an atomic no-overwrite rename
/// ([`renamore::rename_exclusive`]), so an existing backup is never overwritten;
/// the move just lands on the next free name. Returns the path the blocking
/// directory was moved to.
///
/// `base_suffix` is a parameter rather than computed internally so tests can
/// pass a fixed value; [`back_up_clobbered_path_now`] is the production entry
/// point that derives the timestamp.
fn back_up_clobbered_path(blocking_path: &Path, base_suffix: &str) -> anyhow::Result<PathBuf> {
    // Count up until a free name is found. This cannot spin forever: a
    // directory holds finitely many entries, so some `-N` is always unused.
    let mut n: u64 = 1;
    loop {
        // First attempt uses the bare suffix; later ones disambiguate with -N.
        let suffix = if n == 1 {
            base_suffix.to_string()
        } else {
            format!("{base_suffix}-{n}")
        };
        let candidate = generate_backup_path(blocking_path, &suffix)?;
        match renamore::rename_exclusive(blocking_path, &candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(err) => {
                return Err(anyhow::Error::new(err).context(format!(
                    "Failed to move {} to {}",
                    format_path_for_display(blocking_path),
                    format_path_for_display(&candidate),
                )));
            }
        }
    }
}

/// Move `blocking_path` aside to a timestamped `.bak.<YYYYmmdd-HHMMSS>` sibling.
///
/// Wraps [`back_up_clobbered_path`] with the timestamp suffix computed at move
/// time, so the suffix reflects when the path is actually set aside. Returns the
/// path the blocking directory was moved to.
pub(crate) fn back_up_clobbered_path_now(blocking_path: &Path) -> anyhow::Result<PathBuf> {
    let timestamp_secs = worktrunk::utils::epoch_now() as i64;
    let datetime =
        chrono::DateTime::from_timestamp(timestamp_secs, 0).unwrap_or_else(chrono::Utc::now);
    let base_suffix = datetime.format("%Y%m%d-%H%M%S").to_string();
    back_up_clobbered_path(blocking_path, &base_suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_backup_path_with_extension() {
        // Paths with extensions: file.txt -> file.txt.bak.TIMESTAMP
        let path = PathBuf::from("/tmp/repo.feature");
        let backup = generate_backup_path(&path, "20250101-000000").unwrap();
        assert_eq!(
            backup,
            PathBuf::from("/tmp/repo.feature.bak.20250101-000000")
        );

        let path = PathBuf::from("/tmp/file.txt");
        let backup = generate_backup_path(&path, "20250101-000000").unwrap();
        assert_eq!(backup, PathBuf::from("/tmp/file.txt.bak.20250101-000000"));
    }

    #[test]
    fn test_generate_backup_path_without_extension() {
        // Paths without extensions: foo -> foo.bak.TIMESTAMP
        let path = PathBuf::from("/tmp/repo/feature");
        let backup = generate_backup_path(&path, "20250101-000000").unwrap();
        assert_eq!(
            backup,
            PathBuf::from("/tmp/repo/feature.bak.20250101-000000")
        );

        let path = PathBuf::from("/tmp/mydir");
        let backup = generate_backup_path(&path, "20250101-000000").unwrap();
        assert_eq!(backup, PathBuf::from("/tmp/mydir.bak.20250101-000000"));
    }

    #[test]
    fn test_generate_backup_path_unusual_paths() {
        // Root path has no file name
        let path = PathBuf::from("/");
        assert!(generate_backup_path(&path, "20250101-000000").is_err());

        // Parent reference has no file name
        let path = PathBuf::from("..");
        assert!(generate_backup_path(&path, "20250101-000000").is_err());
    }

    #[test]
    fn test_back_up_clobbered_path_moves_to_fresh_suffix() {
        let temp = tempfile::tempdir().unwrap();
        let stale = temp.path().join("feature");
        std::fs::create_dir(&stale).unwrap();
        std::fs::write(stale.join("file"), "content").unwrap();

        let used = back_up_clobbered_path(&stale, "20250101-000000").unwrap();

        assert_eq!(used, temp.path().join("feature.bak.20250101-000000"));
        assert!(!stale.exists(), "stale path should be moved away");
        assert_eq!(
            std::fs::read_to_string(used.join("file")).unwrap(),
            "content"
        );
    }

    #[test]
    fn test_back_up_clobbered_path_falls_back_when_suffix_taken() {
        let temp = tempfile::tempdir().unwrap();
        let stale = temp.path().join("feature");
        std::fs::create_dir(&stale).unwrap();

        // The preferred backup name and its first -N variant are both taken.
        let taken = temp.path().join("feature.bak.20250101-000000");
        std::fs::create_dir(&taken).unwrap();
        std::fs::write(taken.join("keep"), "pre-existing").unwrap();
        std::fs::create_dir(temp.path().join("feature.bak.20250101-000000-2")).unwrap();

        let used = back_up_clobbered_path(&stale, "20250101-000000").unwrap();

        // Lands on -3; neither pre-existing backup is overwritten.
        assert_eq!(used, temp.path().join("feature.bak.20250101-000000-3"));
        assert!(!stale.exists());
        assert_eq!(
            std::fs::read_to_string(taken.join("keep")).unwrap(),
            "pre-existing"
        );
    }

    #[test]
    fn test_back_up_clobbered_path_errors_when_source_missing() {
        // A missing source fails the rename with a non-AlreadyExists error,
        // which surfaces (with the "Failed to move" context) rather than being
        // retried.
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("does-not-exist");
        let err = back_up_clobbered_path(&missing, "20250101-000000").unwrap_err();
        assert!(
            err.to_string().contains("Failed to move"),
            "expected wrapped error, got: {err}"
        );
    }

    #[test]
    fn test_back_up_clobbered_path_keeps_incrementing_past_many_collisions() {
        // There is no attempt cap: the move keeps counting up until a free
        // name is found, however many backups already exist.
        let temp = tempfile::tempdir().unwrap();
        let stale = temp.path().join("feature");
        std::fs::create_dir(&stale).unwrap();

        // Occupy the preferred name and the first 49 -N fallbacks (suffix "S").
        std::fs::create_dir(temp.path().join("feature.bak.S")).unwrap();
        for n in 2..=50 {
            std::fs::create_dir(temp.path().join(format!("feature.bak.S-{n}"))).unwrap();
        }

        let used = back_up_clobbered_path(&stale, "S").unwrap();

        assert_eq!(used, temp.path().join("feature.bak.S-51"));
        assert!(!stale.exists(), "stale path should be moved away");
    }

    #[test]
    fn test_back_up_clobbered_path_now_uses_timestamped_suffix() {
        let temp = tempfile::tempdir().unwrap();
        let stale = temp.path().join("feature");
        std::fs::create_dir(&stale).unwrap();

        let used = back_up_clobbered_path_now(&stale).unwrap();

        let name = used.file_name().unwrap().to_string_lossy();
        assert!(
            name.starts_with("feature.bak."),
            "expected timestamped backup name, got: {name}"
        );
        assert!(!stale.exists(), "stale path should be moved away");
    }
}
