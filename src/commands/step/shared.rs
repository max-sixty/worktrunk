//! Helpers shared between multiple step subcommands.
//!
//! - `print_dry_run` — used by `commit` and `squash` for `--dry-run` output.
//! - Copy-ignored discovery (`list_and_filter_ignored_entries` and friends) —
//!   used by `copy_ignored` and `promote`.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
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
/// Git decides both what is ignored and what `.worktreeinclude` selects:
/// [`list_ignored_entries`] runs once with git's standard exclude sources and,
/// when the include file exists, once more with that file as the only source,
/// and [`overlap`] keeps what both listings cover. Git applies gitignore rules,
/// so a pattern naming a path inside a wholly ignored directory
/// (`.cache/types/`) finds it, and a negation can't re-include a path inside a
/// selected directory. Like every git listing, both stop at a nested
/// repository: a path inside one belongs to that repository.
///
/// Inside a directory git lists whole, only a line that names a path
/// ([`names_path`]) selects anything: a third listing runs with just those
/// lines and the negations. A name matched at any depth (`.env`, `types/`)
/// selects only outside such directories, so `.env` copies the root `.env` and
/// every `.env` git lists on its own, but never a package's `.env` from
/// `node_modules/`. Claude Code, which defines `.worktreeinclude`, agrees on
/// path-naming lines; it opens a directory git lists whole from a line's text,
/// then applies every line inside it. Each include listing walks every
/// untracked directory, about 0.3 s per 100k ignored files with a warm cache.
/// [`Boundaries`] then drops configured excludes, built-in VCS and tool-state
/// directories, and other worktrees.
pub(super) fn list_and_filter_ignored_entries(
    worktree_path: &Path,
    context: &str,
    worktree_paths: &[PathBuf],
    exclude_patterns: &[String],
) -> anyhow::Result<Vec<(PathBuf, bool)>> {
    let ignored = list_ignored_entries(worktree_path, context, &["--exclude-standard"])?;
    let include_path = worktree_path.join(".worktreeinclude");
    let entries = match fs::read_to_string(&include_path) {
        Ok(lines) => {
            let included =
                list_ignored_entries(worktree_path, context, &["--exclude-from=.worktreeinclude"])?;
            let named = if lines.lines().any(names_path) {
                // A file, not `--exclude=` arguments, so git reads these lines
                // exactly as it reads `.worktreeinclude`.
                let naming: String = lines
                    .lines()
                    .filter(|line| names_path(line) || line.starts_with('!'))
                    .map(|line| format!("{line}\n"))
                    .collect();
                #[expect(
                    clippy::disallowed_methods,
                    reason = "production code; the disallowed-methods rule targets the test suite (#4692)"
                )]
                let dir = tempfile::tempdir().context("Failed to create a temporary directory")?;
                let naming_path = dir.path().join("worktreeinclude");
                fs::write(&naming_path, naming).context("Failed to write include lines")?;
                let exclude_from = format!("--exclude-from={}", naming_path.display());
                list_ignored_entries(worktree_path, context, &[&exclude_from])?
            } else {
                Vec::new()
            };
            overlap(&ignored, &included, &named)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ignored,
        Err(error) => {
            return Err(error).with_context(|| {
                format!("Failed to read {}", format_path_for_display(&include_path))
            });
        }
    };
    let boundaries = Boundaries::new(worktree_path, worktree_paths, exclude_patterns)?;
    Ok(entries
        .into_iter()
        .filter(|(path, is_dir)| !boundaries.excludes(path, *is_dir))
        .collect())
}

/// Whether an include line names a path rather than a name to match at any
/// depth: once trailing whitespace, a leading `/` and any leading `**/` are
/// dropped, a slash comes before its end. `.cache/types/` and
/// `**/.cache/types/` name paths; `.env`, `**/.env` and `node_modules/` don't.
fn names_path(line: &str) -> bool {
    let line = line.trim_end();
    if line.starts_with(['#', '!']) {
        return false;
    }
    let mut rest = line.strip_prefix('/').unwrap_or(line);
    while let Some(after) = rest.strip_prefix("**/") {
        rest = after;
    }
    rest.trim_end_matches('/').contains('/')
}

