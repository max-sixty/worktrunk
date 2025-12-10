//! JSON output types for `wt list --format=json`
//!
//! This module defines the structured JSON output format, designed for:
//! - Query-friendly filtering with jq
//! - Self-describing field names
//! - Alignment with CLI status subcolumns
//!
//! ## Structure
//!
//! Fields are organized by concept, matching the status display subcolumns:
//! - `working_tree`: staged/modified/untracked changes
//! - `main_state`: relationship to main (would_conflict, same_commit, integrated, diverged, ahead, behind)
//! - `operation_state`: git operations in progress (conflicts, rebase, merge)
//! - `main`: relationship to main branch (ahead/behind/diff counts)
//! - `remote`: relationship to tracking branch
//! - `worktree`: worktree-specific state (locked, prunable, etc.)

use std::path::PathBuf;

use serde::Serialize;
use worktrunk::git::LineDiff;

use super::ci_status::PrStatus;
use super::model::{DivergenceContext, ItemKind, ListItem, UpstreamStatus};

/// JSON output for a single list item
#[derive(Debug, Clone, Serialize)]
pub struct JsonItem {
    /// Branch name, null for detached HEAD
    pub branch: Option<String>,

    /// Filesystem path to the worktree
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,

    /// Item kind: "worktree" or "branch"
    pub kind: &'static str,

    /// Commit information
    pub commit: JsonCommit,

    /// Working tree state (staged, modified, untracked changes)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_tree: Option<JsonWorkingTree>,

    /// Main branch relationship: would_conflict, same_commit, integrated, diverged, ahead, behind
    /// (null for main branch itself)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub main_state: Option<&'static str>,

    /// Why branch is integrated (only present when main_state == "integrated")
    /// Values: ancestor, trees_match, no_added_changes, merge_adds_nothing
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integration_reason: Option<&'static str>,

    /// Git operation in progress: conflicts, rebase, merge (null when none)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_state: Option<&'static str>,

    /// Relationship to main branch (absent when is_main == true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub main: Option<JsonMain>,

    /// Relationship to remote tracking branch (absent when no tracking branch)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<JsonRemote>,

    /// Worktree-specific state
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<JsonWorktree>,

    /// This is the main worktree
    pub is_main: bool,

    /// This is the current worktree (matches $PWD)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_current: bool,

    /// This was the previous worktree (from wt switch)
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub is_previous: bool,

    /// CI status from PR or branch workflow
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pr: Option<JsonPr>,

    /// Pre-formatted statusline for statusline tools (tmux, starship)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statusline: Option<String>,

    /// Raw status symbols without ANSI colors (e.g., "+! ✖ ↑")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbols: Option<String>,
}

/// Commit information
#[derive(Debug, Clone, Serialize)]
pub struct JsonCommit {
    /// Full commit SHA
    pub sha: String,

    /// Short commit SHA (7 characters)
    pub short_sha: String,

    /// Commit message (first line)
    pub message: String,

    /// Unix timestamp of commit
    pub timestamp: i64,
}

/// Working tree state
#[derive(Debug, Clone, Serialize)]
pub struct JsonWorkingTree {
    /// Has staged files (+)
    pub staged: bool,

    /// Has modified files (!)
    pub modified: bool,

    /// Has untracked files (?)
    pub untracked: bool,

    /// Has renamed files (»)
    pub renamed: bool,

    /// Has deleted files (✘)
    pub deleted: bool,

    /// Lines added/deleted in working tree vs HEAD
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<JsonDiff>,

    /// Lines added/deleted in working tree vs main branch
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff_vs_main: Option<JsonDiff>,
}

/// Line diff statistics
#[derive(Debug, Clone, Serialize)]
pub struct JsonDiff {
    pub added: usize,
    pub deleted: usize,
}

impl From<LineDiff> for JsonDiff {
    fn from(d: LineDiff) -> Self {
        Self {
            added: d.added,
            deleted: d.deleted,
        }
    }
}

/// Relationship to main branch
#[derive(Debug, Clone, Serialize)]
pub struct JsonMain {
    /// Commits ahead of main
    pub ahead: usize,

    /// Commits behind main
    pub behind: usize,

    /// Lines added/deleted vs main branch
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<JsonDiff>,
}

/// Relationship to remote tracking branch
#[derive(Debug, Clone, Serialize)]
pub struct JsonRemote {
    /// Remote name (e.g., "origin")
    pub name: String,

