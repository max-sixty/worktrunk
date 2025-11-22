use anyhow::Context;
use worktrunk::config::CommitGenerationConfig;
use worktrunk::git::Repository;
use worktrunk::styling::{AnstyleStyle, CYAN, GREEN, HINT, format_with_gutter};

use super::command_executor::CommandContext;
use super::hooks::HookPipeline;
use super::repository_ext::RepositoryCliExt;

// Re-export StageMode from config for use by CLI
pub use worktrunk::config::StageMode;

/// Options for committing current changes.
pub struct CommitOptions<'a> {
    pub ctx: &'a CommandContext<'a>,
    pub target_branch: Option<&'a str>,
    pub no_verify: bool,
    pub stage_mode: StageMode,
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
            stage_mode: StageMode::All,
            auto_trust: false,
            warn_about_untracked: true,
            show_no_squash_note: false,
        }
    }
}

pub(crate) struct CommitGenerator<'a> {
    config: &'a CommitGenerationConfig,
}

impl<'a> CommitGenerator<'a> {
    pub fn new(config: &'a CommitGenerationConfig) -> Self {
        Self { config }
    }

    pub fn format_message_for_display(&self, message: &str) -> String {
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

    pub fn emit_hint_if_needed(&self) -> anyhow::Result<()> {
        if !self.config.is_configured() {
            crate::output::hint(format!(
                "{HINT}Using fallback commit message. Run 'wt config --help' for LLM setup guide{HINT:#}"
            ))?;
        }
        Ok(())
    }

    pub fn commit_staged_changes(&self, show_no_squash_note: bool) -> anyhow::Result<()> {
        let repo = Repository::current();

        let stats_parts = repo.diff_stats_summary(&["diff", "--staged", "--shortstat"]);

        let action = if self.config.is_configured() {
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

        self.emit_hint_if_needed()?;
        let commit_message = crate::llm::generate_commit_message(self.config)?;

        let formatted_message = self.format_message_for_display(&commit_message);
        crate::output::gutter(format_with_gutter(&formatted_message, "", None))?;

        repo.run_command(&["commit", "-m", &commit_message])
            .context("Failed to commit")?;

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
}

/// Commit uncommitted changes with the shared commit pipeline.
impl CommitOptions<'_> {
    pub fn commit(self) -> anyhow::Result<()> {
        if !self.no_verify
            && let Some(project_config) = self.ctx.repo.load_project_config()?
        {
            let pipeline = HookPipeline::new(*self.ctx);
            pipeline.run_pre_commit(&project_config, self.target_branch, self.auto_trust)?;
        }

        if self.warn_about_untracked && self.stage_mode == StageMode::All {
            self.ctx.repo.warn_if_auto_staging_untracked()?;
        }

        // Stage changes based on mode
        match self.stage_mode {
            StageMode::All => {
                // Stage everything: tracked modifications + untracked files
                self.ctx
                    .repo
                    .run_command(&["add", "-A"])
                    .context("Failed to stage changes")?;
            }
            StageMode::Tracked => {
                // Stage tracked modifications only (no untracked files)
                self.ctx
                    .repo
                    .run_command(&["add", "-u"])
                    .context("Failed to stage tracked changes")?;
            }
            StageMode::None => {
                // Stage nothing - commit only what's already in the index
            }
        }

        CommitGenerator::new(&self.ctx.config.commit_generation)
            .commit_staged_changes(self.show_no_squash_note)
    }
}
