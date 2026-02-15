use std::collections::HashMap;
use std::path::Path;
use worktrunk::HookType;
use worktrunk::config::{Command, CommandConfig, UserConfig, expand_template};
use worktrunk::git::Repository;
use worktrunk::path::to_posix_path;
use worktrunk::workspace::{Workspace, build_worktree_map};

use super::hook_filter::HookSource;

#[derive(Debug)]
pub struct PreparedCommand {
    pub name: Option<String>,
    pub expanded: String,
    pub context_json: String,
}

#[derive(Clone, Copy)]
pub struct CommandContext<'a> {
    pub workspace: &'a dyn Workspace,
    pub config: &'a UserConfig,
    /// Current branch name, if on a branch (None in detached HEAD state).
    pub branch: Option<&'a str>,
    pub worktree_path: &'a Path,
    pub yes: bool,
}

impl std::fmt::Debug for CommandContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandContext")
            .field("workspace_kind", &self.workspace.kind())
            .field("branch", &self.branch)
            .field("worktree_path", &self.worktree_path)
            .field("yes", &self.yes)
            .finish()
    }
}

impl<'a> CommandContext<'a> {
    pub fn new(
        workspace: &'a dyn Workspace,
        config: &'a UserConfig,
        branch: Option<&'a str>,
        worktree_path: &'a Path,
        yes: bool,
    ) -> Self {
        Self {
            workspace,
            config,
            branch,
            worktree_path,
            yes,
        }
    }

    /// Downcast to git Repository. Returns None for jj workspaces.
    pub fn repo(&self) -> Option<&Repository> {
        self.workspace.as_any().downcast_ref::<Repository>()
    }

    /// Get branch name, using "HEAD" as fallback for detached HEAD state.
    pub fn branch_or_head(&self) -> &str {
        self.branch.unwrap_or("HEAD")
    }

    /// Get the project identifier for per-project config lookup.
    ///
    /// Uses the remote URL if available, otherwise the canonical repository path.
    /// Returns None only if the path is not valid UTF-8.
    pub fn project_id(&self) -> Option<String> {
        self.workspace.project_identifier().ok()
    }

