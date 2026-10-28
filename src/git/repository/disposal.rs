//! Inventory of authorized worktree disposal, shared by inspection, clearing,
//! and the trash drain. Trash entries are disposable. A transaction container
//! under `wt/removing/` enters disposal only after its complete registration
//! moves out of Git by the atomic unregister commit. These owned containers
//! are separate from recovery checkout roots in `wt/retained/`: directories
//! inside a user's checkout never authorize disposal.

use std::path::{Path, PathBuf};

use anyhow::Context as _;

use super::Repository;
use crate::path::format_path_for_display;

pub(in crate::git) const CHECKOUT: &str = "checkout";
pub(in crate::git) const REGISTRATION: &str = "registration";

pub struct DisposalEntry {
    pub path: PathBuf,
    /// Do not follow symlinks when inspecting or choosing how to remove an entry.
    pub metadata: std::fs::Metadata,
}

impl Repository {
    /// Containers reserved before unregister commits.
    pub(crate) fn worktree_staging_dir(&self) -> PathBuf {
        self.wt_dir().join("removing")
    }

    /// Put committed disposal in trash before generic recursive cleanup. There,
    /// partial deletion cannot erase the registration that authorized cleanup
    /// and hide the remaining checkout from later inventory.
    pub fn prepare_disposal(&self, path: &Path) -> anyhow::Result<PathBuf> {
        let trash = self.wt_trash_dir();
        if path.parent() == Some(trash.as_path()) {
            return Ok(path.to_path_buf());
        }
        anyhow::ensure!(
            path.parent() == Some(self.worktree_staging_dir().as_path())
                && std::fs::symlink_metadata(path.join(REGISTRATION))?.is_dir(),
            "Not a committed removal @ {}",
            format_path_for_display(path)
        );
        std::fs::create_dir_all(&trash).with_context(|| {
            format!(
                "Failed to prepare trash @ {}",
                format_path_for_display(&trash)
            )
        })?;
        let destination = trash.join(path.file_name().context("Disposal path has no name")?);
        renamore::rename_exclusive_fallback(path, &destination).with_context(|| {
            format!(
                "Failed to prepare cleanup of committed removal @ {}",
                format_path_for_display(path)
            )
        })?;
        Ok(destination)
    }

    /// Every entry authorized for disposal. Missing entries can race with
    /// cleanup; other read errors fail the inventory before a caller deletes.
    pub fn disposal_entries(&self) -> anyhow::Result<Vec<DisposalEntry>> {
        let mut entries = Vec::new();
        for (directory, needs_registration) in [
            (self.wt_trash_dir(), false),
            (self.worktree_staging_dir(), true),
        ] {
            let context = || {
                format!(
                    "Failed to read disposal @ {}",
                    format_path_for_display(&directory)
                )
            };
            match std::fs::symlink_metadata(&directory) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                result => {
                    let metadata = result.with_context(context)?;
                    anyhow::ensure!(
                        metadata.is_dir(),
                        "Disposal path is not a directory @ {}",
                        format_path_for_display(&directory)
                    );
                }
            }
            for entry in std::fs::read_dir(&directory).with_context(context)? {
                let entry = entry.with_context(context)?;
                let path = entry.path();
                let metadata = match std::fs::symlink_metadata(&path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                    result => result.with_context(|| {
                        format!(
                            "Failed to inspect disposal @ {}",
                            format_path_for_display(&path)
                        )
                    })?,
                };
                if needs_registration {
                    if !metadata.is_dir() {
                        continue;
                    }
                    match std::fs::symlink_metadata(path.join(REGISTRATION)) {
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                        result => {
                            if !result.with_context(context)?.is_dir() {
                                continue;
                            }
                        }
                    }
                }
                entries.push(DisposalEntry { path, metadata });
            }
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestRepo;

    #[test]
    fn disposal_requires_a_committed_owned_container() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        let staging = repo.worktree_staging_dir();
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("stray"), "not a transaction").unwrap();
        let pending = staging.join("pending");
        std::fs::create_dir(&pending).unwrap();
        std::fs::write(pending.join(CHECKOUT), "recoverable checkout").unwrap();
        assert!(repo.prepare_disposal(&pending).is_err());
        std::fs::write(pending.join(REGISTRATION), "not a Git registration").unwrap();
        assert!(repo.prepare_disposal(&pending).is_err());
        assert!(repo.disposal_entries().unwrap().is_empty());
        assert_eq!(
            std::fs::read_to_string(pending.join(CHECKOUT)).unwrap(),
            "recoverable checkout"
        );
    }

    #[test]
    fn disposal_inventory_refuses_partial_reads() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();
        let trash = repo.wt_trash_dir();
        std::fs::create_dir_all(&trash).unwrap();
        let disposable = trash.join("disposable");
        std::fs::write(&disposable, "committed disposal").unwrap();
        // The first root is readable; an invalid second root must fail the
        // entire inventory instead of returning the disposable subset.
        std::fs::write(repo.worktree_staging_dir(), "invalid inventory root").unwrap();
        assert!(repo.disposal_entries().is_err());
        assert!(disposable.is_file());
    }
}
