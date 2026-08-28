//! Schema-2 JSON output for `wt list --format=json` and
//! `wt list statusline --format=json`.
//!
//! One envelope over per-item orthogonal facts, with presentation
//! quarantined under `display`. Selected by `[list] json-schema = 2`;
//! schema 1 is the bare-array format in [`super::json_output`].
//!
//! A JSON Schema derived from these types is published at
//! `https://worktrunk.dev/schema/list-v2.json`, regenerated from
//! `wt list --print-schema` by `test_docs_are_in_sync`. It is generated under
//! schemars' *serialize* contract, so `skip_serializing_if` fields are
//! optional there — adding a field whose absence the schema should allow
//! needs that attribute, not just an `Option`.
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
    GitRepoInfo, InProgressOperation, IntegrationReason, IntegrationSignals, Repository,
    check_integration,
};

use super::ci_status::{CiSource, CiStatus, PrStatus, ReviewState};
use super::custom_columns::ResolvedCustomColumn;
use super::json_output::{JsonDiff, format_raw_symbols};
use super::model::{BranchScope, Collected, ItemKind, ListItem, MainState, WorktreeData};

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

/// The published JSON Schema for [`JsonEnvelope`], as a JSON value.
///
/// `wt list --print-schema` prints this and `test_docs_are_in_sync` commits it
/// to `docs/public/schema/list-v2.json`; it lives here so the document and the
/// types it describes stay in one place, and so `test_schema_accepts_envelopes`
/// checks the same document that ships.
///
/// Generated under schemars' *serialize* contract. The default deserialize
/// contract marks a `skip_serializing_if` field required — nothing supplies it
/// on the way in — which would make the schema reject the output it documents.
pub fn schema_document() -> serde_json::Value {
    let settings = schemars::generate::SchemaSettings::default().for_serialize();
    let schema = schemars::SchemaGenerator::new(settings).into_root_schema_for::<JsonEnvelope>();

    let mut value = serde_json::to_value(&schema).expect("schema serializes");
    let object = value.as_object_mut().expect("schema is an object");
    // Astro serves docs/public/ at the site root, so this is where the
    // committed docs/public/schema/list-v2.json is reachable.
    object.insert(
        "$id".to_string(),
        "https://worktrunk.dev/schema/list-v2.json".into(),
    );
    object.insert(
        "title".to_string(),
        "wt list --format=json (schema 2)".into(),
    );
    value
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
/// `branch_mismatch`, `duplicate_branch`) are independent fields — unlike
/// schema 1's single `state`, they can co-occur.
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

    /// Another worktree has the same branch checked out.
    pub duplicate_branch: bool,

    /// In-progress operation; absent when none, null while unresolved.
    #[serde(skip_serializing_if = "Tri::is_absent")]
    pub operation: Tri<JsonOperation>,

    /// Working-tree state; null while unresolved.
    pub changes: Option<JsonChanges>,
}

/// [`InProgressOperation`] wire values. The domain enum's `strum` names are
/// the same strings, but going through an exhaustive match is what makes a
/// sixth variant a compile error here instead of a value that appears in
/// published output with nothing to catch it.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JsonOperation {
    Merge,
    Rebase,
    CherryPick,
    Revert,
    Bisect,
}

impl From<InProgressOperation> for JsonOperation {
    fn from(operation: InProgressOperation) -> Self {
        match operation {
            InProgressOperation::Merge => Self::Merge,
            InProgressOperation::Rebase => Self::Rebase,
            InProgressOperation::CherryPick => Self::CherryPick,
            InProgressOperation::Revert => Self::Revert,
            InProgressOperation::Bisect => Self::Bisect,
        }
    }
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

    /// A merge into the default branch would conflict based on committed
    /// content and tracked worktree changes (local `merge-tree` simulation);
    /// null while unresolved.
    pub merge_conflicts: Option<bool>,
}

/// Why committed content counts as integrated.
#[derive(Debug, Serialize, JsonSchema)]
pub struct JsonIntegration {
    /// Which check matched.
    pub reason: JsonIntegrationReason,
}

/// `IntegrationReason` wire values. Schema 2 uses snake_case throughout; the
/// enum's own serde rename (kebab-case) is shared with other surfaces, so the
/// mapping lives here instead of on the enum.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JsonIntegrationReason {
    SameCommit,
    Ancestor,
    NoAddedChanges,
    TreesMatch,
    MergeAddsNothing,
    PatchIdMatch,
}

