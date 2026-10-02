//! Repository-owned compare-and-swap deletion queue.
//!
//! The first submitting thread drains ready requests into ordinary atomic
//! `update-ref --stdin` transactions. Other submitters wait for their own
//! outcome. No timer, minimum batch size, or command-level lock is involved;
//! requests arriving during a Git call form the next batch. Registry reads
//! and worktree staging remain outside this owner.
//!
//! A rejected transaction changes no refs. Fresh ref reads isolate moved or
//! missing members, then retry the remaining members with their ORIGINAL
//! expected SHAs. A missing member receives its original error without
//! blocking unchanged peers. A retry must shrink the transaction. When a
//! failed batch cannot isolate members (unchanged refs or an unreadable
//! inventory), it falls back to individual transactions with the same original
//! SHAs. A singleton failure reaches only its own caller, so one ref's lock
//! cannot strand unrelated branches, including requests still queued.
//! Interrupts alone stop the current drain and reach every waiter. The leader
//! guard also releases waiters if a submitting thread unwinds.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, mpsc};

use anyhow::Context;

use super::{ErrorExt, Repository, WorktrunkError};

#[derive(Debug)]
struct Request {
    ref_name: String,
    expected_sha: String,
    reply: mpsc::Sender<anyhow::Result<bool>>,
}

#[derive(Debug, Default)]
struct State {
    active: bool,
    pending: Vec<Request>,
}

#[derive(Debug, Default)]
pub(super) struct RefDeletionQueue {
    state: Mutex<State>,
}

impl RefDeletionQueue {
    /// Delete iff the original SHA still matches; `false` means a moved live ref.
    pub(super) fn delete(
        &self,
        repo: &Repository,
        ref_name: String,
        expected_sha: &str,
    ) -> anyhow::Result<bool> {
        let (reply, response) = mpsc::channel();
        let lead = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let pending = state.pending.len() + 1;
            tracing::debug!(
                pending,
                ref_name = %ref_name,
                "Queued safe branch deletion (pending: {pending})"
            );
            state.pending.push(Request {
                ref_name,
                expected_sha: expected_sha.to_owned(),
                reply,
            });
            if state.active {
                false
            } else {
                state.active = true;
                true
            }
        };
        if lead {
            let mut leader = Leader {
                queue: self,
                armed: true,
            };
            let mut interrupt = None;
            loop {
                let requests = {
                    let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    if state.pending.is_empty() {
                        state.active = false;
                        leader.armed = false;
                        break;
                    }
                    // Git forbids two updates to the same ref in a transaction.
                    // Leave duplicate requests for the next transaction, where
                    // the same original-SHA check still applies.
                    let mut names = HashSet::new();
                    let (ready, deferred) = std::mem::take(&mut state.pending)
                        .into_iter()
                        .partition(|r| names.insert(r.ref_name.clone()));
                    state.pending = deferred;
                    ready
                };
                if let Some(error) = &interrupt {
                    reply_error(requests, error);
                } else {
                    interrupt = delete_batch(repo, requests);
                }
            }
        }
        response
            .recv()
            .context("Branch deletion coordinator stopped")?
    }
}

struct Leader<'a> {
    queue: &'a RefDeletionQueue,
    armed: bool,
}

impl Drop for Leader<'_> {
    fn drop(&mut self) {
        if self.armed {
            let mut state = self.queue.state.lock().unwrap_or_else(|e| e.into_inner());
            state.active = false;
            let error = Arc::new(anyhow::anyhow!("Branch deletion coordinator stopped"));
            reply_error(std::mem::take(&mut state.pending), &error);
        }
    }
}

/// Share captured Git failures without flattening their typed source chain.
#[derive(Debug)]
struct SharedFailure(Arc<anyhow::Error>);

impl std::fmt::Display for SharedFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for SharedFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref().as_ref())
    }
}

fn reply_error(requests: Vec<Request>, error: &Arc<anyhow::Error>) {
    for request in requests {
        let error = match error.interrupt_signal() {
            Some(signal) => WorktrunkError::Interrupted { signal, hint: None }.into(),
            None => SharedFailure(Arc::clone(error)).into(),
        };
        let _ = request.reply.send(Err(error));
    }
}

