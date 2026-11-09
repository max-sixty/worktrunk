//! Helpers shared between multiple step subcommands.
//!
//! - `print_dry_run` — used by `commit` and `squash` for `--dry-run` output.
//! - Copy-ignored discovery (`list_and_filter_ignored_entries` and friends) —
//!   used by `copy_ignored` and `promote`.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use globset::{GlobBuilder, GlobMatcher};
use ignore::gitignore::GitignoreBuilder;
use worktrunk::config::CopyIgnoredConfig;
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;
use worktrunk::shell_exec::Cmd;
use worktrunk::styling::{format_bash_with_gutter, format_heading, format_with_gutter};

use super::super::commit::CommitGenerator;

/// Print the three dry-run sections: rendered prompt, LLM command, generated message.
///
/// The COMMAND and MESSAGE sections use the same gutter treatment as the regular commit
/// flow — `format_bash_with_gutter` for the shell invocation, and the bold-first-line
/// commit message format wrapped in `format_with_gutter`. The PROMPT is left ungutter'd
/// to keep `--dry-run`'s output visually aligned with `--show-prompt`.
///
/// Output is routed through the pager when stdout is a TTY (see writing-user-outputs →
/// "When to page output"). The helper falls back to direct stdout when piped.
pub(super) fn print_dry_run(
    prompt: &str,
    commit_config: &worktrunk::config::CommitGenerationConfig,
    message: &str,
) -> anyhow::Result<()> {
    let command_block = match commit_config.command() {
        Some(cmd) => format_bash_with_gutter(&crate::llm::render_llm_invocation(cmd)?),
        None => format_with_gutter("(LLM not configured — using built-in fallback)", None),
    };
    let formatted = CommitGenerator::new(commit_config, None).format_message_for_display(message);
    let out = format!(
        "{prompt_heading}\n{prompt}\n\n{command_heading}\n{command_block}\n\n{message_heading}\n{message_block}\n",
        prompt_heading = format_heading("PROMPT", None),
        command_heading = format_heading("COMMAND", None),
        message_heading = format_heading("MESSAGE", None),
        message_block = format_with_gutter(&formatted, None),
    );

    crate::help_pager::show_help_in_pager(&out, true)?;
    Ok(())
}

/// Built-in excludes for `wt step copy-ignored`: VCS metadata + tool-state directories.
///
/// VCS directories contain internal state tied to a specific working directory.
/// Git's own `.git` is implicitly excluded (git ls-files never reports it), but
/// other VCS tools colocated with git need explicit exclusion. Tool-state
/// directories (`.conductor/`, `.worktrees/`, etc.) are project-local state that
/// shouldn't be shared between worktrees.
pub(super) const BUILTIN_COPY_IGNORED_EXCLUDES: &[&str] = &[
    ".bzr/",
    ".conductor/",
    ".entire/",
    ".hg/",
    ".jj/",
    ".pijul/",
    ".sl/",
    ".svn/",
    ".worktrees/",
];

fn default_copy_ignored_excludes() -> Vec<String> {
    BUILTIN_COPY_IGNORED_EXCLUDES
        .iter()
        .map(|s| (*s).to_string())
        .collect()
}

/// Resolve the full copy-ignored config by merging built-in defaults, project
/// config (`.config/wt.toml`), and user config (global + per-project overrides).
pub(super) fn resolve_copy_ignored_config(repo: &Repository) -> anyhow::Result<CopyIgnoredConfig> {
    let mut config = CopyIgnoredConfig {
        exclude: default_copy_ignored_excludes(),
    };
    let user_config = repo.user_config();
    // No `.context()`: `project_config()` already wraps the load failure with
    // exactly that header (see `handle_hook_show`).
    let project_config = repo.project_config()?;
    if let Some(project_config) = project_config
        && let Some(project_copy_ignored) = project_config.copy_ignored()
    {
        config = config.merged_with(project_copy_ignored);
    }
    let project_id = repo.project_identifier().ok();
    config = config.merged_with(&user_config.copy_ignored(project_id.as_ref()));
    Ok(config)
}