impl From<IntegrationReason> for JsonIntegrationReason {
    fn from(reason: IntegrationReason) -> Self {
        match reason {
            IntegrationReason::SameCommit => Self::SameCommit,
            IntegrationReason::Ancestor => Self::Ancestor,
            IntegrationReason::NoAddedChanges => Self::NoAddedChanges,
            IntegrationReason::TreesMatch => Self::TreesMatch,
            IntegrationReason::MergeAddsNothing => Self::MergeAddsNothing,
            IntegrationReason::PatchIdMatch => Self::PatchIdMatch,
        }
    }
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
    /// Pipeline outcome; null when a conflicts report masked it.
    pub status: Option<JsonCheckStatus>,

    /// `"pr"` or `"branch"` (branch workflow).
    pub source: CiSource,

    /// Local HEAD is not what CI ran against.
    pub stale: bool,
}

/// The pipeline outcomes that reach JSON. [`CiStatus`] carries three more
/// that never appear here, because each is reported by the shape of `checks`
/// rather than by a value inside it: `Error` makes both `pr` and `checks`
/// unknown, `NoCI` makes `checks` absent, and `Conflicts` leaves `status`
/// null (the conflict itself surfaces as `pr.mergeable`). See
/// [`pr_and_checks`].
#[derive(Debug, PartialEq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JsonCheckStatus {
    Passed,
    Running,
    Failed,
}

impl JsonCheckStatus {
    /// `None` for the statuses the shape of `checks` reports instead (see the
    /// type docs). `Error` and `NoCI` never reach a constructed `JsonChecks`.
    fn from_ci_status(status: CiStatus) -> Option<Self> {
        match status {
            CiStatus::Passed => Some(Self::Passed),
            CiStatus::Running => Some(Self::Running),
            CiStatus::Failed => Some(Self::Failed),
            CiStatus::Conflicts | CiStatus::NoCI | CiStatus::Error => None,
        }
    }
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
    pub state: Option<JsonMainState>,

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

/// [`MainState`] wire values. `MainState::None` renders as no value at all,
/// so it has no variant here — `display.state` is absent instead. The
/// `Integrated` payload is dropped: the reason it carries is already reported
/// as `default_branch.integration.reason`.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JsonMainState {
    IsMain,
    WouldConflict,
    Empty,
    SameCommit,
    Integrated,
    Orphan,
    Diverged,
    Ahead,
    Behind,
}

impl JsonMainState {
    /// `None` for [`MainState::None`], which the table renders as no symbol.
    fn from_main_state(state: MainState) -> Option<Self> {
        match state {
            MainState::None => None,
            MainState::IsMain => Some(Self::IsMain),
            MainState::WouldConflict => Some(Self::WouldConflict),
            MainState::Empty => Some(Self::Empty),
            MainState::SameCommit => Some(Self::SameCommit),
            MainState::Integrated(_) => Some(Self::Integrated),
            MainState::Orphan => Some(Self::Orphan),
            MainState::Diverged => Some(Self::Diverged),
            MainState::Ahead => Some(Self::Ahead),
            MainState::Behind => Some(Self::Behind),
        }
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
    let ci_provider_override = repo.configured_forge_platform();

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
        let (worktree_data, remote_scope) = match item.kind() {
            ItemKind::Worktree(data) => (Some(data.as_ref()), false),
            ItemKind::Branch(scope) => (None, *scope == BranchScope::Remote),
        };

        // Remote rows store the remote-qualified short name ("origin/feature");
        // split it into the remote and the bare branch name.
        let (branch, remote) = match (item.branch(), remote_scope) {
            (Some(name), true) => match name.split_once('/') {
                Some((remote, branch)) => (Some(branch.to_string()), Some(remote.to_string())),
                None => (Some(name.to_string()), None),
            },
            (name, _) => (name.map(str::to_string), None),
        };

        // HEAD — null for unborn branches (no sentinel strings).
        let head =
            (!item.head().is_empty() && item.head() != worktrunk::git::NULL_OID).then(|| {
                JsonHead {
                    sha: item.head().to_string(),
                    short_sha: item.short_sha.clone(),
                    subject: item.commit.as_ref().map(|c| c.commit_message.clone()),
                    committed_at: item
                        .commit
                        .as_ref()
                        .and_then(|c| worktrunk::utils::format_timestamp_iso8601_opt(c.timestamp)),
                }
            });

        let worktree = worktree_data.map(|data| {
            json_worktree(
                data,
                item.worktree_path()
                    .expect("typed worktree rows always carry a worktree path"),
            )
        });

        // The default branch itself gets no relation object. Matching on the
        // worktree's main flag alone would miss a branch-only row for the
        // default branch, so compare names too. Use the remote-stripped
        // `branch`, rather than the remote-qualified name
        // returned by `item.branch()`, so a remote-only row of the default
        // branch (`origin/main`) also matches. Otherwise it gets a spurious
        // self-referential relation.
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

        let vars = take_vars(item.branch(), all_vars);
        let columns = columns_map(custom_columns, &item.custom_values);

        let display = JsonDisplay {
            state: item
                .status_symbols
                .main_state
                .and_then(JsonMainState::from_main_state),
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

fn json_worktree(data: &WorktreeData, path: &std::path::Path) -> JsonWorktreeV2 {
    // Empty lock/prune reasons (git records the state without a message)
    // become `reason: null`.
    let reason = |r: &Option<String>| {
        r.as_ref().map(|reason| JsonReason {
            reason: Some(reason.clone()).filter(|r| !r.is_empty()),
        })
    };

    let operation = match data.git_operation {
        None => Tri::Unknown,
        Some(None) => Tri::Absent,
        Some(Some(operation)) => Tri::Known(operation.into()),
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
        path: path.to_path_buf(),
        main: data.is_main,
        current: data.is_current,
        previous: data.is_previous,
        detached: data.detached,
        locked: reason(&data.locked),
        prunable: reason(&data.prunable),
        branch_mismatch: data.branch_worktree_mismatch,
        duplicate_branch: data.duplicate_branch,
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
            reason: reason.into(),
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
        // Conflicts lands here with a null `status`: the fetch discards the
        // pipeline outcome when the forge reports conflicts, which surfaces
        // as `pr.mergeable` instead.
        CiStatus::Conflicts | CiStatus::Passed | CiStatus::Running | CiStatus::Failed => {
            Tri::Known(JsonChecks {
                status: JsonCheckStatus::from_ci_status(status.ci_status),
                source: status.source,
                stale: status.is_stale,
            })
        }
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
    use worktrunk::git::InProgressOperation;

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
                status: Some(JsonCheckStatus::Passed),
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
                status: Some(JsonCheckStatus::Failed),
                source: CiSource::Branch,
                stale: false,
            })
        );
    }

