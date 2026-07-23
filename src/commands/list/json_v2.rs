//! Schema-2 JSON output for `wt list --format=json` and
//! `wt list statusline --format=json`.
//!
//! One envelope over per-item orthogonal facts, with presentation
//! quarantined under `display`. Selected by `[list] json-schema = 2`;
//! schema 1 is the bare-array format in [`super::json_output`].
//!
//! ## The absence rule
//!
//! - **Absent** — nothing to report: not applicable to this row, not
//!   requested this run (the envelope's `collected` records what was
//!   requested), or determined-empty (no PR, no lock, not integrated).
//! - **`null`** — requested and applicable, but not determined: a gate timed
//!   out, the branch fell past the staleness cutoff, or a forge fetch failed.
//! - **Value** — determined.
//!
//! [`Tri`] encodes the rule. jq treats absent and `null` identically in path
//! expressions, so filters stay one-liners; careful consumers distinguish via
//! `has()`. This is the JSON spelling of the table's `·` placeholder,
//! generalizing the care schema 1 takes only for `working_tree`.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::Serialize;
use worktrunk::git::{
    GitRepoInfo, IntegrationReason, IntegrationSignals, Repository, check_integration,
};

use super::ci_status::{CiSource, CiStatus, PrStatus, ReviewState};
use super::custom_columns::ResolvedCustomColumn;
use super::json_output::{JsonDiff, format_raw_symbols};
use super::model::{ActiveGitOperation, BranchScope, Collected, ItemKind, ListItem, WorktreeData};

/// Tri-state field encoding the absence rule (see module docs).
#[derive(Debug, Clone, PartialEq)]
pub enum Tri<T> {
    /// Nothing to report — the field is skipped entirely.
    Absent,
    /// Requested and applicable but not determined — serializes as `null`.
    Unknown,
    /// Determined.
    Known(T),
}

impl<T> Tri<T> {
    pub fn is_absent(&self) -> bool {
        matches!(self, Tri::Absent)
    }
}

impl<T: Serialize> Serialize for Tri<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            // Absent is normally skipped via `skip_serializing_if`; a bare
            // serialization (e.g. `serde_json::to_value` on the enum alone)
            // degrades to null.
            Tri::Absent | Tri::Unknown => serializer.serialize_none(),
            Tri::Known(v) => v.serialize(serializer),
        }
    }
}

impl<T: JsonSchema> JsonSchema for Tri<T> {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        Option::<T>::schema_name()
    }

    fn schema_id() -> std::borrow::Cow<'static, str> {
        Option::<T>::schema_id()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        Option::<T>::json_schema(generator)
    }
}

/// Root envelope.
#[derive(Debug, Serialize, JsonSchema)]
pub struct JsonEnvelope {
    /// Output schema version. The unversioned bare-array format is 1.
    pub schema: u32,

    /// Repo-wide facts, hoisted out of the items.
    pub repo: JsonRepo,

    /// Which gated fact families this run requested.
    pub collected: Collected,

    /// One entry per row, in table order.
    pub items: Vec<JsonItemV2>,
}

/// Repo-wide facts.
#[derive(Debug, Serialize, JsonSchema)]
pub struct JsonRepo {
    /// The branch every `default_branch` object measures against; absent
    /// when detection failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_branch: Option<String>,

    /// Forge metadata derived from the primary remote; absent when no
    /// remote URL parses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forge: Option<GitRepoInfo>,
}

/// One list row: a worktree, a local branch, or a remote-only branch.
#[derive(Debug, Serialize, JsonSchema)]
pub struct JsonItemV2 {
    /// Branch name; null for a detached-HEAD worktree. Remote rows carry
    /// the bare branch name, with the remote in `remote`.
    pub branch: Option<String>,

    /// Remote name for remote-only branch rows; absent on local rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,

    /// HEAD commit; null for unborn branches.
    pub head: Option<JsonHead>,

    /// Worktree facts; absent on branch-only rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<JsonWorktreeV2>,

    /// Relation to the default branch; absent on the default branch itself.
    #[serde(skip_serializing_if = "Tri::is_absent")]
    pub default_branch: Tri<JsonDefaultBranch>,

    /// Tracking-branch relation; absent when no upstream is configured,
    /// null while unresolved.
    #[serde(skip_serializing_if = "Tri::is_absent")]
    pub upstream: Tri<JsonUpstream>,

    /// Open PR/MR; absent when none exists (or CI wasn't collected), null
    /// when the forge fetch failed.
    #[serde(skip_serializing_if = "Tri::is_absent")]
    pub pr: Tri<JsonPr>,

    /// CI pipeline state; absent when no CI exists (or CI wasn't
    /// collected), null when the forge fetch failed.
    #[serde(skip_serializing_if = "Tri::is_absent")]
    pub checks: Tri<JsonChecks>,

    /// Dev server from the project's `list.url` template; absent when not
    /// configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dev_server: Option<JsonDevServer>,

    /// LLM-generated branch summary; absent when summaries are off or none
    /// was produced, null while pending.
    #[serde(skip_serializing_if = "Tri::is_absent")]
    pub summary: Tri<String>,

    /// Custom variables stored via `wt config state vars`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub vars: BTreeMap<String, String>,

    /// Presentation: rendered strings for humans and prompt tools.
    pub display: JsonDisplay,
}