/// Return only an interrupt, so ordinary failures never poison queued peers.
fn delete_batch(repo: &Repository, mut requests: Vec<Request>) -> Option<Arc<anyhow::Error>> {
    loop {
        let mut input = Vec::new();
        for request in &requests {
            input.extend_from_slice(b"delete ");
            input.extend_from_slice(request.ref_name.as_bytes());
            input.push(0);
            input.extend_from_slice(request.expected_sha.as_bytes());
            input.push(0);
        }
        tracing::debug!(
            size = requests.len(),
            "Deleting {} ready branch refs in one transaction",
            requests.len()
        );
        let error = match repo.run_command_with_input(&["update-ref", "--stdin", "-z"], input) {
            Ok(_) => {
                for request in requests {
                    let _ = request.reply.send(Ok(true));
                }
                return None;
            }
            Err(error) => Arc::new(error),
        };
        if error.interrupt_signal().is_some() {
            reply_error(requests, &error);
            return Some(error);
        }

        // One fresh walk classifies the failed transaction. Prefix patterns
        // can also return descendants; only exact requested names are used.
        let live_refs = match fresh_ref_values(repo, &requests) {
            Ok(refs) => refs,
            Err(read_error) => {
                if read_error.interrupt_signal().is_some() {
                    let interrupt = Arc::new(read_error);
                    reply_error(requests, &interrupt);
                    return Some(interrupt);
                }
                if requests.len() > 1 {
                    return delete_individually(repo, requests);
                }
                reply_error(requests, &error);
                return None;
            }
        };
        let original_len = requests.len();
        let mut unchanged = Vec::new();
        for request in requests {
            match live_refs.get(request.ref_name.as_str()) {
                Some(sha) if sha != &request.expected_sha => {
                    let _ = request.reply.send(Ok(false));
                }
                Some(_) => unchanged.push(request),
                None => reply_error(vec![request], &error),
            }
        }
        if unchanged.is_empty() {
            return None;
        }
        if unchanged.len() == original_len {
            if unchanged.len() > 1 {
                return delete_individually(repo, unchanged);
            }
            reply_error(unchanged, &error);
            return None;
        }
        requests = unchanged;
    }
}

/// Isolate a failed multi-ref transaction once. Singleton failures cannot
/// recurse here; every attempt carries its request's original expected SHA.
fn delete_individually(repo: &Repository, requests: Vec<Request>) -> Option<Arc<anyhow::Error>> {
    let mut remaining = requests.into_iter();
    while let Some(request) = remaining.next() {
        if let Some(interrupt) = delete_batch(repo, vec![request]) {
            reply_error(remaining.collect(), &interrupt);
            return Some(interrupt);
        }
    }
    None
}

