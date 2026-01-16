//! Diff, history, and commit operations for Repository.

use std::collections::HashMap;

use anyhow::Context;

use super::{DiffStats, LineDiff, Repository};

impl Repository {
    /// Count commits between base and head.
    pub fn count_commits(&self, base: &str, head: &str) -> anyhow::Result<usize> {
        // Limit concurrent rev-list operations to reduce mmap thrash on commit-graph
        let _guard = super::super::HEAVY_OPS_SEMAPHORE.acquire();

        let range = format!("{}..{}", base, head);
        let stdout = self.run_command(&["rev-list", "--count", &range])?;

        stdout
            .trim()
            .parse()
            .context("Failed to parse commit count")
    }

    /// Get files changed between base and head.
    ///
    /// For renames and copies, both old and new paths are included to ensure
    /// overlap detection works correctly (e.g., detecting conflicts when a file
    /// is renamed in one branch but has uncommitted changes under the old name).
    pub fn changed_files(&self, base: &str, head: &str) -> anyhow::Result<Vec<String>> {
        let range = format!("{}..{}", base, head);
        let stdout = self.run_command(&["diff", "--name-status", "-z", &range])?;

        // Format: STATUS\0PATH\0 or STATUS\0NEW_PATH\0OLD_PATH\0 for renames/copies
        let mut files = Vec::new();
        let mut parts = stdout.split('\0').filter(|s| !s.is_empty());

        while let Some(status) = parts.next() {
            let path = parts
                .next()
                .ok_or_else(|| anyhow::anyhow!("Malformed git diff output: status without path"))?;
            files.push(path.to_string());

            // For renames (R) and copies (C), the old path follows
            if status.starts_with('R') || status.starts_with('C') {
                let old_path = parts.next().ok_or_else(|| {
                    anyhow::anyhow!("Malformed git diff output: rename/copy without old path")
                })?;
                files.push(old_path.to_string());
            }
        }

        Ok(files)
    }

    /// Get commit timestamp in seconds since epoch.
    pub fn commit_timestamp(&self, commit: &str) -> anyhow::Result<i64> {
        let stdout = self.run_command(&["show", "-s", "--format=%ct", commit])?;
        stdout.trim().parse().context("Failed to parse timestamp")
    }

    /// Get commit timestamps for multiple commits in a single git command.
    ///
    /// Returns a map from commit SHA to timestamp. More efficient than calling
    /// `commit_timestamp` multiple times when you have many commits.
    pub fn commit_timestamps(&self, commits: &[&str]) -> anyhow::Result<HashMap<String, i64>> {
        if commits.is_empty() {
            return Ok(HashMap::new());
        }

        // Build command: git show -s --format='%H %ct' sha1 sha2 sha3 ...
        let mut args = vec!["show", "-s", "--format=%H %ct"];
        args.extend(commits);

        let stdout = self.run_command(&args)?;

        let mut result = HashMap::with_capacity(commits.len());
        for line in stdout.lines() {
            if let Some((sha, timestamp_str)) = line.split_once(' ')
                && let Ok(timestamp) = timestamp_str.parse::<i64>()
            {
                result.insert(sha.to_string(), timestamp);
            }
        }

        Ok(result)
    }

    /// Get commit message (subject line) for a commit.
    pub fn commit_message(&self, commit: &str) -> anyhow::Result<String> {
        let stdout = self.run_command(&["show", "-s", "--format=%s", commit])?;
        Ok(stdout.trim().to_owned())
    }

    /// Get commit timestamp and message in a single git command.
    ///
    /// More efficient than calling `commit_timestamp` and `commit_message` separately.
    pub fn commit_details(&self, commit: &str) -> anyhow::Result<(i64, String)> {
        // Use space separator - timestamps don't contain spaces, and %s (subject)
        // is the first line only (no embedded newlines). Split on first space.
        let stdout = self.run_command(&["show", "-s", "--format=%ct %s", commit])?;
        // Only strip trailing newline, not spaces (empty subject = "timestamp ")
        let line = stdout.trim_end_matches('\n');
        let (timestamp_str, message) = line
            .split_once(' ')
            .context("Failed to parse commit details")?;
        let timestamp = timestamp_str.parse().context("Failed to parse timestamp")?;
        // Trim the message to match commit_message() behavior
        Ok((timestamp, message.trim().to_owned()))
    }

