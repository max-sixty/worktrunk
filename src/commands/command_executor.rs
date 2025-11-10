use std::collections::HashMap;
use std::path::Path;
use worktrunk::config::{Command, CommandConfig, CommandPhase, WorktrunkConfig, expand_template};
use worktrunk::git::{GitError, Repository};

use super::command_approval::approve_command_batch;

#[derive(Debug)]
pub struct PreparedCommand {
    pub name: Option<String>,
    pub expanded: String,
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

/// Expand commands from a CommandConfig without approval
///
/// This is the canonical command expansion implementation.
/// Returns cloned commands with their expanded forms filled in.
fn expand_commands(
    commands: &[Command],
    ctx: &CommandContext<'_>,
    extra_vars: &[(&str, &str)],
) -> Result<Vec<Command>, GitError> {
    if commands.is_empty() {
        return Ok(Vec::new());
    }

    let repo_root = ctx.repo_root;
    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    let mut base_extras = HashMap::new();
    base_extras.insert(
        "worktree".to_string(),
        ctx.worktree_path.to_string_lossy().to_string(),
    );
    base_extras.insert(
        "repo_root".to_string(),
        repo_root.to_string_lossy().to_string(),
    );
    for &(key, value) in extra_vars {
        base_extras.insert(key.to_string(), value.to_string());
    }

    let mut expanded_commands = Vec::new();

    for cmd in commands {
        let extras_owned = base_extras.clone();
        let extras_ref: HashMap<&str, &str> = extras_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let expanded_str = expand_template(&cmd.template, repo_name, ctx.branch, &extras_ref)
            .map_err(|e| {
                GitError::message(format!(
                    "Failed to expand command template '{}': {}",
                    cmd.template, e
                ))
            })?;
        expanded_commands.push(Command::with_expansion(
            cmd.name.clone(),
            cmd.template.clone(),
            expanded_str,
            cmd.phase,
        ));
    }

    Ok(expanded_commands)
}

/// Prepare project commands for execution with approval
///
/// This function:
/// 1. Expands command templates with context variables
/// 2. Requests user approval for unapproved commands (unless auto_trust or force)
/// 3. Returns prepared commands ready for execution
///
/// Returns `Err(GitError::CommandNotApproved)` if the user declines approval.
pub fn prepare_project_commands(
    command_config: &CommandConfig,
    ctx: &CommandContext<'_>,
    auto_trust: bool,
    extra_vars: &[(&str, &str)],
    phase: CommandPhase,
) -> Result<Vec<PreparedCommand>, GitError> {
    let commands = command_config.commands_with_phase(phase);
    if commands.is_empty() {
        return Ok(Vec::new());
    }

    let project_id = ctx.repo.project_identifier()?;

    // Expand commands before approval for transparency
    let expanded_commands = expand_commands(&commands, ctx, extra_vars)?;

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
        return Err(GitError::CommandNotApproved);
    }

    Ok(expanded_commands
        .into_iter()
        .map(|cmd| PreparedCommand {
            name: cmd.name,
            expanded: cmd.expanded,
        })
        .collect())
}