fn fresh_ref_values(
    repo: &Repository,
    requests: &[Request],
) -> anyhow::Result<HashMap<String, String>> {
    let mut args = vec!["for-each-ref", "--format=%(refname)%00%(objectname)", "--"];
    args.extend(requests.iter().map(|r| r.ref_name.as_str()));
    let output = repo.run_command(&args)?;
    output
        .lines()
        .map(|line| {
            let (name, sha) = line
                .split_once('\0')
                .context("Malformed ref inventory record")?;
            Ok((name.to_owned(), sha.to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestRepo;

    fn request(name: &str, sha: &str) -> (Request, mpsc::Receiver<anyhow::Result<bool>>) {
        let (reply, response) = mpsc::channel();
        (
            Request {
                ref_name: format!("refs/heads/{name}"),
                expected_sha: sha.to_owned(),
                reply,
            },
            response,
        )
    }

    /// A stale member aborts the whole ordinary Git transaction, then only
    /// unchanged peers are retried. Packed+loose overrides expose the data-loss
    /// bug in Git 2.50's --batch-updates, which this owner deliberately avoids.
    #[test]
    fn batch_preserves_moved_and_missing_refs_and_deletes_unchanged_peers() {
        let test = TestRepo::with_initial_commit();
        for branch in ["moved", "missing", "unchanged-a", "unchanged-b"] {
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
        test.run_git(&["update-ref", "-d", "refs/heads/missing"]);
        let repo = Repository::at(test.root_path()).unwrap();
        let (requests, responses): (Vec<_>, Vec<_>) =
            ["moved", "missing", "unchanged-a", "unchanged-b"]
                .map(|name| request(name, &original))
                .into_iter()
                .unzip();
        assert!(delete_batch(&repo, requests).is_none());
        let mut responses = responses.into_iter();
        assert!(!responses.next().unwrap().recv().unwrap().unwrap());
        assert!(responses.next().unwrap().recv().unwrap().is_err());
        for response in responses {
            assert!(response.recv().unwrap().unwrap());
        }
        assert_eq!(test.git_output(&["rev-parse", "moved"]), advanced);
        for branch in ["missing", "unchanged-a", "unchanged-b"] {
            assert!(
                repo.run_command(&[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{branch}")
                ])
                .is_err()
            );
        }
    }

    #[test]
    fn one_ref_lock_does_not_fail_unrelated_batch_members() {
        let test = TestRepo::with_initial_commit();
        for branch in ["first", "locked", "last"] {
            test.run_git(&["branch", branch]);
        }
        test.run_git(&["config", "core.filesRefLockTimeout", "0"]);
        let sha = test.git_output(&["rev-parse", "main"]);
        let repo = Repository::at(test.root_path()).unwrap();
        std::fs::write(repo.git_common_dir().join("refs/heads/locked.lock"), "").unwrap();
        let (requests, responses): (Vec<_>, Vec<_>) = ["first", "locked", "last"]
            .map(|name| request(name, &sha))
            .into_iter()
            .unzip();
        assert!(delete_batch(&repo, requests).is_none());
        let mut responses = responses.into_iter();
        assert!(responses.next().unwrap().recv().unwrap().unwrap());
        let locked_error = responses.next().unwrap().recv().unwrap().unwrap_err();
        assert!(locked_error.display_message().contains("locked.lock"));
        assert!(responses.next().unwrap().recv().unwrap().unwrap());
        assert_eq!(test.git_output(&["rev-parse", "locked"]), sha);
        for branch in ["first", "last"] {
            assert!(
                repo.run_command(&[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{branch}")
                ])
                .is_err()
            );
        }
    }

    #[test]
    fn packed_lock_failure_reaches_every_batch_member_with_original_error() {
        let test = TestRepo::with_initial_commit();
        for branch in ["first", "second"] {
            test.run_git(&["branch", branch]);
        }
        test.run_git(&["pack-refs", "--all"]);
        test.run_git(&["config", "core.packedRefsTimeout", "0"]);
        let sha = test.git_output(&["rev-parse", "main"]);
        let repo = Repository::at(test.root_path()).unwrap();
        std::fs::write(repo.git_common_dir().join("packed-refs.lock"), "").unwrap();
        let (requests, responses): (Vec<_>, Vec<_>) = ["first", "second"]
            .map(|name| request(name, &sha))
            .into_iter()
            .unzip();
        assert!(delete_batch(&repo, requests).is_none());
        for response in responses {
            let error = response.recv().unwrap().unwrap_err();
            assert!(error.display_message().contains("packed-refs.lock"));
            assert!(super::super::CommandError::find_in(&error).is_some());
        }
        for branch in ["first", "second"] {
            assert_eq!(test.git_output(&["rev-parse", branch]), sha);
        }
    }

    #[test]
    fn shared_interrupt_stays_typed_for_every_waiter() {
        let error = Arc::new(anyhow::Error::from(WorktrunkError::Interrupted {
            signal: 2,
            hint: None,
        }));
        let (requests, responses): (Vec<_>, Vec<_>) = ["first", "second"]
            .map(|name| request(name, "unused"))
            .into_iter()
            .unzip();
        reply_error(requests, &error);
        for response in responses {
            let error = response
                .recv()
                .unwrap()
                .unwrap_err()
                .context("branch removal");
            assert_eq!(error.interrupt_signal(), Some(2));
            assert_eq!(error.exit_code(), Some(130));
        }
    }

    #[test]
    fn leader_unwind_replies_to_pending_waiters_and_releases_queue() {
        let queue = RefDeletionQueue::default();
        let (request, response) = request("pending", "unused");
        {
            let mut state = queue.state.lock().unwrap();
            state.active = true;
            state.pending.push(request);
        }
        let unwound = std::panic::catch_unwind(|| {
            let _leader = Leader {
                queue: &queue,
                armed: true,
            };
            panic!("failed coordinator");
        });
        assert!(unwound.is_err());
        assert!(response.recv().unwrap().is_err());
        let state = queue.state.lock().unwrap();
        assert!(!state.active);
        assert!(state.pending.is_empty());
    }
}
