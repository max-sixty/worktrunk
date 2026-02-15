//! Jujutsu (jj) implementation of the [`Workspace`] trait.
//!
//! Implements workspace operations by shelling out to `jj` commands
//! and parsing their output.

use std::any::Any;
use std::path::{Path, PathBuf};

use anyhow::Context;
use color_print::cformat;

use super::types::{IntegrationReason, LineDiff, PushDisplay};
use crate::shell_exec::Cmd;
use crate::styling::{eprintln, progress_message};

use super::{PushResult, RebaseOutcome, SquashOutcome, VcsKind, Workspace, WorkspaceItem};

/// Jujutsu-backed workspace implementation.
///
/// Wraps a jj repository root path and implements [`Workspace`] by running
/// `jj` CLI commands. Each method shells out to the appropriate `jj` subcommand.
#[derive(Debug, Clone)]
pub struct JjWorkspace {
    /// Root directory of the jj repository.
    root: PathBuf,
}

impl JjWorkspace {
    /// Create a new `JjWorkspace` rooted at the given path.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Detect and create a `JjWorkspace` from the current directory.
    ///
    /// Runs `jj root` to find the repository root.
    pub fn from_current_dir() -> anyhow::Result<Self> {
        let stdout = run_jj_command(Path::new("."), &["root"])?;
        Ok(Self::new(PathBuf::from(stdout.trim())))
    }

    /// The repository root path.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Run a jj command in this repository's root directory.
    fn run_command(&self, args: &[&str]) -> anyhow::Result<String> {
        run_jj_command(&self.root, args)
    }

    /// Run a jj command in the specified directory.
    ///
    /// Unlike `run_command` which runs in the repo root, this runs in the given
    /// directory — needed when commands must execute in a specific workspace.
    pub fn run_in_dir(&self, dir: &Path, args: &[&str]) -> anyhow::Result<String> {
        run_jj_command(dir, args)
    }

    /// Find which workspace contains the given directory.
    ///
    /// Returns the `WorkspaceItem` whose path is an ancestor of `cwd`.
    pub fn current_workspace(&self, cwd: &Path) -> anyhow::Result<WorkspaceItem> {
        let workspaces = self.list_workspaces()?;
        let cwd = dunce::canonicalize(cwd)?;
        workspaces
            .into_iter()
            .find(|ws| {
                dunce::canonicalize(&ws.path)
                    .map(|p| cwd.starts_with(&p))
                    .unwrap_or(false)
            })
            .ok_or_else(|| anyhow::anyhow!("Not inside a jj workspace"))
    }

    /// Detect the bookmark name associated with `trunk()`.
    ///
    /// Returns `None` if no bookmarks are found on the `trunk()` revset
    /// (common in local-only repos without remote tracking).
    fn trunk_bookmark(&self) -> anyhow::Result<Option<String>> {
        let output = self.run_command(&[
            "log",
            "-r",
            "trunk()",
            "--no-graph",
            "-T",
            r#"self.bookmarks().map(|b| b.name()).join("\n")"#,
        ])?;
        let bookmarks: Vec<&str> = output.trim().lines().filter(|l| !l.is_empty()).collect();

        // Prefer "main", then "master", then first found
        if bookmarks.contains(&"main") {
            Ok(Some("main".to_string()))
        } else if bookmarks.contains(&"master") {
            Ok(Some("master".to_string()))
        } else {
            Ok(bookmarks.first().map(|s| s.to_string()))
        }
    }

