//! `wt sync` — rebase stacked worktree branches in dependency order.
//!
//! Detects the branch dependency tree from git's commit graph using pairwise
//! merge-base analysis, then rebases each branch onto its parent in topological
//! order. Handles integrated (merged) branches by reparenting their children
//! with `rebase --onto`.
//!
//! The dependency tree is persisted to a stack file (`.git/wt/stack`) on every
//! sync. The format is compatible with git-machete: indentation-based, one
//! branch per line. When this file exists, it is used for parent tracking and
//! non-default branch integration detection.
//!
//! Key behaviors:
//! - No configuration needed — dependencies are inferred from git history
//! - Stack file (`.git/wt/stack`) is auto-created and updated on every sync
//! - By default, syncs all stacks
//! - `--stack` restricts to the stack containing the current branch
//! - `--fetch` / `--push` / `--prune` enable optional phases
//! - `--dry-run` previews the plan without executing
//! - Stops on first conflict; user resolves and re-runs

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, bail};
use color_print::cformat;

use worktrunk::git::Repository;
use worktrunk::styling::{eprintln, progress_message, success_message, warning_message};

/// A node in the dependency tree.
#[derive(Debug)]
struct TreeNode {
    branch: String,
    path: PathBuf,
    parent: Option<String>,
    /// If this branch was reparented because its original parent was integrated,
    /// this holds the original parent branch name (for `rebase --onto`).
    original_parent: Option<String>,
    children: Vec<String>,
}

/// The full dependency tree for sync operations.
#[derive(Debug)]
struct DependencyTree {
    /// The root branch (default branch).
    root: String,
    /// All nodes indexed by branch name.
    nodes: HashMap<String, TreeNode>,
}

impl DependencyTree {
    /// Return branches in topological order (parent before children), excluding the root.
    fn topological_order(&self) -> Vec<&str> {
        let mut order = Vec::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(self.root.as_str());

        while let Some(branch) = queue.pop_front() {
            if let Some(node) = self.nodes.get(branch) {
                for child in &node.children {
                    order.push(child.as_str());
                    queue.push_back(child);
                }
            }
        }
        order
    }

    /// Get all branches in the stack containing the given branch.
    fn stack_containing(&self, branch: &str) -> Vec<&str> {
        // Find the branch in our nodes first (ensures we return self-lifetime refs)
        let Some(start_node) = self.nodes.get(branch) else {
            return vec![];
        };

        // Walk up to find the top of the stack (direct child of root).
        // Track visited nodes to detect cycles (safety net against dependency
        // detection bugs that produce circular parent chains).
        let mut current_key = start_node.branch.as_str();
        let mut visited = std::collections::HashSet::new();
        visited.insert(current_key);
        loop {
            let Some(node) = self.nodes.get(current_key) else {
                return vec![];
            };
            match &node.parent {
                Some(p) if p == &self.root => break,
                Some(p) => {
                    current_key = match self.nodes.get(p.as_str()) {
                        Some(n) => {
                            let key = n.branch.as_str();
                            if !visited.insert(key) {
                                // Cycle detected — treat branch as direct child of root
                                break;
                            }
                            key
                        }
                        None => break,
                    };
                }
                None => break, // current is the root
            }
        }

        if current_key == self.root {
            // Branch is a direct child of root or the root itself — return all
            return self.topological_order();
        }

        // `current_key` is the top of the stack (direct child of root).
        // Collect all descendants.
        let mut stack: Vec<&str> = Vec::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(current_key);
        while let Some(b) = queue.pop_front() {
            stack.push(b);
            if let Some(node) = self.nodes.get(b) {
                for child in &node.children {
                    queue.push_back(child.as_str());
                }
            }
        }
        stack
    }
}

/// Options for the sync command.
pub struct SyncOptions {
    pub fetch: bool,
    pub all: bool,
    pub push: bool,
    pub prune: bool,
    pub dry_run: bool,
}