/// HEAD commit facts.
#[derive(Debug, Serialize, JsonSchema)]
pub struct JsonHead {
    /// Full commit SHA.
    pub sha: String,

    /// Abbreviated per `core.abbrev`, auto-extended for ambiguous prefixes.
    pub short_sha: String,

    /// Commit subject (first line); null when not loaded (e.g. prunable
    /// worktrees).
    pub subject: Option<String>,

    /// Committer time, RFC 3339 UTC; null when not loaded.
    pub committed_at: Option<String>,
}

/// Worktree facts. Location attributes (`locked`, `prunable`,
/// `branch_mismatch`) are independent fields — unlike schema 1's single
/// `state`, they can co-occur.
#[derive(Debug, Serialize, JsonSchema)]
pub struct JsonWorktreeV2 {
    /// Filesystem path.
    pub path: PathBuf,

    /// This is the main worktree.
    pub main: bool,

    /// This is the worktree the command ran from.
    pub current: bool,

    /// This was the previous worktree (`wt switch -`).
    pub previous: bool,

    /// HEAD is detached.
    pub detached: bool,

    /// Present when the worktree is locked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locked: Option<JsonReason>,

    /// Present when git considers the worktree prunable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prunable: Option<JsonReason>,

    /// The checked-out branch doesn't match the branch this worktree was
    /// created for.
    pub branch_mismatch: bool,

    /// In-progress operation: `"rebase"` or `"merge"`; absent when none,
    /// null while unresolved.
    #[serde(skip_serializing_if = "Tri::is_absent")]
    pub operation: Tri<&'static str>,

    /// Working-tree state; null while unresolved.
    pub changes: Option<JsonChanges>,
}

/// Reason payload for `locked` / `prunable`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct JsonReason {
    /// Reason git records; null when none was given.
    pub reason: Option<String>,
}

/// Working-tree change facts.
#[derive(Debug, Serialize, JsonSchema)]
pub struct JsonChanges {
    /// Has staged files.
    pub staged: bool,
    /// Has modified (unstaged) files.
    pub modified: bool,
    /// Has untracked files.
    pub untracked: bool,
    /// Has renamed files.
    pub renamed: bool,
    /// Has deleted files.
    pub deleted: bool,
    /// Tracked files carry merge conflicts; null while unresolved.
    pub conflicted: Option<bool>,
    /// Lines added/deleted vs HEAD; null while unresolved.
    pub diff: Option<JsonDiff>,
}

/// Relation to the default branch — independent facts, not the table's
/// priority-collapsed symbol (that lives in `display.state`).
#[derive(Debug, Serialize, JsonSchema)]
pub struct JsonDefaultBranch {
    /// Commits ahead; null while unresolved (and for orphans).
    pub ahead: Option<usize>,

    /// Commits behind; null while unresolved (and for orphans).
    pub behind: Option<usize>,

    /// Lines added/deleted vs the default branch; null while unresolved.
    pub diff: Option<JsonDiff>,

    /// No merge-base with the default branch; null while unresolved.
    pub orphan: Option<bool>,

    /// How committed content is integrated; absent when determined
    /// not-integrated, null when undetermined (dirty trees skip the
    /// expensive checks).
    #[serde(skip_serializing_if = "Tri::is_absent")]
    pub integration: Tri<JsonIntegration>,

    /// A merge into the default branch would conflict (local `merge-tree`
    /// simulation); null while unresolved.
    pub merge_conflicts: Option<bool>,
}

/// Why committed content counts as integrated.
#[derive(Debug, Serialize, JsonSchema)]
pub struct JsonIntegration {
    /// Which check matched: `"same_commit"`, `"ancestor"`,
    /// `"no_added_changes"`, `"trees_match"`, `"merge_adds_nothing"`, or
    /// `"patch_id_match"`.
    pub reason: &'static str,
}

/// Tracking-branch relation.
#[derive(Debug, Serialize, JsonSchema)]
pub struct JsonUpstream {
    /// Remote name.
    pub remote: String,

    /// Branch name on the remote; null when only the remote is known.
    pub branch: Option<String>,

    /// Commits ahead of the upstream.
    pub ahead: usize,

    /// Commits behind the upstream.
    pub behind: usize,
}

/// Open PR/MR facts.
#[derive(Debug, PartialEq, Serialize, JsonSchema)]
pub struct JsonPr {
    /// Forge number; null when the forge reported none.
    pub number: Option<u64>,

    /// URL to the PR/MR page; null when the forge reported none.
    pub url: Option<String>,

    /// Review state; absent when the forge reports no review signal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewState>,

    /// Whether the PR merges cleanly into its target: false when the forge
    /// reports conflicts, null otherwise (the fetch records only the
    /// conflicted case).
    pub mergeable: Option<bool>,

    /// The repository the PR/MR targets (the upstream for fork PRs);
    /// absent when the URL doesn't parse.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<GitRepoInfo>,
}

/// CI pipeline facts.
#[derive(Debug, PartialEq, Serialize, JsonSchema)]
pub struct JsonChecks {
    /// `"passed"`, `"running"`, or `"failed"`; null when a conflicts report
    /// masked the pipeline outcome.
    pub status: Option<&'static str>,

    /// `"pr"` or `"branch"` (branch workflow).
    pub source: CiSource,

    /// Local HEAD is not what CI ran against.
    pub stale: bool,
}

