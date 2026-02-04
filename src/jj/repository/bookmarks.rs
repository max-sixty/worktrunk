//! Bookmark (branch) operations for Repository.
//!
//! In jj, bookmarks are the equivalent of git branches. They are named pointers
//! to commits that can be used to track development lines.

use super::Repository;

impl Repository {
    /// Get the default bookmark (main, master, trunk, etc.).
    ///
    /// Result is cached in the repository's shared cache.
    pub fn default_bookmark(&self) -> Option<String> {
        self.cache
            .default_bookmark
            .get_or_init(|| self.detect_default_bookmark())
            .clone()
    }

    /// Detect the default bookmark by checking common names.
    fn detect_default_bookmark(&self) -> Option<String> {
        // Common default bookmark names in order of preference
        let candidates = ["main", "master", "trunk", "develop"];

        // Get list of bookmarks
        let output = self.run_command(&["bookmark", "list"]).ok()?;

        for candidate in candidates {
            // Check if this bookmark exists
            if output.lines().any(|line| {
                let name = line.split(':').next().unwrap_or("").trim();
                name == candidate
            }) {
                return Some(candidate.to_string());
            }
        }

        // If no standard name found, return the first bookmark
        output
            .lines()
            .next()
            .and_then(|line| line.split(':').next())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Check if a bookmark exists.
    pub fn bookmark_exists(&self, name: &str) -> anyhow::Result<bool> {
        let output = self.run_command(&["bookmark", "list"])?;
        Ok(output.lines().any(|line| {
            let bookmark_name = line.split(':').next().unwrap_or("").trim();
            bookmark_name == name
        }))
    }

    /// List all bookmarks.
    ///
    /// Returns a list of (name, commit_id) tuples.
    pub fn list_bookmarks(&self) -> anyhow::Result<Vec<(String, String)>> {
        let output = self.run_command(&[
            "bookmark",
            "list",
            "--template",
            r#"name ++ "\0" ++ commit_id ++ "\n""#,
        ])?;

        let mut bookmarks = Vec::new();
        for line in output.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if let Some((name, commit)) = line.split_once('\0') {
                bookmarks.push((name.to_string(), commit.to_string()));
            } else if let Some((name, rest)) = line.split_once(": ") {
                // Human-readable format fallback
                let commit = rest.split_whitespace().next().unwrap_or("");
                bookmarks.push((name.to_string(), commit.to_string()));
            }
        }

        Ok(bookmarks)
    }

    /// Create a new bookmark at the current commit or specified revision.
    pub fn create_bookmark(&self, name: &str, revision: Option<&str>) -> anyhow::Result<()> {
        let mut args = vec!["bookmark", "create", name];
        if let Some(rev) = revision {
            args.push("-r");
            args.push(rev);
        }
        self.run_command(&args)?;
        Ok(())
    }

    /// Set (create or move) a bookmark to point to a revision.
    pub fn set_bookmark(&self, name: &str, revision: Option<&str>) -> anyhow::Result<()> {
        let mut args = vec!["bookmark", "set", name];
        if let Some(rev) = revision {
            args.push("-r");
            args.push(rev);
        }
        self.run_command(&args)?;
        Ok(())
    }

    /// Delete a bookmark.
    pub fn delete_bookmark(&self, name: &str) -> anyhow::Result<()> {
        self.run_command(&["bookmark", "delete", name])?;
        Ok(())
    }

    /// Get the commit ID that a bookmark points to.
    pub fn bookmark_commit(&self, name: &str) -> anyhow::Result<Option<String>> {
        let bookmarks = self.list_bookmarks()?;
        Ok(bookmarks
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, commit)| commit))
    }

    // =========================================================================
    // Switch history (for "wt switch -" support)
    // =========================================================================

    /// Get the previous workspace/bookmark from history.
    ///
    /// This is stored in a file in the .jj directory for persistence.
    pub fn switch_previous(&self) -> Option<String> {
        let history_file = self.jj_dir().join("wt-switch-previous");
        std::fs::read_to_string(history_file)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// Set the previous workspace/bookmark in history.
    pub fn set_switch_previous(&self, bookmark: Option<&str>) -> anyhow::Result<()> {
        let history_file = self.jj_dir().join("wt-switch-previous");

        if let Some(bookmark) = bookmark {
            std::fs::write(&history_file, bookmark)?;
        } else {
            let _ = std::fs::remove_file(&history_file);
        }

        Ok(())
    }
}
