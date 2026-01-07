use std::collections::HashMap;
use std::path::Path;
use worktrunk::HookType;
use worktrunk::config::{Command, CommandConfig, WorktrunkConfig, expand_template};
use worktrunk::git::Repository;
use worktrunk::path::to_posix_path;

#[derive(Debug)]
pub struct PreparedCommand {
    pub name: Option<String>,
    pub expanded: String,
    pub context_json: String,
}

#[derive(Clone, Copy, Debug)]
pub struct CommandContext<'a> {
    pub repo: &'a Repository,
    pub config: &'a WorktrunkConfig,
    /// Current branch name, if on a branch (None in detached HEAD state).
    pub branch: Option<&'a str>,
    pub worktree_path: &'a Path,
    pub repo_root: &'a Path,
    pub yes: bool,
}

impl<'a> CommandContext<'a> {
    pub fn new(
        repo: &'a Repository,
        config: &'a WorktrunkConfig,
        branch: Option<&'a str>,
        worktree_path: &'a Path,
        repo_root: &'a Path,
        yes: bool,
    ) -> Self {
        Self {
            repo,
            config,
            branch,
            worktree_path,
            repo_root,
            yes,
        }
    }

    /// Get branch name, using "HEAD" as fallback for detached HEAD state.
    pub fn branch_or_head(&self) -> &str {
        self.branch.unwrap_or("HEAD")
    }
}

/// Build hook context as a HashMap for JSON serialization and template expansion.
///
/// The resulting HashMap is passed to hook commands as JSON on stdin,
/// and used directly for template variable expansion.
pub fn build_hook_context(
    ctx: &CommandContext<'_>,
    extra_vars: &[(&str, &str)],
) -> HashMap<String, String> {
    let repo_root = ctx.repo_root;
    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    // Convert paths to POSIX format for Git Bash compatibility on Windows.
    // This avoids shell escaping of `:` and `\` characters in Windows paths.
    let worktree = to_posix_path(&ctx.worktree_path.to_string_lossy());
    let worktree_name = ctx
        .worktree_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    let repo_path = to_posix_path(&repo_root.to_string_lossy());

    let mut map = HashMap::new();
    map.insert("repo".into(), repo_name.into());
    map.insert("branch".into(), ctx.branch_or_head().into());
    map.insert("worktree_name".into(), worktree_name.into());

    // Canonical path variables
    map.insert("repo_path".into(), repo_path.clone());
    map.insert("worktree_path".into(), worktree.clone());

    // Deprecated aliases (kept for backward compatibility)
    map.insert("main_worktree".into(), repo_name.into());
    map.insert("repo_root".into(), repo_path);
    map.insert("worktree".into(), worktree);

    // Default branch and main worktree path
    if let Ok(default_branch) = ctx.repo.default_branch() {
        map.insert("default_branch".into(), default_branch.clone());

        // main_worktree_path: path to the worktree on the default branch
        if let Ok(Some(path)) = ctx.repo.worktree_for_branch(&default_branch) {
            map.insert(
                "main_worktree_path".into(),
                to_posix_path(&path.to_string_lossy()),
            );
        }
    }

    if let Ok(commit) = ctx.repo.run_command(&["rev-parse", "HEAD"]) {
        let commit = commit.trim();
        map.insert("commit".into(), commit.into());
        if commit.len() >= 7 {
            map.insert("short_commit".into(), commit[..7].into());
        }
    }

    if let Ok(remote) = ctx.repo.primary_remote() {
        map.insert("remote".into(), remote.to_string());
        // Add remote URL for conditional hook execution (e.g., GitLab vs GitHub)
        if let Some(url) = ctx.repo.remote_url(remote) {
            map.insert("remote_url".into(), url);
        }
        if let Some(branch) = ctx.branch
            && let Ok(Some(upstream)) = ctx.repo.upstream_branch(branch)
        {
            map.insert("upstream".into(), upstream);
        }
    }

    // Add extra vars (e.g., target branch for merge)
    for (k, v) in extra_vars {
        map.insert((*k).into(), (*v).into());
    }

    map
}

/// Expand commands from a CommandConfig without approval
///
/// This is the canonical command expansion implementation.
/// Returns cloned commands with their expanded forms filled in, each with per-command JSON context.
fn expand_commands(
    commands: &[Command],
    ctx: &CommandContext<'_>,
    extra_vars: &[(&str, &str)],
    hook_type: HookType,
) -> anyhow::Result<Vec<(Command, String)>> {
    if commands.is_empty() {
        return Ok(Vec::new());
    }

    let base_context = build_hook_context(ctx, extra_vars);

    // Convert to &str references for expand_template
    let vars: HashMap<&str, &str> = base_context
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    let mut result = Vec::new();

    for cmd in commands {
        let expanded_str = expand_template(&cmd.template, &vars, true).map_err(|e| {
            anyhow::anyhow!(
                "Failed to expand command template '{}': {}",
                cmd.template,
                e
            )
        })?;

        // Build per-command JSON with hook_type and hook_name
        let mut cmd_context = base_context.clone();
        cmd_context.insert("hook_type".into(), hook_type.to_string());
        if let Some(ref name) = cmd.name {
            cmd_context.insert("hook_name".into(), name.clone());
        }
        let context_json = serde_json::to_string(&cmd_context)
            .expect("HashMap<String, String> serialization should never fail");

        result.push((
            Command::with_expansion(cmd.name.clone(), cmd.template.clone(), expanded_str),
            context_json,
        ));
    }

    Ok(result)
}

/// Prepare commands for execution.
///
/// Expands command templates with context variables and returns prepared
/// commands ready for execution, each with JSON context for stdin.
///
/// Note: Approval logic (for project commands) is handled at the call site,
/// not here. User commands don't require approval since users implicitly
/// approve them by adding them to their config.
pub fn prepare_commands(
    command_config: &CommandConfig,
    ctx: &CommandContext<'_>,
    extra_vars: &[(&str, &str)],
    hook_type: HookType,
) -> anyhow::Result<Vec<PreparedCommand>> {
    let commands = command_config.commands();
    if commands.is_empty() {
        return Ok(Vec::new());
    }

    let expanded_with_json = expand_commands(commands, ctx, extra_vars, hook_type)?;

    Ok(expanded_with_json
        .into_iter()
        .map(|(cmd, context_json)| PreparedCommand {
            name: cmd.name,
            expanded: cmd.expanded,
            context_json,
        })
        .collect())
}