/// Dev server facts.
#[derive(Debug, Serialize, JsonSchema)]
pub struct JsonDevServer {
    /// URL from the project's `list.url` template.
    pub url: String,

    /// The URL's port is listening; null while unresolved.
    pub listening: Option<bool>,
}

/// Presentation strings. Everything here is a rendering of facts that
/// appear elsewhere in the item.
#[derive(Debug, Serialize, JsonSchema)]
pub struct JsonDisplay {
    /// The table's collapsed default-branch state (one value per row,
    /// highest priority wins); absent when none applies or unresolved.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<&'static str>,

    /// Raw status glyphs without ANSI (e.g. `"+!⊂"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbols: Option<String>,

    /// Pre-formatted one-line status with ANSI colors, for prompt tools.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub statusline: Option<String>,

    /// Rendered `[list.custom-columns]` cells keyed by header; empty cells
    /// omitted.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub columns: BTreeMap<String, String>,
}

/// `IntegrationReason` wire values. Schema 2 uses snake_case throughout;
/// the enum's own serde rename (kebab-case) is shared with other surfaces,
/// so the mapping lives here instead of on the enum.
fn integration_reason_str(reason: IntegrationReason) -> &'static str {
    match reason {
        IntegrationReason::SameCommit => "same_commit",
        IntegrationReason::Ancestor => "ancestor",
        IntegrationReason::NoAddedChanges => "no_added_changes",
        IntegrationReason::TreesMatch => "trees_match",
        IntegrationReason::MergeAddsNothing => "merge_adds_nothing",
        IntegrationReason::PatchIdMatch => "patch_id_match",
    }
}

/// Build the schema-2 envelope.
///
/// Vars come from the bulk config snapshot like schema 1 (see
/// [`super::json_output::to_json_items`] for the rationale).
pub fn to_json_envelope(
    items: &[ListItem],
    custom_columns: &[ResolvedCustomColumn],
    repo: &Repository,
    collected: Collected,
) -> JsonEnvelope {
    let mut all_vars = repo.all_vars_from_snapshot().unwrap_or_default();
    let default_branch = repo.default_branch();
    let ci_provider_override = repo.forge_platform_override();

    let json_items = items
        .iter()
        .map(|item| {
            JsonItemV2::from_list_item(
                item,
                &mut all_vars,
                default_branch.as_deref(),
                collected,
                ci_provider_override.as_deref(),
                custom_columns,
            )
        })
        .collect();

    envelope_with_items(repo, default_branch, collected, json_items)
}

/// Wrap pre-built items in the envelope. Used directly by the statusline
/// path, which builds its single item with pre-fetched vars instead of the
/// bulk snapshot.
///
/// `default_branch` is threaded in rather than resolved here: it must be
/// the same value the items were measured against, and on item-less paths
/// (the statusline outside a worktree) the caller passes `None` so this
/// never triggers detection — `Repository::default_branch()` may reach the
/// network once per fresh repo, which an every-prompt surface must not do.
pub fn envelope_with_items(
    repo: &Repository,
    default_branch: Option<String>,
    collected: Collected,
    items: Vec<JsonItemV2>,
) -> JsonEnvelope {
    JsonEnvelope {
        schema: 2,
        repo: JsonRepo {
            default_branch,
            forge: repo.repo_info(),
        },
        collected,
        items,
    }
}