/// Result of building the dependency tree, including integrated branch info.
struct SyncPlan {
    tree: DependencyTree,
    /// Branches detected as integrated into the default branch, with their worktree paths.
    integrated: Vec<(String, PathBuf)>,
}

/// Stack file name within the wt data directory.
const STACK_FILE: &str = "stack";

/// Parse a stack file (git-machete compatible format) into a parent map.
///
/// The format is indentation-based, one branch per line:
/// ```text
/// main
///     pr1
///         pr2
///             pr3
///     other-pr
/// ```
///
/// Returns a map of branch -> parent. The root branch (first line, no indent)
/// is expected to match the default branch and is not included in the map.
fn parse_stack_file(
    content: &str,
    default_branch: &str,
) -> anyhow::Result<HashMap<String, String>> {
    let mut parent_map: HashMap<String, String> = HashMap::new();
    // Stack of (indent_level, branch_name)
    let mut stack: Vec<(usize, String)> = Vec::new();

    for (line_num, raw_line) in content.lines().enumerate() {
        // Skip empty lines and comments
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Strip annotations after the branch name (machete supports "branch  annotation")
        let branch = trimmed.split_whitespace().next().unwrap();

        // Determine indent level (count leading whitespace: tab=1, space groups of 4=1)
        let indent = if raw_line.starts_with('\t') {
            raw_line.len() - raw_line.trim_start_matches('\t').len()
        } else {
            let spaces = raw_line.len() - raw_line.trim_start().len();
            // Accept any consistent spacing — treat each group as one level
            // by dividing by the first nonzero indent seen
            spaces
        };

        // Pop stack back to find the parent at this indent level
        while let Some(&(level, _)) = stack.last() {
            if level >= indent {
                stack.pop();
            } else {
                break;
            }
        }

        let parent = stack
            .last()
            .map(|(_, b)| b.clone())
            .unwrap_or_else(|| default_branch.to_string());

        // The root entry (default branch itself) is not added to the map
        if branch != default_branch {
            parent_map.insert(branch.to_string(), parent);
        }

        stack.push((indent, branch.to_string()));
    }

    Ok(parent_map)
}

/// Format the dependency tree as a stack file (git-machete compatible format).
fn format_stack_file(tree: &DependencyTree) -> String {
    let mut output = String::new();
    format_stack_node(tree, &tree.root, 0, &mut output);
    output
}

fn format_stack_node(tree: &DependencyTree, branch: &str, depth: usize, output: &mut String) {
    // Don't write the root (default branch) — it's implicit
    if depth > 0 {
        for _ in 0..depth - 1 {
            output.push('\t');
        }
        output.push_str(branch);
        output.push('\n');
    }

    if let Some(node) = tree.nodes.get(branch) {
        for child in &node.children {
            format_stack_node(tree, child, depth + 1, output);
        }
    }
}