    #[test]
    fn test_remote_row_splits_branch_and_remote() {
        let mut item = item_with("origin/feature");
        item.reclassify_as_branch(BranchScope::Remote, "origin/feature".into());
        let json = to_value(&convert(&item, Collected::default()));
        assert_eq!(json["branch"], "feature");
        assert_eq!(json["remote"], "origin");
    }

    #[test]
    fn test_unborn_branch_head_is_null() {
        let item = ListItem::new_branch(worktrunk::git::NULL_OID.to_string(), "new".into());
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
        item.reclassify_as_branch(BranchScope::Remote, "origin/main".into());
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
        ListItem::new_worktree(
            worktrunk::git::WorktreeRef::new("/test", Some(branch), "abc123"),
            data,
        )
    }

    /// `JsonIntegrationReason` covers every `IntegrationReason`, under the
    /// snake_case wire names schema 2 publishes.
    #[test]
    fn test_integration_reason_wire_names() {
        let cases = [
            (IntegrationReason::SameCommit, "same_commit"),
            (IntegrationReason::Ancestor, "ancestor"),
            (IntegrationReason::NoAddedChanges, "no_added_changes"),
            (IntegrationReason::TreesMatch, "trees_match"),
            (IntegrationReason::MergeAddsNothing, "merge_adds_nothing"),
            (IntegrationReason::PatchIdMatch, "patch_id_match"),
        ];
        for (reason, expected) in cases {
            let wire = serde_json::to_value(JsonIntegrationReason::from(reason)).unwrap();
            assert_eq!(wire, expected);
        }
    }

    /// Every [`MainState`] the table can show reaches `display.state` with a
    /// wire name, and `None` reaches it as absent.
    #[test]
    fn test_main_state_wire_names() {
        let cases = [
            (MainState::None, None),
            (MainState::IsMain, Some("is_main")),
            (MainState::WouldConflict, Some("would_conflict")),
            (MainState::Empty, Some("empty")),
            (MainState::SameCommit, Some("same_commit")),
            (
                MainState::Integrated(IntegrationReason::Ancestor),
                Some("integrated"),
            ),
            (MainState::Orphan, Some("orphan")),
            (MainState::Diverged, Some("diverged")),
            (MainState::Ahead, Some("ahead")),
            (MainState::Behind, Some("behind")),
        ];
        for (state, expected) in cases {
            let wire = JsonMainState::from_main_state(state).map(|s| {
                serde_json::to_value(s)
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_string()
            });
            assert_eq!(wire.as_deref(), expected, "for {state:?}");
        }
    }

