//! Worktrunk error types and formatting
//!
//! This module provides typed error handling:
//!
//! - **`GitError`** - A typed enum for domain errors that can be pattern-matched
//!   and tested. Use `.into()` to convert to `anyhow::Error` while preserving the
//!   type for pattern matching.
//!
//! - **`WorktrunkError`** - A minimal enum for semantic errors that need
//!   special handling (exit codes, silent errors).
//!
//! ## Display vs. Diagnostic
//!
//! Each typed error has two faces:
//!
//! - [`Display`](std::fmt::Display) is a single-line label suitable for
//!   embedding in another error's message field (e.g.,
//!   `GitError::Other.message: format!("…: {e}")`).
//! - [`Diagnostic::render`] is the rich terminal block — emoji, color,
//!   gutter, follow-up hints. The renderer in `main.rs` walks the anyhow
//!   chain looking for the first type that implements `Diagnostic` and
//!   emits its rendered output.

use std::borrow::Cow;
use std::path::PathBuf;

use color_print::cformat;
use shell_escape::escape;

use super::HookType;
use crate::path::format_path_for_display;
use crate::styling::{
    error_message, format_bash_with_gutter, format_with_gutter, hint_message, info_message,
    suggest_command,
};

/// Platform-specific reference type (PR vs MR).
///
/// Used to unify error handling for GitHub PRs and GitLab MRs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefType {
    /// GitHub Pull Request
    Pr,
    /// GitLab Merge Request
    Mr,
}

impl RefType {
    /// Returns the number prefix symbol for this reference type.
    /// - PR: "#" (e.g., "PR #42")
    /// - MR: "!" (e.g., "MR !42")
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Pr => "#",
            Self::Mr => "!",
        }
    }

    /// Returns the short name for this reference type.
    pub fn name(self) -> &'static str {
        match self {
            Self::Pr => "PR",
            Self::Mr => "MR",
        }
    }

    /// Returns the plural form of the short name.
    pub fn name_plural(self) -> &'static str {
        match self {
            Self::Pr => "PRs",
            Self::Mr => "MRs",
        }
    }

    /// Returns the CLI syntax prefix (e.g., "pr:" or "mr:").
    pub fn syntax(self) -> &'static str {
        match self {
            Self::Pr => "pr:",
            Self::Mr => "mr:",
        }
    }

    /// Returns a display string like "PR #42" or "MR !42".
    pub fn display(self, number: u32) -> String {
        format!("{} {}{}", self.name(), self.symbol(), number)
    }
}

/// Common display fields for PR/MR context.
///
/// Implemented by both `PrInfo` and `MrInfo` to enable unified formatting.
pub trait RefContext {
    fn ref_type(&self) -> RefType;
    fn number(&self) -> u32;
    fn title(&self) -> &str;
    fn author(&self) -> &str;
    fn state(&self) -> &str;
    fn draft(&self) -> bool;
    fn url(&self) -> &str;

    /// The source branch reference for display.
    ///
    /// For same-repo PRs/MRs: just the branch name (e.g., `feature-auth`)
    /// For fork PRs/MRs: `owner:branch` format (e.g., `contributor:feature-fix`)
    fn source_ref(&self) -> String;
}

/// Multi-line styled rendering for terminal display.
///
/// See the module docstring for the contract: types implementing
/// `Diagnostic` keep [`Display`](std::fmt::Display) as a short single-line
/// label and place the styled multi-line output (emoji, color, gutter,
/// follow-up hints) here.
pub trait Diagnostic {
    /// Render the full styled block. Returned string has no trailing newline.
    fn render(&self) -> String;
}

/// Worktrunk-specific extension methods on [`anyhow::Error`].
///
/// Bring this trait into scope (`use worktrunk::git::ErrorExt;`) to use
/// method syntax on anyhow errors:
///
/// ```text
/// if let Some(code) = err.exit_code() { ... }
/// let detail = err.display_message();
/// let rendered = err.render_diagnostic().unwrap_or_else(|| err.to_string());
/// ```
pub trait ErrorExt {
    /// Render the first [`Diagnostic`]-implementing error in the chain.
    ///
    /// Returns `None` when the chain has no typed diagnostic — callers
    /// typically fall back to `error.to_string()`.
    fn render_diagnostic(&self) -> Option<String>;

    /// User-facing detail string for an error that may carry a [`CommandError`].
    ///
    /// When a [`CommandError`] is anywhere in the chain, returns its captured
    /// stderr/stdout via [`CommandError::combined_output`] — that's git's actual
    /// error message, often multi-line. Otherwise falls back to the top-level
    /// `Display`, which under the [`Diagnostic`] split is the typed error's
    /// short single-line label.
    ///
    /// Use this when embedding a sub-error's text inside another typed error's
    /// message field (e.g., `GitError::WorktreeRemovalFailed::error`,
    /// `GitError::PushFailed::error`) so the user sees git's real reason
    /// rather than just the [`CommandError`] single-line summary.
    fn display_message(&self) -> String;

    /// Extract a propagatable exit code from the error.
    ///
    /// Looks through [`HookErrorWithHint`] wrappers and matches on
    /// [`WorktrunkError`] variants that carry an exit code.
    fn exit_code(&self) -> Option<i32>;

    /// If the error is signal-derived, return the equivalent shell exit
    /// code (`128 + signal`).
    ///
    /// Implements the Ctrl-C cancellation policy: command loops call this
    /// on every per-iteration failure and, when it returns `Some`, abort
    /// the loop rather than continuing. The returned code preserves the
    /// standard `128 + sig` shell convention (130 for SIGINT, 143 for
    /// SIGTERM).
    ///
    /// See the "Signal Handling" section of the project `CLAUDE.md` for
    /// the rationale and the full list of loops that apply this policy.
    fn interrupt_exit_code(&self) -> Option<i32>;
}

/// Information about a failed command, for display in error messages.
///
/// Separates the command string from exit information so Display impls
/// can style each part differently (bold command, gray exit code).
#[derive(Debug, Clone)]
pub struct FailedCommand {
    /// The full command string, e.g., "git worktree add /path -b fix main"
    pub command: String,
    /// Exit information, e.g., "exit code 255" or "killed by signal"
    pub exit_info: String,
}

/// Typed leaf error for a command (e.g., `git`) that exited non-zero with
/// captured stdout/stderr.
///
/// Worktrunk's buffered command wrappers ([`super::Repository::run_command`],
/// [`super::WorkingTree::run_command`]) return this — wrapped via
/// `anyhow::Error` — instead of `bail!("{stderr}")`-ing a multi-line string.
/// The structured form lets the renderer distinguish a command's captured
/// output (which is often multi-line) from a context chain entry (which is
/// always one line per layer in the anyhow model), and lets callers that
/// embed the raw stderr in a higher-level error (`GitError::RebaseConflict`
/// stores git's conflict-marker stderr in `git_output`) read it directly
/// instead of round-tripping through `e.to_string()`.
///
/// `Display` is intentionally a single-line summary — `format_with_gutter`
/// in `print_command_error` renders [`Self::combined_output`] separately for
/// the multi-line body. The streaming path (`run_command_delayed_stream`)
/// uses a sibling crate-private `StreamCommandError` for the same role,
/// where stdout/stderr are interleaved and a string body is the most we can
/// recover.
#[derive(Debug, Clone)]
pub struct CommandError {
    /// Program name, e.g., `"git"`.
    pub program: String,
    /// Arguments, e.g., `["worktree", "list"]`.
    pub args: Vec<String>,
    /// Captured stderr with `\r` normalized to `\n` (git emits `\r` for
    /// progress; non-TTY contexts otherwise produce snapshot instability).
    pub stderr: String,
    /// Captured stdout — kept separate because some git subcommands print
    /// errors here (e.g., `commit` with nothing to commit).
    pub stdout: String,
    /// Process exit code; `None` if the child was killed by a signal.
    pub exit_code: Option<i32>,
}

impl CommandError {
    /// Build from the captured `Output` of a non-zero exit.
    pub fn from_failed_output(
        program: impl Into<String>,
        args: &[&str],
        output: &std::process::Output,
    ) -> Self {
        let stderr = String::from_utf8_lossy(&output.stderr).replace('\r', "\n");
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        Self {
            program: program.into(),
            args: args.iter().map(|&s| s.to_string()).collect(),
            stderr,
            stdout,
            exit_code: output.status.code(),
        }
    }

    /// Reconstruct the command line for display.
    pub fn command_string(&self) -> String {
        if self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        }
    }

    /// Walk an [`anyhow::Error`] chain for a [`CommandError`].
    ///
    /// Returns the first match. Useful for renderers and callers that
    /// need the raw stderr regardless of how many `.context(...)` layers
    /// wrap it.
    pub fn find_in(error: &anyhow::Error) -> Option<&CommandError> {
        error.chain().find_map(|e| e.downcast_ref::<CommandError>())
    }

    /// stderr + stdout, trimmed and joined by `\n`, with empty pieces
    /// dropped. Mirrors the legacy `bail!("{}", error_msg)` payload so
    /// callers that previously parsed `e.to_string()` (notably
    /// `GitError::RebaseConflict`) get the same bytes when they downcast.
    pub fn combined_output(&self) -> String {
        [self.stderr.trim(), self.stdout.trim()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.exit_code {
            Some(code) => write!(f, "{} failed (exit {})", self.command_string(), code),
            None => write!(f, "{} failed", self.command_string()),
        }
    }
}

impl std::error::Error for CommandError {}

impl Diagnostic for CommandError {
    fn render(&self) -> String {
        let header = error_message(self.to_string()).to_string();
        let body = self.combined_output();
        if body.is_empty() {
            header
        } else {
            format!("{header}\n{}", format_with_gutter(&body, None))
        }
    }
}

