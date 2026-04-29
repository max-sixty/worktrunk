//! Canonical assembly of hook template variables for switch and merge.
//!
//! Replaces ad-hoc `Vec<(&str, &str)>` reconstructions across `handle_switch`,
//! `worktree::switch`, and `merge`. Owns its strings — no `pr_number_buf`
//! borrow gymnastics — and emits the deprecated `worktree` alias for
//! `worktree_path` once, here, so call sites don't repeat that aliasing.
//!
//! The struct carries operation-context vars (`base` / `target` directional
//! pairs and `pr_*`) plus optional Active overrides (`worktree_path`,
//! `worktree_name`, `commit`, `short_commit`) for sites whose hooks should
//! reference an Active identity that differs from the execution worktree —
//! e.g., post-merge running in the destination but referencing the feature
//! worktree, or pre-switch on an existing destination.

use std::path::Path;

use worktrunk::path::to_posix_path;

use super::worktree::{SwitchBranchInfo, SwitchResult};

#[derive(Default, Debug)]
pub(crate) struct TemplateVars {
    base: Option<String>,
    base_worktree_path: Option<String>,
    target: Option<String>,
    target_worktree_path: Option<String>,
    /// Override the bare `worktree_path` (and the deprecated `worktree` alias).
    active_worktree_path: Option<String>,
    /// Override the bare `worktree_name`.
    active_worktree_name: Option<String>,
    /// Override the bare `commit`.
    active_commit: Option<String>,
    /// Override the bare `short_commit`.
    active_short_commit: Option<String>,
    pr_number: Option<String>,
    pr_url: Option<String>,
}

impl TemplateVars {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set `base` (source branch) and `base_worktree_path` from a path.
    pub fn with_base(mut self, branch: &str, worktree_path: &Path) -> Self {
        self.base = Some(branch.to_string());
        self.base_worktree_path = Some(to_posix_path(&worktree_path.to_string_lossy()));
        self
    }

    /// Set `base` and `base_worktree_path` from already-POSIX strings; either
    /// may be `None`. Mirrors the call sites that pull these straight off a
    /// `SwitchResult::Created`.
    pub fn with_base_strs(mut self, branch: Option<&str>, worktree_path: Option<&str>) -> Self {
        self.base = branch.map(str::to_owned);
        self.base_worktree_path = worktree_path.map(str::to_owned);
        self
    }

    /// Set `target` (destination branch).
    pub fn with_target(mut self, branch: &str) -> Self {
        self.target = Some(branch.to_string());
        self
    }

    /// Set `target_worktree_path` from a path.
    pub fn with_target_worktree_path(mut self, path: &Path) -> Self {
        self.target_worktree_path = Some(to_posix_path(&path.to_string_lossy()));
        self
    }

    /// Override the Active worktree identity. Sets `worktree_path` (and the
    /// deprecated `worktree` alias) plus `worktree_name`. Falls back to
    /// `"unknown"` for `worktree_name` when the path has no file name or the
    /// file name isn't UTF-8 — matches the pre-refactor merge.rs behavior so
    /// templates that reference `{{ worktree_name }}` keep rendering rather
    /// than failing on undefined.
    pub fn with_active_worktree(mut self, path: &Path) -> Self {
        self.active_worktree_path = Some(to_posix_path(&path.to_string_lossy()));
        self.active_worktree_name = Some(
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string(),
        );
        self
    }

    /// Override `commit`. Also sets `short_commit` to the first 7 characters
    /// when present (commit SHAs are ASCII hex, so the byte slice is safe).
    pub fn with_active_commit(mut self, commit: &str) -> Self {
        self.active_short_commit = commit.get(..7).map(str::to_owned);
        self.active_commit = Some(commit.to_string());
        self
    }

    /// Set `pr_number` and `pr_url` for `pr:N` / `mr:N` creations. Either may
    /// be `None`; both are emitted independently so partial state survives if
    /// callers ever route one without the other.
    pub fn with_pr(mut self, number: Option<u32>, url: Option<&str>) -> Self {
        self.pr_number = number.map(|n| n.to_string());
        self.pr_url = url.map(str::to_owned);
        self
    }

