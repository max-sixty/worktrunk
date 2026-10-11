//! Inventory of authorized worktree disposal, shared by inspection, clearing,
//! and the age-based janitor. Retained checkouts and live registrations never
//! enter it. Payload trash owns every direct entry; the Git common directory
//! owns only the exact metadata prefix with a valid timestamp suffix.

use std::path::PathBuf;

use anyhow::Context as _;

use super::Repository;
use crate::path::format_path_for_display;

pub struct DisposalEntry {
    pub path: PathBuf,
    /// Do not follow symlinks when inspecting or choosing how to remove an entry.
    pub metadata: std::fs::Metadata,
    /// Filename-encoded staging time. Stray payload-trash files have none.
    pub staged_at: Option<u64>,
}

impl Repository {
    /// Temporary metadata disposal directories directly in the Git common dir.
    /// Only complete, unregistered admin directories enter this namespace.
    pub const UNREGISTERED_WORKTREE_PREFIX: &str = "worktrunk-unregistered-";

    /// Every entry authorized for disposal. Missing entries can race with
    /// cleanup; other read errors fail the inventory before a caller deletes.
    pub fn disposal_entries(&self) -> anyhow::Result<Vec<DisposalEntry>> {
        let mut entries = Vec::new();
        for (directory, metadata_only) in [
            (self.wt_trash_dir(), false),
            (self.git_common_dir().to_path_buf(), true),
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
                let name = entry.file_name();
                let staged_at = name.to_str().and_then(disposal_timestamp);
                if metadata_only
                    && !(name
                        .to_str()
                        .is_some_and(|name| name.starts_with(Self::UNREGISTERED_WORKTREE_PREFIX))
                        && staged_at.is_some())
                {
                    continue;
                }
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
                entries.push(DisposalEntry {
                    path,
                    metadata,
                    staged_at,
                });
            }
        }
        Ok(entries)
    }
}

fn disposal_timestamp(name: &str) -> Option<u64> {
    let (_, suffix) = name.rsplit_once('-')?;
    suffix.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disposal_timestamp() {
        assert_eq!(disposal_timestamp("feature-1700000000"), Some(1700000000));
        assert_eq!(
            disposal_timestamp("my-project.feature-branch-1700000000"),
            Some(1700000000)
        );
        assert_eq!(disposal_timestamp("no-timestamp"), None);
        assert_eq!(disposal_timestamp("notimestamp"), None);
        assert_eq!(disposal_timestamp(""), None);
    }
}
