//! `wt step commit` — commit working tree changes.

use std::path::Path;

use anyhow::Context;
use worktrunk::HookType;
use worktrunk::config::UserConfig;
use worktrunk::git::{Repository, TempIndex};
use worktrunk::styling::println;

use super::super::command_approval::{approve_or_skip, resolve_template_for_preview};
use super::super::commit::{CommitOptions, CommitOutcome, HookGate, StageMode};
use super::super::context::CommandEnv;
use super::super::hooks::HookAnnouncer;
use super::shared::print_dry_run;

/// Handle `wt step commit` command
///
/// `stage` is the CLI-provided stage mode. If None, uses the effective config default.
pub fn step_commit(
    branch: Option<String>,
    yes: bool,
    verify: bool,
    stage: Option<StageMode>,
    show_prompt: bool,
    dry_run: bool,
) -> anyhow::Result<Option<CommitOutcome>> {
    // --show-prompt and --dry-run skip hooks and the commit itself; --dry-run still
    // mirrors --stage against a temp index so the previewed prompt matches what a real
    // run would send the LLM. Neither path produces a CommitOutcome.
    if show_prompt || dry_run {
        preview_commit(stage, dry_run, yes)?;
        return Ok(None);
    }

    // Load config once, run LLM setup prompt, then reuse config
    let mut config = UserConfig::load().context("Failed to load config")?;
    // One-time LLM setup prompt (errors logged internally; don't block commit)
    let _ = crate::output::prompt_commit_generation(&mut config);

    let env = match branch {
        Some(ref b) => CommandEnv::for_selector(config, b)?,
        None => CommandEnv::for_action(config)?,
    };
    let ctx = env.context(yes);

    // CLI flag overrides config value
    let stage_mode = stage.unwrap_or(env.resolved().commit.stage());

    // "Approve at the Gate": prompt for approval upfront (when hooks are enabled) so
    // hook execution downstream is fully gated.
    let approved = verify
        && approve_or_skip(
            &ctx,
            &[HookType::PreCommit, HookType::PostCommit],
            "Commands declined, committing without hooks",
        )?;
    let hooks = HookGate::from_approval(verify, approved);

    let mut options = CommitOptions::new(&ctx);
    options.hooks = hooks;
    options.stage_mode = stage_mode;
    options.show_no_squash_note = false;

    let mut announcer = HookAnnouncer::new(ctx.repo, false);
    let outcome = options.commit(&mut announcer)?;
    announcer.flush()?;
    Ok(Some(outcome))
}

/// Handle `wt step commit` in `--show-prompt` or `--dry-run` mode.
///
/// Both modes skip hooks and the commit itself. `--show-prompt` outputs only the
/// rendered prompt against the existing index (cheap, pipeable). `--dry-run` mirrors
/// `--stage` against a temp index — so the previewed prompt matches what a real run
/// would send — then calls the LLM and prints the command and message in three labeled
/// sections. The user's real index is never modified.
fn preview_commit(stage: Option<StageMode>, dry_run: bool, yes: bool) -> anyhow::Result<()> {
    let env = CommandEnv::for_action(UserConfig::load().context("Failed to load config")?)?;
    let commit_config = env.resolved().commit_generation.clone();

    // For --dry-run, stage to a copy of the index so the preview reflects what a real
    // run would send. --show-prompt skips this — it's the cheap "what's already staged"
    // path. StageMode::None has nothing to stage, so we use the existing index as-is.
    let temp_index = if dry_run {
        stage
            .unwrap_or(env.resolved().commit.stage())
            .add_args()
            .map(|args| stage_to_temp_index(&env.repo, args))
            .transpose()?
    } else {
        None
    };
    let index_override = temp_index.as_ref().map(|temp| Path::new(temp.path()));

    let ctx = env.context(yes);
    let project_append = resolve_template_for_preview(&ctx, &commit_config, dry_run)?;

    let prompt =
        crate::llm::build_commit_prompt(&commit_config, index_override, project_append.as_deref())?;
    if !dry_run {
        println!("{}", prompt);
        return Ok(());
    }
    let message = crate::llm::generate_commit_message(
        &commit_config,
        index_override,
        project_append.as_deref(),
    )?;
    print_dry_run(&prompt, &commit_config, &message)
}

/// Stage into the worktree's shared temporary-index abstraction.
///
/// Returning [`TempIndex`] keeps the copied index alive while the caller builds
/// the prompt, then removes it on drop without touching the user's real index.
fn stage_to_temp_index(repo: &Repository, add_args: &[&str]) -> anyhow::Result<TempIndex> {
    let temp = repo.current_worktree().temp_index()?;
    let output = temp
        .git(add_args.iter().copied())
        .run()
        .context("Failed to stage changes into temp index")?;
    if !output.status.success() {
        return Err(
            worktrunk::git::CommandError::from_failed_output("git", add_args, &output).into(),
        );
    }
    Ok(temp)
}

#[cfg(test)]
mod tests {
    use worktrunk::git::{CommandError, Repository};
    use worktrunk::testing::TestRepo;

    /// A failing `git add` into the temp index (here: an invalid pathspec)
    /// must surface as a typed `CommandError`.
    #[test]
    fn stage_to_temp_index_failure_is_command_error() {
        let test = TestRepo::with_initial_commit();
        let repo = Repository::at(test.root_path()).unwrap();

        let err = match super::stage_to_temp_index(&repo, &["add", "--", ":(bad"]) {
            Ok(_) => panic!("invalid pathspec should fail"),
            Err(err) => err,
        };
        let cmd_err = CommandError::find_in(&err).expect("error should carry a CommandError");
        assert_eq!(cmd_err.command_string(), "git add -- :(bad");
    }
}