/// List gitignored entries in a worktree, filtered by `.worktreeinclude` and excluding
/// configured patterns, VCS metadata directories, and other worktrees.
///
/// 1. `list_ignored_entries()` — git ls-files for ignored entries
/// 2. [`Boundaries`] — skip configured excludes, built-in VCS and tool-state
///    directories, and other worktrees, along with anything inside them
/// 3. `.worktreeinclude` — when the file exists, keep only what it selects,
///    including paths inside a directory git lists whole (see [`IncludePatterns`])
pub(super) fn list_and_filter_ignored_entries(
    worktree_path: &Path,
    context: &str,
    worktree_paths: &[PathBuf],
    exclude_patterns: &[String],
) -> anyhow::Result<Vec<(PathBuf, bool)>> {
    let ignored_entries = list_ignored_entries(worktree_path, context)?;
    let boundaries = Boundaries::new(worktree_path, worktree_paths, exclude_patterns)?;

    let include_path = worktree_path.join(".worktreeinclude");
    if !include_path.exists() {
        return Ok(ignored_entries
            .into_iter()
            .filter(|(path, is_dir)| {
                !boundaries.excludes(path, *is_dir) && !(*is_dir && boundaries.holds_worktree(path))
            })
            .collect());
    }
    let include = IncludePatterns::load(worktree_path, &include_path)?;
    let mut selected = Vec::new();
    for (path, is_dir) in ignored_entries {
        include.select(&path, is_dir, &boundaries, &mut selected)?;
    }
    Ok(selected)
}

/// What discovery never selects or walks into: configured excludes, built-in
/// VCS and tool-state directories, and other worktrees.
struct Boundaries<'a> {
    worktree_path: &'a Path,
    /// Every other worktree, except one the current worktree sits inside.
    others: Vec<&'a Path>,
    configured: Option<ignore::gitignore::Gitignore>,
}

impl<'a> Boundaries<'a> {
    fn new(
        worktree_path: &'a Path,
        worktree_paths: &'a [PathBuf],
        exclude_patterns: &[String],
    ) -> anyhow::Result<Self> {
        let configured = if exclude_patterns.is_empty() {
            None
        } else {
            let mut builder = GitignoreBuilder::new(worktree_path);
            for pattern in exclude_patterns {
                builder.add_line(None, pattern).map_err(|error| {
                    anyhow::anyhow!(
                        "Invalid [step.copy-ignored].exclude pattern {:?}: {}",
                        pattern,
                        error
                    )
                })?;
            }
            Some(
                builder
                    .build()
                    .context("Failed to build copy-ignored exclude matcher")?,
            )
        };
        let others = worktree_paths
            .iter()
            .map(PathBuf::as_path)
            .filter(|wt| *wt != worktree_path && !worktree_path.starts_with(wt))
            .collect();
        Ok(Self {
            worktree_path,
            others,
            configured,
        })
    }

    /// Whether `path` is, or sits inside, something discovery skips.
    fn excludes(&self, path: &Path, is_dir: bool) -> bool {
        let relative = relative_entry(path, self.worktree_path);
        let is_builtin = |name: &OsStr| {
            BUILTIN_COPY_IGNORED_EXCLUDES
                .iter()
                .any(|pat| name.to_str() == Some(pat.trim_end_matches('/')))
        };
        self.configured.as_ref().is_some_and(|matcher| {
            matcher
                .matched_path_or_any_parents(relative, is_dir)
                .is_ignore()
        }) || relative
            .parent()
            .is_some_and(|parent| parent.iter().any(is_builtin))
            || (is_dir && relative.file_name().is_some_and(is_builtin))
            || self.others.iter().any(|wt| path.starts_with(wt))
    }

    /// Whether `dir` holds another worktree, which copying `dir` whole would
    /// carry along.
    fn holds_worktree(&self, dir: &Path) -> bool {
        self.others.iter().any(|wt| wt.starts_with(dir))
    }
}

/// A `.worktreeinclude` file: its matcher, and the anchored patterns that can
/// name a path inside a directory `git ls-files --directory` lists whole.
///
/// Git lists a wholly ignored directory as one entry, so a pattern naming a
/// path inside it, such as `.cache/types/` under an ignored `.cache/`, matches
/// no entry git lists. [`IncludePatterns::select`] looks inside such a
/// directory for what those patterns name, as Claude Code does for the same
/// file. Only an anchored pattern leads there: one with a slash before its end
/// whose first component isn't `**`. A pattern that matches at any depth
/// (`.env`, `**/.env`) applies to the entries git lists, so an include file
/// never walks a whole ignored `node_modules/` or `target/` to look for one.
///
/// Selection follows `.gitignore` rules: a path inside a selected directory
/// is selected, and a negation can't take it back out, however git happened
/// to list it. So negations never need a walk.
struct IncludePatterns {
    root: PathBuf,
    matcher: ignore::gitignore::Gitignore,
    anchored: Vec<AnchoredPattern>,
}

