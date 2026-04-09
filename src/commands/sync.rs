//! `wt sync` — rebase stacked worktree branches in dependency order.
//!
//! Detects the branch dependency tree from git's commit graph using pairwise
//! merge-base analysis, then rebases each branch onto its parent in topological
//! order. Handles integrated (merged) branches by reparenting their children
//! with `rebase --onto`.
//!
//! Key behaviors:
//! - No configuration needed — dependencies are inferred from git history
//! - By default, only syncs the stack containing the current branch
//! - `--all` syncs all worktree branches
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
    pub all: bool,
    pub dry_run: bool,
}

/// Build the dependency tree from worktree branches.
///
/// For each branch B, finds the closest parent P where merge_base(P, B) is
/// nearest to B's tip (fewest commits ahead). Integrated branches are excluded
/// and their children reparented.
fn build_dependency_tree(repo: &Repository) -> anyhow::Result<DependencyTree> {
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

    // Check for integrated branches
    let integration_target = repo.integration_target();
    let target_ref = integration_target.as_deref().unwrap_or(&default_branch);

    let mut integrated: HashMap<String, ()> = HashMap::new();
    for (branch, _) in &branches {
        if branch == &default_branch {
            continue;
        }
        let (_, reason) = repo.integration_reason(branch, target_ref)?;
        if reason.is_some() {
            integrated.insert(branch.clone(), ());
        }
    }

    // Compute closest parent for each branch using ALL branches (including
    // integrated ones). This ensures children of integrated branches initially
    // get the integrated branch as their parent, so reparenting can detect and
    // apply `rebase --onto`.
    //
    // For each branch B, the parent P is selected in two tiers:
    //   1. True ancestors (candidate_depth == 0, meaning merge_base == candidate
    //      tip): these are branches whose tip is reachable from B. Among true
    //      ancestors, pick the closest (smallest branch_depth).
    //   2. Diverged candidates: pick by smallest branch_depth, then smallest
    //      candidate_depth.
    //
    // This prevents cycles in stacked branches: if B descends from C (C's tip
    // is on B's history), C is a true ancestor and always wins over siblings
    // that merely share a common fork point.
    let branch_names: Vec<&str> = branches.iter().map(|(b, _)| b.as_str()).collect();
    let mut parent_map: HashMap<String, (String, Option<String>)> = HashMap::new(); // branch -> (parent, original_parent_if_reparented)

    for (branch, _) in &branches {
        if branch == &default_branch || integrated.contains_key(branch) {
            continue;
        }

        // Partition into true ancestors and diverged candidates
        let mut ancestors: Vec<(&str, String, usize)> = Vec::new(); // (candidate, mb, branch_depth)
        let mut diverged: Vec<(&str, String, usize, usize)> = Vec::new(); // (candidate, mb, branch_depth, candidate_depth)

        for candidate in &branch_names {
            if *candidate == branch.as_str() {
                continue;
            }

            let Some(mb) = repo.merge_base(candidate, branch)? else {
                continue;
            };

            let branch_depth = repo.count_commits(&mb, branch)?;

            // Skip descendants: if branch_depth == 0, the branch's tip is the
            // merge-base, meaning branch is fully contained in candidate's
            // history. Candidate is a child, not a parent.
            if branch_depth == 0 {
                continue;
            }

            let candidate_depth = repo.count_commits(&mb, candidate)?;

            if candidate_depth == 0 {
                // True ancestor: candidate's tip IS the merge-base
                ancestors.push((candidate, mb, branch_depth));
            } else {
                diverged.push((candidate, mb, branch_depth, candidate_depth));
            }
        }

        // Prefer true ancestors, then diverged candidates
        let mut best_parent: Option<&str> = None;
        let mut tie_candidates: Vec<(&str, String)> = Vec::new();

        if !ancestors.is_empty() {
            // Among true ancestors, pick the closest (smallest branch_depth)
            ancestors.sort_by_key(|&(_, _, bd)| bd);
            let best_bd = ancestors[0].2;
            tie_candidates = ancestors
                .iter()
                .filter(|&&(_, _, bd)| bd == best_bd)
                .map(|&(c, ref mb, _)| (c, mb.clone()))
                .collect();
            best_parent = Some(ancestors[0].0);
        } else if !diverged.is_empty() {
            // Among diverged, sort by (branch_depth, candidate_depth)
            diverged.sort_by_key(|&(_, _, bd, cd)| (bd, cd));
            let (best_bd, best_cd) = (diverged[0].2, diverged[0].3);
            tie_candidates = diverged
                .iter()
                .filter(|&&(_, _, bd, cd)| bd == best_bd && cd == best_cd)
                .map(|&(c, ref mb, _, _)| (c, mb.clone()))
                .collect();
            best_parent = Some(diverged[0].0);
        }

        // Handle tie-breaking by merge-base timestamp
        if tie_candidates.len() > 1 {
            let mb_shas: Vec<&str> = tie_candidates.iter().map(|(_, mb)| mb.as_str()).collect();
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

    // Reparent children of integrated branches
    // If branch X's parent was integrated, reparent X to the integrated branch's parent
    for (_branch, (parent, original_parent)) in parent_map.iter_mut() {
        if integrated.contains_key(parent.as_str()) {
            // The parent was integrated — find what the integrated branch's parent would have been
            // Since the integrated branch is gone, reparent to the default branch
            let old_parent = parent.clone();
            *parent = default_branch.clone();
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

    Ok(DependencyTree {
        root: default_branch,
        nodes,
    })
}

/// Execute the sync operation.
pub fn handle_sync(opts: SyncOptions) -> anyhow::Result<()> {
    let repo = Repository::current()?;

    // Build dependency tree
    let tree = build_dependency_tree(&repo)?;

    // Determine which branches to sync
    let current_wt = repo.current_worktree();
    let current_branch = current_wt.branch()?;

    let branches_to_sync: Vec<&str> = if opts.all {
        tree.topological_order()
    } else {
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
