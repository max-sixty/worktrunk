use worktrunk::HookType;
use worktrunk::config::{CommandPhase, CommitGenerationConfig, ProjectConfig};
use worktrunk::git::{GitError, GitResultExt, Repository};
use worktrunk::styling::{AnstyleStyle, CYAN, GREEN, HINT, WARNING, format_with_gutter};

use super::command_executor::CommandContext;
use super::hooks::{HookFailureStrategy, HookPipeline};
use super::project_config::ProjectConfigRepoExt;

/// Options for committing current changes.
pub struct CommitOptions<'a> {
    pub ctx: &'a CommandContext<'a>,
    pub target_branch: Option<&'a str>,
    pub no_verify: bool,
    pub tracked_only: bool,
    pub auto_trust: bool,
    pub warn_about_untracked: bool,
    pub show_no_squash_note: bool,
}

impl<'a> CommitOptions<'a> {
    /// Convenience constructor for the common case where untracked files should trigger a warning.
    pub fn new(ctx: &'a CommandContext<'a>) -> Self {
        Self {
            ctx,
            target_branch: None,
            no_verify: false,
            tracked_only: false,
            auto_trust: false,
            warn_about_untracked: true,
            show_no_squash_note: false,
        }
    }
}

/// Format a commit message with the first line in bold, ready for gutter display.
pub fn format_commit_message_for_display(message: &str) -> String {
    let bold = AnstyleStyle::new().bold();
    let lines: Vec<&str> = message.lines().collect();

    if lines.is_empty() {
        return String::new();
    }

    let mut result = format!("{bold}{}{bold:#}", lines[0]);

    if lines.len() > 1 {
        for line in &lines[1..] {
            result.push('\n');
            result.push_str(line);
        }
    }

    result
}

/// Show hint if no LLM command is configured.
pub fn show_llm_config_hint_if_needed(
    commit_generation_config: &CommitGenerationConfig,
) -> Result<(), GitError> {
    if !commit_generation_config.is_configured() {
        crate::output::hint(format!(
            "{HINT}Using fallback commit message. Run 'wt config help' to configure LLM-generated messages{HINT:#}"
        ))?;
    }
    Ok(())
}

fn get_untracked_files(status_output: &str) -> Vec<String> {
    status_output
        .lines()
        .filter_map(|line| line.strip_prefix("?? "))
        .map(|filename| filename.to_string())
        .collect()
}

/// Warn about untracked files being auto-staged.
pub fn warn_untracked_auto_stage(repo: &Repository) -> Result<(), GitError> {
    let status = repo
        .run_command(&["status", "--porcelain"])
        .git_context("Failed to get status")?;
    let untracked = get_untracked_files(&status);

    if untracked.is_empty() {
        return Ok(());
    }

    let count = untracked.len();
    let file_word = if count == 1 { "file" } else { "files" };
    crate::output::warning(format!(
        "{WARNING}Auto-staging {count} untracked {file_word}:{WARNING:#}"
    ))?;

    let joined_files = untracked.join("\n");
    crate::output::gutter(format_with_gutter(&joined_files, "", None))?;

    Ok(())
}

/// Commit already-staged changes with LLM-generated or fallback message.
pub fn commit_staged_changes(
    commit_generation_config: &CommitGenerationConfig,
    show_no_squash_note: bool,
) -> Result<(), GitError> {
    let repo = Repository::current();

    let stats_parts = repo.diff_stats_summary(&["diff", "--staged", "--shortstat"]);

    let action = if commit_generation_config.is_configured() {
        "Generating commit message and committing..."
    } else {
        "Committing with default message..."
    };

    let mut parts = vec![];
    if !stats_parts.is_empty() {
        parts.extend(stats_parts);
    }
    if show_no_squash_note {
        parts.push("no squashing needed".to_string());
    }

    let full_progress_msg = if parts.is_empty() {
        format!("{CYAN}{action}{CYAN:#}")
    } else {
        format!("{CYAN}{action}{CYAN:#} ({})", parts.join(", "))
    };

    crate::output::progress(full_progress_msg)?;

    show_llm_config_hint_if_needed(commit_generation_config)?;
    let commit_message = crate::llm::generate_commit_message(commit_generation_config)?;

    let formatted_message = format_commit_message_for_display(&commit_message);
    crate::output::gutter(format_with_gutter(&formatted_message, "", None))?;

    repo.run_command(&["commit", "-m", &commit_message])
        .git_context("Failed to commit")?;

    let commit_hash = repo
        .run_command(&["rev-parse", "--short", "HEAD"])?
        .trim()
        .to_string();

    let green_dim = GREEN.dimmed();
    crate::output::success(format!(
        "{GREEN}Committed changes @ {green_dim}{commit_hash}{green_dim:#}{GREEN:#}"
    ))?;

    Ok(())
}

/// Commit uncommitted changes with the shared commit pipeline.
pub fn commit_changes(options: CommitOptions<'_>) -> Result<(), GitError> {
    if !options.no_verify
        && let Some(project_config) = options.ctx.repo.load_project_config()?
    {
        run_pre_commit_commands(
            &project_config,
            options.ctx,
            options.target_branch,
            options.auto_trust,
        )?;
    }

    if options.warn_about_untracked && !options.tracked_only {
        warn_untracked_auto_stage(options.ctx.repo)?;
    }

    if options.tracked_only {
        options
            .ctx
            .repo
            .run_command(&["add", "-u"])
            .git_context("Failed to stage tracked changes")?;
    } else {
        options
            .ctx
            .repo
            .run_command(&["add", "-A"])
            .git_context("Failed to stage changes")?;
    }

    commit_staged_changes(
        &options.ctx.config.commit_generation,
        options.show_no_squash_note,
    )
}

/// Run pre-commit commands sequentially (blocking, fail-fast).
pub fn run_pre_commit_commands(
    project_config: &ProjectConfig,
    ctx: &CommandContext,
    target_branch: Option<&str>,
    auto_trust: bool,
) -> Result<(), GitError> {
    let Some(pre_commit_config) = &project_config.pre_commit_command else {
        return Ok(());
    };

    let pipeline = HookPipeline::new(*ctx);

    let extra_vars: Vec<(&str, &str)> = target_branch
        .into_iter()
        .map(|target| ("target", target))
        .collect();

    pipeline.run_sequential(
        pre_commit_config,
        CommandPhase::PreCommit,
        auto_trust,
        &extra_vars,
        "pre-commit",
        HookFailureStrategy::FailFast {
            hook_type: HookType::PreCommit,
        },
    )
}