impl JsonItemV2 {
    pub(crate) fn from_list_item(
        item: &ListItem,
        all_vars: &mut HashMap<String, BTreeMap<String, String>>,
        default_branch: Option<&str>,
        collected: Collected,
        ci_provider_override: Option<&str>,
        custom_columns: &[ResolvedCustomColumn],
    ) -> Self {
        let (worktree_data, remote_scope) = match &item.kind {
            ItemKind::Worktree(data) => (Some(data.as_ref()), false),
            ItemKind::Branch(scope) => (None, *scope == BranchScope::Remote),
        };

        // Remote rows store the remote-qualified short name ("origin/feature");
        // split it into the remote and the bare branch name.
        let (branch, remote) = match (&item.branch, remote_scope) {
            (Some(name), true) => match name.split_once('/') {
                Some((remote, branch)) => (Some(branch.to_string()), Some(remote.to_string())),
                None => (Some(name.clone()), None),
            },
            (name, _) => (name.clone(), None),
        };

        // HEAD — null for unborn branches (no sentinel strings).
        let head =
            (!item.head.is_empty() && item.head != worktrunk::git::NULL_OID).then(|| JsonHead {
                sha: item.head.clone(),
                short_sha: item.short_sha.clone(),
                subject: item.commit.as_ref().map(|c| c.commit_message.clone()),
                committed_at: item
                    .commit
                    .as_ref()
                    .and_then(|c| worktrunk::utils::format_timestamp_iso8601_opt(c.timestamp)),
            });

        let worktree = worktree_data.map(json_worktree);

        // The default branch itself gets no relation object. Matching on the
        // worktree's main flag alone would miss a branch-only row for the
        // default branch, so compare names too. Use the remote-stripped
        // `branch` (not `item.branch`) so a remote-only row of the default
        // branch (`origin/main`) also matches — otherwise it fails the name
        // check and gets a spurious self-referential relation.
        let is_default_branch_row = worktree_data.is_some_and(|d| d.is_main)
            || (branch.is_some() && branch.as_deref() == default_branch);
        let default_branch = if is_default_branch_row {
            Tri::Absent
        } else {
            Tri::Known(json_default_branch(item, worktree_data))
        };

        // Upstream: task unresolved (or seeded on skip) → null; resolved
        // with no upstream configured → absent.
        let upstream = match &item.upstream {
            None => Tri::Unknown,
            Some(_) if item.seeded.upstream => Tri::Unknown,
            Some(status) => match status.active() {
                Some(active) => Tri::Known(JsonUpstream {
                    remote: active.remote.to_string(),
                    branch: active.branch.map(str::to_string),
                    ahead: active.ahead,
                    behind: active.behind,
                }),
                None => Tri::Absent,
            },
        };

        // PR and checks, split from the fetched PrStatus.
        let (pr, checks) = if !collected.ci {
            (Tri::Absent, Tri::Absent)
        } else {
            match &item.pr_status {
                // Requested but the task never reported (timeout).
                None => (Tri::Unknown, Tri::Unknown),
                // Determined: no PR and no branch workflow.
                Some(None) => (Tri::Absent, Tri::Absent),
                Some(Some(status)) => pr_and_checks(status, ci_provider_override),
            }
        };

        let dev_server = item.url.clone().map(|url| JsonDevServer {
            url,
            listening: item.url_active,
        });

        let summary = if !collected.summary {
            Tri::Absent
        } else {
            match &item.summary {
                None => Tri::Unknown,
                Some(None) => Tri::Absent,
                Some(Some(s)) => Tri::Known(s.clone()),
            }
        };

        let vars = take_vars(item.branch.as_deref(), all_vars);
        let columns = columns_map(custom_columns, &item.custom_values);

        let display = JsonDisplay {
            state: item.status_symbols.main_state.and_then(|s| s.as_json_str()),
            symbols: Some(format_raw_symbols(&item.status_symbols)).filter(|s| !s.is_empty()),
            statusline: item.statusline.clone(),
            columns,
        };

        JsonItemV2 {
            branch,
            remote,
            head,
            worktree,
            default_branch,
            upstream,
            pr,
            checks,
            dev_server,
            summary,
            vars,
            display,
        }
    }
}

fn json_worktree(data: &WorktreeData) -> JsonWorktreeV2 {
    // Empty lock/prune reasons (git records the state without a message)
    // become `reason: null`.
    let reason = |r: &Option<String>| {
        r.as_ref().map(|reason| JsonReason {
            reason: Some(reason.clone()).filter(|r| !r.is_empty()),
        })
    };

    let operation = match data.git_operation {
        None => Tri::Unknown,
        Some(ActiveGitOperation::None) => Tri::Absent,
        Some(ActiveGitOperation::Rebase) => Tri::Known("rebase"),
        Some(ActiveGitOperation::Merge) => Tri::Known("merge"),
    };

    let changes = data.working_tree_status.map(|wt| JsonChanges {
        staged: wt.staged,
        modified: wt.modified,
        untracked: wt.untracked,
        renamed: wt.renamed,
        deleted: wt.deleted,
        conflicted: data.has_conflicts,
        diff: data.working_tree_diff.map(JsonDiff::from),
    });

    JsonWorktreeV2 {
        path: data.path.clone(),
        main: data.is_main,
        current: data.is_current,
        previous: data.is_previous,
        detached: data.detached,
        locked: reason(&data.locked),
        prunable: reason(&data.prunable),
        branch_mismatch: data.branch_worktree_mismatch,
        operation,
        changes,
    }
}

fn json_default_branch(item: &ListItem, worktree_data: Option<&WorktreeData>) -> JsonDefaultBranch {
    // Integration is a committed-content fact, so it comes from the same
    // signal fields `wt remove` consults (`check_integration`), not from the
    // table's `main_state` — that collapse folds in working-tree cleanliness,
    // which belongs to `worktree.changes`, and would make a dirty
    // squash-merged branch read differently from a dirty same-commit one.
    //
    // - a positive match on loaded signals → the reason (a dirty tree can be
    //   integrated: the probes run on the committed HEAD regardless)
    // - no match with every signal loaded → absent (determined not-integrated)
    // - any signal unloaded, or seeded on skip → null (undetermined)
    // Orphans get sentinel counts `{0, 0}` from `AheadBehindTask` (there is
    // no merge-base to count against — the table short-circuits them at its
    // orphan tier for the same reason). Guard both readings of the counts:
    // the sentinel is not a same-commit match, and 0/0 is not a real
    // ahead/behind.
    let is_orphan = if item.seeded.orphan {
        None
    } else {
        item.is_orphan
    };
    let counts = match is_orphan {
        Some(false) => item.counts,
        _ => None,
    };
    let signals = IntegrationSignals {
        is_same_commit: match is_orphan {
            Some(false) => counts.map(|c| c.ahead == 0 && c.behind == 0),
            // Unrelated histories cannot be the same commit; unknown
            // orphanness leaves it unknown.
            Some(true) => Some(false),
            None => None,
        },
        is_ancestor: item.is_ancestor,
        has_added_changes: item.has_file_changes,
        trees_match: item.committed_trees_match,
        would_merge_add: item.would_merge_add,
        is_patch_id_match: item.is_patch_id_match,
    };
    let signals_loaded = signals.is_same_commit.is_some()
        && signals.is_ancestor.is_some()
        && signals.has_added_changes.is_some()
        && signals.trees_match.is_some()
        && signals.would_merge_add.is_some()
        && signals.is_patch_id_match.is_some();
    let integration = match check_integration(&signals) {
        // Positive matches are trustworthy even when other signals were
        // seeded: every seed points the negative direction, so a match can
        // only come from a computed signal.
        Some(reason) => Tri::Known(JsonIntegration {
            reason: integration_reason_str(reason),
        }),
        // "Not integrated" is a determination only when every signal was
        // genuinely computed — a seeded signal means a probe never ran.
        None if item.seeded.integration => Tri::Unknown,
        None if signals_loaded => Tri::Absent,
        None => Tri::Unknown,
    };

    // Prefer the dirty-tree conflict probe when it ran and found a dirty
    // tree; otherwise the committed-HEAD merge-tree simulation.
    let merge_conflicts = if item.seeded.merge_conflicts {
        None
    } else {
        match worktree_data.and_then(|d| d.has_working_tree_conflicts) {
            Some(Some(conflicts)) => Some(conflicts),
            _ => item.has_merge_tree_conflicts,
        }
    };

    JsonDefaultBranch {
        ahead: counts.map(|c| c.ahead),
        behind: counts.map(|c| c.behind),
        diff: item.branch_diff.map(|bd| JsonDiff::from(bd.diff)),
        orphan: is_orphan,
        integration,
        merge_conflicts,
    }
}