    /// Materialize as `(name, value)` pairs borrowing from `self`. Emits the
    /// deprecated `worktree` alias for `worktree_path` once, here.
    pub fn as_extra_vars(&self) -> Vec<(&str, &str)> {
        let mut out: Vec<(&str, &str)> = Vec::new();
        if let Some(v) = &self.base {
            out.push(("base", v));
        }
        if let Some(v) = &self.base_worktree_path {
            out.push(("base_worktree_path", v));
        }
        if let Some(v) = &self.target {
            out.push(("target", v));
        }
        if let Some(v) = &self.target_worktree_path {
            out.push(("target_worktree_path", v));
        }
        if let Some(v) = &self.active_worktree_path {
            out.push(("worktree_path", v));
            out.push(("worktree", v));
        }
        if let Some(v) = &self.active_worktree_name {
            out.push(("worktree_name", v));
        }
        if let Some(v) = &self.active_commit {
            out.push(("commit", v));
        }
        if let Some(v) = &self.active_short_commit {
            out.push(("short_commit", v));
        }
        if let Some(v) = &self.pr_number {
            out.push(("pr_number", v));
        }
        if let Some(v) = &self.pr_url {
            out.push(("pr_url", v));
        }
        out
    }

    /// Build the post-switch context (used by foreground execute, background
    /// hooks, and `--execute` template expansion).
    ///
    /// `target` matches the bare vars (the destination); `base` is the source
    /// — the branched-from for creates, the source worktree for existing
    /// switches. PR/MR identity propagates into post-* hooks.
    pub fn for_post_switch(
        result: &SwitchResult,
        branch_info: &SwitchBranchInfo,
        source_branch: &str,
        source_path: &str,
    ) -> Self {
        let mut vars = Self::new().with_target_worktree_path(result.path());
        if let Some(branch) = branch_info.branch.as_deref() {
            vars = vars.with_target(branch);
        }
        match result {
            SwitchResult::Created {
                base_branch,
                base_worktree_path,
                pr_number,
                pr_url,
                ..
            } => vars
                .with_base_strs(base_branch.as_deref(), base_worktree_path.as_deref())
                .with_pr(*pr_number, pr_url.as_deref()),
            SwitchResult::Existing { .. } | SwitchResult::AlreadyAt(_) => {
                let base = (!source_branch.is_empty()).then_some(source_branch);
                let path = (!source_path.is_empty()).then_some(source_path);
                vars.with_base_strs(base, path)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn empty_vars_produces_empty_slice() {
        let vars = TemplateVars::new();
        assert!(vars.as_extra_vars().is_empty());
    }

    #[test]
    fn directional_pairs_round_trip() {
        let vars = TemplateVars::new()
            .with_base("main", &PathBuf::from("/repo"))
            .with_target("feature")
            .with_target_worktree_path(&PathBuf::from("/repo.feature"));
        let pairs = vars.as_extra_vars();
        assert!(pairs.contains(&("base", "main")));
        assert!(pairs.contains(&("base_worktree_path", "/repo")));
        assert!(pairs.contains(&("target", "feature")));
        assert!(pairs.contains(&("target_worktree_path", "/repo.feature")));
    }

    #[test]
    fn active_worktree_emits_deprecated_alias() {
        let vars = TemplateVars::new().with_active_worktree(&PathBuf::from("/repo.feature"));
        let pairs = vars.as_extra_vars();
        // worktree_path is canonical; worktree is the deprecated alias.
        assert!(pairs.contains(&("worktree_path", "/repo.feature")));
        assert!(pairs.contains(&("worktree", "/repo.feature")));
        assert!(pairs.contains(&("worktree_name", "repo.feature")));
    }

    #[test]
    fn active_worktree_name_falls_back_to_unknown() {
        // Pre-refactor merge.rs used `unwrap_or("unknown")` for the name;
        // preserve that so `{{ worktree_name }}` templates keep rendering.
        let vars = TemplateVars::new().with_active_worktree(&PathBuf::from("/"));
        let pairs = vars.as_extra_vars();
        assert!(pairs.contains(&("worktree_name", "unknown")));
    }

    #[test]
    fn active_commit_derives_short_commit() {
        let vars = TemplateVars::new().with_active_commit("0123456789abcdef");
        let pairs = vars.as_extra_vars();
        assert!(pairs.contains(&("commit", "0123456789abcdef")));
        assert!(pairs.contains(&("short_commit", "0123456")));
    }

    #[test]
    fn active_commit_skips_short_when_too_short() {
        // SHAs are always >= 7 chars in practice, but `.get(..7)` returns None
        // for shorter strings rather than panicking — preserves merge.rs's
        // pre-refactor behavior.
        let vars = TemplateVars::new().with_active_commit("abc");
        let pairs = vars.as_extra_vars();
        assert!(pairs.iter().any(|(k, _)| *k == "commit"));
        assert!(!pairs.iter().any(|(k, _)| *k == "short_commit"));
    }

    #[test]
    fn pr_pair_independent() {
        let vars = TemplateVars::new().with_pr(Some(42), Some("https://example.test/pr/42"));
        let pairs = vars.as_extra_vars();
        assert!(pairs.contains(&("pr_number", "42")));
        assert!(pairs.contains(&("pr_url", "https://example.test/pr/42")));
    }

    #[test]
    fn with_base_strs_skips_none() {
        let vars = TemplateVars::new().with_base_strs(Some("main"), None);
        let pairs = vars.as_extra_vars();
        assert!(pairs.contains(&("base", "main")));
        assert!(!pairs.iter().any(|(k, _)| *k == "base_worktree_path"));
    }

    #[test]
    fn for_post_switch_created_with_pr() {
        let result = SwitchResult::Created {
            path: PathBuf::from("/repo.fork"),
            created_branch: false,
            base_branch: Some("main".to_string()),
            base_worktree_path: Some("/repo".to_string()),
            from_remote: None,
            pr_number: Some(42),
            pr_url: Some("https://example.test/pr/42".to_string()),
        };
        let info = SwitchBranchInfo {
            branch: Some("contributor/feature".to_string()),
            expected_path: None,
        };
        let vars = TemplateVars::for_post_switch(&result, &info, "", "");
        let pairs = vars.as_extra_vars();
        assert!(pairs.contains(&("base", "main")));
        assert!(pairs.contains(&("base_worktree_path", "/repo")));
        assert!(pairs.contains(&("target", "contributor/feature")));
        assert!(pairs.contains(&("target_worktree_path", "/repo.fork")));
        assert!(pairs.contains(&("pr_number", "42")));
        assert!(pairs.contains(&("pr_url", "https://example.test/pr/42")));
    }

    #[test]
    fn for_post_switch_existing_uses_source() {
        let result = SwitchResult::Existing {
            path: PathBuf::from("/repo.feature"),
        };
        let info = SwitchBranchInfo {
            branch: Some("feature".to_string()),
            expected_path: None,
        };
        let vars = TemplateVars::for_post_switch(&result, &info, "main", "/repo");
        let pairs = vars.as_extra_vars();
        assert!(pairs.contains(&("base", "main")));
        assert!(pairs.contains(&("base_worktree_path", "/repo")));
        assert!(pairs.contains(&("target", "feature")));
        assert!(pairs.contains(&("target_worktree_path", "/repo.feature")));
        assert!(!pairs.iter().any(|(k, _)| *k == "pr_number"));
    }

    #[test]
    fn for_post_switch_existing_skips_empty_source() {
        let result = SwitchResult::AlreadyAt(PathBuf::from("/repo.feature"));
        let info = SwitchBranchInfo {
            branch: Some("feature".to_string()),
            expected_path: None,
        };
        let vars = TemplateVars::for_post_switch(&result, &info, "", "");
        let pairs = vars.as_extra_vars();
        assert!(!pairs.iter().any(|(k, _)| *k == "base"));
        assert!(!pairs.iter().any(|(k, _)| *k == "base_worktree_path"));
        assert!(pairs.contains(&("target", "feature")));
    }

    #[test]
    fn for_post_switch_detached_omits_target_branch() {
        let result = SwitchResult::Existing {
            path: PathBuf::from("/repo.detached"),
        };
        let info = SwitchBranchInfo {
            branch: None,
            expected_path: None,
        };
        let vars = TemplateVars::for_post_switch(&result, &info, "", "");
        let pairs = vars.as_extra_vars();
        assert!(!pairs.iter().any(|(k, _)| *k == "target"));
        assert!(pairs.contains(&("target_worktree_path", "/repo.detached")));
    }
}