/// Render `err` via [`Diagnostic`] if it's a typed diagnostic, else `None`.
///
/// Rust trait objects can't be trait-downcast (you can't ask `&dyn Error`
/// "do you implement `Diagnostic`?"), so this helper enumerates every
/// known type with a `Diagnostic` impl. Add new types here when they
/// grow one.
///
/// Order matters only inasmuch as we want the most specific wrapper
/// first ([`HookErrorWithHint`] before its [`WorktrunkError`] source).
///
/// To render the first typed diagnostic anywhere in an [`anyhow::Error`]'s
/// chain, use [`ErrorExt::render_diagnostic`].
pub fn try_render_diagnostic(err: &(dyn std::error::Error + 'static)) -> Option<String> {
    if let Some(e) = err.downcast_ref::<GitError>() {
        return Some(e.render());
    }
    if let Some(e) = err.downcast_ref::<HookErrorWithHint>() {
        return Some(e.render());
    }
    if let Some(e) = err.downcast_ref::<WorktrunkError>() {
        return Some(e.render());
    }
    if let Some(e) = err.downcast_ref::<crate::config::TemplateExpandError>() {
        return Some(e.render());
    }
    if let Some(e) = err.downcast_ref::<CommandError>() {
        return Some(e.render());
    }
    None
}

/// Extra CLI context for enriching `wt switch` suggestions in error hints.
///
/// When a switch error is raised deep in the planning layer, the error only knows
/// the branch name. The command handler wraps the error with this context so the
/// Display impl can produce a fully copy-pasteable suggestion including flags like
/// `--execute` and trailing args.
#[derive(Debug, Clone)]
pub struct SwitchSuggestionCtx {
    pub extra_flags: Vec<String>,
    pub trailing_args: Vec<String>,
}

impl SwitchSuggestionCtx {
    /// Append extra flags and trailing args to a suggested command string.
    ///
    /// Clap's `#[arg(last = true)]` on `execute_args` means `--` always routes
    /// to execute_args, so a dash-prefixed branch can't coexist with `--execute`
    /// via the CLI. The suggested command therefore never has a pre-existing `--`
    /// separator when this context is applied.
    fn apply(&self, cmd: String) -> String {
        let mut result = cmd;
        // Flags are pre-escaped at construction (worktree/switch.rs uses shell_escape)
        for flag in &self.extra_flags {
            result.push(' ');
            result.push_str(flag);
        }
        if !self.trailing_args.is_empty() {
            result.push_str(" --");
            for arg in &self.trailing_args {
                result.push(' ');
                result.push_str(&escape(Cow::Borrowed(arg.as_str())));
            }
        }
        result
    }
}

/// Domain errors for git and worktree operations.
///
/// This enum provides structured error data that can be pattern-matched and tested.
/// Each variant stores the data needed to construct a user-facing error message.
///
/// [`Display`](std::fmt::Display) is a short single-line label per variant
/// suitable for embedding in another error's message field;
/// [`Diagnostic::render`] produces the rich styled block (emoji, color,
/// gutter, follow-up hints).
///
/// # Usage
///
/// ```ignore
/// // Return a typed error
/// return Err(GitError::DetachedHead { action: Some("merge".into()) }.into());
///
/// // Pattern match on errors
/// if let Some(GitError::BranchAlreadyExists { branch }) = err.downcast_ref() {
///     println!("Branch {} exists", branch);
/// }
/// ```
#[derive(Debug, Clone)]
pub enum GitError {
    // Git state errors
    DetachedHead {
        action: Option<String>,
    },
    UncommittedChanges {
        action: Option<String>,
        /// Branch name (for multi-worktree operations)
        branch: Option<String>,
        /// When true, hint mentions --force as an alternative to stashing
        force_hint: bool,
    },
    BranchAlreadyExists {
        branch: String,
    },
    BranchNotFound {
        branch: String,
        /// Show hint about creating the branch. Set to false for remove operations
        /// where suggesting creation doesn't make sense.
        show_create_hint: bool,
        /// Pre-formatted label for the last fetch time (e.g., "3h ago", "never").
        /// When present, the list-branches hint includes the fetch age as a parenthetical.
        last_fetch_ago: Option<String>,
        /// Platform's reference type for the PR/MR hint when the branch name is
        /// purely numeric. `None` means the platform is unknown — fall back to
        /// suggesting both `pr:N` and `mr:N`.
        pr_mr_platform: Option<RefType>,
    },
    /// Reference (branch, tag, commit) not found - used when any commit-ish is accepted
    ReferenceNotFound {
        reference: String,
    },
    /// Persisted `worktrunk.default-branch` points at a branch that no longer
    /// resolves locally. Surfaced when a command would use the default branch
    /// (no explicit `--target`) and the cached value is stale, so the user
    /// gets a cache-reset hint instead of a generic "branch not found".
    StaleDefaultBranch {
        branch: String,
    },

    // Worktree errors
    NotInWorktree {
        /// The action that requires being in a worktree
        action: Option<String>,
    },
    WorktreeMissing {
        branch: String,
    },
    RemoteOnlyBranch {
        branch: String,
        remote: String,
    },
    WorktreePathOccupied {
        branch: String,
        path: PathBuf,
        occupant: Option<String>,
    },
    WorktreePathExists {
        branch: String,
        path: PathBuf,
        create: bool,
    },
    WorktreeCreationFailed {
        branch: String,
        base_branch: Option<String>,
        error: String,
        /// The git command that failed, shown separately from git output
        command: Option<FailedCommand>,
    },
    WorktreeRemovalFailed {
        branch: String,
        path: PathBuf,
        error: String,
        /// Top-level entries remaining in the directory (for "Directory not empty" diagnostics)
        remaining_entries: Option<Vec<String>>,
    },
    CannotRemoveMainWorktree,
    CannotRemoveDefaultBranch {
        branch: String,
    },
    WorktreeLocked {
        branch: String,
        path: PathBuf,
        reason: Option<String>,
    },

    // Merge/push errors
    ConflictingChanges {
        target_branch: String,
        files: Vec<String>,
        worktree_path: PathBuf,
    },
    NotFastForward {
        target_branch: String,
        commits_formatted: String,
        in_merge_context: bool,
    },
    RebaseConflict {
        target_branch: String,
        git_output: String,
    },
    NotRebased {
        target_branch: String,
    },
    PushFailed {
        target_branch: String,
        error: String,
    },

    // Validation/other errors
    NotInteractive,
    HookCommandNotFound {
        name: String,
        available: Vec<String>,
    },
    ParseError {
        message: String,
    },
    WorktreeIncludeParseError {
        error: String,
    },
    LlmCommandFailed {
        command: String,
        error: String,
        /// Full command to reproduce the failure, e.g., "wt step commit --show-prompt | llm"
        reproduction_command: Option<String>,
    },
    ProjectConfigNotFound {
        config_path: PathBuf,
    },
    WorktreeNotFound {
        branch: String,
    },
    /// --create flag used with pr:/mr: syntax (conflict - branch already exists)
    RefCreateConflict {
        ref_type: RefType,
        number: u32,
        branch: String,
    },
    /// --base flag used with pr:/mr: syntax (conflict - base is predetermined)
    RefBaseConflict {
        ref_type: RefType,
        number: u32,
    },
    /// Branch exists but is tracking a different PR/MR
    BranchTracksDifferentRef {
        branch: String,
        ref_type: RefType,
        number: u32,
    },
    /// No remote found for the repository where the PR lives
    NoRemoteForRepo {
        owner: String,
        repo: String,
        /// Suggested URL to add as a remote (derived from primary remote's protocol/host)
        suggested_url: String,
    },
    /// CLI API command failed with unrecognized error (gh or glab)
    CliApiError {
        ref_type: RefType,
        /// Short description of what failed
        message: String,
        /// Full stderr output for debugging
        stderr: String,
    },
    Other {
        message: String,
    },

    /// Wrapper that enriches an inner error's switch suggestions with CLI context.
    ///
    /// The inner error renders normally, but any `wt switch` suggestion includes
    /// the extra flags and trailing args from the context.
    WithSwitchSuggestion {
        source: Box<GitError>,
        ctx: SwitchSuggestionCtx,
    },
}

impl std::error::Error for GitError {}

impl GitError {
    /// Styled title for this variant (first line, with inline `<bold>`
    /// highlights on entity names like branch and path).
    ///
    /// Single source of truth: [`Diagnostic::render`] uses this directly
    /// for the header; the [`std::fmt::Display`] impl strips ANSI codes
    /// (via [`ansi_str::AnsiStr::ansi_strip`]) for embedding into other
    /// error messages or non-terminal sinks like JSON.
    fn title(&self) -> String {
        match self {
            GitError::WithSwitchSuggestion { source, .. } => source.title(),

            GitError::DetachedHead { action } => match action {
                Some(action) => cformat!("Cannot {action}: not on a branch (detached HEAD)"),
                None => "Not on a branch (detached HEAD)".to_string(),
            },

            GitError::UncommittedChanges { action, branch, .. } => match (action, branch) {
                (Some(action), Some(b)) => {
                    cformat!("Cannot {action}: <bold>{b}</> has uncommitted changes")
                }
                (Some(action), None) => {
                    format!("Cannot {action}: working tree has uncommitted changes")
                }
                (None, Some(b)) => cformat!("<bold>{b}</> has uncommitted changes"),
                (None, None) => "Working tree has uncommitted changes".to_string(),
            },

            GitError::BranchAlreadyExists { branch } => {
                cformat!("Branch <bold>{branch}</> already exists")
            }

            GitError::BranchNotFound { branch, .. } => {
                cformat!("No branch named <bold>{branch}</>")
            }

            GitError::ReferenceNotFound { reference } => {
                cformat!("No branch, tag, or commit named <bold>{reference}</>")
            }

            GitError::StaleDefaultBranch { branch } => {
                cformat!("Default branch <bold>{branch}</> does not exist locally")
            }

            GitError::NotInWorktree { action } => match action {
                Some(action) => format!("Cannot {action}: not in a worktree"),
                None => "Not in a worktree".to_string(),
            },

            GitError::WorktreeMissing { branch } => {
                cformat!("Worktree directory missing for <bold>{branch}</>")
            }

            GitError::RemoteOnlyBranch { branch, remote } => {
                cformat!("Branch <bold>{branch}</> exists only on remote ({remote}/{branch})")
            }

            GitError::WorktreePathOccupied {
                branch,
                path,
                occupant,
            } => {
                let path_display = format_path_for_display(path);
                match occupant {
                    Some(occupant_branch) => cformat!(
                        "Cannot switch to <bold>{branch}</> — there's a worktree at the expected path <bold>{path_display}</> on branch <bold>{occupant_branch}</>"
                    ),
                    None => cformat!(
                        "Cannot switch to <bold>{branch}</> — there's a detached worktree at the expected path <bold>{path_display}</>"
                    ),
                }
            }

            GitError::WorktreePathExists { path, .. } => {
                let path_display = format_path_for_display(path);
                cformat!("Directory already exists: <bold>{path_display}</>")
            }

            GitError::WorktreeCreationFailed {
                branch,
                base_branch,
                ..
            } => match base_branch {
                Some(base) => cformat!(
                    "Failed to create worktree for <bold>{branch}</> from base <bold>{base}</>"
                ),
                None => cformat!("Failed to create worktree for <bold>{branch}</>"),
            },

            GitError::WorktreeRemovalFailed { branch, path, .. } => {
                let path_display = format_path_for_display(path);
                cformat!(
                    "Failed to remove worktree for <bold>{branch}</> @ <bold>{path_display}</>"
                )
            }

            GitError::CannotRemoveMainWorktree => "The main worktree cannot be removed".to_string(),

            GitError::CannotRemoveDefaultBranch { branch } => {
                cformat!("Cannot remove the default branch <bold>{branch}</>")
            }

            GitError::WorktreeLocked { branch, reason, .. } => {
                let reason_text = match reason {
                    Some(r) if !r.is_empty() => format!(" ({r})"),
                    _ => String::new(),
                };
                cformat!("Cannot remove <bold>{branch}</>, worktree is locked{reason_text}")
            }

            GitError::ConflictingChanges { target_branch, .. } => cformat!(
                "Can't push to local <bold>{target_branch}</> branch: conflicting uncommitted changes"
            ),

            GitError::NotFastForward { target_branch, .. } => cformat!(
                "Can't push to local <bold>{target_branch}</> branch: it has newer commits"
            ),

            GitError::RebaseConflict { target_branch, .. } => {
                cformat!("Rebase onto <bold>{target_branch}</> incomplete")
            }

            GitError::NotRebased { target_branch } => {
                cformat!("Branch not rebased onto <bold>{target_branch}</>")
            }

            GitError::PushFailed { target_branch, .. } => {
                cformat!("Can't push to local <bold>{target_branch}</> branch")
            }

            GitError::NotInteractive => {
                "Cannot prompt for approval in non-interactive environment".to_string()
            }

            GitError::HookCommandNotFound { name, available } => {
                if available.is_empty() {
                    cformat!("No command named <bold>{name}</> (hook has no named commands)")
                } else {
                    let styled_list = available
                        .iter()
                        .map(|s| cformat!("<bold>{s}</>"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    cformat!("No command named <bold>{name}</> (available: {styled_list})")
                }
            }

            GitError::LlmCommandFailed { .. } => "Commit generation command failed".to_string(),

            GitError::ProjectConfigNotFound { .. } => "No project configuration found".to_string(),

            GitError::ParseError { message } => message.clone(),

            GitError::WorktreeIncludeParseError { .. } => {
                cformat!("Error parsing <bold>.worktreeinclude</>")
            }

            GitError::WorktreeNotFound { branch } => {
                cformat!("Branch <bold>{branch}</> has no worktree")
            }

            GitError::RefCreateConflict {
                ref_type,
                number,
                branch,
            } => {
                let name = ref_type.name();
                let syntax = ref_type.syntax();
                cformat!(
                    "Cannot create branch for <bold>{syntax}{number}</> — {name} already has branch <bold>{branch}</>"
                )
            }

            GitError::RefBaseConflict { ref_type, number } => {
                let syntax = ref_type.syntax();
                cformat!("Cannot use <bold>--base</> with <bold>{syntax}{number}</>")
            }

            GitError::BranchTracksDifferentRef {
                branch,
                ref_type,
                number,
            } => {
                let name = ref_type.name();
                let symbol = ref_type.symbol();
                cformat!(
                    "Branch <bold>{branch}</> exists but doesn't track {name} {symbol}{number}"
                )
            }

            GitError::NoRemoteForRepo { owner, repo, .. } => {
                cformat!("No remote found for <bold>{owner}/{repo}</>")
            }

            GitError::CliApiError { message, .. } => message.clone(),

            GitError::Other { message } => message.clone(),
        }
    }

    /// Write the styled diagnostic block, with optional switch-suggestion
    /// context propagated from a [`GitError::WithSwitchSuggestion`] wrapper.
    ///
    /// Most variants ignore `ctx`. The three that render `wt switch` suggestions
    /// (`BranchAlreadyExists`, `BranchNotFound`, `WorktreePathExists`) use it
    /// to append extra flags and trailing args for a copy-pasteable command.
    ///
    /// Called internally by [`Diagnostic::render`]; not part of the public API.
    fn write_render_with_ctx<W: std::fmt::Write>(
        &self,
        f: &mut W,
        ctx: Option<&SwitchSuggestionCtx>,
    ) -> std::fmt::Result {
        match self {
            GitError::WithSwitchSuggestion { source, ctx } => {
                source.write_render_with_ctx(f, Some(ctx))
            }

            GitError::DetachedHead { .. } => {
                let title = self.title();
                write!(
                    f,
                    "{}\n{}",
                    error_message(&title),
                    hint_message(cformat!(
                        "To switch to a branch, run <underline>git switch <<branch>></>"
                    ))
                )
            }

            GitError::UncommittedChanges {
                branch, force_hint, ..
            } => {
                let title = self.title();
                let hint = if *force_hint {
                    // Construct full command: "wt remove [branch] --force"
                    let args: Vec<&str> = branch.as_deref().into_iter().collect();
                    let cmd = suggest_command("remove", &args, &["--force"]);
                    cformat!(
                        "Commit or stash changes first, or to lose uncommitted changes, run <underline>{cmd}</>"
                    )
                } else {
                    "Commit or stash changes first".to_string()
                };
                write!(f, "{}\n{}", error_message(&title), hint_message(hint))
            }

            GitError::BranchAlreadyExists { branch } => {
                let title = self.title();
                let mut switch_cmd = suggest_command("switch", &[branch], &[]);
                if let Some(ctx) = ctx {
                    switch_cmd = ctx.apply(switch_cmd);
                }
                write!(
                    f,
                    "{}\n{}",
                    error_message(&title),
                    hint_message(cformat!(
                        "To switch to the existing branch, run without <underline>--create</>: <underline>{switch_cmd}</>"
                    ))
                )
            }

            GitError::BranchNotFound {
                branch,
                show_create_hint,
                last_fetch_ago,
                pr_mr_platform,
            } => {
                let title = self.title();
                let list_cmd = suggest_command("list", &[], &["--branches", "--remotes"]);
                let fetch_note = last_fetch_ago
                    .as_ref()
                    .map(|ago| cformat!(" ({ago})"))
                    .unwrap_or_default();
                let list_hint =
                    cformat!("to list branches, run <underline>{list_cmd}</>{fetch_note}");
                let hint = if *show_create_hint {
                    let mut create_cmd = suggest_command("switch", &[branch], &["--create"]);
                    if let Some(ctx) = ctx {
                        create_cmd = ctx.apply(create_cmd);
                    }
                    let create_hint =
                        cformat!("to create a new branch, run <underline>{create_cmd}</>");
                    if let Ok(number) = branch.parse::<u32>() {
                        let pr_mr_hint = pr_mr_switch_hint(number, *pr_mr_platform, ctx);
                        cformat!("{pr_mr_hint}; {create_hint}; {list_hint}")
                    } else {
                        // Existing format: capitalize the leading "to".
                        cformat!(
                            "To create a new branch, run <underline>{create_cmd}</>; {list_hint}"
                        )
                    }
                } else {
                    cformat!("To list branches, run <underline>{list_cmd}</>")
                };
                write!(f, "{}\n{}", error_message(&title), hint_message(hint))
            }

            GitError::ReferenceNotFound { .. } => {
                let title = self.title();
                write!(f, "{}", error_message(&title))
            }

            GitError::StaleDefaultBranch { .. } => {
                let title = self.title();
                write!(
                    f,
                    "{}\n{}",
                    error_message(&title),
                    hint_message(cformat!(
                        "Reset the cached value with <underline>wt config state default-branch clear</>, or set it explicitly with <underline>wt config state default-branch set BRANCH</>"
                    ))
                )
            }

            GitError::NotInWorktree { .. } => {
                let title = self.title();
                write!(
                    f,
                    "{}\n{}",
                    error_message(&title),
                    hint_message(cformat!(
                        "Run from inside a worktree, or specify a branch name"
                    ))
                )
            }

            GitError::WorktreeMissing { .. } => {
                let title = self.title();
                write!(
                    f,
                    "{}\n{}",
                    error_message(&title),
                    hint_message(cformat!(
                        "To clean up, run <underline>git worktree prune</>"
                    ))
                )
            }

            GitError::RemoteOnlyBranch { branch, .. } => {
                let title = self.title();
                let cmd = suggest_command("switch", &[branch], &[]);
                write!(
                    f,
                    "{}\n{}",
                    error_message(&title),
                    hint_message(cformat!(
                        "To create a local worktree, run <underline>{cmd}</>"
                    ))
                )
            }

            GitError::WorktreePathOccupied { branch, path, .. } => {
                let title = self.title();
                let path_display = format_path_for_display(path);
                let escaped_path = escape(path.to_string_lossy());
                let escaped_branch = escape(Cow::Borrowed(branch.as_str()));
                let command = format!("cd {escaped_path} && git switch {escaped_branch}");
                write!(
                    f,
                    "{}\n{}",
                    error_message(&title),
                    hint_message(cformat!(
                        "To switch the worktree at <underline>{path_display}</> to <underline>{branch}</>, run <underline>{command}</>"
                    ))
                )
            }

            GitError::WorktreePathExists {
                branch,
                path,
                create,
            } => {
                let title = self.title();
                let path_display = format_path_for_display(path);
                let flags: &[&str] = if *create {
                    &["--create", "--clobber"]
                } else {
                    &["--clobber"]
                };
                let mut switch_cmd = suggest_command("switch", &[branch], flags);
                if let Some(ctx) = ctx {
                    switch_cmd = ctx.apply(switch_cmd);
                }
                write!(
                    f,
                    "{}\n{}",
                    error_message(&title),
                    hint_message(cformat!(
                        "To remove manually, run <underline>rm -rf {path_display}</>; to overwrite (with backup), run <underline>{switch_cmd}</>"
                    ))
                )
            }

            GitError::WorktreeCreationFailed { error, command, .. } => {
                let title = self.title();
                write!(f, "{}", format_error_block(error_message(&title), error))?;
                if let Some(cmd) = command {
                    write!(
                        f,
                        "\n{}\n{}",
                        hint_message(cformat!("Failed command, <underline>{}</>:", cmd.exit_info)),
                        format_bash_with_gutter(&cmd.command)
                    )?;
                }
                Ok(())
            }

            GitError::WorktreeRemovalFailed {
                error,
                remaining_entries,
                ..
            } => {
                let title = self.title();
                write!(f, "{}", format_error_block(error_message(&title), error))?;
                if let Some(entries) = remaining_entries {
                    const MAX_SHOWN: usize = 10;
                    let listing = if entries.len() > MAX_SHOWN {
                        let shown = entries[..MAX_SHOWN].join(", ");
                        let remaining = entries.len() - MAX_SHOWN;
                        format!("{shown}, and {remaining} more")
                    } else {
                        entries.join(", ")
                    };
                    write!(
                        f,
                        "\n{}",
                        hint_message(cformat!("Remaining in directory: <underline>{listing}</>"))
                    )?;
                }
                if error.contains("not empty") {
                    write!(
                        f,
                        "\n{}",
                        hint_message(cformat!(
                            "A background process may be writing files; try <underline>wt remove</> (without --foreground)"
                        ))
                    )?;
                }
                Ok(())
            }

            GitError::CannotRemoveMainWorktree => {
                let title = self.title();
                write!(f, "{}", error_message(&title))
            }

            GitError::CannotRemoveDefaultBranch { branch } => {
                let title = self.title();
                let cmd = suggest_command("remove", &[branch], &["-D"]);
                write!(
                    f,
                    "{}\n{}",
                    error_message(&title),
                    hint_message(cformat!("To force-delete, run <underline>{cmd}</>"))
                )
            }

            GitError::WorktreeLocked { path, .. } => {
                let title = self.title();
                let path_display = format_path_for_display(path);
                write!(
                    f,
                    "{}\n{}",
                    error_message(&title),
                    hint_message(cformat!(
                        "To unlock, run <underline>git worktree unlock {path_display}</>"
                    ))
                )
            }

            GitError::ConflictingChanges {
                files,
                worktree_path,
                ..
            } => {
                let title = self.title();
                write!(f, "{}", error_message(&title))?;
                if !files.is_empty() {
                    let joined_files = files.join("\n");
                    write!(f, "\n{}", format_with_gutter(&joined_files, None))?;
                }
                let path_display = format_path_for_display(worktree_path);
                write!(
                    f,
                    "\n{}",
                    hint_message(format!(
                        "Commit or stash these changes in {path_display} first"
                    ))
                )
            }

            GitError::NotFastForward {
                target_branch,
                commits_formatted,
                in_merge_context,
            } => {
                let title = self.title();
                write!(f, "{}", error_message(&title))?;
                if !commits_formatted.is_empty() {
                    write!(f, "\n{}", format_with_gutter(commits_formatted, None))?;
                }
                // Context-appropriate hint
                let merge_cmd = suggest_command("merge", &[target_branch], &[]);
                if *in_merge_context {
                    write!(
                        f,
                        "\n{}",
                        hint_message(cformat!(
                            "To incorporate these changes, run <underline>{merge_cmd}</> again"
                        ))
                    )
                } else {
                    let rebase_cmd = suggest_command("step", &["rebase", target_branch], &[]);
                    write!(
                        f,
                        "\n{}",
                        hint_message(cformat!(
                            "To rebase onto <underline>{target_branch}</>, run <underline>{rebase_cmd}</>"
                        ))
                    )
                }
            }

            GitError::RebaseConflict { git_output, .. } => {
                let title = self.title();
                write!(f, "{}", error_message(&title))?;
                if !git_output.is_empty() {
                    write!(f, "\n{}", format_with_gutter(git_output, None))
                } else {
                    write!(
                        f,
                        "\n{}\n{}",
                        hint_message(cformat!(
                            "To continue after resolving conflicts, run <underline>git rebase --continue</>"
                        )),
                        hint_message(cformat!("To abort, run <underline>git rebase --abort</>"))
                    )
                }
            }

            GitError::NotRebased { target_branch } => {
                let title = self.title();
                let rebase_cmd = suggest_command("step", &["rebase", target_branch], &[]);
                write!(
                    f,
                    "{}\n{}",
                    error_message(&title),
                    hint_message(cformat!(
                        "To rebase first, run <underline>{rebase_cmd}</>; or remove <underline>--no-rebase</>"
                    ))
                )
            }

            GitError::PushFailed { error, .. } => {
                let title = self.title();
                write!(f, "{}", format_error_block(error_message(&title), error))
            }

            GitError::NotInteractive => {
                let title = self.title();
                let approvals_cmd = suggest_command("config", &["approvals", "add"], &[]);
                write!(
                    f,
                    "{}\n{}",
                    error_message(&title),
                    hint_message(cformat!(
                        "To skip prompts in CI/CD, add <underline>--yes</>; to pre-approve commands, run <underline>{approvals_cmd}</>"
                    ))
                )
            }

            GitError::HookCommandNotFound { .. } => {
                let title = self.title();
                write!(f, "{}", error_message(&title))
            }

            GitError::LlmCommandFailed {
                command,
                error,
                reproduction_command,
            } => {
                let title = self.title();
                let error_block = format_error_block(error_message(&title), error);
                // Show full pipeline command if available, otherwise just the LLM command
                let display_command = reproduction_command.as_ref().unwrap_or(command);
                let command_gutter = format_with_gutter(display_command, None);
                write!(
                    f,
                    "{}\n{}\n{}",
                    error_block,
                    info_message("Ran command:"),
                    command_gutter
                )
            }

            GitError::ProjectConfigNotFound { config_path } => {
                let title = self.title();
                let path_display = format_path_for_display(config_path);
                write!(
                    f,
                    "{}\n{}",
                    error_message(&title),
                    hint_message(cformat!(
                        "Create a config file at: <underline>{path_display}</>"
                    ))
                )
            }

            GitError::ParseError { .. } => {
                let title = self.title();
                write!(f, "{}", error_message(&title))
            }

            GitError::WorktreeIncludeParseError { error } => {
                let title = self.title();
                write!(f, "{}", format_error_block(error_message(&title), error))
            }

            GitError::WorktreeNotFound { branch } => {
                let title = self.title();
                let switch_cmd = suggest_command("switch", &[branch], &[]);
                write!(
                    f,
                    "{}\n{}",
                    error_message(&title),
                    hint_message(cformat!(
                        "To create a worktree, run <underline>{switch_cmd}</>"
                    ))
                )
            }

            GitError::RefCreateConflict {
                ref_type,
                number,
                branch,
            } => {
                let name = ref_type.name();
                let syntax = ref_type.syntax();
                write!(
                    f,
                    "{}\n{}",
                    error_message(cformat!(
                        "Cannot create branch for <bold>{syntax}{number}</> — {name} already has branch <bold>{branch}</>"
                    )),
                    hint_message(cformat!(
                        "To switch to it: <underline>wt switch {syntax}{number}</>"
                    ))
                )
            }

            GitError::RefBaseConflict { ref_type, number } => {
                let syntax = ref_type.syntax();
                let name_plural = ref_type.name_plural();
                write!(
                    f,
                    "{}\n{}",
                    error_message(cformat!(
                        "Cannot use <bold>--base</> with <bold>{syntax}{number}</>"
                    )),
                    hint_message(cformat!(
                        "{name_plural} already have a base; remove <underline>--base</>"
                    ))
                )
            }

            GitError::BranchTracksDifferentRef {
                branch,
                ref_type,
                number,
            } => {
                // The ref's branch name conflicts with an existing local branch.
                // We can't use a different local name because git push requires
                // the local and remote branch names to match (with push.default=current).
                let escaped = escape(Cow::Borrowed(branch.as_str()));
                let old_name = format!("{branch}-old");
                let escaped_old = escape(Cow::Borrowed(&old_name));
                let name = ref_type.name();
                let symbol = ref_type.symbol();
                write!(
                    f,
                    "{}\n{}",
                    error_message(cformat!(
                        "Branch <bold>{branch}</> exists but doesn't track {name} {symbol}{number}"
                    )),
                    hint_message(cformat!(
                        "To free the name, run <underline>git branch -m -- {escaped} {escaped_old}</>"
                    ))
                )
            }

            GitError::NoRemoteForRepo {
                owner,
                repo,
                suggested_url,
            } => {
                write!(
                    f,
                    "{}\n{}",
                    error_message(cformat!("No remote found for <bold>{owner}/{repo}</>")),
                    hint_message(cformat!(
                        "Add the remote: <underline>git remote add upstream {suggested_url}</>"
                    ))
                )
            }

            GitError::CliApiError {
                message, stderr, ..
            } => {
                write!(f, "{}", format_error_block(error_message(message), stderr))
            }

            GitError::Other { message } => {
                write!(f, "{}", error_message(message))
            }
        }
    }
}

impl Diagnostic for GitError {
    fn render(&self) -> String {
        let mut out = String::new();
        self.write_render_with_ctx(&mut out, None)
            .expect("writing to a String never fails");
        out
    }
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The styled title carries inline `<bold>` highlights for terminal
        // rendering. For embedding in another error's message field or
        // any non-terminal sink (JSON, log files, snapshots), strip the
        // ANSI codes so consumers see plain text.
        f.write_str(&ansi_str::AnsiStr::ansi_strip(&self.title()))
    }
}

/// Semantic errors that require special handling in main.rs
///
/// Most errors use anyhow::bail! with formatted messages. This enum is only
/// for cases that need exit code extraction or special handling.
#[derive(Debug)]
pub enum WorktrunkError {
    /// Child process exited with non-zero code (preserves exit code for signals).
    ///
    /// `signal` is `Some(sig)` when the process was terminated by a signal
    /// (on Unix), `None` for a normal non-zero exit. Callers that must treat
    /// interrupts differently from ordinary failures (e.g., aborting a loop
    /// on Ctrl-C) check `signal` rather than inferring from `code`.
    ChildProcessExited {
        code: i32,
        message: String,
        signal: Option<i32>,
    },
    /// Hook command failed
    HookCommandFailed {
        hook_type: HookType,
        command_name: Option<String>,
        error: String,
        exit_code: Option<i32>,
    },
    /// Command was not approved by user (silent error)
    CommandNotApproved,
    /// Error already displayed, just exit with given code (silent error)
    AlreadyDisplayed { exit_code: i32 },
}

impl std::fmt::Display for WorktrunkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorktrunkError::ChildProcessExited { message, .. } => f.write_str(message),
            WorktrunkError::HookCommandFailed {
                hook_type,
                command_name,
                error,
                ..
            } => match command_name {
                Some(name) => write!(f, "{hook_type} command failed: {name}: {error}"),
                None => write!(f, "{hook_type} command failed: {error}"),
            },
            // on_skip callback handles the printing for CommandNotApproved;
            // AlreadyDisplayed has already shown its error via output functions.
            WorktrunkError::CommandNotApproved | WorktrunkError::AlreadyDisplayed { .. } => Ok(()),
        }
    }
}