/// Split a fetched [`PrStatus`] into the PR facts and the pipeline facts.
fn pr_and_checks(
    status: &PrStatus,
    provider_override: Option<&str>,
) -> (Tri<JsonPr>, Tri<JsonChecks>) {
    let checks = match status.ci_status {
        // A fetch error means we know neither whether a PR exists nor what
        // the pipeline did.
        CiStatus::Error => return (Tri::Unknown, Tri::Unknown),
        CiStatus::NoCI => Tri::Absent,
        CiStatus::Conflicts => Tri::Known(JsonChecks {
            // The fetch discards the pipeline outcome when the forge
            // reports conflicts (see `pr.mergeable`).
            status: None,
            source: status.source,
            stale: status.is_stale,
        }),
        CiStatus::Passed | CiStatus::Running | CiStatus::Failed => Tri::Known(JsonChecks {
            status: Some(status.ci_status.into()),
            source: status.source,
            stale: status.is_stale,
        }),
    };

    let pr = if status.source == CiSource::PullRequest {
        let repo = pr_target_repo(status.url.as_deref(), provider_override);
        Tri::Known(JsonPr {
            number: status.number.map(|r| r.number),
            url: status.url.clone(),
            review: status.review_state,
            // The fetch collapses forge mergeability into the status enum,
            // so only the conflicted case is recoverable here.
            mergeable: (status.ci_status == CiStatus::Conflicts).then_some(false),
            repo,
        })
    } else {
        Tri::Absent
    };

    (pr, checks)
}

/// Move a branch's vars out of the pre-fetched map. Shared by both schemas
/// so the move-don't-clone semantics can't drift.
pub(crate) fn take_vars(
    branch: Option<&str>,
    all_vars: &mut HashMap<String, BTreeMap<String, String>>,
) -> BTreeMap<String, String> {
    branch.and_then(|b| all_vars.remove(b)).unwrap_or_default()
}

/// Rendered custom-column cells keyed by header, empty cells omitted.
/// Shared by both schemas.
pub(crate) fn columns_map(
    custom_columns: &[ResolvedCustomColumn],
    values: &[String],
) -> BTreeMap<String, String> {
    custom_columns
        .iter()
        .zip(values)
        .filter(|(_, value)| !value.is_empty())
        .map(|(column, value)| (column.name.clone(), value.clone()))
        .collect()
}

/// The repository a PR/MR targets, derived from its URL. Shared by both
/// schemas so the provider-override plumbing can't drift.
pub(crate) fn pr_target_repo(
    url: Option<&str>,
    provider_override: Option<&str>,
) -> Option<GitRepoInfo> {
    url.and_then(|url| {
        worktrunk::git::remote_ref::repo_info_from_ref_url_with_provider(url, provider_override)
    })
}

