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
    /// Inputs are resolved to commit SHAs (via the cached `commit_shas` map) before
    /// keying the cache, so equivalent forms (e.g., `"main"` vs the SHA `main` points
    /// to) hit the same entry. The key is also order-normalized since merge-base is
    /// symmetric: `merge-base(A, B) == merge-base(B, A)`.
    pub fn merge_base(&self, commit1: &str, commit2: &str) -> anyhow::Result<Option<String>> {
        // Resolve to SHAs so different forms of the same commit dedupe in the cache.
        // `resolve_to_commit_sha` is a no-op for inputs that already look like SHAs.
        let sha1 = self.resolve_to_commit_sha(commit1)?;
        let sha2 = self.resolve_to_commit_sha(commit2)?;

        // Normalize key order since merge-base is symmetric.
        let key = if sha1 <= sha2 {
            (sha1.clone(), sha2.clone())
        } else {
            (sha2.clone(), sha1.clone())
        };

        match self.cache.merge_base.entry(key) {
            Entry::Occupied(e) => Ok(e.get().clone()),
            Entry::Vacant(e) => {
                // Exit codes: 0 = found, 1 = no common ancestor, 128+ = invalid ref
                let output = self.run_command_output(&["merge-base", &sha1, &sha2])?;

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

                Ok(e.insert(result).clone())
            }
        }
    }

    /// Calculate commits ahead and behind between two refs.
    ///
    /// Returns (ahead, behind) where ahead is commits in head not in base,
    /// and behind is commits in base not in head.
    ///
    /// For orphan branches with no common ancestor, returns `(0, 0)`.
    /// Caller should check for orphan status separately via `merge_base()`.
    ///
    /// Results are cached in the shared repo cache. `batch_ahead_behind()`
    /// primes the cache for all local branches at once via a single
    /// `for-each-ref`; subsequent calls here hit the cache. On a miss, falls
    /// back to `merge_base()` + `rev-list --count`, also cached on insert.
    pub fn ahead_behind(&self, base: &str, head: &str) -> anyhow::Result<(usize, usize)> {
        let key = (base.to_string(), head.to_string());
        match self.cache.ahead_behind.entry(key) {
            Entry::Occupied(e) => Ok(*e.get()),
            Entry::Vacant(e) => {
                let counts = self.compute_ahead_behind(base, head)?;
                Ok(*e.insert(counts))
            }
        }
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

    /// Prime `cache.ahead_behind` for all local branches vs a base ref.
    ///
    /// Uses `git for-each-ref --format='%(ahead-behind:BASE)'` (git 2.36+) to
    /// compute all counts in a single command, so subsequent `ahead_behind()`
    /// calls hit the cache.
    ///
    /// On git < 2.36 or if the command fails, this is a no-op and
    /// `ahead_behind()` falls back to per-branch computation.
    pub fn batch_ahead_behind(&self, base: &str) {
        // Uses `%(refname)` (full ref) rather than `%(refname:lstrip=2)` so the
        // cache key matches what `AheadBehindTask` queries — that task passes
        // `BranchRef::full_ref()` (e.g., `refs/heads/feature`). Keying by short
        // names would force every row to miss the batch cache and fall back to
        // per-branch `rev-list --count`.
        let format = format!("%(refname) %(ahead-behind:{})", base);
        let output = match self.run_command(&[
            "for-each-ref",
            &format!("--format={}", format),
            "refs/heads/",
        ]) {
            Ok(output) => output,
            Err(e) => {
                // Fails on git < 2.36 (no %(ahead-behind:) support), invalid base ref, etc.
                log::debug!("batch_ahead_behind({base}): git for-each-ref failed: {e}");
                return;
            }
        };

        output
            .lines()
            .filter_map(|line| {
                // Format: "refs/heads/branch-name ahead behind"
                let mut parts = line.rsplitn(3, ' ');
                let behind: usize = parts.next()?.parse().ok()?;
                let ahead: usize = parts.next()?.parse().ok()?;
                let full_ref = parts.next()?;
                Some((full_ref, ahead, behind))
            })
            .for_each(|(full_ref, ahead, behind)| {
                self.cache
                    .ahead_behind
                    .insert((base.to_string(), full_ref.to_string()), (ahead, behind));
            });
    }

    /// Get line diff statistics between two refs.
    ///
    /// Uses merge-base (cached) to find common ancestor, then two-dot diff
    /// to get the stats. This allows the merge-base result to be reused
    /// across multiple operations.
    ///
    /// For orphan branches with no common ancestor, returns zeros.
    pub fn branch_diff_stats(&self, base: &str, head: &str) -> anyhow::Result<LineDiff> {
        use dashmap::mapref::entry::Entry;

        let base_sha = self.rev_parse_commit(base)?;
        let head_sha = self.rev_parse_commit(head)?;

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
                .entry((base_sha.clone(), head_sha.clone()))
            {
                Entry::Occupied(e) => return Ok(*e.get()),
                Entry::Vacant(e) => {
                    let result =
                        self.compute_branch_diff_stats(&base_sha, &head_sha, sparse_paths)?;
                    return Ok(*e.insert(result));
                }
            }
        }

        self.compute_branch_diff_stats(&base_sha, &head_sha, sparse_paths)
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

#[cfg(test)]
mod tests {
    use crate::git::Repository;
    use crate::testing::TestRepo;

    #[test]
    fn batch_ahead_behind_keys_cache_by_full_ref() {
        // Callers in `wt list` pass `BranchRef::full_ref()` (`refs/heads/<name>`)
        // into `ahead_behind`. This test pins the batch cache to the same key
        // shape — short-name keys would force every row to miss the batch and
        // fall back to per-branch `rev-list --count`.
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.path()).unwrap();

        test.create_branch("feature");
        test.run_git(&["checkout", "feature"]);
        std::fs::write(test.path().join("f.txt"), "x").unwrap();
        test.run_git(&["add", "."]);
        test.run_git(&["commit", "-m", "feature"]);
        test.run_git(&["checkout", "main"]);

        repo.batch_ahead_behind("main");

        assert_eq!(
            repo.cache
                .ahead_behind
                .get(&("main".to_string(), "refs/heads/feature".to_string()))
                .map(|v| *v),
            Some((1, 0)),
            "batch must prime cache under the full ref key"
        );
        assert!(
            repo.cache
                .ahead_behind
                .get(&("main".to_string(), "feature".to_string()))
                .is_none(),
            "short-name key must not be used",
        );
    }
}