impl WorktrunkError {
    /// Exit code carried by this variant, if any.
    pub fn exit_code(&self) -> Option<i32> {
        match self {
            WorktrunkError::ChildProcessExited { code, .. } => Some(*code),
            WorktrunkError::HookCommandFailed { exit_code, .. } => *exit_code,
            WorktrunkError::AlreadyDisplayed { exit_code } => Some(*exit_code),
            WorktrunkError::CommandNotApproved => None,
        }
    }
}

impl Diagnostic for WorktrunkError {
    fn render(&self) -> String {
        match self {
            WorktrunkError::ChildProcessExited { message, .. } => {
                error_message(message).to_string()
            }
            WorktrunkError::HookCommandFailed {
                hook_type,
                command_name,
                error,
                ..
            } => {
                if let Some(name) = command_name {
                    error_message(cformat!(
                        "{hook_type} command failed: <bold>{name}</>: {error}"
                    ))
                    .to_string()
                } else {
                    error_message(format!("{hook_type} command failed: {error}")).to_string()
                }
            }
            // Silent — caller already handled display; render is empty.
            WorktrunkError::CommandNotApproved | WorktrunkError::AlreadyDisplayed { .. } => {
                String::new()
            }
        }
    }
}

impl std::error::Error for WorktrunkError {}