#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    use super::*;
    use crate::commands::list::ci_status::PrRef;
    use crate::commands::list::model::{AheadBehind, SeededFacts, UpstreamStatus};

    fn item_with(branch: &str) -> ListItem {
        ListItem::new_branch("a".repeat(40), branch.to_string())
    }

    fn to_value(item: &JsonItemV2) -> serde_json::Value {
        serde_json::to_value(item).unwrap()
    }

    fn convert(item: &ListItem, collected: Collected) -> JsonItemV2 {
        let mut all_vars = HashMap::new();
        JsonItemV2::from_list_item(item, &mut all_vars, Some("main"), collected, None, &[])
    }

    fn pr_status(ci_status: CiStatus, source: CiSource) -> PrStatus {
        PrStatus {
            ci_status,
            source,
            is_stale: false,
            is_priming: false,
            url: None,
            number: None,
            review_state: None,
            title: None,
            body: None,
            author: None,
            comment_count: None,
            updated_at: None,
        }
    }

    #[test]
    fn test_absence_rule_unresolved_vs_determined() {
        // Fresh branch item: nothing resolved. Gated families absent (not
        // requested); ungated families null (requested, undetermined).
        let item = item_with("feature");
        let json = to_value(&convert(&item, Collected::default()));

        // Not requested → absent.
        assert!(json.get("pr").is_none());
        assert!(json.get("checks").is_none());
        assert!(json.get("summary").is_none());
        // Requested but unresolved → null.
        assert!(json.get("upstream").is_some_and(|v| v.is_null()));
        let db = json.get("default_branch").unwrap();
        assert!(db.get("ahead").unwrap().is_null());
        assert!(db.get("orphan").unwrap().is_null());
        assert!(db.get("integration").is_some_and(|v| v.is_null()));
        assert!(db.get("merge_conflicts").unwrap().is_null());
        // Branch row → no worktree, no remote.
        assert!(json.get("worktree").is_none());
        assert!(json.get("remote").is_none());
    }

    #[test]
    fn test_requested_ci_without_result_is_null() {
        let item = item_with("feature");
        let json = to_value(&convert(
            &item,
            Collected {
                ci: true,
                summary: false,
            },
        ));
        assert!(json.get("pr").is_some_and(|v| v.is_null()));
        assert!(json.get("checks").is_some_and(|v| v.is_null()));
    }

    #[test]
    fn test_determined_no_pr_is_absent() {
        let mut item = item_with("feature");
        item.pr_status = Some(None);
        let json = to_value(&convert(
            &item,
            Collected {
                ci: true,
                summary: false,
            },
        ));
        assert!(json.get("pr").is_none());
        assert!(json.get("checks").is_none());
    }

    #[test]
    fn test_pr_and_checks_split() {
        let status = PrStatus {
            is_stale: true,
            url: Some("https://github.com/org/repo/pull/7".to_string()),
            number: Some(PrRef::pr(7)),
            review_state: Some(ReviewState::Approved),
            ..pr_status(CiStatus::Passed, CiSource::PullRequest)
        };
        let (pr, checks) = pr_and_checks(&status, None);
        assert_eq!(
            pr,
            Tri::Known(JsonPr {
                number: Some(7),
                url: Some("https://github.com/org/repo/pull/7".to_string()),
                review: Some(ReviewState::Approved),
                mergeable: None,
                repo: pr_target_repo(Some("https://github.com/org/repo/pull/7"), None),
            })
        );
        assert_eq!(
            checks,
            Tri::Known(JsonChecks {
                status: Some("passed"),
                source: CiSource::PullRequest,
                stale: true,
            })
        );
    }

    #[test]
    fn test_conflicts_becomes_pr_mergeable() {
        let status = PrStatus {
            number: Some(PrRef::pr(9)),
            ..pr_status(CiStatus::Conflicts, CiSource::PullRequest)
        };
        let (pr, checks) = pr_and_checks(&status, None);
        assert_eq!(
            pr,
            Tri::Known(JsonPr {
                number: Some(9),
                url: None,
                review: None,
                mergeable: Some(false),
                repo: None,
            })
        );
        // Conflicts masks the pipeline outcome.
        assert_eq!(
            checks,
            Tri::Known(JsonChecks {
                status: None,
                source: CiSource::PullRequest,
                stale: false,
            })
        );
    }

    #[test]
    fn test_fetch_error_is_unknown() {
        let status = pr_status(CiStatus::Error, CiSource::Branch);
        let (pr, checks) = pr_and_checks(&status, None);
        assert_eq!(pr, Tri::Unknown);
        assert_eq!(checks, Tri::Unknown);
    }

    #[test]
    fn test_branch_workflow_has_checks_but_no_pr() {
        let status = pr_status(CiStatus::Failed, CiSource::Branch);
        let (pr, checks) = pr_and_checks(&status, None);
        assert!(pr.is_absent());
        assert_eq!(
            checks,
            Tri::Known(JsonChecks {
                status: Some("failed"),
                source: CiSource::Branch,
                stale: false,
            })
        );
    }

    #[test]
    fn test_remote_row_splits_branch_and_remote() {
        let mut item = item_with("origin/feature");
        item.kind = ItemKind::Branch(BranchScope::Remote);
        let json = to_value(&convert(&item, Collected::default()));
        assert_eq!(json["branch"], "feature");
        assert_eq!(json["remote"], "origin");
    }

    #[test]
    fn test_unborn_branch_head_is_null() {
        let mut item = item_with("new");
        item.head = worktrunk::git::NULL_OID.to_string();
        let json = to_value(&convert(&item, Collected::default()));
        assert!(json["head"].is_null());
    }

    #[test]
    fn test_default_branch_row_has_no_relation() {
        let item = item_with("main");
        let json = to_value(&convert(&item, Collected::default()));
        assert!(json.get("default_branch").is_none());
    }

    #[test]
    fn test_remote_default_branch_row_has_no_relation() {
        // A remote-only row of the default branch ("origin/main") is still the
        // default branch — the name check must compare the remote-stripped
        // "main", not the raw "origin/main", so it gets no self-referential
        // relation object.
        let mut item = item_with("origin/main");
        item.kind = ItemKind::Branch(BranchScope::Remote);
        let json = to_value(&convert(&item, Collected::default()));
        assert_eq!(json["branch"], "main");
        assert!(json.get("default_branch").is_none());
    }

    /// Seed every integration signal to "not integrated" — the state after
    /// a full drain with no positive match.
    fn load_not_integrated_signals(item: &mut ListItem) {
        item.is_orphan = Some(false);
        item.counts = Some(AheadBehind {
            ahead: 1,
            behind: 0,
        });
        item.is_ancestor = Some(false);
        item.has_file_changes = Some(true);
        item.committed_trees_match = Some(false);
        item.would_merge_add = Some(true);
        item.is_patch_id_match = Some(false);
    }

    #[test]
    fn test_orphan_sentinel_counts_do_not_fabricate_integration() {
        // AheadBehindTask reports orphans with sentinel counts {0, 0}; the
        // sentinel must not read as a same-commit match nor as real counts.
        let mut item = item_with("orphan");
        item.is_orphan = Some(true);
        item.counts = Some(AheadBehind {
            ahead: 0,
            behind: 0,
        });
        let json = to_value(&convert(&item, Collected::default()));
        let db = &json["default_branch"];
        assert_eq!(db["orphan"], true);
        assert!(db["ahead"].is_null(), "sentinel counts must not serialize");
        assert!(db["behind"].is_null());
        assert!(
            db.get("integration").is_some_and(|v| v.is_null()),
            "orphan sentinel must not fabricate same_commit"
        );
    }

    #[test]
    fn test_integration_same_commit_from_signals() {
        let mut item = item_with("feature");
        item.is_orphan = Some(false);
        item.counts = Some(AheadBehind {
            ahead: 0,
            behind: 0,
        });
        let json = to_value(&convert(&item, Collected::default()));
        // A positive match fires even with the other signals unloaded.
        assert_eq!(
            json["default_branch"]["integration"]["reason"],
            "same_commit"
        );
    }

    #[test]
    fn test_integration_positive_match_ignores_cleanliness() {
        // Integration is a committed-content fact: a squash-merged branch
        // reports its reason even though new uncommitted edits exist (the
        // dirty tree lives in worktree.changes, not here).
        let mut item = item_with("feature");
        load_not_integrated_signals(&mut item);
        item.committed_trees_match = Some(true);
        let json = to_value(&convert(&item, Collected::default()));
        assert_eq!(
            json["default_branch"]["integration"]["reason"],
            "trees_match"
        );
    }

    #[test]
    fn test_integration_all_signals_loaded_not_integrated_is_absent() {
        let mut item = item_with("feature");
        load_not_integrated_signals(&mut item);
        let json = to_value(&convert(&item, Collected::default()));
        // Every signal computed and none matched: determined → absent.
        assert!(json["default_branch"].get("integration").is_none());
    }

    #[test]
    fn test_integration_partial_signals_is_null() {
        let mut item = item_with("feature");
        load_not_integrated_signals(&mut item);
        item.would_merge_add = None; // probe still pending
        let json = to_value(&convert(&item, Collected::default()));
        assert!(
            json["default_branch"]
                .get("integration")
                .is_some_and(|v| v.is_null()),
            "unloaded signal cannot rule integration out"
        );
    }

    #[test]
    fn test_seeded_facts_serialize_as_null() {
        // Seeded conservative defaults (timeout, unborn branch) must not
        // read as determined facts.
        let mut item = item_with("feature");
        load_not_integrated_signals(&mut item);
        item.is_orphan = Some(false);
        item.has_merge_tree_conflicts = Some(false);
        item.upstream = Some(UpstreamStatus::default());
        item.seeded = SeededFacts {
            orphan: true,
            upstream: true,
            merge_conflicts: true,
            integration: true,
        };
        let json = to_value(&convert(&item, Collected::default()));
        let db = &json["default_branch"];
        assert!(db["orphan"].is_null());
        assert!(db["merge_conflicts"].is_null());
        assert!(db.get("integration").is_some_and(|v| v.is_null()));
        assert!(json.get("upstream").is_some_and(|v| v.is_null()));
    }

    #[test]
    fn test_seeded_integration_still_reports_positive_match() {
        // Seeds all point the negative direction, so a computed positive
        // signal stays trustworthy even when siblings were seeded.
        let mut item = item_with("feature");
        load_not_integrated_signals(&mut item);
        item.is_ancestor = Some(true);
        item.seeded.integration = true;
        let json = to_value(&convert(&item, Collected::default()));
        assert_eq!(json["default_branch"]["integration"]["reason"], "ancestor");
    }

    #[test]
    fn test_upstream_known_with_branch() {
        let mut item = item_with("feature");
        item.upstream = Some(UpstreamStatus {
            remote: Some("origin".to_string()),
            upstream_short: Some("origin/feature".to_string()),
            ahead: 2,
            behind: 1,
        });
        let json = to_value(&convert(&item, Collected::default()));
        assert_eq!(json["upstream"]["remote"], "origin");
        assert_eq!(json["upstream"]["branch"], "feature");
        assert_eq!(json["upstream"]["ahead"], 2);
    }

    #[test]
    fn test_no_upstream_configured_is_absent() {
        let mut item = item_with("feature");
        item.upstream = Some(UpstreamStatus::default());
        let json = to_value(&convert(&item, Collected::default()));
        assert!(json.get("upstream").is_none());
    }

    fn worktree_item(branch: &str, data: WorktreeData) -> ListItem {
        let mut item = item_with(branch);
        item.kind = ItemKind::Worktree(Box::new(data));
        item
    }

    /// The derived schema must generate cleanly — `Tri<T>`'s JsonSchema
    /// delegation to `Option<T>` is only exercised here until the schema
    /// export ships.
    #[test]
    fn test_schema_generates() {
        let schema = schemars::schema_for!(JsonEnvelope);
        let json = serde_json::to_value(&schema).unwrap();
        assert!(json["properties"]["items"].is_object());
    }

    #[test]
    fn test_integration_reason_str_covers_every_reason() {
        let cases = [
            (IntegrationReason::SameCommit, "same_commit"),
            (IntegrationReason::Ancestor, "ancestor"),
            (IntegrationReason::NoAddedChanges, "no_added_changes"),
            (IntegrationReason::TreesMatch, "trees_match"),
            (IntegrationReason::MergeAddsNothing, "merge_adds_nothing"),
            (IntegrationReason::PatchIdMatch, "patch_id_match"),
        ];
        for (reason, expected) in cases {
            assert_eq!(integration_reason_str(reason), expected);
        }
    }

    #[test]
    fn test_remote_row_without_slash_keeps_name() {
        let mut item = item_with("weird");
        item.kind = ItemKind::Branch(BranchScope::Remote);
        let json = to_value(&convert(&item, Collected::default()));
        assert_eq!(json["branch"], "weird");
        assert!(json.get("remote").is_none());
    }

    #[test]
    fn test_loaded_item_serializes_gated_families() {
        // One item with every gated family loaded: PR + checks through the
        // envelope path, dev server, and a produced summary.
        let mut item = item_with("feature");
        item.pr_status = Some(Some(PrStatus {
            url: Some("https://github.com/org/repo/pull/7".to_string()),
            number: Some(PrRef::pr(7)),
            ..pr_status(CiStatus::Passed, CiSource::PullRequest)
        }));
        item.url = Some("http://127.0.0.1:3000".to_string());
        item.url_active = Some(true);
        item.summary = Some(Some("Adds the login page".to_string()));
        let collected = Collected {
            ci: true,
            summary: true,
        };
        let json = to_value(&convert(&item, collected));
        assert_eq!(json["pr"]["number"], 7);
        assert_eq!(json["checks"]["status"], "passed");
        assert_eq!(json["dev_server"]["url"], "http://127.0.0.1:3000");
        assert_eq!(json["dev_server"]["listening"], true);
        assert_eq!(json["summary"], "Adds the login page");
    }

    #[test]
    fn test_summary_requested_arms() {
        // Requested but pending → null; produced-nothing → absent.
        let collected = Collected {
            ci: false,
            summary: true,
        };
        let json = to_value(&convert(&item_with("feature"), collected));
        assert!(json.get("summary").is_some_and(|v| v.is_null()));

        let mut item = item_with("feature");
        item.summary = Some(None);
        let json = to_value(&convert(&item, collected));
        assert!(json.get("summary").is_none());
    }

    #[test]
    fn test_operation_arms() {
        let unresolved = worktree_item("feature", WorktreeData::default());
        let json = to_value(&convert(&unresolved, Collected::default()));
        assert!(
            json["worktree"]
                .get("operation")
                .is_some_and(|v| v.is_null())
        );

        for (op, expected) in [
            (ActiveGitOperation::Rebase, "rebase"),
            (ActiveGitOperation::Merge, "merge"),
        ] {
            let item = worktree_item(
                "feature",
                WorktreeData {
                    git_operation: Some(op),
                    ..Default::default()
                },
            );
            let json = to_value(&convert(&item, Collected::default()));
            assert_eq!(json["worktree"]["operation"], expected);
        }
    }

    #[test]
    fn test_dirty_tree_conflict_probe_wins() {
        let mut item = worktree_item(
            "feature",
            WorktreeData {
                has_working_tree_conflicts: Some(Some(true)),
                ..Default::default()
            },
        );
        item.has_merge_tree_conflicts = Some(false);
        let json = to_value(&convert(&item, Collected::default()));
        assert_eq!(json["default_branch"]["merge_conflicts"], true);
    }

    #[test]
    fn test_no_ci_checks_absent_pr_known() {
        let status = PrStatus {
            number: Some(PrRef::pr(4)),
            ..pr_status(CiStatus::NoCI, CiSource::PullRequest)
        };
        let (pr, checks) = pr_and_checks(&status, None);
        assert!(checks.is_absent(), "no CI configured is determined-empty");
        assert_eq!(
            pr,
            Tri::Known(JsonPr {
                number: Some(4),
                url: None,
                review: None,
                mergeable: None,
                repo: None,
            })
        );
    }

    #[test]
    fn test_envelope_snapshot_shape() {
        // Serialize a minimal envelope to pin the top-level shape.
        let envelope = JsonEnvelope {
            schema: 2,
            repo: JsonRepo {
                default_branch: Some("main".to_string()),
                forge: None,
            },
            collected: Collected {
                ci: false,
                summary: false,
            },
            items: vec![],
        };
        assert_snapshot!(serde_json::to_string_pretty(&envelope).unwrap(), @r#"
        {
          "schema": 2,
          "repo": {
            "default_branch": "main"
          },
          "collected": {
            "ci": false,
            "summary": false
          },
          "items": []
        }
        "#);
    }
}