    /// Get the commit generation config, merging project-specific settings.
    pub fn commit_generation(&self) -> worktrunk::config::CommitGenerationConfig {
        self.config.commit_generation(self.project_id().as_deref())
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
    let repo_root = ctx.workspace.root_path().unwrap_or_default();
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

    // Default branch
    if let Some(default_branch) = ctx.workspace.default_branch_name() {
        map.insert("default_branch".into(), default_branch);
    }

    // Primary worktree path (where established files live)
    if let Ok(Some(path)) = ctx.workspace.default_workspace_path() {
        let path_str = to_posix_path(&path.to_string_lossy());
        map.insert("primary_worktree_path".into(), path_str.clone());
        // Deprecated alias
        map.insert("main_worktree_path".into(), path_str);
    }

    // Git-specific context (commit SHA, remote, upstream)
    if let Some(repo) = ctx.repo() {
        if let Ok(commit) = repo.run_command(&["rev-parse", "HEAD"]) {
            let commit = commit.trim();
            map.insert("commit".into(), commit.into());
            if commit.len() >= 7 {
                map.insert("short_commit".into(), commit[..7].into());
            }
        }

        if let Ok(remote) = repo.primary_remote() {
            map.insert("remote".into(), remote.to_string());
            if let Some(url) = repo.remote_url(&remote) {
                map.insert("remote_url".into(), url);
            }
            if let Some(branch) = ctx.branch
                && let Ok(Some(upstream)) = repo.branch(branch).upstream()
            {
                map.insert("upstream".into(), upstream);
            }
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
    source: HookSource,
) -> anyhow::Result<Vec<(Command, String)>> {
    if commands.is_empty() {
        return Ok(Vec::new());
    }

    let base_context = build_hook_context(ctx, extra_vars);
    let worktree_map = build_worktree_map(ctx.workspace);

    // Convert to &str references for expand_template
    let vars: HashMap<&str, &str> = base_context
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();

    let mut result = Vec::new();

    for cmd in commands {
        let template_name = match &cmd.name {
            Some(name) => format!("{}:{}", source, name),
            None => format!("{} {} hook", source, hook_type),
        };
        let expanded_str =
            expand_template(&cmd.template, &vars, true, &worktree_map, &template_name)?;

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
    source: HookSource,
) -> anyhow::Result<Vec<PreparedCommand>> {
    let commands = command_config.commands();
    if commands.is_empty() {
        return Ok(Vec::new());
    }

    let expanded_with_json = expand_commands(commands, ctx, extra_vars, hook_type, source)?;

    Ok(expanded_with_json
        .into_iter()
        .map(|(cmd, context_json)| PreparedCommand {
            name: cmd.name,
            expanded: cmd.expanded,
            context_json,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use worktrunk::config::UserConfig;
    use worktrunk::git::Repository;

    /// Helper to init a git repo and return (temp_dir, repo_path).
    fn init_test_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let repo_path = temp.path().join("repo");
        std::fs::create_dir(&repo_path).unwrap();
        let out = std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(&repo_path)
            .output()
            .unwrap();
        assert!(out.status.success());
        let out = std::process::Command::new("git")
            .args(["commit", "--allow-empty", "-m", "init"])
            .current_dir(&repo_path)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.com")
            .output()
            .unwrap();
        assert!(out.status.success());
        (temp, repo_path)
    }

    #[test]
    fn test_command_context_debug_format() {
        let (_temp, repo_path) = init_test_repo();
        let repo = Repository::at(&repo_path).unwrap();
        let config = UserConfig::default();
        let ctx = CommandContext::new(&repo, &config, Some("main"), &repo_path, false);
        let debug = format!("{ctx:?}");
        assert!(debug.contains("CommandContext"));
        assert!(debug.contains("Git"));
    }

    #[test]
    fn test_command_context_repo_downcast() {
        let (_temp, repo_path) = init_test_repo();
        let repo = Repository::at(&repo_path).unwrap();
        let config = UserConfig::default();
        let ctx = CommandContext::new(&repo, &config, Some("main"), &repo_path, false);
        // repo() should succeed for git repositories
        assert!(ctx.repo().is_some());
    }

    #[test]
    fn test_command_context_branch_or_head() {
        let (_temp, repo_path) = init_test_repo();
        let repo = Repository::at(&repo_path).unwrap();
        let config = UserConfig::default();

        // With a branch set
        let ctx = CommandContext::new(&repo, &config, Some("main"), &repo_path, false);
        assert_eq!(ctx.branch_or_head(), "main");

        // Without a branch (detached HEAD)
        let ctx = CommandContext::new(&repo, &config, None, &repo_path, false);
        assert_eq!(ctx.branch_or_head(), "HEAD");
    }

    #[test]
    fn test_command_context_project_id() {
        let (_temp, repo_path) = init_test_repo();
        let repo = Repository::at(&repo_path).unwrap();
        let config = UserConfig::default();
        let ctx = CommandContext::new(&repo, &config, Some("main"), &repo_path, false);
        // project_id should return Some (path-based, since no remote)
        assert!(ctx.project_id().is_some());
    }

    #[test]
    fn test_command_context_commit_generation() {
        let (_temp, repo_path) = init_test_repo();
        let repo = Repository::at(&repo_path).unwrap();
        let config = UserConfig::default();
        let ctx = CommandContext::new(&repo, &config, Some("main"), &repo_path, false);
        // commit_generation returns default config (no command set)
        let cg = ctx.commit_generation();
        assert!(cg.command.is_none());
    }

    #[test]
    fn test_build_hook_context() {
        let (_temp, repo_path) = init_test_repo();
        let repo = Repository::at(&repo_path).unwrap();
        let config = UserConfig::default();
        let ctx = CommandContext::new(&repo, &config, Some("main"), &repo_path, false);
        let context = build_hook_context(&ctx, &[("extra_key", "extra_val")]);
        assert_eq!(context.get("branch").map(|s| s.as_str()), Some("main"));
        assert_eq!(context.get("repo").map(|s| s.as_str()), Some("repo"));
        assert!(context.contains_key("worktree_path"));
        assert!(context.contains_key("repo_path"));
        assert_eq!(
            context.get("extra_key").map(|s| s.as_str()),
            Some("extra_val")
        );
    }

    #[test]
    fn test_build_hook_context_detached_head() {
        let (_temp, repo_path) = init_test_repo();
        let repo = Repository::at(&repo_path).unwrap();
        let config = UserConfig::default();
        // Detached HEAD: branch is None, branch_or_head returns "HEAD"
        let ctx = CommandContext::new(&repo, &config, None, &repo_path, false);
        let context = build_hook_context(&ctx, &[]);
        assert_eq!(context.get("branch").map(|s| s.as_str()), Some("HEAD"));
    }

    #[test]
    fn test_build_hook_context_includes_git_specifics() {
        let (_temp, repo_path) = init_test_repo();
        let repo = Repository::at(&repo_path).unwrap();
        let config = UserConfig::default();
        let ctx = CommandContext::new(&repo, &config, Some("main"), &repo_path, false);
        let context = build_hook_context(&ctx, &[]);
        // Git-specific: commit and short_commit should be present
        assert!(context.contains_key("commit"));
        assert!(context.contains_key("short_commit"));
        // Deprecated aliases should still be present
        assert!(context.contains_key("main_worktree"));
        assert!(context.contains_key("repo_root"));
        assert!(context.contains_key("worktree"));
    }

    /// Deserialize a CommandConfig from a TOML string command.
    fn make_command_config(toml_value: &str) -> worktrunk::config::CommandConfig {
        #[derive(serde::Deserialize)]
        struct W {
            cmd: worktrunk::config::CommandConfig,
        }
        let toml_str = format!("cmd = {toml_value}");
        toml::from_str::<W>(&toml_str).unwrap().cmd
    }

    #[test]
    fn test_prepare_commands_empty_template() {
        let (_temp, repo_path) = init_test_repo();
        let repo = Repository::at(&repo_path).unwrap();
        let config = UserConfig::default();
        let ctx = CommandContext::new(&repo, &config, Some("main"), &repo_path, false);
        let cmd_config = make_command_config("\"\"");
        // Empty template still counts as one command
        let result = prepare_commands(
            &cmd_config,
            &ctx,
            &[],
            HookType::PreCommit,
            HookSource::User,
        )
        .unwrap();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_prepare_commands_single() {
        let (_temp, repo_path) = init_test_repo();
        let repo = Repository::at(&repo_path).unwrap();
        let config = UserConfig::default();
        let ctx = CommandContext::new(&repo, &config, Some("main"), &repo_path, false);
        let cmd_config = make_command_config("\"echo hello\"");
        let result = prepare_commands(
            &cmd_config,
            &ctx,
            &[],
            HookType::PreCommit,
            HookSource::User,
        )
        .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].expanded, "echo hello");
        assert!(result[0].name.is_none());
        // context_json should contain hook_type
        assert!(result[0].context_json.contains("pre-commit"));
    }

    #[test]
    fn test_prepare_commands_with_template_vars() {
        let (_temp, repo_path) = init_test_repo();
        let repo = Repository::at(&repo_path).unwrap();
        let config = UserConfig::default();
        let ctx = CommandContext::new(&repo, &config, Some("main"), &repo_path, false);
        let cmd_config = make_command_config("\"echo {{ branch }}\"");
        let result = prepare_commands(
            &cmd_config,
            &ctx,
            &[],
            HookType::PostCreate,
            HookSource::User,
        )
        .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].expanded, "echo main");
    }

    #[test]
    fn test_prepare_commands_named() {
        let (_temp, repo_path) = init_test_repo();
        let repo = Repository::at(&repo_path).unwrap();
        let config = UserConfig::default();
        let ctx = CommandContext::new(&repo, &config, Some("main"), &repo_path, false);
        let cmd_config = make_command_config("{ build = \"cargo build\", test = \"cargo test\" }");
        let result = prepare_commands(
            &cmd_config,
            &ctx,
            &[],
            HookType::PreMerge,
            HookSource::Project,
        )
        .unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name.as_deref(), Some("build"));
        assert_eq!(result[0].expanded, "cargo build");
        assert_eq!(result[1].name.as_deref(), Some("test"));
        assert_eq!(result[1].expanded, "cargo test");
        // Named commands should have hook_name in JSON context
        assert!(result[0].context_json.contains("hook_name"));
        assert!(result[0].context_json.contains("build"));
    }

    #[test]
    fn test_prepare_commands_with_extra_vars() {
        let (_temp, repo_path) = init_test_repo();
        let repo = Repository::at(&repo_path).unwrap();
        let config = UserConfig::default();
        let ctx = CommandContext::new(&repo, &config, Some("main"), &repo_path, false);
        let cmd_config = make_command_config("\"echo {{ target }}\"");
        let result = prepare_commands(
            &cmd_config,
            &ctx,
            &[("target", "develop")],
            HookType::PreMerge,
            HookSource::User,
        )
        .unwrap();
        assert_eq!(result[0].expanded, "echo develop");
    }
}
