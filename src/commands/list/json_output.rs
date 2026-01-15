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
//! - `main_state`: relationship to the default branch (would_conflict, same_commit, integrated, diverged, ahead, behind)
//! - `operation_state`: git operations in progress (conflicts, rebase, merge)
//! - `main`: relationship to the default branch (ahead/behind/diff counts)
//! - `remote`: relationship to tracking branch
//! - `worktree`: worktree-specific state (locked, prunable, etc.)

use std::path::PathBuf;

use serde::Serialize;
use worktrunk::git::LineDiff;

use super::ci_status::{CiSource, PrStatus};
use super::model::{ItemKind, ListItem, UpstreamStatus};

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

    /// Default branch relationship: would_conflict, same_commit, integrated, diverged, ahead, behind
    /// (null for default branch itself)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub main_state: Option<&'static str>,

    /// Why branch is integrated (only present when main_state == "integrated")
    /// Values: ancestor, trees_match, no_added_changes, merge_adds_nothing
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integration_reason: Option<&'static str>,

    /// Git operation in progress: conflicts, rebase, merge (null when none)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_state: Option<&'static str>,

    /// Relationship to default branch (absent when is_main == true)
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

    /// This is the current worktree (matches repo discovery path: PWD or `-C`)
    pub is_current: bool,

    /// This was the previous worktree (from `worktrunk.history`)
    pub is_previous: bool,

    /// CI status from PR or branch workflow
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ci: Option<JsonCi>,

    /// Dev server URL from project config template
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// Whether the dev server URL's port is listening
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url_active: Option<bool>,

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

/// Relationship to default branch
#[derive(Debug, Clone, Serialize)]
pub struct JsonMain {
    /// Commits ahead of default branch
    pub ahead: usize,

    /// Commits behind default branch
    pub behind: usize,

    /// Lines added/deleted vs default branch
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
    /// Worktree state: "branch_worktree_mismatch", "prunable", "locked" (absent when normal)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<&'static str>,

    /// Reason for locked/prunable state
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,

    /// HEAD is detached (not on a branch)
    pub detached: bool,
}

/// CI status from PR or branch workflow
#[derive(Debug, Clone, Serialize)]
pub struct JsonCi {
    /// CI status: "passed", "running", "failed", "conflicts", "no-ci", "error"
    pub status: &'static str,

    /// Source: "pr" or "branch"
    pub source: CiSource,

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

        let is_main = worktree_data.is_some_and(|d| d.is_main);
        let is_current = worktree_data.is_some_and(|d| d.is_current);
        let is_previous = worktree_data.is_some_and(|d| d.is_previous);

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
                JsonWorkingTree {
                    staged: wt.staged,
                    modified: wt.modified,
                    untracked: wt.untracked,
                    renamed: wt.renamed,
                    deleted: wt.deleted,
                    diff: data.working_tree_diff.map(JsonDiff::from),
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
            }
        });

        // Path
        let path = worktree_data.map(|d| d.path.clone());

        // CI status
        let ci = item
            .pr_status
            .as_ref()
            .and_then(|opt| opt.as_ref())
            .map(JsonCi::from);

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
            ci,
            url: item.url.clone(),
            url_active: item.url_active,
            statusline,
            symbols,
        }
    }
}

