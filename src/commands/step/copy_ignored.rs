//! `wt step copy-ignored` — copy gitignored files matching `.worktreeinclude`.

use std::fs;
use std::path::PathBuf;

use anyhow::Context;
use worktrunk::copy::{copy_dir_recursive, copy_leaf};
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;
use worktrunk::progress::{Progress, format_bytes};
use worktrunk::styling::{
    eprintln, format_with_gutter, info_message, println, success_message, verbosity,
};

use super::shared::{list_and_filter_ignored_entries, resolve_copy_ignored_config};

/// Handle `wt step copy-ignored` command
///
/// Copies gitignored files from a source worktree to a destination worktree.
/// If a `.worktreeinclude` file exists, only files matching both `.worktreeinclude`
/// and gitignore patterns are copied. Without `.worktreeinclude`, all gitignored
/// files are copied. Uses COW (reflink) when available for efficient copying of
/// large directories like `target/`.
pub fn step_copy_ignored(
    from: Option<&str>,
    to: Option<&str>,
    dry_run: bool,
    force: bool,
    format: crate::cli::SwitchFormat,
) -> anyhow::Result<()> {
    // Self-lower only when we're running inside a background hook pipeline
    // (parent `wt` sets `WORKTRUNK_FOREGROUND=-1` on the detached runner).
    // Foreground callers — interactive `wt step copy-ignored` and synchronous
    // `pre-*` hook pipelines — are the UI the user is waiting on and must not
    // be I/O-throttled by `taskpolicy -b` on macOS.
    if worktrunk::priority::in_background_hook() {
        worktrunk::priority::lower_current_process();
    }
    let json_mode = format == crate::cli::SwitchFormat::Json;
    let repo = Repository::current()?;
    let copy_ignored_config = resolve_copy_ignored_config(&repo)?;

    // Resolve source and destination worktree paths
    let (source_path, source_context) = match from {
        Some(branch) => {
            let path = repo.worktree_for_branch(branch)?.ok_or_else(|| {
                worktrunk::git::GitError::WorktreeNotFound {
                    branch: branch.to_string(),
                }
            })?;
            (path, branch.to_string())
        }
        None => {
            // Default source is the primary worktree (main worktree for normal repos,
            // default branch worktree for bare repos).
            let path = repo.primary_worktree()?.ok_or_else(|| {
                anyhow::anyhow!(
                    "No primary worktree found (bare repo with no default branch worktree)"
                )
            })?;
            let context = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (path, context)
        }
    };

    let dest_path = match to {
        Some(branch) => repo.worktree_for_branch(branch)?.ok_or_else(|| {
            worktrunk::git::GitError::WorktreeNotFound {
                branch: branch.to_string(),
            }
        })?,
        None => repo.current_worktree().root()?,
    };

    if source_path == dest_path {
        if json_mode {
            let payload = serde_json::json!({
                "outcome": "same_worktree",
                "from": source_path,
                "to": dest_path,
                "entries": Vec::<serde_json::Value>::new(),
                "files": 0,
                "bytes": 0,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        } else {
            eprintln!(
                "{}",
                info_message("Source and destination are the same worktree")
            );
        }
        return Ok(());
    }

    let worktree_paths: Vec<PathBuf> = repo
        .list_worktrees()?
        .iter()
        .map(|wt| wt.path.clone())
        .collect();
    let entries_to_copy = list_and_filter_ignored_entries(
        &source_path,
        &source_context,
        &worktree_paths,
        &copy_ignored_config.exclude,
    )?;

    if entries_to_copy.is_empty() {
        if json_mode {
            let payload = serde_json::json!({
                "outcome": if dry_run { "planned" } else { "copied" },
                "dry_run": dry_run,
                "from": source_path,
                "to": dest_path,
                "entries": Vec::<serde_json::Value>::new(),
                "files": 0,
                "bytes": 0,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        } else {
            eprintln!("{}", info_message("No matching files to copy"));
        }
        return Ok(());
    }

    let verbose = verbosity();

    if dry_run {
        if json_mode {
            let entries: Vec<_> = entries_to_copy
                .iter()
                .map(|(src_entry, is_dir)| {
                    let relative = src_entry
                        .strip_prefix(&source_path)
                        .unwrap_or(src_entry.as_path());
                    serde_json::json!({
                        "path": relative,
                        "kind": if *is_dir { "dir" } else { "file" },
                    })
                })
                .collect();
            let payload = serde_json::json!({
                "outcome": "planned",
                "dry_run": true,
                "from": source_path,
                "to": dest_path,
                "entries": entries,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
            return Ok(());
        }
        let items: Vec<String> = entries_to_copy
            .iter()
            .map(|(src_entry, is_dir)| {
                let relative = src_entry
                    .strip_prefix(&source_path)
                    .unwrap_or(src_entry.as_path());
                let entry_type = if *is_dir { "dir" } else { "file" };
                format!("{} ({})", format_path_for_display(relative), entry_type)
            })
            .collect();
        let entry_word = if items.len() == 1 { "entry" } else { "entries" };
        eprintln!(
            "{}",
            info_message(format!(
                "Would copy {} {}:\n{}",
                items.len(),
                entry_word,
                format_with_gutter(&items.join("\n"), None)
            ))
        );
        return Ok(());
    }

    // Show entries in verbose mode (text only — JSON mode emits the full list at the end).
    if verbose >= 1 && !json_mode {
        let items: Vec<String> = entries_to_copy
            .iter()
            .map(|(src_entry, is_dir)| {
                let relative = src_entry
                    .strip_prefix(&source_path)
                    .unwrap_or(src_entry.as_path());
                let entry_type = if *is_dir { "dir" } else { "file" };
                format!("{} ({})", format_path_for_display(relative), entry_type)
            })
            .collect();
        let entry_word = if items.len() == 1 { "entry" } else { "entries" };
        eprintln!(
            "{}",
            info_message(format!(
                "Copying {} {}:\n{}",
                items.len(),
                entry_word,
                format_with_gutter(&items.join("\n"), None)
            ))
        );
    }

    // `start` auto-detects the TTY; verbose/dry-run already print enough.
    let progress = if verbose >= 1 || json_mode {
        Progress::disabled()
    } else {
        Progress::start("Copying")
    };

    let mut copied_count = 0usize;
    let mut copied_bytes = 0u64;
    for (src_entry, is_dir) in &entries_to_copy {
        let relative = src_entry
            .strip_prefix(&source_path)
            .expect("git ls-files path under worktree");
        let dest_entry = dest_path.join(relative);

        if *is_dir {
            let (n, b) =
                copy_dir_recursive(src_entry, &dest_entry, Some(&dest_path), force, &progress)
                    .with_context(|| {
                        format!("copying directory {}", format_path_for_display(relative))
                    })?;
            copied_count += n;
            copied_bytes += b;
        } else {
            if let Some(parent) = dest_entry.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!(
                        "creating directory for {}",
                        format_path_for_display(relative)
                    )
                })?;
            }
            if let Some(bytes) = copy_leaf(src_entry, &dest_entry, Some(&dest_path), force)? {
                copied_count += 1;
                copied_bytes += bytes;
                progress.record(bytes);
            }
        }
    }
    progress.finish();

    if json_mode {
        // `entries` mirrors dry-run: the top-level units selected for copy
        // (files and dirs). `files` counts the actual leaves written
        // (recursive + skipping pre-existing files), `bytes` sums their size.
        let entries: Vec<_> = entries_to_copy
            .iter()
            .map(|(src_entry, is_dir)| {
                let relative = src_entry
                    .strip_prefix(&source_path)
                    .unwrap_or(src_entry.as_path());
                serde_json::json!({
                    "path": relative,
                    "kind": if *is_dir { "dir" } else { "file" },
                })
            })
            .collect();
        let payload = serde_json::json!({
            "outcome": "copied",
            "dry_run": false,
            "from": source_path,
            "to": dest_path,
            "entries": entries,
            "files": copied_count,
            "bytes": copied_bytes,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        // Show summary
        let file_word = if copied_count == 1 { "file" } else { "files" };
        eprintln!(
            "{}",
            success_message(format!(
                "Copied {copied_count} {file_word} · {}",
                format_bytes(copied_bytes)
            ))
        );
    }

    Ok(())
}