impl ErrorExt for anyhow::Error {
    fn render_diagnostic(&self) -> Option<String> {
        self.chain().find_map(try_render_diagnostic)
    }

    fn display_message(&self) -> String {
        // For chains carrying a `CommandError`, prefer git's captured
        // stderr/stdout. Fall back to the command summary when the
        // capture is empty (e.g., signal-killed before output) so a
        // higher-level error like `PushFailed { error: … }` doesn't end
        // up with a blank detail.
        if let Some(cmd_err) = CommandError::find_in(self) {
            let body = cmd_err.combined_output();
            return if body.is_empty() {
                cmd_err.to_string()
            } else {
                body
            };
        }
        self.to_string()
    }

    fn exit_code(&self) -> Option<i32> {
        // Walks past `HookErrorWithHint` (whose `Error::source` exposes
        // the inner `WorktrunkError`) without a special case here.
        self.chain()
            .find_map(|e| e.downcast_ref::<WorktrunkError>())
            .and_then(WorktrunkError::exit_code)
    }

    fn interrupt_exit_code(&self) -> Option<i32> {
        if let Some(WorktrunkError::ChildProcessExited {
            signal: Some(sig), ..
        }) = self.downcast_ref::<WorktrunkError>()
        {
            Some(128 + sig)
        } else {
            None
        }
    }
}