/// Convert UpstreamStatus to JsonRemote
fn upstream_to_json(upstream: &UpstreamStatus, branch: &Option<String>) -> Option<JsonRemote> {
    upstream.active().map(|active| {
        // Use local branch name since UpstreamStatus only stores the remote name,
        // not the full tracking refspec. In most cases these match (e.g., feature -> origin/feature).
        JsonRemote {
            name: active.remote.to_string(),
            branch: branch.clone().unwrap_or_default(),
            ahead: active.ahead,
            behind: active.behind,
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
            WorktreeState::BranchWorktreeMismatch => {
                return (Some("branch_worktree_mismatch"), None);
            }
            WorktreeState::Prunable => return (Some("prunable"), data.prunable.clone()),
            WorktreeState::Locked => return (Some("locked"), data.locked.clone()),
        }
    }

    // Fallback: check direct fields when status_symbols is None
    // This can happen early in progressive rendering before status is computed
    if data.is_prunable() {
        return (Some("prunable"), data.prunable.clone());
    }
    if data.locked.is_some() {
        return (Some("locked"), data.locked.clone());
    }

    (None, None)
}

impl From<&PrStatus> for JsonCi {
    fn from(pr: &PrStatus) -> Self {
        Self {
            status: pr.ci_status.into(),
            source: pr.source,
            stale: pr.is_stale,
            url: pr.url.clone(),
        }
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
    let upstream_div = symbols.upstream_divergence.symbol();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::list::ci_status::{CiSource, CiStatus};
    use crate::commands::list::model::{
        Divergence, GitOperationState, MainState, OperationState, StatusSymbols, WorkingTreeStatus,
        WorktreeData, WorktreeState,
    };
    use std::path::PathBuf;

    // ============================================================================
    // JsonDiff Tests
    // ============================================================================

    #[test]
    fn test_json_diff_from_line_diff() {
        let line_diff = LineDiff {
            added: 10,
            deleted: 5,
        };
        let json_diff = JsonDiff::from(line_diff);
        assert_eq!(json_diff.added, 10);
        assert_eq!(json_diff.deleted, 5);
    }

    #[test]
    fn test_json_diff_from_line_diff_zeros() {
        let line_diff = LineDiff {
            added: 0,
            deleted: 0,
        };
        let json_diff = JsonDiff::from(line_diff);
        assert_eq!(json_diff.added, 0);
        assert_eq!(json_diff.deleted, 0);
    }

    // ============================================================================
    // JsonCi::from Tests
    // ============================================================================

    #[test]
    fn test_json_ci_from_passed() {
        let pr = PrStatus {
            ci_status: CiStatus::Passed,
            source: CiSource::PullRequest,
            is_stale: false,
            url: Some("https://github.com/org/repo/pull/123".to_string()),
        };
        let json = JsonCi::from(&pr);
        assert_eq!(json.status, "passed");
        assert_eq!(json.source, CiSource::PullRequest);
        assert!(!json.stale);
        assert_eq!(
            json.url,
            Some("https://github.com/org/repo/pull/123".to_string())
        );
    }

    #[test]
    fn test_json_ci_from_failed_branch() {
        let pr = PrStatus {
            ci_status: CiStatus::Failed,
            source: CiSource::Branch,
            is_stale: true,
            url: None,
        };
        let json = JsonCi::from(&pr);
        assert_eq!(json.status, "failed");
        assert_eq!(json.source, CiSource::Branch);
        assert!(json.stale);
        assert!(json.url.is_none());
    }

    #[test]
    fn test_json_ci_from_running() {
        let pr = PrStatus {
            ci_status: CiStatus::Running,
            source: CiSource::PullRequest,
            is_stale: false,
            url: None,
        };
        let json = JsonCi::from(&pr);
        assert_eq!(json.status, "running");
    }

    #[test]
    fn test_json_ci_from_conflicts() {
        let pr = PrStatus {
            ci_status: CiStatus::Conflicts,
            source: CiSource::PullRequest,
            is_stale: false,
            url: None,
        };
        let json = JsonCi::from(&pr);
        assert_eq!(json.status, "conflicts");
    }

    #[test]
    fn test_json_ci_from_no_ci() {
        let pr = PrStatus {
            ci_status: CiStatus::NoCI,
            source: CiSource::Branch,
            is_stale: false,
            url: None,
        };
        let json = JsonCi::from(&pr);
        assert_eq!(json.status, "no-ci");
    }

    #[test]
    fn test_json_ci_from_error() {
        let pr = PrStatus {
            ci_status: CiStatus::Error,
            source: CiSource::Branch,
            is_stale: false,
            url: None,
        };
        let json = JsonCi::from(&pr);
        assert_eq!(json.status, "error");
    }

    // ============================================================================
    // upstream_to_json Tests
    // ============================================================================

    #[test]
    fn test_upstream_to_json_with_remote() {
        let upstream = UpstreamStatus {
            remote: Some("origin".to_string()),
            ahead: 3,
            behind: 2,
        };
        let branch = Some("feature".to_string());
        let json = upstream_to_json(&upstream, &branch);
        assert!(json.is_some());
        let json = json.unwrap();
        assert_eq!(json.name, "origin");
        assert_eq!(json.branch, "feature");
        assert_eq!(json.ahead, 3);
        assert_eq!(json.behind, 2);
    }

    #[test]
    fn test_upstream_to_json_no_remote() {
        let upstream = UpstreamStatus {
            remote: None,
            ahead: 0,
            behind: 0,
        };
        let branch = Some("feature".to_string());
        let json = upstream_to_json(&upstream, &branch);
        assert!(json.is_none());
    }

    #[test]
    fn test_upstream_to_json_no_branch() {
        let upstream = UpstreamStatus {
            remote: Some("origin".to_string()),
            ahead: 1,
            behind: 0,
        };
        let branch = None;
        let json = upstream_to_json(&upstream, &branch);
        assert!(json.is_some());
        let json = json.unwrap();
        assert_eq!(json.branch, ""); // Empty string when branch is None
    }

    // ============================================================================
    // worktree_state_to_json Tests
    // ============================================================================

    fn make_worktree_data() -> WorktreeData {
        WorktreeData {
            path: PathBuf::from("/test/path"),
            is_main: false,
            is_current: false,
            is_previous: false,
            detached: false,
            locked: None,
            prunable: None,
            working_tree_diff: None,
            git_operation: GitOperationState::None,
            branch_worktree_mismatch: false,
            working_diff_display: None,
        }
    }

    fn make_status_symbols_with_worktree_state(state: WorktreeState) -> StatusSymbols {
        StatusSymbols {
            working_tree: WorkingTreeStatus::default(),
            worktree_state: state,
            main_state: MainState::None,
            operation_state: OperationState::None,
            upstream_divergence: Divergence::None,
            user_marker: None,
        }
    }

    #[test]
    fn test_worktree_state_to_json_none() {
        let data = make_worktree_data();
        let symbols = make_status_symbols_with_worktree_state(WorktreeState::None);
        let (state, reason) = worktree_state_to_json(&data, Some(&symbols));
        assert!(state.is_none());
        assert!(reason.is_none());
    }

    #[test]
    fn test_worktree_state_to_json_no_worktree() {
        let data = make_worktree_data();
        let symbols = make_status_symbols_with_worktree_state(WorktreeState::Branch);
        let (state, reason) = worktree_state_to_json(&data, Some(&symbols));
        assert_eq!(state, Some("no_worktree"));
        assert!(reason.is_none());
    }

    #[test]
    fn test_worktree_state_to_json_branch_worktree_mismatch() {
        let data = make_worktree_data();
        let symbols =
            make_status_symbols_with_worktree_state(WorktreeState::BranchWorktreeMismatch);
        let (state, reason) = worktree_state_to_json(&data, Some(&symbols));
        assert_eq!(state, Some("branch_worktree_mismatch"));
        assert!(reason.is_none());
    }

    #[test]
    fn test_worktree_state_to_json_locked() {
        let mut data = make_worktree_data();
        data.locked = Some("manual lock".to_string());
        let symbols = make_status_symbols_with_worktree_state(WorktreeState::Locked);
        let (state, reason) = worktree_state_to_json(&data, Some(&symbols));
        assert_eq!(state, Some("locked"));
        assert_eq!(reason, Some("manual lock".to_string()));
    }

    #[test]
    fn test_worktree_state_to_json_prunable() {
        let mut data = make_worktree_data();
        data.prunable = Some("gitdir file missing".to_string());
        let symbols = make_status_symbols_with_worktree_state(WorktreeState::Prunable);
        let (state, reason) = worktree_state_to_json(&data, Some(&symbols));
        assert_eq!(state, Some("prunable"));
        assert_eq!(reason, Some("gitdir file missing".to_string()));
    }

    #[test]
    fn test_worktree_state_to_json_fallback_prunable() {
        let mut data = make_worktree_data();
        data.prunable = Some("missing gitdir".to_string());
        // No status symbols provided - fallback to data fields
        let (state, reason) = worktree_state_to_json(&data, None);
        assert_eq!(state, Some("prunable"));
        assert_eq!(reason, Some("missing gitdir".to_string()));
    }

    #[test]
    fn test_worktree_state_to_json_fallback_locked() {
        let mut data = make_worktree_data();
        data.locked = Some("in use".to_string());
        let (state, reason) = worktree_state_to_json(&data, None);
        assert_eq!(state, Some("locked"));
        assert_eq!(reason, Some("in use".to_string()));
    }

    // ============================================================================
    // format_raw_symbols Tests
    // ============================================================================

    #[test]
    fn test_format_raw_symbols_empty() {
        let symbols = StatusSymbols::default();
        let result = format_raw_symbols(&symbols);
        assert!(result.is_empty());
    }

    #[test]
    fn test_format_raw_symbols_working_tree() {
        let symbols = StatusSymbols {
            working_tree: WorkingTreeStatus::new(true, true, true, false, false),
            ..Default::default()
        };
        let result = format_raw_symbols(&symbols);
        assert!(result.contains('+'));
        assert!(result.contains('!'));
        assert!(result.contains('?'));
    }

    #[test]
    fn test_format_raw_symbols_main_state() {
        let symbols = StatusSymbols {
            main_state: MainState::Ahead,
            ..Default::default()
        };
        let result = format_raw_symbols(&symbols);
        assert!(result.contains('↑'));
    }

    #[test]
    fn test_format_raw_symbols_upstream_divergence() {
        let symbols = StatusSymbols {
            upstream_divergence: Divergence::Behind,
            ..Default::default()
        };
        let result = format_raw_symbols(&symbols);
        assert!(result.contains('⇣'));
    }

    #[test]
    fn test_format_raw_symbols_operation_state() {
        let symbols = StatusSymbols {
            operation_state: OperationState::Rebase,
            ..Default::default()
        };
        let result = format_raw_symbols(&symbols);
        assert!(result.contains('⤴'));
    }

    #[test]
    fn test_format_raw_symbols_worktree_state() {
        let symbols = StatusSymbols {
            worktree_state: WorktreeState::Locked,
            ..Default::default()
        };
        let result = format_raw_symbols(&symbols);
        assert!(result.contains('⊞'));
    }

    #[test]
    fn test_format_raw_symbols_user_marker() {
        let symbols = StatusSymbols {
            user_marker: Some("🔥".to_string()),
            ..Default::default()
        };
        let result = format_raw_symbols(&symbols);
        assert!(result.contains("🔥"));
    }

    #[test]
    fn test_format_raw_symbols_combined() {
        let symbols = StatusSymbols {
            working_tree: WorkingTreeStatus::new(true, false, false, false, false),
            main_state: MainState::Behind,
            upstream_divergence: Divergence::Ahead,
            ..Default::default()
        };
        let result = format_raw_symbols(&symbols);
        assert!(result.contains('+'));
        assert!(result.contains('↓'));
        assert!(result.contains('⇡'));
    }

    // ============================================================================
    // JSON Serialization Tests
    // ============================================================================

    #[test]
    fn test_json_commit_serialization() {
        let commit = JsonCommit {
            sha: "abc123def456".to_string(),
            short_sha: "abc123d".to_string(),
            message: "Fix bug".to_string(),
            timestamp: 1700000000,
        };
        let json = serde_json::to_string(&commit).unwrap();
        assert!(json.contains("abc123def456"));
        assert!(json.contains("Fix bug"));
        assert!(json.contains("1700000000"));
    }

    #[test]
    fn test_json_working_tree_serialization() {
        let wt = JsonWorkingTree {
            staged: true,
            modified: false,
            untracked: true,
            renamed: false,
            deleted: false,
            diff: Some(JsonDiff {
                added: 10,
                deleted: 5,
            }),
        };
        let json = serde_json::to_string(&wt).unwrap();
        assert!(json.contains("\"staged\":true"));
        assert!(json.contains("\"modified\":false"));
        assert!(json.contains("\"added\":10"));
    }

    #[test]
    fn test_json_main_serialization() {
        let main = JsonMain {
            ahead: 3,
            behind: 1,
            diff: Some(JsonDiff {
                added: 50,
                deleted: 20,
            }),
        };
        let json = serde_json::to_string(&main).unwrap();
        assert!(json.contains("\"ahead\":3"));
        assert!(json.contains("\"behind\":1"));
    }

    #[test]
    fn test_json_remote_serialization() {
        let remote = JsonRemote {
            name: "origin".to_string(),
            branch: "feature".to_string(),
            ahead: 2,
            behind: 0,
        };
        let json = serde_json::to_string(&remote).unwrap();
        assert!(json.contains("\"name\":\"origin\""));
        assert!(json.contains("\"branch\":\"feature\""));
    }

    #[test]
    fn test_json_worktree_serialization() {
        let wt = JsonWorktree {
            state: Some("locked"),
            reason: Some("manual".to_string()),
            detached: false,
        };
        let json = serde_json::to_string(&wt).unwrap();
        assert!(json.contains("\"state\":\"locked\""));
        assert!(json.contains("\"reason\":\"manual\""));
    }

    #[test]
    fn test_json_ci_serialization() {
        let ci = JsonCi {
            status: "passed",
            source: CiSource::PullRequest,
            stale: false,
            url: Some("https://example.com".to_string()),
        };
        let json = serde_json::to_string(&ci).unwrap();
        assert!(json.contains("\"status\":\"passed\""));
        assert!(json.contains("\"source\":\"pr\""));
    }
}
