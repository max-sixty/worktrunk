use anyhow::Context;
use color_print::cformat;
use worktrunk::HookType;
use worktrunk::config::CommitGenerationConfig;
use worktrunk::git::Repository;
use worktrunk::styling::{
    format_with_gutter, hint_message, info_message, progress_message, success_message,
};

use super::command_executor::CommandContext;
use super::hooks::{HookFailureStrategy, HookPipeline, HookSource};
use super::repository_ext::RepositoryCliExt;

// Re-export StageMode from config for use by CLI
pub use worktrunk::config::StageMode;

/// Options for committing current changes.
pub struct CommitOptions<'a> {
    pub ctx: &'a CommandContext<'a>,
    pub target_branch: Option<&'a str>,
    pub no_verify: bool,
    pub stage_mode: StageMode,
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
        let lines: Vec<&str> = message.lines().collect();

        if lines.is_empty() {
            return String::new();
        }

        let mut result = cformat!("<bold>{}</>", lines[0]);

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
            crate::output::print(hint_message(cformat!(
                "Using fallback commit message. Run <bright-black>wt config --help</> for LLM setup guide"
            )))?;
        }
        Ok(())
    }

    pub fn commit_staged_changes(
        &self,
        show_no_squash_note: bool,
        stage_mode: StageMode,
    ) -> anyhow::Result<()> {
        let repo = Repository::current();

        // Fail early if nothing is staged (avoids confusing LLM prompt with empty diff)
        if !repo.has_staged_changes()? {
            anyhow::bail!("Nothing to commit");
        }

        let stats_parts = repo.diff_stats_summary(&["diff", "--staged", "--shortstat"]);

        let changes_type = match stage_mode {
            StageMode::Tracked => "tracked changes",
            _ => "changes",
        };

        let action = if self.config.is_configured() {
            format!("Generating commit message and committing {changes_type}...")
        } else {
            format!("Committing {changes_type} with default message...")
        };

        let mut parts = vec![];
        if !stats_parts.is_empty() {
            parts.extend(stats_parts);
        }
        if show_no_squash_note {
            parts.push("no squashing needed".to_string());
        }

        let full_progress_msg = if parts.is_empty() {
            action
        } else {
            // Gray parenthetical with separate cformat for closing paren (avoids optimizer)
            let parts_str = parts.join(", ");
            let paren_close = cformat!("<bright-black>)</>");
            cformat!("{action} <bright-black>({parts_str}</>{paren_close}")
        };

        crate::output::print(progress_message(full_progress_msg))?;

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

        crate::output::print(success_message(cformat!(
            "Committed changes @ <dim>{commit_hash}</>"
        )))?;

        Ok(())
    }
}

/// Commit uncommitted changes with the shared commit pipeline.
impl CommitOptions<'_> {
    pub fn commit(self) -> anyhow::Result<()> {
        let project_config = self.ctx.repo.load_project_config()?;
        let user_hooks_exist = self.ctx.config.pre_commit.is_some();
        let project_hooks_exist = project_config
            .as_ref()
            .map(|c| c.pre_commit.is_some())
            .unwrap_or(false);
        let any_hooks_exist = user_hooks_exist || project_hooks_exist;

        // Show skip message
        if self.no_verify && any_hooks_exist {
            crate::output::print(info_message(cformat!(
                "Skipping pre-commit hooks (<bright-black>--no-verify</>)"
            )))?;
        }

        if !self.no_verify {
            let pipeline = HookPipeline::new(*self.ctx);
            let extra_vars: Vec<(&str, &str)> = self
                .target_branch
                .into_iter()
                .map(|target| ("target", target))
                .collect();

            // Run user pre-commit hooks first (no approval required)
            if let Some(user_config) = &self.ctx.config.pre_commit {
                pipeline
                    .run_sequential(
                        user_config,
                        HookType::PreCommit,
                        HookSource::User,
                        &extra_vars,
                        HookFailureStrategy::FailFast,
                        None,
                    )
                    .map_err(worktrunk::git::add_hook_skip_hint)?;
            }

            // Then run project pre-commit hooks (require approval)
            if let Some(ref config) = project_config {
                pipeline
                    .run_pre_commit(config, self.target_branch, None)
                    .map_err(worktrunk::git::add_hook_skip_hint)?;
            }
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
            .commit_staged_changes(self.show_no_squash_note, self.stage_mode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use worktrunk::config::CommitGenerationConfig;

    #[test]
    fn test_format_message_for_display_single_line() {
        let config = CommitGenerationConfig::default();
        let generator = CommitGenerator::new(&config);
        let result = generator.format_message_for_display("Simple commit message");
        // Should contain the message text with styling
        assert!(result.contains("Simple commit message"));
        // Should be styled (output differs from plain input)
        assert!(result.len() > "Simple commit message".len());
    }

    #[test]
    fn test_format_message_for_display_multiline() {
        let config = CommitGenerationConfig::default();
        let generator = CommitGenerator::new(&config);
        let result = generator.format_message_for_display("First line\nSecond line\nThird line");
        assert!(result.contains("First line"));
        assert!(result.contains("Second line"));
        assert!(result.contains("Third line"));
    }

    #[test]
    fn test_format_message_for_display_empty() {
        let config = CommitGenerationConfig::default();
        let generator = CommitGenerator::new(&config);
        let result = generator.format_message_for_display("");
        assert_eq!(result, "");
    }

    #[test]
    fn test_commit_options_new() {
        // CommitOptions::new requires a CommandContext, which requires a Repository.
        // Instead, test the struct fields directly
        let stage_mode = StageMode::default();
        assert!(matches!(stage_mode, StageMode::All));
    }

    #[test]
    fn test_stage_mode_variants() {
        // Test that all StageMode variants can be matched
        let modes = [StageMode::All, StageMode::Tracked, StageMode::None];
        for mode in modes {
            match mode {
                StageMode::All => assert_eq!(format!("{:?}", mode), "All"),
                StageMode::Tracked => assert_eq!(format!("{:?}", mode), "Tracked"),
                StageMode::None => assert_eq!(format!("{:?}", mode), "None"),
            }
        }
    }
}
