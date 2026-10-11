//! Repository-owned serialization of compare-and-swap branch deletion.
//!
//! Each call runs one `git update-ref -d <ref> <original-sha>`. Serialization
//! avoids Worktrunk's own contention on Git's packed-refs lock; the original
//! SHA comparison protects commits from concurrent writers. Integration and
//! checkout reads remain outside this coordinator.
//!
//! A failed deletion returns `false` only when a fresh exact ref read confirms
//! a different live SHA. Missing, unchanged or unreadable refs keep the original
//! Git error. Ordinary failures affect only their caller. An interruption in
//! either the mutation or its failure classification is latched before releasing
//! the mutex, so waiting and later calls cannot mutate after cancellation. The
//! repository's process-wide coordinator lives for one CLI invocation.

use std::sync::Mutex;

use super::{ErrorExt, Repository, WorktrunkError};

#[derive(Debug, Default)]
pub(super) struct RefDeletionCoordinator {
    interrupted: Mutex<Option<i32>>,
}

impl RefDeletionCoordinator {
    /// Delete iff the original SHA still matches; `false` means a moved live ref.
    pub(super) fn delete(
        &self,
        repo: &Repository,
        ref_name: &str,
        expected_sha: &str,
    ) -> anyhow::Result<bool> {
        tracing::debug!(ref_name, "Waiting for safe branch deletion");
        let mut interrupted = self
            .interrupted
            .lock()
            .map_err(|_| anyhow::anyhow!("Branch deletion coordinator stopped"))?;
        if let Some(signal) = *interrupted {
            return Err(WorktrunkError::Interrupted { signal, hint: None }.into());
        }

        let result = match repo.run_command(&["update-ref", "-d", ref_name, expected_sha]) {
            Ok(_) => Ok(true),
            Err(error) if error.interrupt_signal().is_some() => Err(error),
            Err(error) => {
                match repo.run_command(&["show-ref", "--verify", "--hash", "--", ref_name]) {
                    Ok(live_sha) if live_sha.trim() != expected_sha => Ok(false),
                    Err(read_error) if read_error.interrupt_signal().is_some() => Err(read_error),
                    _ => Err(error),
                }
            }
        };
        if let Err(error) = &result
            && let Some(signal) = error.interrupt_signal()
        {
            *interrupted = Some(signal);
            return Err(WorktrunkError::Interrupted { signal, hint: None }.into());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestRepo;

    /// A packed ref with an advanced loose override must retain its new commit.
    #[test]
    fn original_sha_preserves_moved_and_missing_refs_and_deletes_unchanged_ref() {
        let test = TestRepo::with_initial_commit();
        for branch in ["moved", "missing", "unchanged"] {
            test.run_git(&["branch", branch]);
        }
        let original = test.git_output(&["rev-parse", "main"]);
        test.run_git(&["pack-refs", "--all"]);
        test.run_git(&["checkout", "moved"]);
        std::fs::write(test.root_path().join("new.txt"), "new work\n").unwrap();
        test.run_git(&["add", "new.txt"]);
        test.run_git(&["commit", "-m", "advance after snapshot"]);
        test.run_git(&["checkout", "main"]);
        let advanced = test.git_output(&["rev-parse", "moved"]);
        assert_ne!(original, advanced);
        test.run_git(&["tag", "refs/heads/missing", &advanced]);
        test.run_git(&["update-ref", "-d", "refs/heads/missing"]);
        let repo = Repository::at(test.root_path()).unwrap();
        let coordinator = repo.branch_deletions();
        assert!(
            !coordinator
                .delete(&repo, "refs/heads/moved", &original)
                .unwrap()
        );
        let missing_error = coordinator
            .delete(&repo, "refs/heads/missing", &original)
            .unwrap_err();
        assert!(super::super::CommandError::find_in(&missing_error).is_some());
        assert!(
            coordinator
                .delete(&repo, "refs/heads/unchanged", &original)
                .unwrap()
        );
        assert_eq!(test.git_output(&["rev-parse", "moved"]), advanced);
        assert_eq!(
            test.git_output(&["rev-parse", "refs/tags/refs/heads/missing"]),
            advanced
        );
        for branch in ["missing", "unchanged"] {
            assert!(
                repo.run_command(&[
                    "show-ref",
                    "--verify",
                    "--hash",
                    "--",
                    &format!("refs/heads/{branch}")
                ])
                .is_err()
            );
        }
    }

    #[test]
    fn ref_lock_failures_keep_the_original_error_without_cancelling_other_calls() {
        for packed in [false, true] {
            let test = TestRepo::with_initial_commit();
            for branch in ["blocked", "independent"] {
                test.run_git(&["branch", branch]);
            }
            let lock_name = if packed {
                test.run_git(&["pack-refs", "--all"]);
                test.run_git(&["config", "core.packedRefsTimeout", "0"]);
                "packed-refs.lock"
            } else {
                test.run_git(&["config", "core.filesRefLockTimeout", "0"]);
                "refs/heads/blocked.lock"
            };
            let sha = test.git_output(&["rev-parse", "main"]);
            let repo = Repository::at(test.root_path()).unwrap();
            let lock_path = repo.git_common_dir().join(lock_name);
            std::fs::write(&lock_path, "").unwrap();
            let error = repo
                .branch_deletions()
                .delete(&repo, "refs/heads/blocked", &sha)
                .unwrap_err();
            assert!(error.display_message().contains(lock_name));
            assert!(super::super::CommandError::find_in(&error).is_some());
            assert_eq!(test.git_output(&["rev-parse", "blocked"]), sha);
            std::fs::remove_file(lock_path).unwrap();
            assert!(
                repo.branch_deletions()
                    .delete(&repo, "refs/heads/independent", &sha)
                    .unwrap()
            );
            assert!(
                repo.run_command(&["rev-parse", "--verify", "--quiet", "refs/heads/independent"])
                    .is_err()
            );
        }
    }

    #[test]
    fn cancellation_prevents_later_calls_from_mutating_refs() {
        let test = TestRepo::with_initial_commit();
        test.run_git(&["branch", "candidate"]);
        let sha = test.git_output(&["rev-parse", "candidate"]);
        let repo = Repository::at(test.root_path()).unwrap();
        *repo.branch_deletions().interrupted.lock().unwrap() = Some(2);
        // A fresh repository shares the same command-lifetime cancellation.
        let fresh = Repository::at(test.root_path()).unwrap();
        for repo in [&repo, &fresh] {
            let error = repo
                .branch_deletions()
                .delete(repo, "refs/heads/candidate", &sha)
                .unwrap_err()
                .context("branch removal");
            assert_eq!(error.interrupt_signal(), Some(2));
            assert_eq!(error.exit_code(), Some(130));
        }
        assert_eq!(test.git_output(&["rev-parse", "candidate"]), sha);
    }
}