    #[test]
    fn test_remote_row_without_slash_keeps_name() {
        let mut item = item_with("weird");
        item.reclassify_as_branch(BranchScope::Remote, "weird".into());
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

        // Every operation keeps its own name in JSON, including the three that
        // share the `↻` symbol with rebase and merge.
        for (op, expected) in [
            (InProgressOperation::Rebase, "rebase"),
            (InProgressOperation::Merge, "merge"),
            (InProgressOperation::CherryPick, "cherry_pick"),
            (InProgressOperation::Revert, "revert"),
            (InProgressOperation::Bisect, "bisect"),
        ] {
            let item = worktree_item(
                "feature",
                WorktreeData {
                    git_operation: Some(Some(op)),
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

    /// Every path in `value` holding a non-null value, with array indices
    /// collapsed to `*` so a path matches whichever battery row supplies it.
    fn non_null_paths(value: &serde_json::Value) -> std::collections::BTreeSet<String> {
        fn walk(
            value: &serde_json::Value,
            path: &str,
            out: &mut std::collections::BTreeSet<String>,
        ) {
            if value.is_null() {
                return;
            }
            out.insert(path.to_string());
            match value {
                serde_json::Value::Object(map) => {
                    for (key, child) in map {
                        walk(child, &format!("{path}/{key}"), out);
                    }
                }
                serde_json::Value::Array(entries) => {
                    for child in entries {
                        walk(child, &format!("{path}/*"), out);
                    }
                }
                _ => {}
            }
        }
        let mut out = std::collections::BTreeSet::new();
        walk(value, "", &mut out);
        out
    }

    /// The published schema must accept what `wt list --format=json`
    /// actually writes.
    ///
    /// schemars derives the document from the types without ever seeing
    /// output, so nothing else pins the two together. This caught the schema
    /// being generated under the deserialize contract, where every
    /// `skip_serializing_if` field is required and the document rejects its
    /// own output.
    ///
    /// The battery walks the axes the schema is most likely to get wrong:
    /// every `CiStatus` (which drives the `pr`/`checks` tri-state arms and
    /// `JsonCheckStatus`) and every `MainState` (which drives
    /// `JsonMainState`).
    #[test]
    fn test_schema_accepts_envelopes() {
        let collected = Collected {
            ci: true,
            summary: true,
        };

        let mut items = Vec::new();

        // Bare row: every gated family absent.
        items.push(convert(
            &item_with("bare"),
            Collected {
                ci: false,
                summary: false,
            },
        ));

        // A fully-populated worktree row. `worktree` and everything under it
        // appear nowhere else in the battery, and each field left at its
        // default takes one more type out of the validator's reach: git
        // records a lock with or without a message, so both `JsonReason` arms
        // need a row.
        items.push(convert(
            &worktree_item(
                "worktree",
                WorktreeData {
                    git_operation: Some(Some(InProgressOperation::Rebase)),
                    locked: Some("manual lock".to_string()),
                    prunable: Some(String::new()),
                    working_tree_status: Some(Default::default()),
                    has_conflicts: Some(true),
                    working_tree_diff: Some(worktrunk::git::LineDiff {
                        added: 3,
                        deleted: 1,
                    }),
                    ..Default::default()
                },
            ),
            collected,
        ));

        // An integrated row with an upstream and a dev server. Without the
        // signals `check_integration` returns `None` and `integration` is
        // null throughout, so `JsonIntegration` and `JsonIntegrationReason`
        // would never meet a real object.
        let mut integrated = item_with("integrated");
        integrated.is_ancestor = Some(true);
        integrated.is_orphan = Some(false);
        integrated.has_file_changes = Some(false);
        integrated.committed_trees_match = Some(true);
        integrated.would_merge_add = Some(false);
        integrated.is_patch_id_match = Some(true);
        integrated.upstream = Some(UpstreamStatus {
            remote: Some("origin".to_string()),
            upstream_short: Some("origin/integrated".to_string()),
            ahead: 2,
            behind: 1,
        });
        integrated.url = Some("http://127.0.0.1:3000".to_string());
        integrated.url_active = Some(true);
        items.push(convert(&integrated, collected));

        // Every CI status, over both sources.
        for source in [CiSource::PullRequest, CiSource::Branch] {
            for ci in [
                CiStatus::Passed,
                CiStatus::Running,
                CiStatus::Failed,
                CiStatus::Conflicts,
                CiStatus::NoCI,
                CiStatus::Error,
            ] {
                let mut item = item_with("ci");
                item.pr_status = Some(Some(PrStatus {
                    url: Some("https://github.com/org/repo/pull/7".to_string()),
                    number: Some(PrRef::pr(7)),
                    review_state: Some(ReviewState::Approved),
                    ..pr_status(ci, source)
                }));
                items.push(convert(&item, collected));
            }
        }

        // Every collapsed default-branch state.
        for state in [
            MainState::None,
            MainState::IsMain,
            MainState::WouldConflict,
            MainState::Empty,
            MainState::SameCommit,
            MainState::Integrated(IntegrationReason::PatchIdMatch),
            MainState::Orphan,
            MainState::Diverged,
            MainState::Ahead,
            MainState::Behind,
        ] {
            let mut item = item_with("state");
            item.status_symbols.main_state = Some(state);
            items.push(convert(&item, collected));
        }

        // Requested-but-undetermined: the null arm of the absence rule.
        let mut seeded = item_with("seeded");
        seeded.summary = Some(None);
        seeded.pr_status = Some(None);
        items.push(convert(&seeded, collected));

        let envelope = JsonEnvelope {
            schema: 2,
            repo: JsonRepo {
                default_branch: Some("main".to_string()),
                // `forge` is the only place `GitRepoInfo` and
                // `GitRepoProvider` appear in the document.
                forge: Some(GitRepoInfo {
                    url: "https://github.com/org/repo".to_string(),
                    provider: worktrunk::git::GitRepoProvider::GitHub,
                    host: "github.com".to_string(),
                    owner: "org".to_string(),
                    name: "repo".to_string(),
                    project: None,
                    remote: Some("origin".to_string()),
                }),
            },
            collected,
            items,
        };

        let schema = schema_document();
        let validator = jsonschema::validator_for(&schema).expect("schema compiles");
        let instance = serde_json::to_value(&envelope).unwrap();

        // Validating proves nothing about a type the battery never
        // instantiates: a document can accept an envelope while whole
        // subtrees of it go unexercised. Pin every type the document defines
        // to a path that must carry a real value, so a new `Json*` type fails
        // here until the battery reaches it, and a row that stops populating
        // one fails too.
        let reached = non_null_paths(&instance);
        let pinned: &[(&str, &str)] = &[
            ("CiSource", "/items/*/checks/source"),
            ("Collected", "/collected"),
            ("GitRepoInfo", "/repo/forge"),
            ("GitRepoProvider", "/repo/forge/provider"),
            ("JsonChanges", "/items/*/worktree/changes"),
            ("JsonCheckStatus", "/items/*/checks/status"),
            ("JsonChecks", "/items/*/checks"),
            ("JsonDefaultBranch", "/items/*/default_branch"),
            ("JsonDevServer", "/items/*/dev_server"),
            ("JsonDiff", "/items/*/worktree/changes/diff"),
            ("JsonDisplay", "/items/*/display"),
            ("JsonHead", "/items/*/head"),
            ("JsonIntegration", "/items/*/default_branch/integration"),
            (
                "JsonIntegrationReason",
                "/items/*/default_branch/integration/reason",
            ),
            ("JsonItemV2", "/items/*"),
            ("JsonMainState", "/items/*/display/state"),
            ("JsonOperation", "/items/*/worktree/operation"),
            ("JsonPr", "/items/*/pr"),
            ("JsonReason", "/items/*/worktree/locked"),
            ("JsonRepo", "/repo"),
            ("JsonUpstream", "/items/*/upstream"),
            ("JsonWorktreeV2", "/items/*/worktree"),
            ("ReviewState", "/items/*/pr/review"),
        ];

        // `Nullable_X` is a schemars wrapper over `X`, which carries its own
        // row, so it needs no path of its own.
        let defined: Vec<&str> = schema["$defs"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .filter(|name| !name.starts_with("Nullable_"))
            .collect();
        let unpinned: Vec<&str> = defined
            .iter()
            .copied()
            .filter(|name| !pinned.iter().any(|(pinned_name, _)| pinned_name == name))
            .collect();
        assert!(
            unpinned.is_empty(),
            "published types with no path pinned here: {unpinned:?}"
        );

        let unreached: Vec<&str> = pinned
            .iter()
            .filter(|(_, path)| !reached.contains(*path))
            .map(|(name, _)| *name)
            .collect();
        assert!(
            unreached.is_empty(),
            "published types the battery never instantiates: {unreached:?}"
        );
        let errors: Vec<String> = validator
            .iter_errors(&instance)
            .map(|e| format!("{}: {e}", e.instance_path()))
            .collect();
        assert!(
            errors.is_empty(),
            "schema rejected its own output:\n{errors:#?}"
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
