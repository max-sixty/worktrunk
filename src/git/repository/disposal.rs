//! Inventory of authorized worktree disposal, shared by inspection, clearing,
//! and the age-based janitor. Trash entries are disposable. A retained staging
//! container enters disposal only when it holds the complete registration moved
//! out of Git by the atomic unregister commit. Recoverable checkouts stay out.

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
    /// Containers reserved by removal, distinct from older retained checkout
    /// payloads whose own files cannot authorize disposal.
    pub(crate) fn worktree_staging_dir(&self) -> PathBuf {
        self.wt_dir().join("retained/removing")
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
                let staged_at = entry.file_name().to_str().and_then(disposal_timestamp);
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
                    match std::fs::symlink_metadata(path.join("registration")) {
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                        result => {
                            if !result.with_context(context)?.is_dir() {
                                continue;
                            }
                        }
                    }
                }
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