    /// Get commit subjects (first line of commit message) from a range.
    pub fn commit_subjects(&self, range: &str) -> anyhow::Result<Vec<String>> {
        let output = self.run_command(&["log", "--format=%s", range])?;
        Ok(output.lines().map(String::from).collect())
    }

    /// Get recent commit subjects for style reference.
    ///
    /// Returns up to `count` commit subjects (first line of message), excluding merges.
    /// If `start_ref` is provided, gets commits starting from that ref.
    /// Returns `None` if no commits are found or the command fails.
    pub fn recent_commit_subjects(
        &self,
        start_ref: Option<&str>,
        count: usize,
    ) -> Option<Vec<String>> {
        let count_str = count.to_string();
        let mut args = vec!["log", "--pretty=format:%s", "-n", &count_str, "--no-merges"];
        if let Some(ref_name) = start_ref {
            args.push(ref_name);
        }
        self.run_command(&args).ok().and_then(|output| {
            if output.trim().is_empty() {
                None
            } else {
                Some(output.lines().map(String::from).collect())
            }
        })
    }

    /// Get the merge base between two commits.
    ///
    /// Returns `Ok(Some(sha))` if a merge base exists, `Ok(None)` for orphan branches
    /// with no common ancestor (git exit code 1), or `Err` for invalid refs.
    ///
    /// Results are cached in the shared repo cache to avoid redundant git commands
    /// when multiple tasks need the same merge-base (e.g., parallel `wt list` tasks).
    /// The cache key is normalized (sorted) since merge-base(A, B) == merge-base(B, A).
    pub fn merge_base(&self, commit1: &str, commit2: &str) -> anyhow::Result<Option<String>> {
        use anyhow::bail;

        // Normalize key order since merge-base is symmetric: merge-base(A, B) == merge-base(B, A)
        let key = if commit1 <= commit2 {
            (commit1.to_string(), commit2.to_string())
        } else {
            (commit2.to_string(), commit1.to_string())
        };

        // Check cache first
        if let Some(cached) = self.cache.merge_base.get(&key) {
            return Ok(cached.clone());
        }

        // Exit codes: 0 = found, 1 = no common ancestor, 128+ = invalid ref
        let output = self.run_command_output(&["merge-base", commit1, commit2])?;

        let result = if output.status.success() {
            Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else if output.status.code() == Some(1) {
            None
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "git merge-base failed for {commit1} {commit2}: {}",
                stderr.trim()
            );
        };

        self.cache.merge_base.insert(key, result.clone());
        Ok(result)
    }

    /// Calculate commits ahead and behind between two refs.
    ///
    /// Returns (ahead, behind) where ahead is commits in head not in base,
    /// and behind is commits in base not in head.
    ///
    /// For orphan branches with no common ancestor, returns `(0, 0)`.
    /// Caller should check for orphan status separately via `merge_base()`.
    ///
    /// Uses `merge_base()` internally (which is cached) to compute the common
    /// ancestor, then counts commits using two-dot syntax. This allows the
    /// merge-base result to be reused across multiple operations.
    pub fn ahead_behind(&self, base: &str, head: &str) -> anyhow::Result<(usize, usize)> {
        // Get merge-base (cached in shared repo cache)
        let Some(merge_base) = self.merge_base(base, head)? else {
            // Orphan branch - no common ancestor
            return Ok((0, 0));
        };

        // Count commits using two-dot syntax (faster when merge-base is cached)
        // ahead = commits in head but not in merge_base
        // behind = commits in base but not in merge_base
        //
        // Skip rev-list when merge_base equals head (count would be 0).
        // Note: we don't check merge_base == base because base is typically a
        // refname like "main" while merge_base is a SHA.
        let ahead = if merge_base == head {
            0
        } else {
            let output =
                self.run_command(&["rev-list", "--count", &format!("{}..{}", merge_base, head)])?;
            output
                .trim()
                .parse()
                .context("Failed to parse ahead count")?
        };

        let behind_output =
            self.run_command(&["rev-list", "--count", &format!("{}..{}", merge_base, base)])?;
        let behind = behind_output
            .trim()
            .parse()
            .context("Failed to parse behind count")?;

        Ok((ahead, behind))
    }