/// Build the dependency tree from worktree branches.
///
/// For each branch B, finds the closest parent P where merge_base(P, B) is
/// nearest to B's tip (fewest commits ahead). Integrated branches are excluded
/// and their children reparented.
///
/// If a stack file (`.git/wt/stack`) exists, it is used for parent detection
/// instead of merge-base inference.
fn build_dependency_tree(repo: &Repository) -> anyhow::Result<SyncPlan> {
    let default_branch = repo
        .default_branch()
        .context("Cannot determine default branch")?;

    let worktrees = repo.list_worktrees()?;

    // Collect branches with worktrees, filtering detached/bare
    let mut branches: Vec<(String, PathBuf)> = Vec::new();
    for wt in &worktrees {
        if wt.bare || wt.detached {
            continue;
        }
        if let Some(ref branch) = wt.branch {
            branches.push((branch.clone(), wt.path.clone()));
        }
    }

    // Ensure default branch is included (may be the main worktree)
    let has_default = branches.iter().any(|(b, _)| b == &default_branch);
    if !has_default {
        // The default branch might not have a worktree — use the repo path
        bail!(
            "Default branch '{}' has no worktree. Cannot build dependency tree.",
            default_branch
        );
    }

    // Check for integrated branches.
    //
    // Without a stack file, we only check against the default branch (main).
    // With a stack file, we also check against each branch's explicit parent,
    // which detects merges between non-default branches (e.g., PR2 squash-merged
    // into PR1).
    let integration_target = repo.integration_target();
    let target_ref = integration_target.as_deref().unwrap_or(&default_branch);

    let mut integrated: HashMap<String, PathBuf> = HashMap::new();

    // Parse stack file once (used for parent detection, integration checks, and reparenting)
    let stack_file_path = repo.wt_dir().join(STACK_FILE);
    let explicit_parents: HashMap<String, String> = if stack_file_path.exists() {
        let content = std::fs::read_to_string(&stack_file_path)
            .context("Failed to read stack file")?;
        parse_stack_file(&content, &default_branch)?
    } else {
        HashMap::new()
    };
    let has_stack_file = !explicit_parents.is_empty();

    // Determine parent for each branch. If a stack file exists, use it;
    // otherwise infer parents from the commit graph.
    let mut parent_map: HashMap<String, (String, Option<String>)> = HashMap::new(); // branch -> (parent, original_parent_if_reparented)

    // Phase 1: Check integration against default branch (always)
    for (branch, path) in &branches {
        if branch == &default_branch {
            continue;
        }
        let (_, reason) = repo.integration_reason(branch, target_ref)?;
        if reason.is_some() {
            integrated.insert(branch.clone(), path.clone());
        }
    }

    // Phase 2: With stack file, also check integration against each branch's
    // explicit parent. This catches merges between stacked branches (e.g.,
    // PR2 squash-merged into PR1).
    if has_stack_file {
        for (branch, path) in &branches {
            if branch == &default_branch || integrated.contains_key(branch) {
                continue;
            }
            if let Some(parent) = explicit_parents.get(branch) {
                if parent != target_ref {
                    let (_, reason) = repo.integration_reason(branch, parent)?;
                    if reason.is_some() {
                        integrated.insert(branch.clone(), path.clone());
                    }
                }
            }
        }
    }

    if has_stack_file {
        for (branch, _) in &branches {
            if branch == &default_branch || integrated.contains_key(branch) {
                continue;
            }
            let parent = explicit_parents
                .get(branch)
                .cloned()
                .unwrap_or_else(|| default_branch.clone());
            parent_map.insert(branch.clone(), (parent, None));
        }
    } else {
        // Infer parents from the commit graph using merge-base analysis.
        //
        // For each branch B, the parent P is selected in two tiers:
        //   1. True ancestors (candidate_depth == 0, meaning merge_base ==
        //      candidate tip): branches whose tip is reachable from B. Among
        //      true ancestors, pick the closest (smallest branch_depth).
        //   2. Diverged candidates (only if no true ancestors): pick by
        //      smallest branch_depth, then smallest candidate_depth.
        //
        // This prevents cycles in stacked branches: if B descends from C
        // (C's tip is on B's history), C is a true ancestor and always wins
        // over siblings that merely share a common fork point.
        //
        // Limitation: after syncing + adding a mid-stack commit, the parent
        // branch becomes "diverged" and may lose to the default branch (a
        // true ancestor). The auto-saved stack file (`.git/wt/stack`) avoids
        // this by preserving explicit parent relationships across syncs.
        let branch_names: Vec<&str> = branches.iter().map(|(b, _)| b.as_str()).collect();

        for (branch, _) in &branches {
            if branch == &default_branch || integrated.contains_key(branch) {
                continue;
            }

            let mut ancestors: Vec<(&str, String, usize)> = Vec::new();
            let mut diverged: Vec<(&str, String, usize, usize)> = Vec::new();

            for candidate in &branch_names {
                if *candidate == branch.as_str() {
                    continue;
                }

                let Some(mb) = repo.merge_base(candidate, branch)? else {
                    continue;
                };

                let branch_depth = repo.count_commits(&mb, branch)?;

                if branch_depth == 0 {
                    continue;
                }

                let candidate_depth = repo.count_commits(&mb, candidate)?;

                if candidate_depth == 0 {
                    ancestors.push((candidate, mb, branch_depth));
                } else {
                    diverged.push((candidate, mb, branch_depth, candidate_depth));
                }
            }

            let mut best_parent: Option<&str> = None;
            let mut tie_candidates: Vec<(&str, String)> = Vec::new();

            if !ancestors.is_empty() {
                ancestors.sort_by_key(|&(_, _, bd)| bd);
                let best_bd = ancestors[0].2;
                tie_candidates = ancestors
                    .iter()
                    .filter(|&&(_, _, bd)| bd == best_bd)
                    .map(|&(c, ref mb, _)| (c, mb.clone()))
                    .collect();
                best_parent = Some(ancestors[0].0);
            } else if !diverged.is_empty() {
                diverged.sort_by_key(|&(_, _, bd, cd)| (bd, cd));
                let (best_bd, best_cd) = (diverged[0].2, diverged[0].3);
                tie_candidates = diverged
                    .iter()
                    .filter(|&&(_, _, bd, cd)| bd == best_bd && cd == best_cd)
                    .map(|&(c, ref mb, _, _)| (c, mb.clone()))
                    .collect();
                best_parent = Some(diverged[0].0);
            }

            if tie_candidates.len() > 1 {
                let mb_shas: Vec<&str> =
                    tie_candidates.iter().map(|(_, mb)| mb.as_str()).collect();
                let timestamps = repo.commit_timestamps(&mb_shas)?;

                let mut best_ts = i64::MIN;
                let mut resolved_parent: Option<&str> = None;
                for (candidate, mb) in &tie_candidates {
                    if let Some(&ts) = timestamps.get(mb.as_str()) {
                        if ts > best_ts {
                            best_ts = ts;
                            resolved_parent = Some(candidate);
                        }
                    }
                }
                if let Some(p) = resolved_parent {
                    best_parent = Some(p);
                }

                let names: Vec<&str> = tie_candidates.iter().map(|(c, _)| *c).collect();
                eprintln!(
                    "{}",
                    warning_message(cformat!(
                        "Branch <bold>{}</> has equidistant parents: {}. Picked <bold>{}</>.",
                        branch,
                        names.join(", "),
                        best_parent.unwrap_or("unknown"),
                    ))
                );
            }

            if let Some(parent) = best_parent {
                parent_map.insert(branch.clone(), (parent.to_string(), None));
            }
        }
    }

    // Reparent children of integrated branches.
    //
    // If branch X's parent was integrated, walk up the tree to find the first
    // non-integrated ancestor. With a stack file, this uses the explicit parent
    // chain (e.g., pr2 integrated into pr1 → pr3 reparents to pr1). Without a
    // stack file, falls back to the default branch.
    //
    for (_branch, (parent, original_parent)) in parent_map.iter_mut() {
        if integrated.contains_key(parent.as_str()) {
            let old_parent = parent.clone();
            // Walk up the tree to find the first non-integrated ancestor.
            // With a stack file, use the explicit parent chain; without, fall
            // back to the default branch.
            let mut new_parent = explicit_parents
                .get(parent.as_str())
                .cloned()
                .unwrap_or_else(|| default_branch.clone());
            // Keep walking if that ancestor is also integrated.
            // Track visited to prevent infinite loops from cycles in the stack file.
            let mut visited = std::collections::HashSet::new();
            while integrated.contains_key(new_parent.as_str()) {
                if !visited.insert(new_parent.clone()) {
                    break; // cycle detected, fall through to current new_parent
                }
                new_parent = explicit_parents
                    .get(new_parent.as_str())
                    .cloned()
                    .unwrap_or_else(|| default_branch.clone());
            }
            *parent = new_parent;
            *original_parent = Some(old_parent);
        }
    }

    // Build the tree structure
    let mut nodes: HashMap<String, TreeNode> = HashMap::new();

    // Add root node
    let root_path = branches
        .iter()
        .find(|(b, _)| b == &default_branch)
        .map(|(_, p)| p.clone())
        .unwrap_or_default();

    nodes.insert(
        default_branch.clone(),
        TreeNode {
            branch: default_branch.clone(),
            path: root_path,
            parent: None,
            original_parent: None,
            children: Vec::new(),
        },
    );

    // Add all other nodes (skip integrated branches)
    for (branch, path) in &branches {
        if branch == &default_branch || integrated.contains_key(branch) {
            continue;
        }
        let (parent, orig_parent) = parent_map
            .get(branch)
            .cloned()
            .unwrap_or((default_branch.clone(), None));

        nodes.insert(
            branch.clone(),
            TreeNode {
                branch: branch.clone(),
                path: path.clone(),
                parent: Some(parent.clone()),
                original_parent: orig_parent,
                children: Vec::new(),
            },
        );
    }

    // Wire up children
    let branches_with_parents: Vec<(String, String)> = nodes
        .iter()
        .filter_map(|(b, n)| n.parent.as_ref().map(|p| (b.clone(), p.clone())))
        .collect();

    for (branch, parent) in branches_with_parents {
        if let Some(parent_node) = nodes.get_mut(&parent) {
            parent_node.children.push(branch);
        }
    }

    // Sort children for deterministic order
    for node in nodes.values_mut() {
        node.children.sort();
    }

    let integrated_list: Vec<(String, PathBuf)> = integrated.into_iter().collect();

    Ok(SyncPlan {
        tree: DependencyTree {
            root: default_branch,
            nodes,
        },
        integrated: integrated_list,
    })
}

