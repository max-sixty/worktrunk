//! Jujutsu (jj) implementation of the [`Workspace`] trait.
//!
//! Implements workspace operations by shelling out to `jj` commands
//! and parsing their output.

use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::git::{IntegrationReason, LineDiff};
use crate::shell_exec::Cmd;

use super::{VcsKind, Workspace, WorkspaceItem};

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

    fn default_branch_name(&self) -> anyhow::Result<Option<String>> {
        // jj uses trunk() revset instead of a named default branch
        Ok(None)
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

    fn has_staging_area(&self) -> bool {
        false
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
