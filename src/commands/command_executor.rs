use std::collections::HashMap;
use std::path::Path;
use worktrunk::HookType;
use worktrunk::config::{
    Command, CommandConfig, WorktrunkConfig, expand_template, sanitize_branch_name,
};
use worktrunk::git::Repository;

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
    pub force: bool,
}

impl<'a> CommandContext<'a> {
    pub fn new(
        repo: &'a Repository,
        config: &'a WorktrunkConfig,
        branch: Option<&'a str>,
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

    let worktree = ctx.worktree_path.to_string_lossy();
    let worktree_name = ctx
        .worktree_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    let mut map = HashMap::new();
    map.insert("repo".into(), repo_name.into());
    map.insert("branch".into(), sanitize_branch_name(ctx.branch_or_head()));
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
        map.insert("remote".into(), remote.clone());
        // Add remote URL for conditional hook execution (e.g., GitLab vs GitHub)
        if let Ok(url) = ctx.repo.run_command(&["remote", "get-url", &remote]) {
            map.insert("remote_url".into(), url.trim().into());
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
        let expanded_str =
            expand_template(&cmd.template, repo_name, ctx.branch_or_head(), &extras_ref).map_err(
                |e| {
                    anyhow::anyhow!(
                        "Failed to expand command template '{}': {}",
                        cmd.template,
                        e
                    )
                },
            )?;

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

/// Prepare project commands for execution
///
/// This function:
/// 1. Expands command templates with context variables
/// 2. Returns prepared commands ready for execution, each with JSON context for stdin
///
/// Note: Approval is handled at the gate (command entry point), not here.
pub fn prepare_project_commands(
    command_config: &CommandConfig,
    ctx: &CommandContext<'_>,
    extra_vars: &[(&str, &str)],
    hook_type: HookType,
) -> anyhow::Result<Vec<PreparedCommand>> {
    let commands = command_config.commands_with_phase(hook_type);
    if commands.is_empty() {
        return Ok(Vec::new());
    }

    let expanded_with_json = expand_commands(&commands, ctx, extra_vars)?;

    Ok(expanded_with_json
        .into_iter()
        .map(|(cmd, context_json)| PreparedCommand {
            name: cmd.name,
            expanded: cmd.expanded,
            context_json,
        })
        .collect())
}

/// Prepare user hooks for execution without approval
///
/// Unlike project commands, user hooks don't require approval because they're
/// defined in the user's own config file. The user implicitly approves them
/// by adding them to their config.
///
/// This function:
/// 1. Expands command templates with context variables
/// 2. Returns prepared commands ready for execution, each with JSON context for stdin
pub fn prepare_user_commands(
    command_config: &CommandConfig,
    ctx: &CommandContext<'_>,
    extra_vars: &[(&str, &str)],
    hook_type: HookType,
) -> anyhow::Result<Vec<PreparedCommand>> {
    let commands = command_config.commands_with_phase(hook_type);
    if commands.is_empty() {
        return Ok(Vec::new());
    }

    // Expand commands (no approval needed for user hooks)
    let expanded_with_json = expand_commands(&commands, ctx, extra_vars)?;

    Ok(expanded_with_json
        .into_iter()
        .map(|(cmd, context_json)| PreparedCommand {
            name: cmd.name,
            expanded: cmd.expanded,
            context_json,
        })
        .collect())
}