/// If the error wraps a [`WorktrunkError::HookCommandFailed`], wrap it
/// with [`HookErrorWithHint`] to surface a `--no-hooks` hint when
/// rendered. Pass-through for any other error.
///
/// Domain behavior, not a general property of an error — kept as a free
/// function so it stays out of [`ErrorExt`].
///
/// ## When to use
///
/// Use this for commands where a hook runs as a side effect of the
/// user's intent:
/// - `wt merge` — user wants to merge, hooks run as part of that
/// - `wt commit` — user wants to commit, pre-commit hooks run
/// - `wt switch --create` — user wants a worktree, post-create hooks run
///
/// ## When NOT to use
///
/// Don't use for `wt hook <type>` — the user explicitly asked to run
/// hooks, so suggesting `--no-hooks` makes no sense.
pub fn add_hook_skip_hint(err: anyhow::Error) -> anyhow::Error {
    let inner = match err.downcast::<WorktrunkError>() {
        Ok(inner) => inner,
        Err(err) => return err,
    };
    let hook_type = match &inner {
        WorktrunkError::HookCommandFailed { hook_type, .. } => *hook_type,
        // Not a hook failure — pass the typed leaf through unchanged.
        _ => return inner.into(),
    };
    HookErrorWithHint { inner, hook_type }.into()
}

/// Wrapper that displays a [`WorktrunkError::HookCommandFailed`] with a
/// `--no-hooks` hint. Created by [`add_hook_skip_hint`] for commands that
/// support `--no-hooks`.
///
/// The typed inner field captures the invariant that this only ever wraps
/// a hook failure — no runtime downcast or fallback in `render`.
#[derive(Debug)]
pub struct HookErrorWithHint {
    inner: WorktrunkError,
    hook_type: HookType,
}

impl std::fmt::Display for HookErrorWithHint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl Diagnostic for HookErrorWithHint {
    fn render(&self) -> String {
        // The wrapper is only constructed for `HookCommandFailed`, whose
        // render always emits an `error_message(…)` line — never empty.
        // No empty-body fallback needed.
        // Can't derive command from hook type (e.g., PreRemove is used
        // by both `wt remove` and `wt merge`), so the hint stays generic.
        format!(
            "{}\n{}",
            self.inner.render(),
            hint_message(cformat!(
                "To skip {} hooks, re-run with <underline>--no-hooks</>",
                self.hook_type
            ))
        )
    }
}

impl std::error::Error for HookErrorWithHint {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // Expose the typed leaf so chain walks (e.g., `ErrorExt::exit_code`)
        // can find the underlying `WorktrunkError` without knowing about
        // this wrapper.
        Some(&self.inner)
    }
}

/// Build the leading "To switch to PR/MR …" hint shown when a numeric branch
/// name doesn't exist locally.
///
/// Returns a sentence-cased clause without trailing punctuation so callers can
/// chain it with `; …` follow-ups. Capitalization matches the existing hint
/// style ("To switch …", "to create …").
///
/// `number` is the same digits as the branch (the caller has already parsed
/// the branch as `u32`); combined with the literal `pr:` / `mr:` prefix the
/// result is shell-safe, so the command is formatted directly to avoid
/// `shell_escape` quoting (`'pr:42'`).
fn pr_mr_switch_hint(
    number: u32,
    platform: Option<RefType>,
    ctx: Option<&SwitchSuggestionCtx>,
) -> String {
    let make_cmd = |ref_type: RefType| {
        let cmd = format!("wt switch {}{number}", ref_type.syntax());
        match ctx {
            Some(ctx) => ctx.apply(cmd),
            None => cmd,
        }
    };
    match platform {
        Some(ref_type) => {
            let label = ref_type.display(number);
            let cmd = make_cmd(ref_type);
            cformat!("To switch to {label}, run <underline>{cmd}</>")
        }
        None => {
            let pr_cmd = make_cmd(RefType::Pr);
            let mr_cmd = make_cmd(RefType::Mr);
            cformat!(
                "To switch to PR #{number} or MR !{number}, run <underline>{pr_cmd}</> or <underline>{mr_cmd}</>"
            )
        }
    }
}