    /// Batch-fetch ahead/behind counts for all local branches vs a base ref.
    ///
    /// Uses `git for-each-ref --format='%(ahead-behind:BASE)'` (git 2.36+) to get
    /// all counts in a single command. Returns a map from branch name to (ahead, behind).
    ///
    /// Results are cached so subsequent lookups via `get_cached_ahead_behind()` avoid
    /// running individual git commands (though cache access still has minor overhead).
    ///
    /// On git < 2.36 or if the command fails, returns an empty map.
    pub fn batch_ahead_behind(&self, base: &str) -> HashMap<String, (usize, usize)> {
        let format = format!("%(refname:lstrip=2) %(ahead-behind:{})", base);
        let output = match self.run_command(&[
            "for-each-ref",
            &format!("--format={}", format),
            "refs/heads/",
        ]) {
            Ok(output) => output,
            Err(e) => {
                // Fails on git < 2.36 (no %(ahead-behind:) support), invalid base ref, etc.
                log::debug!("batch_ahead_behind({base}): git for-each-ref failed: {e}");
                return HashMap::new();
            }
        };

        let results: HashMap<String, (usize, usize)> = output
            .lines()
            .filter_map(|line| {
                // Format: "branch-name ahead behind"
                let mut parts = line.rsplitn(3, ' ');
                let behind: usize = parts.next()?.parse().ok()?;
                let ahead: usize = parts.next()?.parse().ok()?;
                let branch = parts.next()?.to_string();
                // Cache each result for later lookup
                self.cache
                    .ahead_behind
                    .insert((base.to_string(), branch.clone()), (ahead, behind));
                Some((branch, (ahead, behind)))
            })
            .collect();

        results
    }

    /// Get cached ahead/behind counts for a branch.
    ///
    /// Returns cached results from a prior `batch_ahead_behind()` call, or None
    /// if the branch wasn't in the batch or batch wasn't run.
    pub fn get_cached_ahead_behind(&self, base: &str, branch: &str) -> Option<(usize, usize)> {
        self.cache
            .ahead_behind
            .get(&(base.to_string(), branch.to_string()))
            .map(|r| *r)
    }

    /// Get line diff statistics between two refs.
    ///
    /// Uses merge-base (cached) to find common ancestor, then two-dot diff
    /// to get the stats. This allows the merge-base result to be reused
    /// across multiple operations.
    ///
    /// For orphan branches with no common ancestor, returns zeros.
    pub fn branch_diff_stats(&self, base: &str, head: &str) -> anyhow::Result<LineDiff> {
        // Limit concurrent diff operations to reduce mmap thrash on pack files
        let _guard = super::super::HEAVY_OPS_SEMAPHORE.acquire();

        // Get merge-base (cached in shared repo cache)
        let Some(merge_base) = self.merge_base(base, head)? else {
            return Ok(LineDiff::default());
        };

        // Use two-dot syntax with the cached merge-base
        let range = format!("{}..{}", merge_base, head);
        let stdout = self.run_command(&["diff", "--numstat", &range])?;
        LineDiff::from_numstat(&stdout)
    }

    /// Get formatted diff stats summary for display.
    ///
    /// Returns a vector of formatted strings like ["3 files", "+45", "-12"].
    /// Returns empty vector if diff command fails or produces no output.
    ///
    /// Callers should pass `--shortstat` in args for compatibility; this method
    /// internally replaces it with `--numstat` for locale-independent parsing.
    pub fn diff_stats_summary(&self, args: &[&str]) -> Vec<String> {
        // Replace --shortstat with --numstat for locale-independent parsing
        let args: Vec<&str> = args
            .iter()
            .map(|&arg| {
                if arg == "--shortstat" {
                    "--numstat"
                } else {
                    arg
                }
            })
            .collect();

        self.run_command(&args)
            .ok()
            .map(|output| DiffStats::from_numstat(&output).format_summary())
            .unwrap_or_default()
    }
}
