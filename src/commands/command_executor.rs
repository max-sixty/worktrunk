use std::collections::HashMap;
use std::path::Path;
use worktrunk::config::{
    Command, CommandConfig, CommandPhase, WorktrunkConfig, expand_template, sanitize_branch_name,
};
use worktrunk::git::{Repository, WorktrunkError};

use super::command_approval::approve_command_batch;

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
    pub branch: &'a str,
    pub worktree_path: &'a Path,
    pub repo_root: &'a Path,
    pub force: bool,
}

impl<'a> CommandContext<'a> {
    pub fn new(
        repo: &'a Repository,
        config: &'a WorktrunkConfig,
        branch: &'a str,
        worktree_path: &'a Path,
        repo_root: &'a Path,
        force: bool,
    ) -> Self {
        Self {
            repo,
            config,
            branch,
            worktree_path,
            repo_root,
            force,
        }
    }
}

/// Build hook context as a HashMap for JSON serialization and template expansion.
///
/// The resulting HashMap is passed to hook commands as JSON on stdin,
/// and used directly for template variable expansion.
fn build_hook_context(
    ctx: &CommandContext<'_>,
    extra_vars: &[(&str, &str)],
) -> HashMap<String, String> {
    let repo_root = ctx.repo_root;
    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    let worktree = ctx.worktree_path.to_string_lossy();
    let worktree_name = ctx
        .worktree_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    let mut map = HashMap::new();
    map.insert("repo".into(), repo_name.into());
    map.insert("branch".into(), sanitize_branch_name(ctx.branch));
    map.insert("worktree".into(), worktree.into());
    map.insert("worktree_name".into(), worktree_name.into());
    map.insert("repo_root".into(), repo_root.to_string_lossy().into());

    if let Ok(default_branch) = ctx.repo.default_branch() {
        map.insert("default_branch".into(), default_branch);
    }

    if let Ok(commit) = ctx.repo.run_command(&["rev-parse", "HEAD"]) {
        let commit = commit.trim();
        map.insert("commit".into(), commit.into());
        if commit.len() >= 7 {
            map.insert("short_commit".into(), commit[..7].into());
        }
    }

    if let Ok(remote) = ctx.repo.primary_remote() {
        map.insert("remote".into(), remote);
        if let Ok(Some(upstream)) = ctx.repo.upstream_branch(ctx.branch) {
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
) -> anyhow::Result<Vec<(Command, String)>> {
    if commands.is_empty() {
        return Ok(Vec::new());
    }

    let base_context = build_hook_context(ctx, extra_vars);

    let repo_name = ctx
        .repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    // Convert to &str references for expand_template
    let extras_ref: HashMap<&str, &str> = base_context
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    let mut result = Vec::new();

    for cmd in commands {
        let expanded_str = expand_template(&cmd.template, repo_name, ctx.branch, &extras_ref)
            .map_err(|e| {
                anyhow::anyhow!(
                    "Failed to expand command template '{}': {}",
                    cmd.template,
                    e
                )
            })?;

        // Build per-command JSON with hook_type and hook_name
        let mut cmd_context = base_context.clone();
        cmd_context.insert("hook_type".into(), cmd.phase.to_string());
        if let Some(ref name) = cmd.name {
            cmd_context.insert("hook_name".into(), name.clone());
        }
        let context_json = serde_json::to_string(&cmd_context)
            .expect("HashMap<String, String> serialization should never fail");

        result.push((
            Command::with_expansion(
                cmd.name.clone(),
                cmd.template.clone(),
                expanded_str,
                cmd.phase,
            ),
            context_json,
        ));
    }

    Ok(result)
}

/// Prepare project commands for execution with approval
///
/// This function:
/// 1. Expands command templates with context variables
/// 2. Requests user approval for unapproved commands (unless auto_trust or force)
/// 3. Returns prepared commands ready for execution, each with JSON context for stdin
///
/// Returns `Err(WorktrunkError::CommandNotApproved)` if the user declines approval.
pub fn prepare_project_commands(
    command_config: &CommandConfig,
    ctx: &CommandContext<'_>,
    auto_trust: bool,
    extra_vars: &[(&str, &str)],
    phase: CommandPhase,
) -> anyhow::Result<Vec<PreparedCommand>> {
    let commands = command_config.commands_with_phase(phase);
    if commands.is_empty() {
        return Ok(Vec::new());
    }

    let project_id = ctx.repo.project_identifier()?;

    // Expand commands before approval for transparency
    let expanded_with_json = expand_commands(&commands, ctx, extra_vars)?;

    // Extract just the commands for approval
    let expanded_commands: Vec<_> = expanded_with_json
        .iter()
        .map(|(cmd, _)| cmd.clone())
        .collect();

    // Flush stdout before prompting on stderr to ensure correct output ordering
    // This prevents the approval prompt from appearing before previous success messages
    crate::output::flush()?;

    // Approve using expanded commands (which have both template and expanded forms)
    if !auto_trust
        && !approve_command_batch(
            &expanded_commands,
            &project_id,
            ctx.config,
            ctx.force,
            false,
        )?
    {
        return Err(WorktrunkError::CommandNotApproved.into());
    }

    Ok(expanded_with_json
        .into_iter()
        .map(|(cmd, context_json)| PreparedCommand {
            name: cmd.name,
            expanded: cmd.expanded,
            context_json,
        })
        .collect())
}