impl IncludePatterns {
    fn load(worktree_path: &Path, include_path: &Path) -> anyhow::Result<Self> {
        let mut builder = GitignoreBuilder::new(worktree_path);
        if let Some(err) = builder.add(include_path) {
            // The `ignore` crate formats the path with OS-native separators;
            // normalize to forward slashes for consistent display.
            return Err(worktrunk::git::GitError::WorktreeIncludeParseError {
                error: err.to_string().replace('\\', "/"),
            }
            .into());
        }
        let matcher = builder.build().context("Failed to build include matcher")?;
        let contents = fs::read_to_string(include_path)
            .with_context(|| format!("Failed to read {}", format_path_for_display(include_path)))?;
        let anchored = contents
            .lines()
            .filter_map(AnchoredPattern::parse)
            .collect();
        Ok(Self {
            root: worktree_path.to_path_buf(),
            matcher,
            anchored,
        })
    }

    /// Push the part of an ignored entry this file selects: the entry itself,
    /// or, for a directory it doesn't select whole, what an anchored pattern
    /// selects inside it. A directory holding another worktree is never pushed
    /// whole. Everything inside an ignored directory is ignored too, so the
    /// walk needs no further word from git.
    fn select(
        &self,
        path: &Path,
        is_dir: bool,
        boundaries: &Boundaries,
        out: &mut Vec<(PathBuf, bool)>,
    ) -> anyhow::Result<()> {
        if boundaries.excludes(path, is_dir) {
            return Ok(());
        }
        let relative = relative_entry(path, &self.root);
        if !(is_dir && boundaries.holds_worktree(path)) && self.selects(relative, is_dir) {
            out.push((path.to_path_buf(), is_dir));
            return Ok(());
        }
        if !is_dir
            || !self
                .anchored
                .iter()
                .any(|pattern| pattern.reaches_below(relative))
        {
            return Ok(());
        }
        // Directories, files and symlinks, as `git ls-files` would list them;
        // a socket or FIFO holds nothing to copy.
        let mut children = fs::read_dir(path)
            .and_then(|entries| {
                entries
                    .filter_map(|child| {
                        let entry = child.and_then(|child| {
                            let kind = child.file_type()?;
                            Ok((kind.is_dir() || kind.is_file() || kind.is_symlink())
                                .then(|| (child.path(), kind.is_dir())))
                        });
                        entry.transpose()
                    })
                    .collect::<std::io::Result<Vec<_>>>()
            })
            .with_context(|| format!("Failed to read {}", format_path_for_display(path)))?;
        children.sort();
        for (child, child_is_dir) in children {
            self.select(&child, child_is_dir, boundaries, out)?;
        }
        Ok(())
    }

    /// Whether the file selects `relative`, by `.gitignore` rules: it or one of
    /// its parent directories matches, and a negation can't re-include a path
    /// whose parent is selected.
    fn selects(&self, relative: &Path, is_dir: bool) -> bool {
        relative
            .ancestors()
            .skip(1)
            .filter(|parent| !parent.as_os_str().is_empty())
            .any(|parent| self.matcher.matched(parent, true).is_ignore())
            || self.matcher.matched(relative, is_dir).is_ignore()
    }
}

/// An anchored `.worktreeinclude` pattern (see [`IncludePatterns`]), as one
/// glob per path component; `None` stands for `**`.
struct AnchoredPattern(Vec<Option<GlobMatcher>>);