/// Execute the sync operation.
pub fn handle_sync(opts: SyncOptions) -> anyhow::Result<()> {
    let repo = Repository::current()?;

    // Fetch from remote if requested
    if opts.fetch {
        eprintln!(
            "{}",
            progress_message(cformat!("Fetching from remote..."))
        );
        repo.run_command(&["fetch", "--prune"])
            .context("git fetch failed")?;
        eprintln!("{}", success_message("Fetch complete"));
    }

    // Build dependency tree
    let plan = build_dependency_tree(&repo)?;
    let tree = plan.tree;

    // Always persist the dependency tree to the stack file. This ensures
    // parent-based integration detection works (e.g., PR2 merged into PR1)
    // and keeps the file in sync after integrated branches are removed.
    {
        let stack_file_path = repo.wt_dir().join(STACK_FILE);
        let content = format_stack_file(&tree);
        std::fs::create_dir_all(repo.wt_dir())
            .context("Failed to create .git/wt directory")?;
        std::fs::write(&stack_file_path, &content)
            .context("Failed to write stack file")?;
    }

    // Determine which branches to sync
    let current_wt = repo.current_worktree();
    let current_branch = current_wt.branch()?;

    let branches_to_sync: Vec<&str> = if !opts.all {
        let Some(ref current) = current_branch else {
            bail!("Current worktree has no branch. Use --all to sync all branches.");
        };
        let stack = tree.stack_containing(current);
        if stack.is_empty() {
            eprintln!(
                "{}",
                success_message(cformat!(
                    "Branch <bold>{current}</> is not part of any stack. Nothing to sync."
                ))
            );
            return Ok(());
        }
        stack
    } else {
        tree.topological_order()
    };

    if branches_to_sync.is_empty() {
        eprintln!("{}", success_message("All branches are up to date."));
        return Ok(());
    }

    // Dry-run mode: show plan and exit
    if opts.dry_run {
        print_sync_plan(&tree, &branches_to_sync);
        return Ok(());
    }

    // Pre-check: ensure all participating worktrees are clean
    let mut dirty_branches = Vec::new();
    for &branch in &branches_to_sync {
        if let Some(node) = tree.nodes.get(branch) {
            let wt = repo.worktree_at(&node.path);
            if wt.is_dirty()? {
                dirty_branches.push(branch);
            }
        }
    }

    if !dirty_branches.is_empty() {
        let list = dirty_branches
            .iter()
            .map(|b| format!("  - {b}"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(anyhow::anyhow!(
            "{list}\n\nCommit or stash changes before running `wt sync`."
        ))
        .context("worktrees have uncommitted changes");
    }

    // Also check for any in-progress rebases
    for &branch in &branches_to_sync {
        if let Some(node) = tree.nodes.get(branch) {
            let wt = repo.worktree_at(&node.path);
            if wt.is_rebasing()? {
                return Err(anyhow::anyhow!(
                    "Resolve it with `git rebase --continue` or `git rebase --abort` first."
                ))
                .context(format!("branch '{branch}' has a rebase in progress"));
            }
        }
    }

    // Execute rebases in topological order
    let mut rebased_count = 0;
    let mut skipped_count = 0;
    let mut rebased_branches: Vec<String> = Vec::new();

    for &branch in &branches_to_sync {
        let Some(node) = tree.nodes.get(branch) else {
            continue;
        };
        let Some(ref parent) = node.parent else {
            continue; // root node
        };

        let wt = repo.worktree_at(&node.path);

        // Check if already up-to-date
        let Some(mb) = repo.merge_base(parent, branch)? else {
            continue;
        };
        let parent_sha = repo
            .run_command(&["rev-parse", parent])?
            .trim()
            .to_string();

        if mb == parent_sha {
            skipped_count += 1;
            eprintln!(
                "{}",
                success_message(cformat!(
                    "<bold>{branch}</> is up to date with <bold>{parent}</>"
                ))
            );
            continue;
        }

        // Perform the rebase
        if let Some(ref orig_parent) = node.original_parent {
            // Reparented branch — use rebase --onto
            eprintln!(
                "{}",
                progress_message(cformat!(
                    "Rebasing <bold>{branch}</> onto <bold>{parent}</> (was on integrated <bold>{orig_parent}</>)..."
                ))
            );
            let result = wt.run_command(&["rebase", "--onto", parent, orig_parent, branch]);
            if let Err(e) = result {
                if wt.is_rebasing()? {
                    eprintln!(
                        "{}",
                        worktrunk::styling::error_message(cformat!(
                            "Rebase conflict while rebasing <bold>{branch}</> onto <bold>{parent}</>"
                        ))
                    );
                    eprintln!(
                        "{}",
                        worktrunk::styling::hint_message(cformat!(
                            "Resolve conflicts in {}, then run:\n  cd {}\n  git rebase --continue\n  wt sync",
                            node.path.display(),
                            node.path.display(),
                        ))
                    );
                    return Ok(());
                }
                return Err(e.context(format!("Failed to rebase {branch} onto {parent}")));
            }
        } else {
            // Normal rebase
            eprintln!(
                "{}",
                progress_message(cformat!(
                    "Rebasing <bold>{branch}</> onto <bold>{parent}</>..."
                ))
            );
            let result = wt.run_command(&["rebase", parent]);
            if let Err(e) = result {
                if wt.is_rebasing()? {
                    eprintln!(
                        "{}",
                        worktrunk::styling::error_message(cformat!(
                            "Rebase conflict while rebasing <bold>{branch}</> onto <bold>{parent}</>"
                        ))
                    );
                    eprintln!(
                        "{}",
                        worktrunk::styling::hint_message(cformat!(
                            "Resolve conflicts in {}, then run:\n  cd {}\n  git rebase --continue\n  wt sync",
                            node.path.display(),
                            node.path.display(),
                        ))
                    );
                    return Ok(());
                }
                return Err(e.context(format!("Failed to rebase {branch} onto {parent}")));
            }
        }

        rebased_count += 1;
        rebased_branches.push(branch.to_string());
        eprintln!(
            "{}",
            success_message(cformat!("Rebased <bold>{branch}</> onto <bold>{parent}</>"))
        );
    }

    // Summary
    if rebased_count == 0 && skipped_count > 0 {
        eprintln!("{}", success_message("All branches are up to date."));
    } else if rebased_count > 0 {
        eprintln!(
            "{}",
            success_message(cformat!(
                "Sync complete: {} rebased, {} already up to date.",
                rebased_count,
                skipped_count,
            ))
        );
    }

    // Push rebased branches
    if opts.push && !rebased_branches.is_empty() {
        eprintln!();
        for branch in &rebased_branches {
            // Skip branches without an upstream
            if repo.branch(branch).upstream()?.is_none() {
                continue;
            }
            eprintln!(
                "{}",
                progress_message(cformat!("Pushing <bold>{branch}</>..."))
            );
            let result = repo.run_command(&[
                "push",
                "--force-with-lease",
                "origin",
                branch,
            ]);
            match result {
                Ok(_) => {
                    eprintln!(
                        "{}",
                        success_message(cformat!("Pushed <bold>{branch}</>"))
                    );
                }
                Err(e) => {
                    eprintln!(
                        "{}",
                        worktrunk::styling::error_message(cformat!(
                            "Failed to push <bold>{branch}</>: {e}"
                        ))
                    );
                }
            }
        }
    }

    // Prune integrated worktrees
    if opts.prune && !plan.integrated.is_empty() {
        eprintln!();
        for (branch, path) in &plan.integrated {
            eprintln!(
                "{}",
                progress_message(cformat!(
                    "Removing integrated worktree <bold>{branch}</>..."
                ))
            );
            // Remove the worktree
            let result = repo.run_command(&[
                "worktree",
                "remove",
                "--force",
                &path.to_string_lossy(),
            ]);
            if let Err(e) = result {
                eprintln!(
                    "{}",
                    worktrunk::styling::error_message(cformat!(
                        "Failed to remove worktree for <bold>{branch}</>: {e}"
                    ))
                );
                continue;
            }
            // Delete the local branch
            let _ = repo.run_command(&["branch", "-D", branch]);
            // Delete remote branch if it exists
            if repo.branch(branch).upstream()?.is_some() {
                let _ = repo.run_command(&["push", "origin", "--delete", branch]);
            }
            eprintln!(
                "{}",
                success_message(cformat!("Removed integrated worktree <bold>{branch}</>"))
            );
        }
    }

    Ok(())
}

/// Print the sync plan (dry-run mode).
fn print_sync_plan(tree: &DependencyTree, branches: &[&str]) {
    eprintln!("Dependency tree:");
    print_tree_node(tree, &tree.root, "", true);

    eprintln!();
    eprintln!("Planned operations:");
    let mut has_ops = false;
    for &branch in branches {
        let Some(node) = tree.nodes.get(branch) else {
            continue;
        };
        let Some(ref parent) = node.parent else {
            continue;
        };

        if let Some(ref orig_parent) = node.original_parent {
            eprintln!(
                "  rebase --onto {parent} {orig_parent} {branch}  (reparented from integrated {orig_parent})"
            );
        } else {
            eprintln!("  rebase {branch} onto {parent}");
        }
        has_ops = true;
    }
    if !has_ops {
        eprintln!("  (none)");
    }
}

/// Print a tree node with indentation.
fn print_tree_node(tree: &DependencyTree, branch: &str, prefix: &str, is_last: bool) {
    let connector = if prefix.is_empty() {
        ""
    } else if is_last {
        "└── "
    } else {
        "├── "
    };
    eprintln!("{prefix}{connector}{branch}");

    let Some(node) = tree.nodes.get(branch) else {
        return;
    };

    let child_prefix = if prefix.is_empty() {
        "".to_string()
    } else if is_last {
        format!("{prefix}    ")
    } else {
        format!("{prefix}│   ")
    };

    for (i, child) in node.children.iter().enumerate() {
        let is_last_child = i == node.children.len() - 1;
        print_tree_node(tree, child, &child_prefix, is_last_child);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topological_order_linear() {
        let mut nodes = HashMap::new();
        nodes.insert(
            "main".to_string(),
            TreeNode {
                branch: "main".to_string(),
                path: PathBuf::new(),
                parent: None,
                original_parent: None,
                children: vec!["pr1".to_string()],
            },
        );
        nodes.insert(
            "pr1".to_string(),
            TreeNode {
                branch: "pr1".to_string(),
                path: PathBuf::new(),
                parent: Some("main".to_string()),
                original_parent: None,
                children: vec!["pr2".to_string()],
            },
        );
        nodes.insert(
            "pr2".to_string(),
            TreeNode {
                branch: "pr2".to_string(),
                path: PathBuf::new(),
                parent: Some("pr1".to_string()),
                original_parent: None,
                children: vec!["pr3".to_string()],
            },
        );
        nodes.insert(
            "pr3".to_string(),
            TreeNode {
                branch: "pr3".to_string(),
                path: PathBuf::new(),
                parent: Some("pr2".to_string()),
                original_parent: None,
                children: vec![],
            },
        );

        let tree = DependencyTree {
            root: "main".to_string(),
            nodes,
        };

        assert_eq!(tree.topological_order(), vec!["pr1", "pr2", "pr3"]);
    }

    #[test]
    fn test_topological_order_fan_out() {
        let mut nodes = HashMap::new();
        nodes.insert(
            "main".to_string(),
            TreeNode {
                branch: "main".to_string(),
                path: PathBuf::new(),
                parent: None,
                original_parent: None,
                children: vec!["feature-a".to_string(), "feature-b".to_string()],
            },
        );
        nodes.insert(
            "feature-a".to_string(),
            TreeNode {
                branch: "feature-a".to_string(),
                path: PathBuf::new(),
                parent: Some("main".to_string()),
                original_parent: None,
                children: vec![],
            },
        );
        nodes.insert(
            "feature-b".to_string(),
            TreeNode {
                branch: "feature-b".to_string(),
                path: PathBuf::new(),
                parent: Some("main".to_string()),
                original_parent: None,
                children: vec![],
            },
        );

        let tree = DependencyTree {
            root: "main".to_string(),
            nodes,
        };

        let order = tree.topological_order();
        assert_eq!(order.len(), 2);
        // Both should appear, order is children sorted alphabetically
        assert!(order.contains(&"feature-a"));
        assert!(order.contains(&"feature-b"));
    }

    #[test]
    fn test_stack_containing_middle_branch() {
        let mut nodes = HashMap::new();
        nodes.insert(
            "main".to_string(),
            TreeNode {
                branch: "main".to_string(),
                path: PathBuf::new(),
                parent: None,
                original_parent: None,
                children: vec!["pr1".to_string(), "feature-x".to_string()],
            },
        );
        nodes.insert(
            "pr1".to_string(),
            TreeNode {
                branch: "pr1".to_string(),
                path: PathBuf::new(),
                parent: Some("main".to_string()),
                original_parent: None,
                children: vec!["pr2".to_string()],
            },
        );
        nodes.insert(
            "pr2".to_string(),
            TreeNode {
                branch: "pr2".to_string(),
                path: PathBuf::new(),
                parent: Some("pr1".to_string()),
                original_parent: None,
                children: vec!["pr3".to_string()],
            },
        );
        nodes.insert(
            "pr3".to_string(),
            TreeNode {
                branch: "pr3".to_string(),
                path: PathBuf::new(),
                parent: Some("pr2".to_string()),
                original_parent: None,
                children: vec![],
            },
        );
        nodes.insert(
            "feature-x".to_string(),
            TreeNode {
                branch: "feature-x".to_string(),
                path: PathBuf::new(),
                parent: Some("main".to_string()),
                original_parent: None,
                children: vec![],
            },
        );

        let tree = DependencyTree {
            root: "main".to_string(),
            nodes,
        };

        // When on pr2, should get pr1, pr2, pr3 (the pr1 stack) but not feature-x
        let stack = tree.stack_containing("pr2");
        assert!(stack.contains(&"pr1"));
        assert!(stack.contains(&"pr2"));
        assert!(stack.contains(&"pr3"));
        assert!(!stack.contains(&"feature-x"));
        assert!(!stack.contains(&"main"));
    }
}