    /// Remote branch name (e.g., "feature-login")
    pub branch: String,

    /// Commits ahead of remote
    pub ahead: usize,

    /// Commits behind remote
    pub behind: usize,
}

/// Worktree-specific state
#[derive(Debug, Clone, Serialize)]
pub struct JsonWorktree {
    /// Worktree state: "no_worktree", "path_mismatch", "prunable", "locked" (absent when normal)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<&'static str>,

    /// Reason for locked/prunable state
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    /// HEAD is detached (not on a branch)
    pub detached: bool,

    /// Bare repository
    pub bare: bool,
}

/// CI status from PR or branch workflow
#[derive(Debug, Clone, Serialize)]
pub struct JsonPr {
    /// CI status: "passed", "running", "failed", "conflicts", "no_ci", "error"
    pub ci: &'static str,

    /// Source: "pull_request" or "branch"
    pub source: &'static str,

    /// True if local HEAD differs from remote HEAD (unpushed changes)
    pub stale: bool,

    /// URL to the PR/MR (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl JsonItem {
    /// Convert a ListItem to the new JSON structure
    pub fn from_list_item(item: &ListItem) -> Self {
        let (kind_str, worktree_data) = match &item.kind {
            ItemKind::Worktree(data) => ("worktree", Some(data.as_ref())),
            ItemKind::Branch => ("branch", None),
        };

        let is_main = worktree_data.map(|d| d.is_main).unwrap_or(false);
        let is_current = worktree_data.map(|d| d.is_current).unwrap_or(false);
        let is_previous = worktree_data.map(|d| d.is_previous).unwrap_or(false);

        // Commit info
        let sha = item.head.clone();
        let short_sha = if sha.len() >= 7 {
            sha[..7].to_string()
        } else {
            sha.clone()
        };
        let commit = JsonCommit {
            sha,
            short_sha,
            message: item
                .commit
                .as_ref()
                .map(|c| c.commit_message.clone())
                .unwrap_or_default(),
            timestamp: item.commit.as_ref().map(|c| c.timestamp).unwrap_or(0),
        };

        // Working tree (only for worktrees with status symbols)
        let working_tree = worktree_data.and_then(|data| {
            item.status_symbols.as_ref().map(|symbols| {
                let wt = &symbols.working_tree;
                // working_tree_diff_with_main is Option<Option<LineDiff>>:
                // None = not computed, Some(None) = skipped, Some(Some(diff)) = computed
                let diff_vs_main = data
                    .working_tree_diff_with_main
                    .flatten()
                    .map(JsonDiff::from);
                JsonWorkingTree {
                    staged: wt.staged,
                    modified: wt.modified,
                    untracked: wt.untracked,
                    renamed: wt.renamed,
                    deleted: wt.deleted,
                    diff: data.working_tree_diff.map(JsonDiff::from),
                    diff_vs_main,
                }
            })
        });

        // Main state and integration reason
        let (main_state, integration_reason) = item
            .status_symbols
            .as_ref()
            .map(|symbols| {
                let state = symbols.main_state.as_json_str();
                let reason = symbols.main_state.integration_reason().map(|r| r.into());
                (state, reason)
            })
            .unwrap_or((None, None));

        // Operation state (conflicts, rebase, merge)
        let operation_state = item
            .status_symbols
            .as_ref()
            .and_then(|symbols| symbols.operation_state.as_json_str());

        // Main relationship (absent when is_main)
        let main = if is_main {
            None
        } else {
            item.counts.map(|counts| JsonMain {
                ahead: counts.ahead,
                behind: counts.behind,
                diff: item.branch_diff.map(|bd| JsonDiff::from(bd.diff)),
            })
        };

        // Remote relationship
        let remote = item
            .upstream
            .as_ref()
            .and_then(|u| upstream_to_json(u, &item.branch));

        // Worktree state
        let worktree = worktree_data.map(|data| {
            let (state, reason) = worktree_state_to_json(data, item.status_symbols.as_ref());
            JsonWorktree {
                state,
                reason,
                detached: data.detached,
                bare: data.bare,
            }
        });

        // Path
        let path = worktree_data.map(|d| d.path.clone());

        // PR status
        let pr = item
            .pr_status
            .as_ref()
            .and_then(|opt| opt.as_ref())
            .map(pr_status_to_json);

        // Statusline and symbols (raw, without ANSI codes)
        let statusline = item.display.statusline.clone();
        let symbols = item
            .status_symbols
            .as_ref()
            .map(format_raw_symbols)
            .filter(|s| !s.is_empty());

        JsonItem {
            branch: item.branch.clone(),
            path,
            kind: kind_str,
            commit,
            working_tree,
            main_state,
            integration_reason,
            operation_state,
            main,
            remote,
            worktree,
            is_main,
            is_current,
            is_previous,
            pr,
            statusline,
            symbols,
        }
    }
}

