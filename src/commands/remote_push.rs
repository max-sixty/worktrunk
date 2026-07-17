//! Remote publication for a named linked-worktree branch.

use color_print::cformat;
use worktrunk::git::{Repository, WorktrunkError};
use worktrunk::path::format_path_for_display;
use worktrunk::styling::{
    eprintln, error_message, hint_message, info_message, progress_message, success_message,
};

/// Push `branch` from its registered linked worktree.
pub(crate) fn handle_remote_push(
    branch: &str,
    explicit_remote: Option<&str>,
    dry_run: bool,
) -> anyhow::Result<()> {
    let repo = Repository::current()?;
    let worktree_path = repo.worktree_for_branch(branch)?.ok_or_else(|| {
        displayed_error(
            cformat!("No linked worktree for <bold>{branch}</>"),
            cformat!("Create or switch to the worktree: <underline>wt switch {branch}</>"),
        )
    })?;
    let worktree_repo = Repository::at(&worktree_path)?;
    let remote = match explicit_remote {
        Some(remote) => remote.to_string(),
        None => configured_push_remote(&worktree_repo, branch)?.ok_or_else(|| {
            let remote_argument = "<remote>";
            displayed_error(
                cformat!("No push remote configured for <bold>{branch}</>"),
                cformat!("Pass a remote: <underline>wt push {branch} {remote_argument}</>"),
            )
        })?,
    };

    let upstream = worktree_repo.branch(branch).upstream()?;
    let refspec = push_refspec(branch, upstream.as_deref(), &remote);
    let path_display = format_path_for_display(&worktree_path);

    if dry_run {
        worktrunk::styling::println!(
            "{}",
            info_message(cformat!(
                "Would push <bold>{branch}</> to <bold>{remote}</> @ {path_display}"
            ))
        );
    } else {
        eprintln!(
            "{}",
            progress_message(cformat!(
                "Pushing <bold>{branch}</> to <bold>{remote}</> @ {path_display}"
            ))
        );
    }

    let mut args = vec!["push"];
    if dry_run {
        args.push("--dry-run");
    }
    if upstream.is_none() && !is_remote_url(&remote) && !dry_run {
        args.push("--set-upstream");
    }
    args.push("--");
    args.push(&remote);
    args.push(&refspec);

    worktree_repo.run_command_delayed_stream(&args, Repository::SLOW_OPERATION_DELAY_MS, None)?;

    if !dry_run {
        eprintln!(
            "{}",
            success_message(cformat!(
                "Pushed <bold>{branch}</> to <bold>{remote}</> @ {path_display}"
            ))
        );
    }
    Ok(())
}

fn configured_push_remote(repo: &Repository, branch: &str) -> anyhow::Result<Option<String>> {
    // Don't use `Branch::push_remote()` here: Git's `@{push}` cannot resolve a
    // branch-specific push remote until an upstream ref exists, precisely the
    // first-push case this command supports.
    for key in [
        format!("branch.{branch}.pushRemote"),
        "remote.pushDefault".to_string(),
    ] {
        if let Some(remote) = repo
            .config_value(&key)?
            .filter(|remote| !remote.trim().is_empty())
        {
            return Ok(Some(remote));
        }
    }

    if let Some(remote) = repo.branch(branch).upstream()?.and_then(|upstream| {
        upstream
            .split_once('/')
            .map(|(remote, _)| remote.to_string())
    }) {
        return Ok(Some(remote));
    }

    // A first push has no tracking metadata. Git conventionally pushes to
    // `origin` when it exists; then use the repository-wide fallback
    // (`checkout.defaultRemote`, then the first configured remote).
    if repo.remote_url("origin").is_some() {
        return Ok(Some("origin".to_string()));
    }

    Ok(repo.primary_remote().ok())
}

fn push_refspec(branch: &str, upstream: Option<&str>, remote: &str) -> String {
    let upstream_branch = upstream.and_then(|upstream| upstream.split_once('/'));
    let destination = match upstream_branch {
        Some((upstream_remote, upstream_branch)) if upstream_remote == remote => upstream_branch,
        _ => branch,
    };
    format!("HEAD:refs/heads/{destination}")
}

fn is_remote_url(remote: &str) -> bool {
    remote.contains("://") || remote.starts_with("git@")
}

fn displayed_error(message: String, hint: String) -> anyhow::Error {
    eprintln!("{}", error_message(message));
    eprintln!("{}", hint_message(hint));
    WorktrunkError::AlreadyDisplayed { exit_code: 1 }.into()
}