    /// Check if common default branch bookmarks ("main", "master") exist locally.
    ///
    /// Used as a fallback when `trunk()` doesn't resolve to a bookmark (e.g.,
    /// local-only repos without remote tracking).
    fn find_local_default_bookmark(&self) -> Option<String> {
        let output = self
            .run_command(&["bookmark", "list", "-T", r#"name ++ "\n""#])
            .ok()?;
        let bookmarks: Vec<&str> = output.lines().filter(|l| !l.is_empty()).collect();
        for candidate in ["main", "master"] {
            if bookmarks.contains(&candidate) {
                return Some(candidate.to_string());
            }
        }
        None
    }

    /// Determine the feature tip change ID.
    ///
    /// In jj, the working copy (@) is often an empty auto-snapshot commit.
    /// When @ is empty, the real feature tip is @- (the parent).
    pub fn feature_tip(&self, ws_path: &Path) -> anyhow::Result<String> {
        let empty_check = run_jj_command(
            ws_path,
            &[
                "log",
                "-r",
                "@",
                "--no-graph",
                "-T",
                r#"if(self.empty(), "empty", "content")"#,
            ],
        )?;

        let revset = if empty_check.trim() == "empty" {
            "@-"
        } else {
            "@"
        };

        let output = run_jj_command(
            ws_path,
            &[
                "log",
                "-r",
                revset,
                "--no-graph",
                "-T",
                r#"self.change_id().short(12)"#,
            ],
        )?;

        Ok(output.trim().to_string())
    }

    /// Get commit details (timestamp, description) for the working-copy commit
    /// in a specific workspace directory.
    ///
    /// Returns `(unix_timestamp, first_line_of_description)`.
    pub fn commit_details(&self, ws_path: &Path) -> anyhow::Result<(i64, String)> {
        let template = r#"self.committer().timestamp().utc().format("%s") ++ "\t" ++ self.description().first_line()"#;
        let output = run_jj_command(ws_path, &["log", "-r", "@", "--no-graph", "-T", template])?;
        let line = output.trim();
        let (timestamp_str, message) = line
            .split_once('\t')
            .ok_or_else(|| anyhow::anyhow!("unexpected commit details format: {line}"))?;
        let timestamp = timestamp_str
            .parse::<i64>()
            .with_context(|| format!("invalid timestamp: {timestamp_str}"))?;
        Ok((timestamp, message.to_string()))
    }
}

/// Run a jj command at the given directory, returning stdout on success.
fn run_jj_command(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let mut cmd_args = vec!["--no-pager", "--color", "never"];
    cmd_args.extend_from_slice(args);

    let output = Cmd::new("jj")
        .args(cmd_args.iter().copied())
        .current_dir(dir)
        .run()
        .with_context(|| format!("Failed to execute: jj {}", args.join(" ")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let error_msg = [stderr.trim(), stdout.trim()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        anyhow::bail!("{}", error_msg);
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parse the summary line from `jj diff --stat` output.
///
/// Format: `N files changed, N insertions(+), N deletions(-)`
/// Returns `(insertions, deletions)`.
fn parse_diff_stat_summary(output: &str) -> LineDiff {
    // The summary line is the last non-empty line
    let summary = output.lines().rev().find(|l| !l.is_empty()).unwrap_or("");

    let mut added = 0usize;
    let mut deleted = 0usize;

    // Parse "N insertions(+)" and "N deletions(-)"
    for part in summary.split(", ") {
        let part = part.trim();
        if part.contains("insertion")
            && let Some(n) = part.split_whitespace().next().and_then(|s| s.parse().ok())
        {
            added = n;
        } else if part.contains("deletion")
            && let Some(n) = part.split_whitespace().next().and_then(|s| s.parse().ok())
        {
            deleted = n;
        }
    }

    LineDiff { added, deleted }
}

impl Workspace for JjWorkspace {
    fn kind(&self) -> VcsKind {
        VcsKind::Jj
    }

    fn list_workspaces(&self) -> anyhow::Result<Vec<WorkspaceItem>> {
        // Template outputs: name\tchange_id_short\n
        let template = r#"name ++ "\t" ++ target.change_id().short(12) ++ "\n""#;
        let output = self.run_command(&["workspace", "list", "-T", template])?;

        let mut items = Vec::new();
        for line in output.lines() {
            if line.is_empty() {
                continue;
            }
            let Some((name, change_id)) = line.split_once('\t') else {
                continue;
            };

            // Get workspace path
            let path_output = self.run_command(&["workspace", "root", "--name", name])?;
            let path = PathBuf::from(path_output.trim());

            let is_default = name == "default";

            items.push(WorkspaceItem {
                path,
                name: name.to_string(),
                head: change_id.to_string(),
                branch: None,
                is_default,
                locked: None,
                prunable: None,
            });
        }

        Ok(items)
    }

    fn workspace_path(&self, name: &str) -> anyhow::Result<PathBuf> {
        let output = self.run_command(&["workspace", "root", "--name", name])?;
        Ok(PathBuf::from(output.trim()))
    }

    fn default_workspace_path(&self) -> anyhow::Result<Option<PathBuf>> {
        // Try "default" workspace; if it doesn't exist, return None
        match self.run_command(&["workspace", "root", "--name", "default"]) {
            Ok(output) => Ok(Some(PathBuf::from(output.trim()))),
            Err(_) => Ok(None),
        }
    }

    fn default_branch_name(&self) -> Option<String> {
        // Check explicit config override first
        if let Some(name) = self
            .run_command(&["config", "get", "worktrunk.default-branch"])
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        {
            return Some(name);
        }
        // Fall back to trunk() revset detection (requires remote bookmarks)
        if let Some(name) = self.trunk_bookmark().ok().flatten() {
            return Some(name);
        }
        // Local-only repos: check if common default branch bookmarks exist
        self.find_local_default_bookmark()
    }

    fn set_default_branch(&self, name: &str) -> anyhow::Result<()> {
        self.run_command(&["config", "set", "--repo", "worktrunk.default-branch", name])?;
        Ok(())
    }

    fn clear_default_branch(&self) -> anyhow::Result<bool> {
        Ok(self
            .run_command(&["config", "unset", "--repo", "worktrunk.default-branch"])
            .is_ok())
    }

    fn is_dirty(&self, path: &Path) -> anyhow::Result<bool> {
        // jj auto-snapshots the working copy, so "dirty" means the working-copy
        // commit has file changes (is not empty)
        let output = run_jj_command(
            path,
            &[
                "log",
                "-r",
                "@",
                "--no-graph",
                "-T",
                r#"if(self.empty(), "clean", "dirty")"#,
            ],
        )?;
        Ok(output.trim() == "dirty")
    }

    fn working_diff(&self, path: &Path) -> anyhow::Result<LineDiff> {
        let output = run_jj_command(path, &["diff", "--stat"])?;
        Ok(parse_diff_stat_summary(&output))
    }

    fn ahead_behind(&self, base: &str, head: &str) -> anyhow::Result<(usize, usize)> {
        // Count commits in head that aren't in base (ahead)
        let ahead_revset = format!("{base}..{head}");
        let ahead_output =
            self.run_command(&["log", "-r", &ahead_revset, "--no-graph", "-T", r#""x\n""#])?;
        let ahead = ahead_output.lines().filter(|l| !l.is_empty()).count();

        // Count commits in base that aren't in head (behind)
        let behind_revset = format!("{head}..{base}");
        let behind_output =
            self.run_command(&["log", "-r", &behind_revset, "--no-graph", "-T", r#""x\n""#])?;
        let behind = behind_output.lines().filter(|l| !l.is_empty()).count();

        Ok((ahead, behind))
    }

    fn is_integrated(&self, id: &str, target: &str) -> anyhow::Result<Option<IntegrationReason>> {
        // Check if the change is an ancestor of (or same as) the target
        let revset = format!("{id} & ::{target}");
        let output = self.run_command(&["log", "-r", &revset, "--no-graph", "-T", r#""x""#])?;

        if !output.trim().is_empty() {
            return Ok(Some(IntegrationReason::Ancestor));
        }

        Ok(None)
    }

    fn branch_diff_stats(&self, base: &str, head: &str) -> anyhow::Result<LineDiff> {
        let output = self.run_command(&["diff", "--stat", "--from", base, "--to", head])?;
        Ok(parse_diff_stat_summary(&output))
    }

    fn create_workspace(&self, name: &str, base: Option<&str>, path: &Path) -> anyhow::Result<()> {
        let path_str = path.to_str().ok_or_else(|| {
            anyhow::anyhow!("Workspace path contains invalid UTF-8: {}", path.display())
        })?;

        let mut args = vec!["workspace", "add", "--name", name, path_str];
        if let Some(revision) = base {
            args.extend_from_slice(&["--revision", revision]);
        }
        self.run_command(&args)?;
        Ok(())
    }

    fn remove_workspace(&self, name: &str) -> anyhow::Result<()> {
        self.run_command(&["workspace", "forget", name])?;
        Ok(())
    }

    fn resolve_integration_target(&self, target: Option<&str>) -> anyhow::Result<String> {
        match target {
            Some(t) => Ok(t.to_string()),
            None => self.default_branch_name().ok_or_else(|| {
                anyhow::anyhow!("Cannot determine default branch. Specify target explicitly or run: wt config state default-branch set BRANCH")
            }),
        }
    }

    fn is_rebased_onto(&self, target: &str, path: &Path) -> anyhow::Result<bool> {
        let feature_tip = self.feature_tip(path)?;
        // target is ancestor of feature tip iff "target & ::feature_tip" is non-empty
        let check = run_jj_command(
            path,
            &[
                "log",
                "-r",
                &format!("{target} & ::{feature_tip}"),
                "--no-graph",
                "-T",
                r#""x""#,
            ],
        )?;
        Ok(!check.trim().is_empty())
    }

    fn rebase_onto(&self, target: &str, path: &Path) -> anyhow::Result<RebaseOutcome> {
        eprintln!(
            "{}",
            progress_message(cformat!("Rebasing onto <bold>{target}</>..."))
        );
        run_jj_command(path, &["rebase", "-b", "@", "-d", target])?;
        // jj doesn't distinguish fast-forward from true rebase
        Ok(RebaseOutcome::Rebased)
    }

    fn root_path(&self) -> anyhow::Result<PathBuf> {
        Ok(self.root.clone())
    }

    fn current_workspace_path(&self) -> anyhow::Result<PathBuf> {
        let cwd = std::env::current_dir()?;
        Ok(self.current_workspace(&cwd)?.path)
    }

    fn current_name(&self, path: &Path) -> anyhow::Result<Option<String>> {
        Ok(Some(self.current_workspace(path)?.name))
    }

    fn project_identifier(&self) -> anyhow::Result<String> {
        // Most jj repos are git-backed. Try to get the git remote URL for a stable
        // project identifier (same clone = same ID, regardless of directory name).
        if let Ok(output) = self.run_command(&["git", "remote", "list"]) {
            // Format: "remote_name url\n" — prefer "origin" if present
            let url = output
                .lines()
                .find(|l| l.starts_with("origin "))
                .or_else(|| output.lines().next())
                .and_then(|l| l.split_whitespace().nth(1));
            if let Some(url) = url {
                return Ok(url.to_string());
            }
        }
        // Fallback: use directory name
        self.root
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Repository path has no filename"))
    }

    fn commit(&self, message: &str, path: &Path) -> anyhow::Result<String> {
        run_jj_command(path, &["describe", "-m", message])?;
        run_jj_command(path, &["new"])?;
        // Return the change ID of the just-described commit (now @-)
        let output = run_jj_command(
            path,
            &[
                "log",
                "-r",
                "@-",
                "--no-graph",
                "-T",
                r#"self.change_id().short(12)"#,
            ],
        )?;
        Ok(output.trim().to_string())
    }

    fn commit_subjects(&self, base: &str, head: &str) -> anyhow::Result<Vec<String>> {
        let revset = format!("{base}..{head}");
        let output = run_jj_command(
            &self.root,
            &[
                "log",
                "-r",
                &revset,
                "--no-graph",
                "-T",
                r#"self.description().first_line() ++ "\n""#,
            ],
        )?;
        Ok(output
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect())
    }

    fn push_to_target(&self, target: &str, path: &Path) -> anyhow::Result<()> {
        run_jj_command(path, &["git", "push", "--bookmark", target])?;
        Ok(())
    }

    fn advance_and_push(
        &self,
        target: &str,
        path: &Path,
        _display: PushDisplay<'_>,
    ) -> anyhow::Result<PushResult> {
        // Guard: target must be an ancestor of the feature tip.
        // Prevents moving the bookmark sideways or backward (which would lose commits).
        if !self.is_rebased_onto(target, path)? {
            anyhow::bail!(
                "Cannot push: feature is not ahead of {target}. Rebase first with `wt step rebase`."
            );
        }

        let feature_tip = self.feature_tip(path)?;

        // Count commits ahead of target
        let revset = format!("{target}..{feature_tip}");
        let count_output = run_jj_command(
            path,
            &["log", "-r", &revset, "--no-graph", "-T", r#""x\n""#],
        )?;
        let commit_count = count_output.lines().filter(|l| !l.is_empty()).count();

        if commit_count == 0 {
            return Ok(PushResult {
                commit_count: 0,
                stats_summary: Vec::new(),
            });
        }

        // Move bookmark to feature tip (local only — no remote push)
        run_jj_command(path, &["bookmark", "set", target, "-r", &feature_tip])?;

        Ok(PushResult {
            commit_count,
            stats_summary: Vec::new(),
        })
    }

    fn feature_head(&self, path: &Path) -> anyhow::Result<String> {
        self.feature_tip(path)
    }

    fn diff_for_prompt(
        &self,
        base: &str,
        head: &str,
        path: &Path,
    ) -> anyhow::Result<(String, String)> {
        let diff = run_jj_command(path, &["diff", "--from", base, "--to", head])?;
        let stat = run_jj_command(path, &["diff", "--stat", "--from", base, "--to", head])?;
        Ok((diff, stat))
    }

    fn recent_subjects(&self, start_ref: Option<&str>, count: usize) -> Option<Vec<String>> {
        let count_str = count.to_string();
        // ..@- = ancestors of parent (skip empty working-copy commit)
        // ..{ref} = ancestors of the given ref (e.g., trunk bookmark)
        let rev = match start_ref {
            Some(r) => format!("..{r}"),
            None => "..@-".to_string(),
        };
        let output = self
            .run_command(&[
                "log",
                "--no-graph",
                "-r",
                &rev,
                "--limit",
                &count_str,
                "-T",
                r#"description.first_line() ++ "\n""#,
            ])
            .ok()?;

        let subjects: Vec<String> = output
            .lines()
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect();

        if subjects.is_empty() {
            None
        } else {
            Some(subjects)
        }
    }

    fn squash_commits(
        &self,
        target: &str,
        message: &str,
        path: &Path,
    ) -> anyhow::Result<SquashOutcome> {
        let feature_tip = self.feature_tip(path)?;
        let from_revset = format!("{target}..{feature_tip}");

        // Create empty commit on target, squash feature into it
        run_jj_command(path, &["new", target])?;
        run_jj_command(
            path,
            &[
                "squash",
                "--from",
                &from_revset,
                "--into",
                "@",
                "-m",
                message,
            ],
        )?;

        // Update bookmark
        run_jj_command(path, &["bookmark", "set", target, "-r", "@"])?;

        // Return the change ID of the squashed commit
        let output = run_jj_command(
            path,
            &[
                "log",
                "-r",
                "@",
                "--no-graph",
                "-T",
                r#"self.change_id().short(12)"#,
            ],
        )?;

        Ok(SquashOutcome::Squashed(output.trim().to_string()))
    }

    fn committable_diff_for_prompt(&self, path: &Path) -> anyhow::Result<(String, String)> {
        let diff = run_jj_command(path, &["diff", "-r", "@"])?;
        let stat = run_jj_command(path, &["diff", "-r", "@", "--stat"])?;
        Ok((diff, stat))
    }

    fn list_ignored_entries(&self, path: &Path) -> anyhow::Result<Vec<(PathBuf, bool)>> {
        // jj repos have a git backend — find the git dir
        let git_dir = self.root.join(".git");
        if !git_dir.exists() {
            anyhow::bail!(
                "No git backend found at {}; copy-ignored requires a git backend",
                git_dir.display()
            );
        }

        let output = crate::shell_exec::Cmd::new("git")
            .args([
                &format!("--git-dir={}", git_dir.display()),
                &format!("--work-tree={}", path.display()),
                "ls-files",
                "--ignored",
                "--exclude-standard",
                "-o",
                "--directory",
            ])
            .current_dir(path)
            .run()
            .context("Failed to run git ls-files in jj workspace")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git ls-files failed: {}", stderr.trim());
        }

        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| {
                // Filter out .jj/ entries (jj internal directory, gitignored in default workspace)
                let trimmed = line.trim_end_matches('/');
                trimmed != ".jj" && !trimmed.starts_with(".jj/")
            })
            .map(|line| {
                let is_dir = line.ends_with('/');
                let entry_path = path.join(line.trim_end_matches('/'));
                (entry_path, is_dir)
            })
            .collect())
    }

    fn has_staging_area(&self) -> bool {
        false
    }

    fn load_project_config(&self) -> anyhow::Result<Option<crate::config::ProjectConfig>> {
        crate::config::ProjectConfig::load_from_root(&self.root).map_err(|e| anyhow::anyhow!("{e}"))
    }

    fn wt_logs_dir(&self) -> PathBuf {
        self.root.join(".jj").join("wt-logs")
    }

    fn switch_previous(&self) -> Option<String> {
        // Best-effort: read from jj repo config
        self.run_command(&["config", "get", "worktrunk.history"])
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn set_switch_previous(&self, name: Option<&str>) -> anyhow::Result<()> {
        match name {
            Some(name) => {
                self.run_command(&["config", "set", "--repo", "worktrunk.history", name])?;
            }
            None => {
                // Best-effort unset — jj config unset may not exist in older versions
                let _ = self.run_command(&["config", "unset", "--repo", "worktrunk.history"]);
            }
        }
        Ok(())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_diff_stat_summary_with_changes() {
        let output = "file.txt    | 3 ++-\nnew.txt     | 1 +\n2 files changed, 3 insertions(+), 1 deletion(-)";
        let diff = parse_diff_stat_summary(output);
        assert_eq!(diff.added, 3);
        assert_eq!(diff.deleted, 1);
    }

    #[test]
    fn test_parse_diff_stat_summary_no_changes() {
        let output = "0 files changed, 0 insertions(+), 0 deletions(-)";
        let diff = parse_diff_stat_summary(output);
        assert_eq!(diff.added, 0);
        assert_eq!(diff.deleted, 0);
    }

    #[test]
    fn test_parse_diff_stat_summary_empty() {
        let diff = parse_diff_stat_summary("");
        assert_eq!(diff.added, 0);
        assert_eq!(diff.deleted, 0);
    }

    #[test]
    fn test_parse_diff_stat_summary_insertions_only() {
        let output = "1 file changed, 5 insertions(+)";
        let diff = parse_diff_stat_summary(output);
        assert_eq!(diff.added, 5);
        assert_eq!(diff.deleted, 0);
    }

    #[test]
    fn test_parse_diff_stat_summary_deletions_only() {
        let output = "1 file changed, 3 deletions(-)";
        let diff = parse_diff_stat_summary(output);
        assert_eq!(diff.added, 0);
        assert_eq!(diff.deleted, 3);
    }
}