impl AnchoredPattern {
    /// Parse one `.worktreeinclude` line, or `None` for a line that leads into
    /// no ignored directory: blank, a comment, a negation, or a pattern that
    /// matches at any depth.
    fn parse(line: &str) -> Option<Self> {
        // As in a `.gitignore`, trailing spaces drop unless a backslash escapes
        // the last one.
        let trimmed = line.trim_end_matches(' ');
        let line = if trimmed.ends_with('\\') && line.len() > trimmed.len() {
            &line[..=trimmed.len()]
        } else {
            trimmed
        };
        if line.is_empty() || line.starts_with(['#', '!']) {
            return None;
        }
        let pattern = line.strip_prefix('/').unwrap_or(line).trim_end_matches('/');
        let parts: Vec<&str> = pattern.split('/').collect();
        if parts.len() < 2 || parts[0] == "**" {
            return None;
        }
        parts
            .into_iter()
            .map(|part| match part {
                "**" => Some(None),
                _ => GlobBuilder::new(part)
                    .literal_separator(true)
                    .backslash_escape(true)
                    .build()
                    .ok()
                    .map(|glob| Some(glob.compile_matcher())),
            })
            .collect::<Option<_>>()
            .map(Self)
    }

    /// Whether this pattern can match a path below the directory `dir`,
    /// relative to the worktree: each of `dir`'s components matches the
    /// pattern's at the same depth and the pattern goes deeper, or a `**`
    /// comes first.
    fn reaches_below(&self, dir: &Path) -> bool {
        let mut parts = self.0.iter();
        for component in dir.iter() {
            match parts.next() {
                None => return false,
                Some(None) => return true,
                Some(Some(glob)) if !glob.is_match(component) => return false,
                Some(Some(_)) => {}
            }
        }
        parts.next().is_some()
    }
}

/// An entry's path relative to its worktree, for gitignore matching.
///
/// Matching walks the entry's parents (`matched_path_or_any_parents`) because
/// `git ls-files --directory` lists an ignored file individually when its
/// directory also holds tracked files, so a `config/` pattern has to reach
/// `config/local.yml` through its parent, as it would in a `.gitignore`.
/// Every entry is `worktree_path.join(..)` (see `list_ignored_entries`), so the
/// strip always succeeds.
fn relative_entry<'a>(path: &'a Path, worktree_path: &Path) -> &'a Path {
    path.strip_prefix(worktree_path).unwrap_or(path)
}

/// List ignored entries using git ls-files
///
/// Uses `git ls-files -z --ignored --exclude-standard -o --directory` which:
/// - Handles all gitignore sources (global, .gitignore, .git/info/exclude, nested)
/// - Stops at directory boundaries (--directory) to avoid listing thousands of files
///
/// `-z` NUL-terminates the entries and, crucially, disables path quoting. Without
/// it, `core.quotePath` (git's default) renders a non-ASCII name like
/// `data/Report — Q1.txt` as `"data/Report \342\200\224 Q1.txt"` — surrounding
/// quotes and octal escapes included — which is then treated as a literal path and
/// never found on disk. NUL splitting also sidesteps the newline-in-filename edge
/// case that line splitting can't represent.
fn list_ignored_entries(
    worktree_path: &Path,
    context: &str,
) -> anyhow::Result<Vec<(std::path::PathBuf, bool)>> {
    let args = [
        "ls-files",
        "-z",
        "--ignored",
        "--exclude-standard",
        "-o",
        "--directory",
    ];
    let output = Cmd::new("git")
        .args(args)
        .current_dir(worktree_path)
        .context(context)
        .scrub_git_discovery_env()
        .run()
        .context("Failed to run git ls-files")?;

    if !output.status.success() {
        return Err(worktrunk::git::CommandError::from_failed_output("git", &args, &output).into());
    }

    // Git's -z output contains path bytes, which need not be UTF-8 on Unix.
    let entries = output
        .stdout
        .split(|&byte| byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let (relative, is_dir) = match entry.strip_suffix(b"/") {
                Some(relative) => (relative, true),
                None => (entry, false),
            };
            (worktree_path.join(path_from_git_bytes(relative)), is_dir)
        })
        .collect();

    Ok(entries)
}

#[cfg(unix)]
fn path_from_git_bytes(bytes: &[u8]) -> PathBuf {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    OsString::from_vec(bytes.to_vec()).into()
}

#[cfg(not(unix))]
fn path_from_git_bytes(bytes: &[u8]) -> PathBuf {
    String::from_utf8_lossy(bytes).into_owned().into()
}

#[cfg(test)]
mod tests {
    use worktrunk::git::CommandError;
    use worktrunk::testing::TestRepo;

