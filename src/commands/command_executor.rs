use std::collections::HashMap;
use std::path::Path;
use worktrunk::config::{CommandConfig, WorktrunkConfig, expand_template};
use worktrunk::git::{GitError, Repository};

use super::command_approval::approve_command_batch;

#[derive(Debug)]
pub struct PreparedCommand {
    pub name: Option<String>,
    pub expanded: String,
}

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
/// Returns expanded commands as (name, command) tuples.
fn expand_commands(
    command_config: &CommandConfig,
    ctx: &CommandContext<'_>,
    extra_vars: &[(&str, &str)],
) -> Result<Vec<(Option<String>, String)>, GitError> {
    let commands = command_config.commands();
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
        repo_root.to_str().unwrap_or("").to_string(),
    );
    for &(key, value) in extra_vars {
        base_extras.insert(key.to_string(), value.to_string());
    }

    let mut expanded = Vec::new();

    for (name, command) in commands {
        let extras_owned = base_extras.clone();
        let extras_ref: HashMap<&str, &str> = extras_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let expanded_cmd = expand_template(command, repo_name, ctx.branch, &extras_ref);
        expanded.push((name.clone(), expanded_cmd));
    }

    Ok(expanded)
}

pub fn prepare_project_commands<F>(
    command_config: &CommandConfig,
    ctx: &CommandContext<'_>,
    auto_trust: bool,
    extra_vars: &[(&str, &str)],
    approval_context: &str,
    mut on_skip: F,
) -> Result<Vec<PreparedCommand>, GitError>
where
    F: FnMut(Option<&str>, &str),
{
    let commands = command_config.commands();
    if commands.is_empty() {
        return Ok(Vec::new());
    }

    let project_id = ctx.repo.project_identifier()?;

    // Approve using original command strings (before expansion)
    if !auto_trust
        && !approve_command_batch(
            commands,
            &project_id,
            ctx.config,
            ctx.force,
            approval_context,
        )?
    {
        for (name, command) in commands {
            on_skip(name.as_deref(), command);
        }
        return Err(GitError::CommandNotApproved);
    }

    // Expand commands after approval
    let expanded = expand_commands(command_config, ctx, extra_vars)?;

    Ok(expanded
        .into_iter()
        .map(|(name, expanded)| PreparedCommand { name, expanded })
        .collect())
}