/// The ignored entries the include file selects, in git's byte order. The
/// include listing selects where it isn't inside a directory git lists whole:
/// an entry both listings name, and an ignored entry inside a directory it
/// names. Inside a directory git lists whole, what the path-naming lines
/// (`named`) match is selected, unless a selected directory already holds it.
fn overlap(
    ignored: &[(PathBuf, bool)],
    included: &[(PathBuf, bool)],
    named: &[(PathBuf, bool)],
) -> Vec<(PathBuf, bool)> {
    let (ignored, included, named) = (leaves(ignored), leaves(included), leaves(named));
    let inside =
        |path: &Path, set: &HashSet<&Path>| path.ancestors().skip(1).any(|dir| set.contains(dir));
    let ignored_paths: HashSet<&Path> = ignored.iter().map(|(path, _)| path.as_path()).collect();
    let selecting: HashSet<&Path> = included
        .iter()
        .map(|(path, _)| path.as_path())
        .filter(|path| !inside(path, &ignored_paths))
        .collect();
    let mut entries: Vec<(PathBuf, bool)> = included
        .iter()
        .filter(|(path, _)| ignored_paths.contains(path.as_path()))
        .chain(ignored.iter().filter(|(path, _)| inside(path, &selecting)))
        .chain(
            named
                .iter()
                .filter(|(path, _)| inside(path, &ignored_paths) && !inside(path, &selecting)),
        )
        .map(|entry| (*entry).clone())
        .collect();
    entries.sort_by(|(a, _), (b, _)| a.as_os_str().cmp(b.as_os_str()));
    entries
}

/// A listing's entries with no other entry beneath them. Besides each excluded
/// entry, `git ls-files -i --directory` lists every untracked directory that
/// holds only excluded entries (`d/` beside `d/f`): no pattern selected that
/// directory, its contents fill it.
fn leaves(entries: &[(PathBuf, bool)]) -> Vec<&(PathBuf, bool)> {
    let parents: HashSet<&Path> = entries
        .iter()
        .flat_map(|(path, _)| path.ancestors().skip(1))
        .collect();
    entries
        .iter()
        .filter(|(path, _)| !parents.contains(path.as_path()))
        .collect()
}

/// What discovery never selects: configured excludes, built-in VCS and
/// tool-state directories, and other worktrees.
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

    /// Whether discovery drops `path`: it is, or sits inside, something
    /// configured or built in, or it is a directory holding another worktree,
    /// which copying it whole would carry along.
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
            || (is_dir && self.others.iter().any(|wt| wt.starts_with(path)))
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

/// List untracked entries that an exclude source ignores, using git ls-files
///
/// Runs `git ls-files -z --ignored <excludes> -o --directory`, where `excludes`
/// is `--exclude-standard` (global excludes, `.gitignore` files and
/// `.git/info/exclude`), `--exclude-from=<file>`, or `--exclude=<pattern>`
/// lines. `--directory` lists a wholly ignored directory as one entry rather
/// than every file inside it.
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
    excludes: &[&str],
) -> anyhow::Result<Vec<(std::path::PathBuf, bool)>> {
    let args: Vec<&str> = ["ls-files", "-z", "--ignored"]
        .into_iter()
        .chain(excludes.iter().copied())
        .chain(["-o", "--directory"])
        .collect();
    let output = Cmd::new("git")
        .args(&args)
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

        let err = super::list_ignored_entries(test.root_path(), "test", &["--exclude-standard"])
            .unwrap_err();
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

        let entries = super::list_ignored_entries(root, "test", &["--exclude-standard"]).unwrap();
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

    /// A line names a path when a slash comes before its end past any leading
    /// `/` or `**/`; a name matched at any depth, a comment and a negation
    /// don't.
    #[test]
    fn include_line_names_path() {
        let cases = [
            (".cache/types/", true),
            ("/.cache/types", true),
            ("packages/*/.cache/types/", true),
            ("**/.cache/types/", true),
            (".env", false),
            ("/.env", false),
            ("**/.env", false),
            ("node_modules/", false),
            ("# .cache/types/", false),
            ("!.cache/types/", false),
        ];
        for (line, expected) in cases {
            assert_eq!(super::names_path(line), expected, "{line:?}");
        }
    }

    /// A path-naming match inside a directory git lists whole is dropped only
    /// when a selected directory already holds it. `foo/` selecting the
    /// untracked `packages/foo/` carries the ignored `packages/foo/.cache/`, so
    /// `packages/foo/.cache/types/` adds nothing; but `foo/` matching
    /// `node_modules/foo/` selects nothing inside the ignored `node_modules/`,
    /// so `node_modules/foo/types/` still counts.
    #[test]
    fn overlap_drops_only_entries_a_selection_holds() {
        let entries = |path: &str| vec![(std::path::PathBuf::from(path), true)];
        let selected = |ignored, included, named| {
            super::overlap(&entries(ignored), &entries(included), &entries(named))
        };
        assert_eq!(
            selected(
                "packages/foo/.cache",
                "packages/foo",
                "packages/foo/.cache/types"
            ),
            entries("packages/foo/.cache")
        );
        assert_eq!(
            selected("node_modules", "node_modules/foo", "node_modules/foo/types"),
            entries("node_modules/foo/types")
        );
    }
}