/// Convert UpstreamStatus to JsonRemote
fn upstream_to_json(upstream: &UpstreamStatus, branch: &Option<String>) -> Option<JsonRemote> {
    upstream.active().map(|(remote, ahead, behind)| {
        // Use local branch name since UpstreamStatus only stores the remote name,
        // not the full tracking refspec. In most cases these match (e.g., feature -> origin/feature).
        JsonRemote {
            name: remote.to_string(),
            branch: branch.clone().unwrap_or_default(),
            ahead,
            behind,
        }
    })
}

/// Extract worktree state and reason from WorktreeData
fn worktree_state_to_json(
    data: &super::model::WorktreeData,
    status_symbols: Option<&super::model::StatusSymbols>,
) -> (Option<&'static str>, Option<String>) {
    use super::model::WorktreeState;

    // Check status symbols for worktree state
    if let Some(symbols) = status_symbols {
        match symbols.worktree_state {
            WorktreeState::None => {}
            WorktreeState::Branch => return (Some("no_worktree"), None),
            WorktreeState::PathMismatch => return (Some("path_mismatch"), None),
            WorktreeState::Prunable => return (Some("prunable"), data.prunable.clone()),
            WorktreeState::Locked => return (Some("locked"), data.locked.clone()),
        }
    }

    // Fallback: check direct fields when status_symbols is None
    // This can happen early in progressive rendering before status is computed
    if data.prunable.is_some() {
        return (Some("prunable"), data.prunable.clone());
    }
    if data.locked.is_some() {
        return (Some("locked"), data.locked.clone());
    }

    (None, None)
}

/// Convert PrStatus to JsonPr
fn pr_status_to_json(pr: &PrStatus) -> JsonPr {
    use super::ci_status::{CiSource, CiStatus};

    let ci = match pr.ci_status {
        CiStatus::Passed => "passed",
        CiStatus::Running => "running",
        CiStatus::Failed => "failed",
        CiStatus::Conflicts => "conflicts",
        CiStatus::NoCI => "no_ci",
        CiStatus::Error => "error",
    };

    let source = match pr.source {
        CiSource::PullRequest => "pull_request",
        CiSource::Branch => "branch",
    };

    JsonPr {
        ci,
        source,
        stale: pr.is_stale,
        url: pr.url.clone(),
    }
}

/// Format status symbols as raw characters (no ANSI codes)
fn format_raw_symbols(symbols: &super::model::StatusSymbols) -> String {
    let mut result = String::new();

    // Working tree symbols
    let wt_symbols = symbols.working_tree.to_symbols();
    if !wt_symbols.is_empty() {
        result.push_str(&wt_symbols);
    }

    // Main state (merged: ^✗_⊂↕↑↓)
    let main_state = symbols.main_state.to_string();
    if !main_state.is_empty() {
        result.push_str(&main_state);
    }

    // Upstream divergence
    let upstream_div = symbols
        .upstream_divergence
        .symbol(DivergenceContext::Upstream);
    if !upstream_div.is_empty() {
        result.push_str(upstream_div);
    }

    // Worktree state (operations ✘⤴⤵ take priority over location /⚑⊟⊞)
    let op_state = symbols.operation_state.to_string();
    if !op_state.is_empty() {
        result.push_str(&op_state);
    } else {
        let wt_state = symbols.worktree_state.to_string();
        if !wt_state.is_empty() {
            result.push_str(&wt_state);
        }
    }

    // User marker
    if let Some(ref marker) = symbols.user_marker {
        result.push_str(marker);
    }

    result
}

/// Convert a list of ListItems to JSON output
pub fn to_json_items(items: &[ListItem]) -> Vec<JsonItem> {
    items.iter().map(JsonItem::from_list_item).collect()
}