    /// A hard `git ls-files` failure (here: a fatal config error) must
    /// surface as a typed `CommandError`.
    #[test]
    fn list_ignored_entries_failure_is_command_error() {
        let test = TestRepo::with_initial_commit();
        std::fs::write(test.root_path().join(".git/config"), "[bad\n").unwrap();

        let err = super::list_ignored_entries(test.root_path(), "test").unwrap_err();
        let cmd_err = CommandError::find_in(&err).expect("error should carry a CommandError");
        assert!(cmd_err.command_string().starts_with("git ls-files"));
    }

    /// An ignored file whose name contains non-ASCII characters must be
    /// discovered with a real filesystem path. With `core.quotePath` (git's
    /// default), `git ls-files` renders such a name as `"data/Report
    /// \342\200\224 Q1.txt"` — quotes and octal escapes included — which does
    /// not exist on disk. Discovery must return the actual path.
    #[test]
    fn list_ignored_entries_handles_non_ascii_names() {
        let test = TestRepo::with_initial_commit();
        let root = test.root_path();

        // A tracked file in data/ keeps `--directory` from collapsing the
        // directory into a single entry, so the individual filename is listed.
        std::fs::create_dir(root.join("data")).unwrap();
        std::fs::write(root.join("data/keep.md"), "tracked").unwrap();
        std::fs::write(root.join(".gitignore"), "*.txt\n").unwrap();
        test.run_git(&["add", "-A"]);
        test.run_git(&["commit", "-m", "add tracked"]);

        // Ignored file with an em dash (U+2014) in its name.
        let ignored_name = "Report — Q1.txt";
        let ignored_path = root.join("data").join(ignored_name);
        std::fs::write(&ignored_path, "x").unwrap();

        let entries = super::list_ignored_entries(root, "test").unwrap();
        let (path, is_dir) = entries
            .iter()
            .find(|(path, _)| path.file_name().and_then(|n| n.to_str()) == Some(ignored_name))
            .expect("non-ASCII ignored file should be discovered");
        assert!(!is_dir);
        assert!(
            path.exists(),
            "discovered path does not exist on disk: {path:?}"
        );
    }

    /// Which include lines lead below an ignored directory: each of the
    /// directory's components must match the pattern's glob at that depth, a
    /// `**` reaches any depth below, and a line that matches at any depth, or
    /// negates, leads nowhere.
    #[test]
    fn anchored_pattern_reaches_below() {
        let reaches = |line: &str, dir: &str| {
            super::AnchoredPattern::parse(line)
                .is_some_and(|pattern| pattern.reaches_below(std::path::Path::new(dir)))
        };
        assert!(reaches("/.cache/types/", ".cache"));
        assert!(!reaches(".cache/types", "node_modules"));
        assert!(!reaches(".cache/types", ".cache/types"));
        assert!(reaches(".cach?/types", ".cache"));
        assert!(reaches(r"\#cache/types", "#cache"));
        assert!(reaches(".cache/*/types", ".cache/claude"));
        assert!(!reaches(".cache/*/types", ".cache/claude/other"));
        assert!(reaches(".cache/**/types", ".cache/a/b/c"));
        assert!(!reaches(".cache/wanted*/**/types", ".cache/unrelated"));
        assert!(reaches("**cache/types", ".cache"));
        assert!(reaches(r"cache/file\ ", "cache"));
        assert!(reaches("cache/a\\\\\u{2003}", "cache"));
        assert!(!reaches("**/types/x", ".cache"));
        assert!(!reaches("types/", ".cache"));
        assert!(!reaches("!.cache/types/", ".cache"));
    }

    /// Selection follows `.gitignore` rules whichever way git listed a path: a
    /// negation can't re-include a path inside a selected directory, but can
    /// keep out one whose parent only matches through a wildcard child pattern.
    #[test]
    fn include_selects_by_gitignore_parent_rule() {
        let root = std::path::Path::new("/worktree");
        let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
        for line in [
            ".cache/types/",
            "!.cache/types/private/",
            "cfg/*",
            "!cfg/keep/",
        ] {
            builder.add_line(None, line).unwrap();
        }
        let include = super::IncludePatterns {
            root: root.to_path_buf(),
            matcher: builder.build().unwrap(),
            anchored: Vec::new(),
        };
        let selects = |path: &str| include.selects(std::path::Path::new(path), false);
        assert!(selects(".cache/types/private/secret"));
        assert!(selects("cfg/other"));
        assert!(!selects("cfg/keep/file"));
    }
}
