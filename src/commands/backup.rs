//! Atomic no-overwrite backup of a path blocking a `--clobber` move.
//!
//! Both `wt switch --clobber` and `wt step relocate --clobber` need to move a
//! stale or blocking path aside before they can take its place. They share this
//! helper so the two paths share Git-storage/worktree protection, backup names,
//! and the atomic no-overwrite rename.
//!
//! # Why an atomic rename
//!
//! The backup name is only second-resolution (`.bak.<YYYYmmdd-HHMMSS>`), so an
//! `exists()` check followed by a rename would race: another process could
//! create that path in the gap. [`renamore::rename_exclusive_fallback`] closes
//! the gap — an atomic no-overwrite rename (`renameat2(RENAME_NOREPLACE)` on
//! Linux, `renamex_np(RENAME_EXCL)` on macOS, `MoveFileExW` on Windows) that
//! fails closed rather than overwriting an existing backup. `std::fs::rename`
//! silently replaces an existing file or empty directory and cannot be used
//! here. A name collision is not fatal: the move counts up (`…-2`, `…-3`, …)
//! until it lands on a free name.
//!
//! # Why the `_fallback` variant
//!
//! Some filesystems don't implement the atomic no-overwrite syscall:
//! `renameat2(RENAME_NOREPLACE)` returns `EINVAL` on NFS, overlayfs, FUSE, and
//! older kernels, which surfaces as [`std::io::ErrorKind::Unsupported`]. Using
//! the non-fallback [`renamore::rename_exclusive`] there would abort the
//! clobber outright. [`renamore::rename_exclusive_fallback`] instead degrades
//! to a non-atomic `try_exists()` + `rename` when the atomic path is
//! unsupported; it still returns [`std::io::ErrorKind::AlreadyExists`] on a
//! collision, so the count-up loop is unaffected. The small race window the
//! fallback reintroduces only applies on those filesystems, and is preferable
//! to failing the command.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use color_print::cformat;
use worktrunk::git::Repository;
use worktrunk::path::{canonicalize_with_parents, format_path_for_display};

