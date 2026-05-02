//! Diff, history, and commit operations for Repository.

use std::collections::HashMap;

use anyhow::{Context, bail};
use dashmap::mapref::entry::Entry;

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

    /// Get commit timestamp and subject for multiple commits in a single git command.
    ///
    /// Returns a map from commit SHA to `(timestamp, subject)`. Uses NUL
    /// separators between fields so subjects containing spaces or other
    /// whitespace parse unambiguously. `%s` is the subject line only, so no
    /// multi-line handling is needed.
    ///
    /// Fails if any SHA is invalid — `git log --no-walk` refuses the whole
    /// batch on a single bad ref. Callers should surface the error rather
    /// than fall back to per-SHA fetches: the batch is the only commit-detail
    /// fetch path left, and quietly swallowing the failure produces empty
    /// cells without telling the user why.
    pub fn commit_details_many(
        &self,
        commits: &[&str],
    ) -> anyhow::Result<HashMap<String, (i64, String)>> {
        if commits.is_empty() {
            return Ok(HashMap::new());
        }

        // --no-walk shows exactly the named commits without DAG walking.
        // --no-show-signature suppresses GPG verification output that otherwise
        // contaminates stdout when log.showSignature is set.
        let mut args = vec![
            "log",
            "--no-walk",
            "--no-show-signature",
            "--format=%H%x00%ct%x00%s",
        ];
        args.extend(commits);

        let stdout = self.run_command(&args)?;

        let mut result = HashMap::with_capacity(commits.len());
        for line in stdout.lines() {
            let mut parts = line.splitn(3, '\0');
            let (Some(sha), Some(timestamp_str), Some(subject)) =
                (parts.next(), parts.next(), parts.next())
            else {
                bail!(
                    "Malformed git log output: expected '<sha>\\0<ts>\\0<subject>', got {line:?}"
                );
            };
            let timestamp: i64 = timestamp_str
                .parse()
                .with_context(|| format!("Failed to parse timestamp {timestamp_str:?}"))?;
            result.insert(sha.to_string(), (timestamp, subject.to_owned()));
        }

        Ok(result)
    }

    /// Get commit subjects (first line of commit message) from a range.
    pub fn commit_subjects(&self, range: &str) -> anyhow::Result<Vec<String>> {
        let output = self.run_command(&["log", "--no-show-signature", "--format=%s", range])?;
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
        let mut args = vec![
            "log",
            "--pretty=format:%s",
            "--no-show-signature",
            "-n",
            &count_str,
            "--no-merges",
        ];
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
    /// Inputs are resolved to commit SHAs (the resolver short-circuits on
    /// hex-shaped inputs and otherwise spawns `git rev-parse`) before keying
    /// the cache, so equivalent forms (e.g., `"main"` vs the SHA `main` points
    /// to) hit the same entry. The key is also order-normalized since
    /// merge-base is symmetric: `merge-base(A, B) == merge-base(B, A)`.
    pub fn merge_base(&self, commit1: &str, commit2: &str) -> anyhow::Result<Option<String>> {
        // Resolve to SHAs so different forms of the same commit dedupe in the cache.
        // `resolve_to_commit_sha` is a no-op for inputs that already look like SHAs.
        let sha1 = self.resolve_to_commit_sha(commit1)?;
        let sha2 = self.resolve_to_commit_sha(commit2)?;
        self.merge_base_by_sha(&sha1, &sha2)
    }

    /// SHA-keyed variant of [`Self::merge_base`].
    ///
    /// Inputs are commit SHAs. Skips the ambient ref→SHA conversion
    /// entirely; cache key is `(min(sha1, sha2), max(sha1, sha2))`.
    pub fn merge_base_by_sha(&self, sha1: &str, sha2: &str) -> anyhow::Result<Option<String>> {
        // Normalize key order since merge-base is symmetric.
        let key = if sha1 <= sha2 {
            (sha1.to_string(), sha2.to_string())
        } else {
            (sha2.to_string(), sha1.to_string())
        };

        match self.cache.merge_base.entry(key) {
            Entry::Occupied(e) => Ok(e.get().clone()),
            Entry::Vacant(e) => {
                // Exit codes: 0 = found, 1 = no common ancestor, 128+ = invalid ref
                let output = self.run_command_output(&["merge-base", sha1, sha2])?;

                let result = if output.status.success() {
                    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
                } else if output.status.code() == Some(1) {
                    None
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    bail!("git merge-base failed for {sha1} {sha2}: {}", stderr.trim());
                };

                Ok(e.insert(result).clone())
            }
        }
    }

    /// SHA-keyed ahead/behind counts.
    ///
    /// Inputs are commit SHAs. Uncached — counts are typically primed in
    /// bulk by [`crate::git::RefSnapshot`]'s ahead/behind batch; this is
    /// the per-pair fallback for snapshot misses or callers that already
    /// hold SHAs. Returns `(ahead, behind)` where ahead is commits in
    /// head not in base; orphan branches (no common ancestor) yield
    /// `(0, 0)` — caller should distinguish via [`Self::merge_base_by_sha`].
    pub fn ahead_behind_by_sha(
        &self,
        base_sha: &str,
        head_sha: &str,
    ) -> anyhow::Result<(usize, usize)> {
        self.compute_ahead_behind(base_sha, head_sha)
    }

    fn compute_ahead_behind(&self, base: &str, head: &str) -> anyhow::Result<(usize, usize)> {
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

    /// Get line diff statistics between two refs.
    ///
    /// Uses merge-base (cached) to find common ancestor, then two-dot diff
    /// to get the stats. This allows the merge-base result to be reused
    /// across multiple operations.
    ///
    /// For orphan branches with no common ancestor, returns zeros.
    pub fn branch_diff_stats(&self, base: &str, head: &str) -> anyhow::Result<LineDiff> {
        let base_sha = self.rev_parse_commit(base)?;
        let head_sha = self.rev_parse_commit(head)?;
        self.branch_diff_stats_by_sha(&base_sha, &head_sha)
    }

    /// SHA-keyed variant of [`Self::branch_diff_stats`].
    ///
    /// Inputs are commit SHAs. Bypasses the ambient ref→SHA cache.
    pub fn branch_diff_stats_by_sha(
        &self,
        base_sha: &str,
        head_sha: &str,
    ) -> anyhow::Result<LineDiff> {
        use dashmap::mapref::entry::Entry;

        // Sparse checkout filters the diff by path, making the result
        // environment-dependent rather than purely SHA-determined. Skip
        // caches when sparse checkout is active.
        let sparse_paths = self.sparse_checkout_paths();
        let use_cache = sparse_paths.is_empty();

        if use_cache {
            // In-memory entry lock prevents parallel tasks from racing through
            // the file-based cache for the same SHA pair.
            match self
                .cache
                .diff_stats
                .entry((base_sha.to_string(), head_sha.to_string()))
            {
                Entry::Occupied(e) => return Ok(*e.get()),
                Entry::Vacant(e) => {
                    let result =
                        self.compute_branch_diff_stats(base_sha, head_sha, sparse_paths)?;
                    return Ok(*e.insert(result));
                }
            }
        }

        self.compute_branch_diff_stats(base_sha, head_sha, sparse_paths)
    }

    fn compute_branch_diff_stats(
        &self,
        base_sha: &str,
        head_sha: &str,
        sparse_paths: &[String],
    ) -> anyhow::Result<LineDiff> {
        let use_cache = sparse_paths.is_empty();

        if use_cache && let Some(cached) = super::sha_cache::diff_stats(self, base_sha, head_sha) {
            return Ok(cached);
        }

        // Limit concurrent diff operations to reduce mmap thrash on pack files.
        // Acquired after cache check to avoid holding the semaphore on cache hits.
        let _guard = super::super::HEAVY_OPS_SEMAPHORE.acquire();

        // Get merge-base (cached in shared repo cache)
        let Some(merge_base) = self.merge_base(base_sha, head_sha)? else {
            if use_cache {
                super::sha_cache::put_diff_stats(self, base_sha, head_sha, LineDiff::default());
            }
            return Ok(LineDiff::default());
        };

        let range = format!("{}..{}", merge_base, head_sha);
        let mut args = vec!["diff", "--shortstat", &range];

        if !sparse_paths.is_empty() {
            args.push("--");
            args.extend(sparse_paths.iter().map(|s| s.as_str()));
        }

        let stdout = self.run_command(&args)?;
        let result = LineDiff::from_shortstat(&stdout);
        if use_cache {
            super::sha_cache::put_diff_stats(self, base_sha, head_sha, result);
        }
        Ok(result)
    }

    /// Get formatted diff stats summary for display.
    ///
    /// Returns a vector of formatted strings like ["3 files", "+45", "-12"].
    /// Returns empty vector if diff command fails or produces no output.
    ///
    /// Callers pass args including `--shortstat` which produces a single summary line.
    pub fn diff_stats_summary(&self, args: &[&str]) -> Vec<String> {
        self.run_command(args)
            .ok()
            .map(|output| DiffStats::from_shortstat(&output).format_summary())
            .unwrap_or_default()
    }
}