/// Format an error with header and gutter content
fn format_error_block(header: impl Into<String>, error: &str) -> String {
    let header = header.into();
    let trimmed = error.trim();
    if trimmed.is_empty() {
        header
    } else {
        format!("{header}\n{}", format_with_gutter(trimmed, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;
    use insta::assert_snapshot;

    /// Render the first `Diagnostic`-implementing error in `err`'s chain,
    /// or fall back to the top-level `Display`. Mirrors what
    /// `format_command_error` in `main.rs` does for the chain-walk case.
    fn render_anyhow(err: &anyhow::Error) -> String {
        for cause in err.chain() {
            if let Some(e) = cause.downcast_ref::<GitError>() {
                return e.render();
            }
            if let Some(e) = cause.downcast_ref::<HookErrorWithHint>() {
                return e.render();
            }
            if let Some(e) = cause.downcast_ref::<WorktrunkError>() {
                return e.render();
            }
            if let Some(e) = cause.downcast_ref::<CommandError>() {
                return e.render();
            }
        }
        err.to_string()
    }

    #[test]
    fn snapshot_into_preserves_type_for_display() {
        // .into() preserves type so we can downcast and use Display
        let err: anyhow::Error = GitError::BranchAlreadyExists {
            branch: "main".into(),
        }
        .into();

        let downcast = err.downcast_ref::<GitError>().expect("Should downcast");
        // Display is the short single-line label
        assert_snapshot!(downcast.to_string(), @"Branch main already exists");
        // Diagnostic::render produces the full styled block
        assert_snapshot!(downcast.render(), @"
        [31m✗[39m [31mBranch [1mmain[22m already exists[39m
        [2m↳[22m [2mTo switch to the existing branch, run without [4m--create[24m: [4mwt switch main[24m[22m
        ");
    }

    #[test]
    fn test_pattern_matching_with_into() {
        let err: anyhow::Error = GitError::BranchAlreadyExists {
            branch: "main".into(),
        }
        .into();

        if let Some(GitError::BranchAlreadyExists { branch }) = err.downcast_ref::<GitError>() {
            assert_eq!(branch, "main");
        } else {
            panic!("Failed to downcast and pattern match");
        }
    }

    #[test]
    fn snapshot_worktree_error_with_path_and_create() {
        let err = GitError::WorktreePathExists {
            branch: "feature".to_string(),
            path: PathBuf::from("/some/path"),
            create: true,
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mDirectory already exists: [1m/some/path[22m[39m
        [2m↳[22m [2mTo remove manually, run [4mrm -rf /some/path[24m; to overwrite (with backup), run [4mwt switch --create --clobber feature[24m[22m
        ");
    }

    #[test]
    fn test_exit_code() {
        // ChildProcessExited
        let err: anyhow::Error = WorktrunkError::ChildProcessExited {
            code: 42,
            message: "test".into(),
            signal: None,
        }
        .into();
        assert_eq!(err.exit_code(), Some(42));

        // HookCommandFailed with code
        let err: anyhow::Error = WorktrunkError::HookCommandFailed {
            hook_type: HookType::PreMerge,
            command_name: Some("test".into()),
            error: "failed".into(),
            exit_code: Some(1),
        }
        .into();
        assert_eq!(err.exit_code(), Some(1));

        // HookCommandFailed without code
        let err: anyhow::Error = WorktrunkError::HookCommandFailed {
            hook_type: HookType::PreMerge,
            command_name: None,
            error: "failed".into(),
            exit_code: None,
        }
        .into();
        assert_eq!(err.exit_code(), None);

        // CommandNotApproved, AlreadyDisplayed, GitError
        assert_eq!(
            anyhow::Error::from(WorktrunkError::CommandNotApproved).exit_code(),
            None
        );
        assert_eq!(
            anyhow::Error::from(WorktrunkError::AlreadyDisplayed { exit_code: 5 }).exit_code(),
            Some(5)
        );
        assert_eq!(
            anyhow::Error::from(GitError::DetachedHead { action: None }).exit_code(),
            None
        );

        // Wrapped hook error
        let inner: anyhow::Error = WorktrunkError::HookCommandFailed {
            hook_type: HookType::PreCommit,
            command_name: Some("lint".into()),
            error: "failed".into(),
            exit_code: Some(7),
        }
        .into();
        assert_eq!(add_hook_skip_hint(inner).exit_code(), Some(7));
    }

    #[test]
    fn test_interrupt_exit_code() {
        // Signal-derived child exit → 128 + sig
        let err: anyhow::Error = WorktrunkError::ChildProcessExited {
            code: 130,
            message: "terminated by signal 2".into(),
            signal: Some(2),
        }
        .into();
        assert_eq!(err.interrupt_exit_code(), Some(130));

        let err: anyhow::Error = WorktrunkError::ChildProcessExited {
            code: 143,
            message: "terminated by signal 15".into(),
            signal: Some(15),
        }
        .into();
        assert_eq!(err.interrupt_exit_code(), Some(143));

        // Ordinary non-zero exit → not an interrupt
        let err: anyhow::Error = WorktrunkError::ChildProcessExited {
            code: 1,
            message: "exit status: 1".into(),
            signal: None,
        }
        .into();
        assert_eq!(err.interrupt_exit_code(), None);

        // Other WorktrunkError variants → not an interrupt
        assert_eq!(
            anyhow::Error::from(WorktrunkError::AlreadyDisplayed { exit_code: 130 })
                .interrupt_exit_code(),
            None,
        );
        assert_eq!(
            anyhow::Error::from(WorktrunkError::CommandNotApproved).interrupt_exit_code(),
            None,
        );

        // Plain anyhow error → not an interrupt
        assert_eq!(
            anyhow::anyhow!("some unrelated failure").interrupt_exit_code(),
            None,
        );
    }

    #[test]
    fn snapshot_add_hook_skip_hint() {
        // Wraps HookCommandFailed with --no-hooks hint
        let inner: anyhow::Error = WorktrunkError::HookCommandFailed {
            hook_type: HookType::PreMerge,
            command_name: Some("test".into()),
            error: "failed".into(),
            exit_code: Some(1),
        }
        .into();
        assert_snapshot!(render_anyhow(&add_hook_skip_hint(inner)), @"
        [31m✗[39m [31mpre-merge command failed: [1mtest[22m: failed[39m
        [2m↳[22m [2mTo skip pre-merge hooks, re-run with [4m--no-hooks[24m[22m
        ");

        // pre-commit hook type
        let inner: anyhow::Error = WorktrunkError::HookCommandFailed {
            hook_type: HookType::PreCommit,
            command_name: Some("build".into()),
            error: "Build failed".into(),
            exit_code: Some(1),
        }
        .into();
        assert_snapshot!(render_anyhow(&add_hook_skip_hint(inner)), @"
        [31m✗[39m [31mpre-commit command failed: [1mbuild[22m: Build failed[39m
        [2m↳[22m [2mTo skip pre-commit hooks, re-run with [4m--no-hooks[24m[22m
        ");

        // Passes through non-hook errors unchanged (no --no-hooks hint)
        let err: anyhow::Error = WorktrunkError::ChildProcessExited {
            code: 1,
            message: "test".into(),
            signal: None,
        }
        .into();
        assert!(!render_anyhow(&add_hook_skip_hint(err)).contains("--no-hooks"));

        let err: anyhow::Error = GitError::DetachedHead { action: None }.into();
        assert!(!render_anyhow(&add_hook_skip_hint(err)).contains("--no-hooks"));

        let err: anyhow::Error = GitError::Other {
            message: "some error".into(),
        }
        .into();
        assert!(!render_anyhow(&add_hook_skip_hint(err)).contains("--no-hooks"));
    }

    /// `Display` is the short single-line label per variant — no symbol,
    /// no color codes, no hint. It's what callers embed in another error's
    /// message field (e.g., `format!("…: {e}")`).
    #[test]
    fn snapshot_short_display_per_variant() {
        // GitError variants
        assert_snapshot!(
            GitError::BranchAlreadyExists { branch: "feature".into() }.to_string(),
            @"Branch feature already exists"
        );
        assert_snapshot!(
            GitError::WorktreeRemovalFailed {
                branch: "feature".into(),
                path: PathBuf::from("/tmp/repo.feature"),
                error: "fatal: …".into(),
                remaining_entries: None,
            }.to_string(),
            @"Failed to remove worktree for feature @ /tmp/repo.feature"
        );
        assert_snapshot!(
            GitError::RebaseConflict {
                target_branch: "main".into(),
                git_output: "CONFLICT (content): …".into(),
            }.to_string(),
            @"Rebase onto main incomplete"
        );
        assert_snapshot!(
            GitError::DetachedHead { action: Some("merge".into()) }.to_string(),
            @"Cannot merge: not on a branch (detached HEAD)"
        );
        assert_snapshot!(
            GitError::DetachedHead { action: None }.to_string(),
            @"Not on a branch (detached HEAD)"
        );

        // WithSwitchSuggestion delegates to inner — ctx only affects render
        let inner = GitError::BranchAlreadyExists {
            branch: "feature".into(),
        };
        let wrapped = GitError::WithSwitchSuggestion {
            source: Box::new(inner.clone()),
            ctx: SwitchSuggestionCtx {
                extra_flags: vec!["--execute=claude".into()],
                trailing_args: vec![],
            },
        };
        assert_eq!(inner.to_string(), wrapped.to_string());

        // WorktrunkError variants
        assert_snapshot!(
            WorktrunkError::HookCommandFailed {
                hook_type: HookType::PreMerge,
                command_name: Some("lint".into()),
                error: "lint failed".into(),
                exit_code: Some(1),
            }.to_string(),
            @"pre-merge command failed: lint: lint failed"
        );
        assert_snapshot!(
            WorktrunkError::ChildProcessExited {
                code: 1,
                message: "exit status: 1".into(),
                signal: None,
            }.to_string(),
            @"exit status: 1"
        );
        // Silent variants
        assert_eq!(WorktrunkError::CommandNotApproved.to_string(), "");
        assert_eq!(
            WorktrunkError::AlreadyDisplayed { exit_code: 1 }.to_string(),
            ""
        );
    }

    #[test]
    fn test_format_error_block() {
        let header = "Error occurred".to_string();
        assert_snapshot!(format_error_block(header.clone(), "  some error text  "), @"
        Error occurred
        [107m [0m some error text
        ");

        // Empty/whitespace returns header only
        assert_eq!(format_error_block(header.clone(), ""), header);
        assert_eq!(format_error_block(header.clone(), "   \n\t  "), header);
    }

    #[test]
    fn snapshot_worktrunk_error_display() {
        let err = WorktrunkError::ChildProcessExited {
            code: 1,
            message: "Command failed".into(),
            signal: None,
        };
        assert_snapshot!(err.render(), @"[31m✗[39m [31mCommand failed[39m");

        let err = WorktrunkError::HookCommandFailed {
            hook_type: HookType::PreMerge,
            command_name: Some("lint".into()),
            error: "lint failed".into(),
            exit_code: Some(1),
        };
        assert_snapshot!(err.render(), @"[31m✗[39m [31mpre-merge command failed: [1mlint[22m: lint failed[39m");

        let err = WorktrunkError::HookCommandFailed {
            hook_type: HookType::PreStart,
            command_name: None,
            error: "setup failed".into(),
            exit_code: None,
        };
        assert_snapshot!(err.render(), @"[31m✗[39m [31mpre-start command failed: setup failed[39m");

        // Silent errors produce empty output
        assert_eq!(format!("{}", WorktrunkError::CommandNotApproved), "");
        assert_eq!(
            format!("{}", WorktrunkError::AlreadyDisplayed { exit_code: 1 }),
            ""
        );
    }

    #[test]
    fn snapshot_not_in_worktree() {
        let err = GitError::NotInWorktree {
            action: Some("resolve @".into()),
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mCannot resolve @: not in a worktree[39m
        [2m↳[22m [2mRun from inside a worktree, or specify a branch name[22m
        ");

        let err = GitError::NotInWorktree { action: None };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mNot in a worktree[39m
        [2m↳[22m [2mRun from inside a worktree, or specify a branch name[22m
        ");
    }

    #[test]
    fn snapshot_worktree_path_occupied() {
        let err = GitError::WorktreePathOccupied {
            branch: "feature".into(),
            path: PathBuf::from("/tmp/repo"),
            occupant: Some("main".into()),
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mCannot switch to [1mfeature[22m — there's a worktree at the expected path [1m/tmp/repo[22m on branch [1mmain[22m[39m
        [2m↳[22m [2mTo switch the worktree at [4m/tmp/repo[24m to [4mfeature[24m, run [4mcd /tmp/repo && git switch feature[24m[22m
        ");

        let err = GitError::WorktreePathOccupied {
            branch: "feature".into(),
            path: PathBuf::from("/tmp/repo"),
            occupant: None,
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mCannot switch to [1mfeature[22m — there's a detached worktree at the expected path [1m/tmp/repo[22m[39m
        [2m↳[22m [2mTo switch the worktree at [4m/tmp/repo[24m to [4mfeature[24m, run [4mcd /tmp/repo && git switch feature[24m[22m
        ");
    }

    #[test]
    fn snapshot_worktree_path_occupied_special_chars() {
        // Spaces in path and branch name require shell escaping in the hint command
        let err = GitError::WorktreePathOccupied {
            branch: "feature/my branch".into(),
            path: PathBuf::from("/tmp/my repo"),
            occupant: Some("main".into()),
        };
        let output = err.render();
        // The hint command must quote the path and branch for safe shell execution
        assert!(
            output.contains("cd '/tmp/my repo' && git switch 'feature/my branch'"),
            "expected shell-escaped command in hint, got: {output}"
        );
    }

    #[test]
    fn snapshot_worktree_creation_failed() {
        let err = GitError::WorktreeCreationFailed {
            branch: "feature".into(),
            base_branch: Some("main".into()),
            error: "git error".into(),
            command: None,
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mFailed to create worktree for [1mfeature[22m from base [1mmain[22m[39m
        [107m [0m git error
        ");

        let err = GitError::WorktreeCreationFailed {
            branch: "feature".into(),
            base_branch: None,
            error: "git error".into(),
            command: None,
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mFailed to create worktree for [1mfeature[22m[39m
        [107m [0m git error
        ");

        let err = GitError::WorktreeCreationFailed {
            branch: "feature".into(),
            base_branch: Some("main".into()),
            error: "fatal: ref exists".into(),
            command: Some(FailedCommand {
                command: "git worktree add /path -b feature main".into(),
                exit_info: "exit code 128".into(),
            }),
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mFailed to create worktree for [1mfeature[22m from base [1mmain[22m[39m
        [107m [0m fatal: ref exists
        [2m↳[22m [2mFailed command, [4mexit code 128[24m:[22m
        [107m [0m [2m[0m[2m[34mgit[0m[2m worktree add /path [0m[2m[36m-b[0m[2m feature main[0m
        ");
    }

    #[test]
    fn snapshot_worktree_locked() {
        let err = GitError::WorktreeLocked {
            branch: "feature".into(),
            path: PathBuf::from("/tmp/repo.feature"),
            reason: Some("Testing lock".into()),
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mCannot remove [1mfeature[22m, worktree is locked (Testing lock)[39m
        [2m↳[22m [2mTo unlock, run [4mgit worktree unlock /tmp/repo.feature[24m[22m
        ");

        // Empty reason should not show parentheses
        let err = GitError::WorktreeLocked {
            branch: "feature".into(),
            path: PathBuf::from("/tmp/repo.feature"),
            reason: Some("".into()),
        };
        let display = err.render();
        assert_snapshot!(display, @"
        [31m✗[39m [31mCannot remove [1mfeature[22m, worktree is locked[39m
        [2m↳[22m [2mTo unlock, run [4mgit worktree unlock /tmp/repo.feature[24m[22m
        ");
        assert!(
            !display.contains("locked ("),
            "should not show parentheses without reason"
        );
    }

    #[test]
    fn snapshot_not_rebased() {
        let err = GitError::NotRebased {
            target_branch: "main".into(),
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mBranch not rebased onto [1mmain[22m[39m
        [2m↳[22m [2mTo rebase first, run [4mwt step rebase main[24m; or remove [4m--no-rebase[24m[22m
        ");
    }

    #[test]
    fn snapshot_hook_command_not_found() {
        let err = GitError::HookCommandNotFound {
            name: "unknown".into(),
            available: vec!["lint".into(), "test".into()],
        };
        assert_snapshot!(err.render(), @"[31m✗[39m [31mNo command named [1munknown[22m (available: [1mlint[22m, [1mtest[22m)[39m");

        let err = GitError::HookCommandNotFound {
            name: "unknown".into(),
            available: vec![],
        };
        assert_snapshot!(err.render(), @"[31m✗[39m [31mNo command named [1munknown[22m (hook has no named commands)[39m");
    }

    #[test]
    fn snapshot_llm_command_failed() {
        let err = GitError::LlmCommandFailed {
            command: "llm".into(),
            error: "connection failed".into(),
            reproduction_command: Some("wt step commit --show-prompt | llm".into()),
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mCommit generation command failed[39m
        [107m [0m connection failed
        [2m○[22m Ran command:
        [107m [0m wt step commit --show-prompt | llm
        ");

        let err = GitError::LlmCommandFailed {
            command: "llm --model gpt-4".into(),
            error: "timeout".into(),
            reproduction_command: None,
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mCommit generation command failed[39m
        [107m [0m timeout
        [2m○[22m Ran command:
        [107m [0m llm --model gpt-4
        ");
    }

    #[test]
    fn snapshot_uncommitted_changes() {
        // Action only (negative assertion kept: no --force)
        let err = GitError::UncommittedChanges {
            action: Some("push".into()),
            branch: None,
            force_hint: false,
        };
        let display = err.render();
        assert_snapshot!(display, @"
        [31m✗[39m [31mCannot push: working tree has uncommitted changes[39m
        [2m↳[22m [2mCommit or stash changes first[22m
        ");
        assert!(!display.contains("--force"));

        // Branch only
        let err = GitError::UncommittedChanges {
            action: None,
            branch: Some("feature".into()),
            force_hint: false,
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31m[1mfeature[22m has uncommitted changes[39m
        [2m↳[22m [2mCommit or stash changes first[22m
        ");

        // Neither action nor branch
        let err = GitError::UncommittedChanges {
            action: None,
            branch: None,
            force_hint: false,
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mWorking tree has uncommitted changes[39m
        [2m↳[22m [2mCommit or stash changes first[22m
        ");

        // With force_hint
        let err = GitError::UncommittedChanges {
            action: Some("remove worktree".into()),
            branch: Some("feature".into()),
            force_hint: true,
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mCannot remove worktree: [1mfeature[22m has uncommitted changes[39m
        [2m↳[22m [2mCommit or stash changes first, or to lose uncommitted changes, run [4mwt remove --force feature[24m[22m
        ");
    }

    #[test]
    fn snapshot_not_fast_forward() {
        // Empty commits, outside merge context
        let err = GitError::NotFastForward {
            target_branch: "main".into(),
            commits_formatted: "".into(),
            in_merge_context: false,
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mCan't push to local [1mmain[22m branch: it has newer commits[39m
        [2m↳[22m [2mTo rebase onto [4mmain[24m, run [4mwt step rebase main[24m[22m
        ");

        // With commits, outside merge context
        let err = GitError::NotFastForward {
            target_branch: "develop".into(),
            commits_formatted: "abc123 Some commit".into(),
            in_merge_context: false,
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mCan't push to local [1mdevelop[22m branch: it has newer commits[39m
        [107m [0m abc123 Some commit
        [2m↳[22m [2mTo rebase onto [4mdevelop[24m, run [4mwt step rebase develop[24m[22m
        ");

        // In merge context
        let err = GitError::NotFastForward {
            target_branch: "main".into(),
            commits_formatted: "def456 Another commit".into(),
            in_merge_context: true,
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mCan't push to local [1mmain[22m branch: it has newer commits[39m
        [107m [0m def456 Another commit
        [2m↳[22m [2mTo incorporate these changes, run [4mwt merge main[24m again[22m
        ");
    }

    #[test]
    fn snapshot_conflicting_changes_empty_files() {
        let err = GitError::ConflictingChanges {
            target_branch: "main".into(),
            files: vec![],
            worktree_path: PathBuf::from("/tmp/repo"),
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mCan't push to local [1mmain[22m branch: conflicting uncommitted changes[39m
        [2m↳[22m [2mCommit or stash these changes in /tmp/repo first[22m
        ");
    }

    #[test]
    fn snapshot_cli_api_error() {
        let err = GitError::CliApiError {
            ref_type: RefType::Pr,
            message: "gh api failed for PR #42".into(),
            stderr: "error: unexpected response\ncode: 500".into(),
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mgh api failed for PR #42[39m
        [107m [0m error: unexpected response
        [107m [0m code: 500
        ");
    }

    #[test]
    fn snapshot_no_remote_for_repo() {
        let err = GitError::NoRemoteForRepo {
            owner: "upstream-owner".into(),
            repo: "upstream-repo".into(),
            suggested_url: "https://github.com/upstream-owner/upstream-repo.git".into(),
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mNo remote found for [1mupstream-owner/upstream-repo[22m[39m
        [2m↳[22m [2mAdd the remote: [4mgit remote add upstream https://github.com/upstream-owner/upstream-repo.git[24m[22m
        ");
    }

    #[test]
    fn snapshot_rebase_conflict_empty_output() {
        let err = GitError::RebaseConflict {
            target_branch: "main".into(),
            git_output: "".into(),
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mRebase onto [1mmain[22m incomplete[39m
        [2m↳[22m [2mTo continue after resolving conflicts, run [4mgit rebase --continue[24m[22m
        [2m↳[22m [2mTo abort, run [4mgit rebase --abort[24m[22m
        ");
    }

    #[test]
    fn snapshot_with_switch_suggestion_branch_already_exists() {
        let err = GitError::WithSwitchSuggestion {
            source: Box::new(GitError::BranchAlreadyExists {
                branch: "emails".into(),
            }),
            ctx: SwitchSuggestionCtx {
                extra_flags: vec!["--execute=claude".into()],
                trailing_args: vec!["Check my emails".into()],
            },
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mBranch [1memails[22m already exists[39m
        [2m↳[22m [2mTo switch to the existing branch, run without [4m--create[24m: [4mwt switch emails --execute=claude -- 'Check my emails'[24m[22m
        ");
    }

    #[test]
    fn snapshot_with_switch_suggestion_worktree_path_exists() {
        let err = GitError::WithSwitchSuggestion {
            source: Box::new(GitError::WorktreePathExists {
                branch: "emails".into(),
                path: PathBuf::from("/tmp/repo.emails"),
                create: true,
            }),
            ctx: SwitchSuggestionCtx {
                extra_flags: vec!["--execute=claude".into()],
                trailing_args: vec!["Check my emails".into()],
            },
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mDirectory already exists: [1m/tmp/repo.emails[22m[39m
        [2m↳[22m [2mTo remove manually, run [4mrm -rf /tmp/repo.emails[24m; to overwrite (with backup), run [4mwt switch --create --clobber emails --execute=claude -- 'Check my emails'[24m[22m
        ");
    }

    #[test]
    fn snapshot_with_switch_suggestion_no_trailing_args() {
        let err = GitError::WithSwitchSuggestion {
            source: Box::new(GitError::BranchAlreadyExists {
                branch: "emails".into(),
            }),
            ctx: SwitchSuggestionCtx {
                extra_flags: vec!["--execute=claude".into()],
                trailing_args: vec![],
            },
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mBranch [1memails[22m already exists[39m
        [2m↳[22m [2mTo switch to the existing branch, run without [4m--create[24m: [4mwt switch emails --execute=claude[24m[22m
        ");
    }

    #[test]
    fn snapshot_with_switch_suggestion_branch_not_found() {
        let err = GitError::WithSwitchSuggestion {
            source: Box::new(GitError::BranchNotFound {
                branch: "emails".into(),
                show_create_hint: true,
                last_fetch_ago: None,
                pr_mr_platform: None,
            }),
            ctx: SwitchSuggestionCtx {
                extra_flags: vec!["--execute=claude".into()],
                trailing_args: vec!["Check my emails".into()],
            },
        };
        assert_snapshot!(err.render(), @"
        [31m✗[39m [31mNo branch named [1memails[22m[39m
        [2m↳[22m [2mTo create a new branch, run [4mwt switch --create emails --execute=claude -- 'Check my emails'[24m; to list branches, run [4mwt list --branches --remotes[24m[22m
        ");
    }

    #[test]
    fn test_with_switch_suggestion_unwrapped_errors_unaffected() {
        // Non-switch-suggestion errors should be completely unaffected by the wrapper
        let inner = GitError::DetachedHead {
            action: Some("merge".into()),
        };
        let wrapped = GitError::WithSwitchSuggestion {
            source: Box::new(inner.clone()),
            ctx: SwitchSuggestionCtx {
                extra_flags: vec!["--execute=claude".into()],
                trailing_args: vec!["Check my emails".into()],
            },
        };
        // Errors without switch suggestions should render identically
        assert_eq!(inner.to_string(), wrapped.to_string());
    }

    fn sample_command_error() -> CommandError {
        CommandError {
            program: "git".into(),
            args: vec!["worktree".into(), "list".into()],
            stderr: "fatal: not a git repository\n".into(),
            stdout: String::new(),
            exit_code: Some(128),
        }
    }

    #[test]
    fn command_error_display_is_single_line() {
        let err = sample_command_error();
        let s = err.to_string();
        assert_eq!(s, "git worktree list failed (exit 128)");
        assert!(!s.contains('\n'));
    }

    #[test]
    fn command_error_command_string_handles_empty_args() {
        // Degenerate but reachable when a `Cmd` is built with no
        // `.args(...)` call. Covers the args-empty branch of
        // `command_string()` (codecov flagged this line).
        let err = CommandError {
            program: "git".into(),
            args: Vec::new(),
            stderr: String::new(),
            stdout: String::new(),
            exit_code: Some(1),
        };
        assert_eq!(err.command_string(), "git");
        assert_eq!(err.to_string(), "git failed (exit 1)");
    }

    #[test]
    fn command_error_combined_output_strips_trailing_whitespace_and_joins() {
        let err = CommandError {
            program: "git".into(),
            args: vec!["push".into()],
            stderr: "warning: line 1\nfatal: line 2\n\n".into(),
            stdout: "  trailing-stdout-error\n".into(),
            exit_code: Some(1),
        };
        // Both streams are trimmed, then joined with `\n`.
        assert_eq!(
            err.combined_output(),
            "warning: line 1\nfatal: line 2\ntrailing-stdout-error",
        );
    }

    #[test]
    fn command_error_combined_output_drops_empty_streams() {
        let err = CommandError {
            program: "git".into(),
            args: vec!["status".into()],
            stderr: "   ".into(),
            stdout: "actual error on stdout".into(),
            exit_code: Some(1),
        };
        assert_eq!(err.combined_output(), "actual error on stdout");
    }

    #[test]
    fn command_error_signal_kill_omits_exit_code() {
        let err = CommandError {
            program: "git".into(),
            args: vec!["fetch".into()],
            stderr: String::new(),
            stdout: String::new(),
            exit_code: None,
        };
        assert_eq!(err.to_string(), "git fetch failed");
    }

    #[test]
    fn command_error_find_in_walks_anyhow_chain() {
        // Wrapping with `.context(...)` must not hide the typed leaf — the
        // helper should pull it out regardless of how many layers wrap.
        let err: anyhow::Error = Err::<(), _>(sample_command_error())
            .context("listing worktrees")
            .context("running prune")
            .unwrap_err();

        let cmd_err = CommandError::find_in(&err).expect("CommandError should be found in chain");
        assert_eq!(cmd_err.program, "git");
        assert_eq!(
            cmd_err.args,
            vec!["worktree".to_string(), "list".to_string()]
        );
        assert!(cmd_err.stderr.contains("not a git repository"));
    }

    #[test]
    fn command_error_find_in_returns_none_for_unrelated_error() {
        let err = anyhow::anyhow!("some other failure");
        assert!(CommandError::find_in(&err).is_none());
    }
}