/// Protect Git storage and registered worktrees before a clobber backup.
/// Explicit switch paths also refuse directories inside worktrees. Template
/// paths may replace stale directories there, but never tracked content.
pub(crate) fn require_safe_clobber_target(
    repo: &Repository,
    requested: &Path,
    explicit_path: bool,
) -> anyhow::Result<()> {
    if !requested.try_exists().with_context(|| {
        format!(
            "Failed to resolve worktree path {}",
            format_path_for_display(requested)
        )
    })? {
        bail!(
            "Failed to resolve worktree path {}: path does not exist",
            format_path_for_display(requested)
        );
    }
    let resolved = canonicalize_with_parents(requested);
    // Rename moves the directory entry, not a symlink's referent. Normalize
    // its parents while retaining the leaf for Git's tracked-entry lookup.
    let absolute = std::path::absolute(requested)?;
    let entry = match (absolute.parent(), absolute.file_name()) {
        (Some(parent), Some(name)) => canonicalize_with_parents(parent).join(name),
        _ => resolved.clone(),
    };
    let git_dir = canonicalize_with_parents(repo.git_common_dir());
    // Execution must not reuse planning's cached registrations.
    let fresh = Repository::at(repo.git_common_dir())?;
    let registered = fresh.list_worktrees()?;
    for requested in std::iter::once(&entry).chain((entry != resolved).then_some(&resolved)) {
        if requested == &git_dir {
            bail!(cformat!(
                "Worktree path <bold>{}</> is the repository's Git directory; choose a directory outside the repository and its worktrees",
                format_path_for_display(requested)
            ));
        }
        for existing in registered
            .iter()
            .map(|wt| wt.path.as_path())
            .chain([git_dir.as_path()])
        {
            let existing = canonicalize_with_parents(existing);
            // Linked and separate-git-dir checkouts use a Git linkage file
            // rather than storing Git data at this path.
            if requested == &existing.join(".git") {
                bail!(cformat!(
                    "Worktree path <bold>{}</> is the worktree's Git entry; choose a different directory",
                    format_path_for_display(requested)
                ));
            }
            let relation = if existing.starts_with(requested) {
                "contains"
            } else if let Ok(relative) = requested.strip_prefix(&existing) {
                if explicit_path || existing == git_dir {
                    "is inside"
                } else {
                    let relative = relative
                        .to_str()
                        .context("Cannot inspect a non-UTF-8 worktree destination")?;
                    // Git's literal lookup needs explicit case folding when
                    // its index treats differently cased paths as the same.
                    let (literal_mode, pathspec) = if fresh.config_bool("core.ignorecase")? {
                        (
                            "--no-literal-pathspecs",
                            format!(":(literal,icase){relative}"),
                        )
                    } else {
                        ("--literal-pathspecs", relative.to_owned())
                    };
                    let output = fresh.worktree_at(&existing).run_command_bytes(&[
                        literal_mode,
                        "ls-files",
                        "-z",
                        "--",
                        &pathspec,
                    ])?;
                    if output.is_empty() {
                        continue;
                    }
                    bail!(cformat!(
                        "Worktree path <bold>{}</> contains tracked files from <bold>{}</>; choose a different directory",
                        format_path_for_display(requested),
                        format_path_for_display(&existing),
                    ));
                }
            } else {
                continue;
            };
            bail!(cformat!(
                "Worktree path <bold>{}</> {relation} <bold>{}</>; choose a directory outside the repository and its worktrees",
                format_path_for_display(requested),
                format_path_for_display(&existing),
            ));
        }
    }
    Ok(())
}

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
/// name. Every attempt is a no-overwrite rename
/// ([`renamore::rename_exclusive_fallback`] — atomic where the filesystem
/// supports it, a non-atomic fallback otherwise), so an existing backup is
/// never overwritten; the move just lands on the next free name. Returns the
/// path the blocking directory was moved to.
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
        // `_fallback` degrades to a non-atomic rename on filesystems without
        // `renameat2(RENAME_NOREPLACE)` (NFS, overlayfs, FUSE, older kernels),
        // where the atomic variant would error with `Unsupported`. The `bool`
        // it returns (whether the atomic path was taken) is irrelevant here.
        match renamore::rename_exclusive_fallback(blocking_path, &candidate) {
            Ok(_) => return Ok(candidate),
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
pub(crate) fn back_up_clobbered_path_now(
    repo: &Repository,
    blocking_path: &Path,
) -> anyhow::Result<PathBuf> {
    require_safe_clobber_target(repo, blocking_path, false)?;
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
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        let err = back_up_clobbered_path_now(&repo, &missing).unwrap_err();
        assert!(err.to_string().contains("Failed to resolve worktree path"));
        let err = back_up_clobbered_path(&missing, "20250101-000000").unwrap_err();
        assert!(
            err.to_string().contains("Failed to move"),
            "expected wrapped error, got: {err}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_backup_preserves_unresolvable_symlink() {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        let link = test.root_path().join("loop");
        std::os::unix::fs::symlink("loop", &link).unwrap();

        let err = back_up_clobbered_path_now(&repo, &link).unwrap_err();
        assert!(err.to_string().contains("Failed to resolve worktree path"));
        assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("loop"));
    }

    #[rstest::rstest]
    #[case::ordinary_directory(false)]
    #[case::git_directory(true)]
    #[cfg(unix)]
    fn test_backup_checks_symlink_referent(#[case] git_directory: bool) {
        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        let target = if git_directory {
            repo.git_common_dir().to_path_buf()
        } else {
            let target = test.home_path().join("outside");
            std::fs::create_dir(&target).unwrap();
            target
        };
        let link = test.home_path().join("alias");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let result = back_up_clobbered_path_now(&repo, &link);
        if git_directory {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("is the repository's Git directory")
            );
            assert_eq!(std::fs::read_link(&link).unwrap(), target);
        } else {
            let backup = result.unwrap();
            assert!(!link.exists());
            assert_eq!(std::fs::read_link(backup).unwrap(), target);
        }
        assert!(target.is_dir());
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

        let test = worktrunk::testing::TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        let used = back_up_clobbered_path_now(&repo, &stale).unwrap();

        let name = used.file_name().unwrap().to_string_lossy();
        assert!(
            name.starts_with("feature.bak."),
            "expected timestamped backup name, got: {name}"
        );
        assert!(!stale.exists(), "stale path should be moved away");
    }
}
