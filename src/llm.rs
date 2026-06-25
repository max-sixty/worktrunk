use anyhow::Context;
use color_print::cformat;
use shell_escape::unix::escape;
use std::borrow::Cow;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use worktrunk::config::CommitGenerationConfig;
use worktrunk::git::{CommitMessageDetail, Repository};
use worktrunk::path::format_path_for_display;
use worktrunk::shell_exec::{Cmd, ShellConfig};
use worktrunk::styling::{eprintln, warning_message};

use minijinja::Environment;
use minijinja::value::{Enumerator, Object, Value};

/// minijinja view of one squashed commit, exposed to squash templates as an
/// element of `commit_details`.
///
/// It renders as its bare subject (`{{ detail }}` yields the subject line) so a
/// template that iterates the list and prints the loop variable directly
/// behaves exactly like the deprecated `commits` list of subject strings. That
/// equivalence is what lets `wt config update` migrate a `commits` template to
/// `commit_details` as a plain identifier rename — no shape-changing hand edits
/// (see #2984). The `.subject` and `.body` properties remain available for
/// templates that want the structured form, and because minijinja coerces an
/// object to a string via its `render`, string filters (`{{ c | upper }}`)
/// operate on the subject too.
#[derive(Debug)]
struct CommitDetailValue {
    subject: String,
    body: String,
}

impl Object for CommitDetailValue {
    // `repr` is left at the trait default (`ObjectRepr::Map`).

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str()? {
            "subject" => Some(Value::from(self.subject.clone())),
            "body" => Some(Value::from(self.body.clone())),
            _ => None,
        }
    }

    // Report the two keys so the object is non-empty: with the default
    // `Map`-repr enumerator (`Empty`) the object would be falsy in `{% if c %}`,
    // breaking the equivalence with the old non-empty subject string.
    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Str(&["subject", "body"])
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.subject)
    }
}

/// Characters that require shell wrapping when used in a command.
/// If a command contains any of these, it needs `sh -c '...'` to execute correctly.
const SHELL_METACHARACTERS: &[char] = &[
    '&', '|', ';', '<', '>', '$', '`', '\'', '"', '(', ')', '{', '}', '*', '?', '[', ']', '~', '!',
    '\\',
];

/// Render the shell invocation worktrunk would use to run `command`.
///
/// Mirrors the wrapping done by [`execute_llm_command`]: every LLM command is passed as
/// a single argument to the platform shell, so the displayed form is always
/// `<shell> <shell-args> <quoted-command>` (e.g. `sh -c 'claude -p'`).
///
/// Uses the shell's basename rather than the full path so the displayed command stays
/// short and doesn't leak the user's install path (`C:\Program Files\Git\bin\bash.exe`
/// becomes `bash.exe`).
pub(crate) fn render_llm_invocation(command: &str) -> anyhow::Result<String> {
    let shell = ShellConfig::get()?;
    let mut rendered = shell
        .executable
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| shell.executable.to_string_lossy().into_owned());
    for arg in &shell.args {
        rendered.push(' ');
        rendered.push_str(arg);
    }
    rendered.push(' ');
    rendered.push_str(&escape(Cow::Borrowed(command)));
    Ok(rendered)
}

/// Start a "still waiting" watchdog for a *foreground* LLM shell-out.
///
/// [`execute_llm_command`] captures stdout, so a slow or hung command is
/// otherwise silent. The watchdog shows a dim status line that escalates to
/// reveal the exact invocation (via [`render_llm_invocation`]) in a gutter. The
/// caller holds the returned guard until the command returns; on drop it clears
/// the status before the result is printed.
///
/// Only for single, foreground calls — never the concurrent summary path
/// ([`generate_summary_core`](crate::summary::generate_summary_core), up to 8
/// under a semaphore), where
/// per-call spinners would interleave.
fn watch_llm_command(command: &str, waiting_for: &str) -> worktrunk::progress::Watchdog {
    let invocation = render_llm_invocation(command).ok();
    worktrunk::progress::Watchdog::start(waiting_for, invocation.as_deref())
}

/// Format a reproduction command, only wrapping with `sh -c` if needed.
///
/// Simple commands like `llm -m haiku` are shown as-is.
/// Complex commands with shell syntax are wrapped: `sh -c 'complex && command'`
fn format_reproduction_command(base_cmd: &str, llm_command: &str) -> String {
    let needs_shell = llm_command.contains(SHELL_METACHARACTERS)
        || llm_command
            .split_whitespace()
            .next()
            .is_some_and(|first| first.contains('='));

    if needs_shell {
        format!(
            "{} | sh -c {}",
            base_cmd,
            escape(Cow::Borrowed(llm_command))
        )
    } else {
        format!("{} | {}", base_cmd, llm_command)
    }
}

/// Track whether template-file deprecation warning has been shown this session
static TEMPLATE_FILE_WARNING_SHOWN: AtomicBool = AtomicBool::new(false);

/// Maximum diff size in characters before filtering kicks in
const DIFF_SIZE_THRESHOLD: usize = 400_000;

/// Maximum lines per file after truncation
const MAX_LINES_PER_FILE: usize = 50;

/// Maximum number of files to include after truncation
const MAX_FILES: usize = 50;

/// Lock file patterns that are filtered out when diff is too large
const LOCK_FILE_PATTERNS: &[&str] = &[".lock", "-lock.json", "-lock.yaml", ".lock.hcl"];

/// Prepared diff output with optional filtering applied
pub(crate) struct PreparedDiff {
    /// The diff content (possibly filtered/truncated)
    pub(crate) diff: String,
    /// The diffstat output
    pub(crate) stat: String,
}

/// Check if a filename matches lock file patterns
fn is_lock_file(filename: &str) -> bool {
    LOCK_FILE_PATTERNS
        .iter()
        .any(|pattern| filename.ends_with(pattern))
}

/// Parse a diff into individual file sections
///
/// Returns Vec of (filename, diff_content) pairs
fn parse_diff_sections(diff: &str) -> Vec<(&str, &str)> {
    let mut sections = Vec::new();
    let mut current_file: Option<&str> = None;
    let mut section_start_byte = 0;
    let mut current_byte = 0;

    // Iterate with `split_inclusive` so each chunk includes its line terminator
    // (`\n` or `\r\n`). Advancing by the chunk length keeps `current_byte` aligned
    // with real byte offsets — `str::lines()` strips terminators, so reconstructing
    // the offset as `line.len() + 1` under-counts by one byte per CRLF line and
    // can end up slicing inside a multi-byte UTF-8 character.
    for full_line in diff.split_inclusive('\n') {
        let line = full_line
            .strip_suffix("\r\n")
            .or_else(|| full_line.strip_suffix('\n'))
            .unwrap_or(full_line);
        if line.starts_with("diff --git ") {
            // Save previous section
            if let Some(file) = current_file
                && current_byte > section_start_byte
            {
                sections.push((file, &diff[section_start_byte..current_byte]));
            }

            // Extract filename from "diff --git a/path b/path"
            current_file = line.split(" b/").nth(1);
            section_start_byte = current_byte;
        }
        current_byte += full_line.len();
    }

    // Save final section
    if let Some(file) = current_file
        && section_start_byte < diff.len()
    {
        sections.push((file, &diff[section_start_byte..]));
    }

    sections
}

/// Truncate a diff section to max lines, keeping the header
fn truncate_diff_section(section: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = section.lines().collect();
    if lines.len() <= max_lines {
        return section.to_string();
    }

    // Find where the actual diff content starts (after the @@ line)
    let header_end = lines.iter().position(|l| l.starts_with("@@")).unwrap_or(0);
    let header_lines = header_end + 1; // Include the first @@ line

    let content_lines = max_lines.saturating_sub(header_lines);
    let total_lines = header_lines + content_lines;

    let mut result: String = lines
        .iter()
        .take(total_lines)
        .map(|l| format!("{}\n", l))
        .collect();
    let omitted = lines.len() - total_lines;
    if omitted > 0 {
        result.push_str(&format!("\n... ({} lines omitted)\n", omitted));
    }

    result
}

/// Prepare diff for LLM consumption, applying filtering if needed
pub(crate) fn prepare_diff(diff: String, stat: String) -> PreparedDiff {
    // If under threshold, pass through unchanged
    if diff.len() < DIFF_SIZE_THRESHOLD {
        return PreparedDiff { diff, stat };
    }

    tracing::debug!(
        count = diff.len(),
        threshold = DIFF_SIZE_THRESHOLD,
        "Diff size ({} chars) exceeds threshold ({}), filtering",
        diff.len(),
        DIFF_SIZE_THRESHOLD
    );

    // Step 1: Filter out lock files
    let sections = parse_diff_sections(&diff);
    let filtered_sections: Vec<_> = sections
        .iter()
        .filter(|(filename, _)| !is_lock_file(filename))
        .collect();

    let lock_files_removed = sections.len() - filtered_sections.len();
    if lock_files_removed > 0 {
        tracing::debug!(
            count = lock_files_removed,
            "Filtered out {} lock file(s)",
            lock_files_removed
        );
    }

    let filtered_diff: String = filtered_sections
        .iter()
        .map(|(_, content)| *content)
        .collect();

    // If filtering lock files brought us under threshold, we're done
    if filtered_diff.len() < DIFF_SIZE_THRESHOLD {
        return PreparedDiff {
            diff: filtered_diff,
            stat,
        };
    }

    // Step 2: Truncate each file and limit file count
    tracing::debug!(
        count = filtered_diff.len(),
        "Still too large ({} chars), truncating to {} lines/file, {} files max",
        filtered_diff.len(),
        MAX_LINES_PER_FILE,
        MAX_FILES
    );

    let truncated: String = filtered_sections
        .iter()
        .take(MAX_FILES)
        .map(|(_, content)| truncate_diff_section(content, MAX_LINES_PER_FILE))
        .collect();

    let files_omitted = filtered_sections.len().saturating_sub(MAX_FILES);
    let final_diff = if files_omitted > 0 {
        format!("{}\n... ({} files omitted)\n", truncated, files_omitted)
    } else {
        truncated
    };

    PreparedDiff {
        diff: final_diff,
        stat,
    }
}

/// Context data for building LLM prompts
///
/// All fields are available to both commit and squash templates.
/// Squash-specific fields (`commit_details`, `target_branch`) are empty/None for regular commits.
struct TemplateContext<'a> {
    /// The diff to describe (staged changes for commit, combined diff for squash)
    git_diff: &'a str,
    /// Diff statistics summary (output of git diff --stat)
    git_diff_stat: &'a str,
    /// Current branch name
    branch: &'a str,
    /// Recent commit subjects for style reference
    recent_commits: Option<&'a Vec<String>>,
    /// Repository name
    repo_name: &'a str,
    /// Subject/body details for commits being squashed (squash only)
    commit_details: &'a [CommitMessageDetail],
    /// Target branch for merge (squash only)
    target_branch: Option<&'a str>,
    /// Approved project-level append fragment. `None` when no project
    /// `template-append` is set or the user declined approval. The
    /// user-level append fragment is read from the [`CommitGenerationConfig`]
    /// directly (it needs no approval). Both fragments are themselves
    /// minijinja templates — `build_prompt` renders each one and exposes
    /// them as the `user_guidance` and `project_guidance` variables, which
    /// the default templates wrap in `<user-guidance>` / `<project-guidance>`
    /// blocks.
    project_append: Option<&'a str>,
}

/// Default template for commit message prompts
///
/// Synced to dev/config.example.toml by `cargo test readme_sync`
const DEFAULT_TEMPLATE: &str = r#"<task>Write a commit message for the staged changes below.</task>

<format>
- Subject line under 50 chars
- For material changes, add a blank line then a body paragraph explaining the change
- Output only the commit message, no quotes or code blocks
</format>

<style>
- Imperative mood: "Add feature" not "Added feature"
- Match recent commit style (conventional commits if used)
- Describe the change, not the intent or benefit
</style>
{% if user_guidance %}
<user-guidance>
{{ user_guidance }}
</user-guidance>
{% endif %}{% if project_guidance %}
<project-guidance>
{{ project_guidance }}
</project-guidance>
{% endif %}
<diffstat>
{{ git_diff_stat }}
</diffstat>

<diff>
{{ git_diff }}
</diff>

<context>
Branch: {{ branch }}
{% if recent_commits %}<recent_commits>
{% for commit in recent_commits %}- {{ commit }}
{% endfor %}</recent_commits>{% endif %}
</context>
"#;

/// Default template for squash commit message prompts
///
/// Synced to dev/config.example.toml by `cargo test readme_sync`
const DEFAULT_SQUASH_TEMPLATE: &str = r#"<task>Write a commit message for the combined effect of these commits.</task>

<format>
- Subject line under 50 chars
- For material changes, add a blank line then a body paragraph explaining the change
- Output only the commit message, no quotes or code blocks
</format>

<style>
- Imperative mood: "Add feature" not "Added feature"
- Match the style of commits being squashed (conventional commits if used)
- Describe the change, not the intent or benefit
</style>
{% if user_guidance %}
<user-guidance>
{{ user_guidance }}
</user-guidance>
{% endif %}{% if project_guidance %}
<project-guidance>
{{ project_guidance }}
</project-guidance>
{% endif %}
<commits branch="{{ branch }}" target="{{ target_branch }}">
{% for detail in commit_details %}- {{ detail.subject }}
{% endfor %}</commits>

<diffstat>
{{ git_diff_stat }}
</diffstat>

<diff>
{{ git_diff }}
</diff>
"#;

/// Execute an LLM command with the given prompt via stdin.
///
/// The command is a shell string executed via the platform shell (sh on Unix,
/// Git Bash on Windows), allowing environment variables to be set inline
/// (e.g., `MAX_THINKING_TOKENS=0 claude -p ...`).
///
/// This is the canonical way to execute LLM commands in this codebase.
/// All LLM execution should go through this function to maintain consistency.
pub(crate) fn execute_llm_command(command: &str, prompt: &str) -> anyhow::Result<String> {
    // TODO(diff-pipe): Consider splitting the prompt template around
    // `{{ git_diff }}` and piping `git diff` directly into the LLM via
    // `Cmd::pipe_into` (preamble + epilogue through env vars). Avoids buffering
    // MB-scale diffs in our process memory and removes them from our logs
    // entirely. See conversation around PR #2136 for sketch.

    let shell = ShellConfig::get()?;
    let output = Cmd::new(shell.executable.to_string_lossy())
        .args(&shell.args)
        .arg(command)
        .external("commit.generation")
        .stdin_bytes(prompt)
        .run()
        .context("Failed to spawn LLM command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        if stderr.is_empty() {
            // Fall back to stdout or exit code when stderr is empty
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stdout = stdout.trim();
            if stdout.is_empty() {
                anyhow::bail!(
                    "LLM command failed with exit code {}",
                    output.status.code().unwrap_or(-1)
                );
            } else {
                anyhow::bail!("{}", stdout);
            }
        } else {
            anyhow::bail!("{}", stderr);
        }
    }

    let message = String::from_utf8_lossy(&output.stdout).trim().to_owned();

    if message.is_empty() {
        return Err(worktrunk::git::GitError::Other {
            message: "LLM returned empty message".into(),
        }
        .into());
    }

    Ok(message)
}

/// Template type for selecting the appropriate template source
enum TemplateType {
    Commit,
    Squash,
}

/// Load template from inline, file, or default
fn load_template(
    inline: Option<&String>,
    file: Option<&String>,
    default: &str,
    file_type_name: &str,
) -> anyhow::Result<String> {
    match (inline, file) {
        (Some(inline), None) => Ok(inline.clone()),
        (None, Some(path)) => {
            // Show deprecation warning once per session
            if !TEMPLATE_FILE_WARNING_SHOWN.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "{}",
                    warning_message(format!(
                        "{file_type_name} is deprecated and will be removed in a future release. Use inline template instead. To request this feature, comment on: https://github.com/max-sixty/worktrunk/issues/444"
                    ))
                );
            }

            let expanded_path = PathBuf::from(shellexpand::tilde(path).as_ref());
            std::fs::read_to_string(&expanded_path).map_err(|e| {
                anyhow::Error::from(worktrunk::git::GitError::Other {
                    message: cformat!(
                        "Failed to read {} <bold>{}</>: {}",
                        file_type_name,
                        format_path_for_display(&expanded_path),
                        e
                    ),
                })
            })
        }
        (None, None) => Ok(default.to_string()),
        (Some(_), Some(_)) => {
            unreachable!(
                "Config validation should prevent both {} options",
                file_type_name
            )
        }
    }
}

/// Build prompt from template using minijinja
///
/// Template variables available to both commit and squash templates:
/// - `git_diff`: The diff to describe
/// - `branch`: Current branch name
/// - `recent_commits`: Recent commit subjects for style reference
/// - `repo`: Repository directory name
///
/// Squash-specific variables (empty for regular commits):
/// - `commit_details`: Commits being squashed. Each element renders as its
///   subject when printed bare and exposes `.subject` / `.body` properties.
/// - `commits`: Commit subjects being squashed (deprecated — see #2984;
///   `wt config update` rewrites it to `commit_details`)
/// - `target_branch`: Target branch for merge
fn build_prompt(
    config: &CommitGenerationConfig,
    template_type: TemplateType,
    context: &TemplateContext<'_>,
) -> anyhow::Result<String> {
    // Get template source based on type
    let (template, type_name) = match template_type {
        TemplateType::Commit => (
            load_template(
                config.template.as_ref(),
                config.template_file.as_ref(),
                DEFAULT_TEMPLATE,
                "template-file",
            )?,
            "Template",
        ),
        TemplateType::Squash => (
            load_template(
                config.squash_template.as_ref(),
                config.squash_template_file.as_ref(),
                DEFAULT_SQUASH_TEMPLATE,
                "squash-template-file",
            )?,
            "Squash template",
        ),
    };

    // Validate non-empty
    if template.trim().is_empty() {
        return Err(worktrunk::git::GitError::Other {
            message: format!("{} is empty", type_name),
        }
        .into());
    }

    // Render template with minijinja - all variables available to all templates
    let env = Environment::new();
    let tmpl = env.template_from_str(&template)?;

    // Reverse commits so they're in chronological order (oldest first).
    //
    // `commits` (a list of bare subject strings) is deprecated in favor of
    // `commit_details` (see #2984). The deprecation warning and the
    // `wt config update` rewrite both go through the standard config
    // deprecation framework (`DEPRECATED_VARS`), so nothing is detected or
    // warned here — `commits` is simply still rendered for templates that
    // haven't migrated yet. The rename is safe because each `commit_details`
    // element renders as its subject (see `CommitDetailValue`), so a migrated
    // `{% for c in commit_details %}{{ c }}` reads identically to the old
    // `{% for c in commits %}{{ c }}`.
    let commits_chronological: Vec<&String> = context
        .commit_details
        .iter()
        .rev()
        .map(|detail| &detail.subject)
        .collect();
    let commit_details_chronological: Vec<Value> = context
        .commit_details
        .iter()
        .rev()
        .map(|detail| {
            Value::from_object(CommitDetailValue {
                subject: detail.subject.clone(),
                body: detail.body.clone(),
            })
        })
        .collect();
    let empty_commits: Vec<String> = vec![];

    // The append fragments are themselves minijinja templates. Render each
    // one in its own pass with the same variable context so it doesn't share
    // scope with the parent template (and can't recursively reference
    // itself), then expose them separately as `user_guidance` /
    // `project_guidance` so the default templates can label each by
    // provenance. The user fragment needs no approval (it's the developer's
    // own config); the project fragment is gated upstream and arrives here
    // as `context.project_append` (or `None` if declined). Empty string when
    // a source is absent — the templates gate the block on truthiness.
    let render_fragment = |fragment: &str| -> anyhow::Result<String> {
        let frag_tmpl = env.template_from_str(fragment)?;
        Ok(frag_tmpl.render(minijinja::context! {
            git_diff => context.git_diff,
            git_diff_stat => context.git_diff_stat,
            branch => context.branch,
            recent_commits => context.recent_commits.unwrap_or(&empty_commits),
            repo => context.repo_name,
            commits => &commits_chronological,
            commit_details => &commit_details_chronological,
            target_branch => context.target_branch.unwrap_or(""),
        })?)
    };
    let user_guidance = match config
        .template_append
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(fragment) => render_fragment(fragment)?,
        None => String::new(),
    };
    let project_guidance = match context.project_append {
        Some(fragment) => render_fragment(fragment)?,
        None => String::new(),
    };

    let rendered = tmpl.render(minijinja::context! {
        git_diff => context.git_diff,
        git_diff_stat => context.git_diff_stat,
        branch => context.branch,
        recent_commits => context.recent_commits.unwrap_or(&empty_commits),
        repo => context.repo_name,
        commits => commits_chronological,
        commit_details => commit_details_chronological,
        target_branch => context.target_branch.unwrap_or(""),
        user_guidance => user_guidance,
        project_guidance => project_guidance,
    })?;

    Ok(rendered)
}

/// `index_override` is forwarded to git operations that read the staging area, so
/// `--dry-run` can preview against a temp index without touching the user's real one.
///
/// `project_append` is the approved project-level append fragment (or
/// `None` to skip). It is rendered with the main template's context and
/// appended to the prompt inside a `<project-guidance>` block; the
/// user-level append fragment from the [`CommitGenerationConfig`] renders
/// separately into `<user-guidance>`.
pub(crate) fn generate_commit_message(
    commit_generation_config: &CommitGenerationConfig,
    index_override: Option<&Path>,
    project_append: Option<&str>,
) -> anyhow::Result<String> {
    // Check if commit generation is configured (non-empty command)
    if commit_generation_config.is_configured() {
        let command = commit_generation_config.command.as_ref().unwrap();
        // A slow or hung command is otherwise silent (stdout is captured); the
        // watchdog surfaces a "still waiting" status. Held until this function
        // returns, clearing the block before the caller prints the message.
        let _watchdog = watch_llm_command(command, "the commit message");
        // Commit generation is explicitly configured - fail if it doesn't work
        return try_generate_commit_message(
            command,
            commit_generation_config,
            index_override,
            project_append,
        )
        .map_err(|e| {
            worktrunk::git::GitError::LlmCommandFailed {
                command: command.clone(),
                error: e.to_string(),
                reproduction_command: Some(format_reproduction_command(
                    "wt step commit --show-prompt",
                    command,
                )),
            }
            .into()
        });
    }

    // Fallback: generate a descriptive commit message based on changed files
    let repo = Repository::current()?;
    let mut name_only = Cmd::new("git")
        .args(["diff", "--staged", "--name-only", "-z"])
        .current_dir(repo.discovery_path());
    if let Some(path) = index_override {
        name_only = name_only.env("GIT_INDEX_FILE", path);
    }
    let file_list = run_git_capture(name_only, "diff --staged --name-only")?;
    let staged_files = file_list
        .split('\0')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|path| {
            Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(path)
        })
        .collect::<Vec<_>>();

    let message = match staged_files.len() {
        0 => "WIP: Changes".to_string(),
        1 => format!("Changes to {}", staged_files[0]),
        2 => format!("Changes to {} & {}", staged_files[0], staged_files[1]),
        3 => format!(
            "Changes to {}, {} & {}",
            staged_files[0], staged_files[1], staged_files[2]
        ),
        n => format!("Changes to {} files", n),
    };

    Ok(message)
}

fn try_generate_commit_message(
    command: &str,
    config: &CommitGenerationConfig,
    index_override: Option<&Path>,
    project_append: Option<&str>,
) -> anyhow::Result<String> {
    let prompt = build_commit_prompt(config, index_override, project_append)?;
    execute_llm_command(command, &prompt)
}

/// Run a git `Cmd` and bail on non-zero exit, mirroring [`Repository::run_command`].
///
/// Used by call sites that need to set `GIT_INDEX_FILE` (`--dry-run`) and so can't go
/// through `Repository::run_command`. Without this check, a failing `git diff` would
/// silently feed an empty diff to the LLM.
fn run_git_capture(cmd: Cmd, what: &str) -> anyhow::Result<String> {
    let output = cmd
        .run()
        .with_context(|| format!("Failed to execute git {what}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("git {what} failed: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Build the commit prompt from staged changes.
///
/// Gathers the staged diff, branch name, repo name, and recent commits, then renders
/// the prompt template. Used by normal commit generation, `--show-prompt`, and
/// `--dry-run`.
///
/// `index_override` points git at an alternate index via `GIT_INDEX_FILE` — used by
/// `--dry-run` to preview what `git add` per the user's `--stage` flag would produce
/// without modifying the real index.
pub(crate) fn build_commit_prompt(
    config: &CommitGenerationConfig,
    index_override: Option<&Path>,
    project_append: Option<&str>,
) -> anyhow::Result<String> {
    let repo = Repository::current()?;
    let cwd = repo.discovery_path().to_path_buf();

    // Use -c flags to ensure consistent format regardless of user's git config
    // (diff.noprefix, diff.mnemonicPrefix, etc. could break our parsing)
    let mut diff_cmd = Cmd::new("git")
        .args([
            "-c",
            "diff.noprefix=false",
            "-c",
            "diff.mnemonicPrefix=false",
            "--no-pager",
            "diff",
            "--staged",
        ])
        .current_dir(&cwd);
    let mut diff_stat_cmd = Cmd::new("git")
        .args(["--no-pager", "diff", "--staged", "--stat"])
        .current_dir(&cwd);
    if let Some(index) = index_override {
        diff_cmd = diff_cmd.env("GIT_INDEX_FILE", index);
        diff_stat_cmd = diff_stat_cmd.env("GIT_INDEX_FILE", index);
    }
    let diff_output = run_git_capture(diff_cmd, "diff --staged")?;
    let diff_stat = run_git_capture(diff_stat_cmd, "diff --staged --stat")?;

    // Prepare diff (may filter if too large)
    let prepared = prepare_diff(diff_output, diff_stat);

    // Get current branch and repo root
    let wt = repo.current_worktree();
    let current_branch = wt.branch()?.unwrap_or_else(|| "HEAD".to_string());
    let repo_root = wt.root()?;
    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");

    let recent_commits = repo.recent_commit_subjects(None, 5);

    let context = TemplateContext {
        git_diff: &prepared.diff,
        git_diff_stat: &prepared.stat,
        branch: &current_branch,
        recent_commits: recent_commits.as_ref(),
        repo_name,
        commit_details: &[],
        target_branch: None,
        project_append,
    };
    build_prompt(config, TemplateType::Commit, &context)
}

pub(crate) fn generate_squash_message(
    target_branch: &str,
    merge_base: &str,
    commit_details: &[CommitMessageDetail],
    current_branch: &str,
    repo_name: &str,
    commit_generation_config: &CommitGenerationConfig,
    project_append: Option<&str>,
) -> anyhow::Result<String> {
    // Check if commit generation is configured (non-empty command)
    if commit_generation_config.is_configured() {
        let command = commit_generation_config.command.as_ref().unwrap();

        let prompt = build_squash_prompt(
            target_branch,
            merge_base,
            commit_details,
            current_branch,
            repo_name,
            commit_generation_config,
            project_append,
        )?;

        // See `generate_commit_message` — keep a slow squash-message generation
        // from being silent.
        let _watchdog = watch_llm_command(command, "the squash commit message");
        return execute_llm_command(command, &prompt).map_err(|e| {
            worktrunk::git::GitError::LlmCommandFailed {
                command: command.clone(),
                error: e.to_string(),
                reproduction_command: Some(format_reproduction_command(
                    "wt step squash --show-prompt",
                    command,
                )),
            }
            .into()
        });
    }

    // Fallback: deterministic commit message (only when not configured)
    let mut commit_message = format!("Squash commits from {}\n\n", current_branch);
    commit_message.push_str("Combined commits:\n");
    for detail in commit_details.iter().rev() {
        // Reverse so they're in chronological order
        commit_message.push_str(&format!("- {}\n", detail.subject));
    }
    Ok(commit_message)
}

/// Build the squash prompt from commits being squashed.
///
/// Gathers the combined diff, commit message details, branch names, and recent commits, then
/// renders the prompt template. Used by both normal squash generation and `--show-prompt`.
pub(crate) fn build_squash_prompt(
    target_branch: &str,
    merge_base: &str,
    commit_details: &[CommitMessageDetail],
    current_branch: &str,
    repo_name: &str,
    config: &CommitGenerationConfig,
    project_append: Option<&str>,
) -> anyhow::Result<String> {
    let repo = Repository::current()?;

    // Get the combined diff and diffstat for all commits being squashed
    // Use -c flags to ensure consistent format regardless of user's git config
    let diff_output = repo.run_command(&[
        "-c",
        "diff.noprefix=false",
        "-c",
        "diff.mnemonicPrefix=false",
        "--no-pager",
        "diff",
        merge_base,
        "HEAD",
    ])?;
    let diff_stat = repo.run_command(&["--no-pager", "diff", merge_base, "HEAD", "--stat"])?;

    // Prepare diff (may filter if too large)
    let prepared = prepare_diff(diff_output, diff_stat);

    let recent_commits = repo.recent_commit_subjects(Some(merge_base), 5);
    let context = TemplateContext {
        git_diff: &prepared.diff,
        git_diff_stat: &prepared.stat,
        branch: current_branch,
        recent_commits: recent_commits.as_ref(),
        repo_name,
        commit_details,
        target_branch: Some(target_branch),
        project_append,
    };
    build_prompt(config, TemplateType::Squash, &context)
}

/// Synthetic diff for testing commit generation
const SYNTHETIC_DIFF: &str = r#"diff --git a/src/main.rs b/src/main.rs
index abc1234..def5678 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -10,6 +10,10 @@ fn main() {
     println!("Hello, world!");
+
+    // Add new feature
+    let config = load_config();
+    process_data(&config);
 }
"#;

/// Synthetic diffstat for testing commit generation
const SYNTHETIC_DIFF_STAT: &str = " src/main.rs | 4 ++++
 1 file changed, 4 insertions(+)";

/// Test commit generation with a synthetic diff.
///
/// Returns Ok(message) if the LLM command succeeds, or an error describing
/// what went wrong (command not found, API error, empty response, etc.)
pub(crate) fn test_commit_generation(
    commit_generation_config: &CommitGenerationConfig,
) -> anyhow::Result<String> {
    if !commit_generation_config.is_configured() {
        anyhow::bail!(
            "Commit generation is not configured. Add [commit.generation] to the config."
        );
    }

    let command = commit_generation_config.command.as_ref().unwrap();

    // Build prompt with synthetic data
    let recent_commits = vec![
        "feat: Add user authentication".to_string(),
        "fix: Handle edge case in parser".to_string(),
        "docs: Update README".to_string(),
    ];
    let context = TemplateContext {
        git_diff: SYNTHETIC_DIFF,
        git_diff_stat: SYNTHETIC_DIFF_STAT,
        branch: "feature/example",
        recent_commits: Some(&recent_commits),
        repo_name: "test-repo",
        commit_details: &[],
        target_branch: None,
        // The connectivity test sends a synthetic prompt — keep it independent
        // of any project guidance so it doesn't surface team-policy text in
        // `wt config show`.
        project_append: None,
    };
    let prompt = build_prompt(commit_generation_config, TemplateType::Commit, &context)?;

    // The connectivity test shells out the same way real generation does, so a
    // slow command would be just as silent — surface the same waiting status.
    let _watchdog = watch_llm_command(command, "the test commit message");
    execute_llm_command(command, &prompt).map_err(|e| {
        worktrunk::git::GitError::LlmCommandFailed {
            command: command.clone(),
            error: e.to_string(),
            reproduction_command: None, // Already a test command
        }
        .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    /// `render_llm_invocation` should wrap the command through the platform shell with
    /// the shell's basename (no full install path) and shell-escape the command argument
    /// so paths/quotes survive the round-trip.
    #[test]
    fn test_render_llm_invocation_basics() {
        let rendered = render_llm_invocation("llm -m haiku").unwrap();
        // Linux: "sh -c 'llm -m haiku'" — Windows: "bash.exe -c 'llm -m haiku'"
        assert!(
            rendered.ends_with(" -c 'llm -m haiku'"),
            "expected '<shell> -c <quoted-command>', got: {rendered}"
        );
        // Basename only — no install-path leakage.
        assert!(
            !rendered.contains('/') && !rendered.contains('\\'),
            "shell rendered with directory components: {rendered}"
        );
    }

    /// `run_git_capture` must surface a non-zero exit as an error that names the `what`
    /// label and includes the captured stderr — without that, the dry-run path would
    /// feed an empty diff to the LLM on a `git` failure.
    #[test]
    fn test_run_git_capture_bails_on_nonzero_exit() {
        let cmd = Cmd::new("git").args(["frobnicate-nonexistent"]);
        let err = run_git_capture(cmd, "frobnicate").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("git frobnicate failed:"),
            "error message should name the `what` label; got: {msg}"
        );
    }

    /// Single-quotes in the user's command must be escaped so the displayed string is a
    /// faithful, copy-pasteable shell invocation.
    #[test]
    fn test_render_llm_invocation_escapes_quotes() {
        let rendered = render_llm_invocation("echo 'hi'").unwrap();
        assert!(
            rendered.contains(r#"'echo '\''hi'\'''"#),
            "single quotes not escaped: {rendered}"
        );
    }

    /// Render a one-off template with a single `CommitDetailValue` bound to `c`.
    fn render_with_detail(template: &str, subject: &str, body: &str) -> String {
        let env = Environment::new();
        let detail = Value::from_object(CommitDetailValue {
            subject: subject.to_string(),
            body: body.to_string(),
        });
        env.template_from_str(template)
            .unwrap()
            .render(minijinja::context! { c => detail })
            .unwrap()
    }

    /// A `commit_details` element renders as its bare subject and exposes
    /// `.subject` / `.body`. This is the equivalence that lets the
    /// `commits` → `commit_details` rename be a mechanical identifier rewrite
    /// (see #2984 and `CommitDetailValue`).
    #[test]
    fn test_commit_detail_value_render_and_properties() {
        assert_eq!(render_with_detail("{{ c }}", "Add a", "body a"), "Add a");
        assert_eq!(
            render_with_detail("{{ c.subject }}|{{ c.body }}", "Add a", "body a"),
            "Add a|body a"
        );
    }

    /// An unknown attribute resolves to undefined (renders empty), exercising the
    /// catch-all in `get_value`. A bare `{% if c %}` is truthy because
    /// `enumerate` reports two keys — without that override a `Map`-repr object
    /// defaults to an empty enumerator and would read as falsy, diverging from
    /// the old non-empty subject string.
    #[test]
    fn test_commit_detail_value_unknown_key_and_truthiness() {
        assert_eq!(
            render_with_detail("[{{ c.nope }}]", "Add a", "body a"),
            "[]"
        );
        assert_eq!(
            render_with_detail("{% if c %}yes{% else %}no{% endif %}", "Add a", "body a"),
            "yes"
        );
    }

    /// Helper to create a commit context (no squash-specific fields)
    fn commit_context<'a>(
        git_diff: &'a str,
        branch: &'a str,
        recent_commits: Option<&'a Vec<String>>,
        repo_name: &'a str,
    ) -> TemplateContext<'a> {
        TemplateContext {
            git_diff,
            git_diff_stat: "",
            branch,
            recent_commits,
            repo_name,
            commit_details: &[],
            target_branch: None,
            project_append: None,
        }
    }

    /// Helper to create a squash context (all fields)
    fn squash_context<'a>(
        git_diff: &'a str,
        branch: &'a str,
        recent_commits: Option<&'a Vec<String>>,
        repo_name: &'a str,
        commit_details: &'a [CommitMessageDetail],
        target_branch: &'a str,
    ) -> TemplateContext<'a> {
        TemplateContext {
            git_diff,
            git_diff_stat: "",
            branch,
            recent_commits,
            repo_name,
            commit_details,
            target_branch: Some(target_branch),
            project_append: None,
        }
    }

    #[test]
    fn test_build_commit_prompt_with_default_template() {
        let config = CommitGenerationConfig::default();

        // No recent commits
        let context = commit_context("diff content", "main", None, "myrepo");
        let prompt = build_prompt(&config, TemplateType::Commit, &context).unwrap();
        assert_snapshot!(prompt, @r#"
        <task>Write a commit message for the staged changes below.</task>

        <format>
        - Subject line under 50 chars
        - For material changes, add a blank line then a body paragraph explaining the change
        - Output only the commit message, no quotes or code blocks
        </format>

        <style>
        - Imperative mood: "Add feature" not "Added feature"
        - Match recent commit style (conventional commits if used)
        - Describe the change, not the intent or benefit
        </style>

        <diffstat>

        </diffstat>

        <diff>
        diff content
        </diff>

        <context>
        Branch: main

        </context>
        "#);

        // With recent commits
        let commits = vec!["feat: add feature".to_string(), "fix: bug".to_string()];
        let context = commit_context("diff", "main", Some(&commits), "repo");
        let prompt = build_prompt(&config, TemplateType::Commit, &context).unwrap();
        assert_snapshot!(prompt, @r#"
        <task>Write a commit message for the staged changes below.</task>

        <format>
        - Subject line under 50 chars
        - For material changes, add a blank line then a body paragraph explaining the change
        - Output only the commit message, no quotes or code blocks
        </format>

        <style>
        - Imperative mood: "Add feature" not "Added feature"
        - Match recent commit style (conventional commits if used)
        - Describe the change, not the intent or benefit
        </style>

        <diffstat>

        </diffstat>

        <diff>
        diff
        </diff>

        <context>
        Branch: main
        <recent_commits>
        - feat: add feature
        - fix: bug
        </recent_commits>
        </context>
        "#);

        // Empty recent commits list — should not render commit data section
        let commits = vec![];
        let context = commit_context("diff", "main", Some(&commits), "repo");
        let prompt = build_prompt(&config, TemplateType::Commit, &context).unwrap();
        assert_snapshot!(prompt, @r#"
        <task>Write a commit message for the staged changes below.</task>

        <format>
        - Subject line under 50 chars
        - For material changes, add a blank line then a body paragraph explaining the change
        - Output only the commit message, no quotes or code blocks
        </format>

        <style>
        - Imperative mood: "Add feature" not "Added feature"
        - Match recent commit style (conventional commits if used)
        - Describe the change, not the intent or benefit
        </style>

        <diffstat>

        </diffstat>

        <diff>
        diff
        </diff>

        <context>
        Branch: main

        </context>
        "#);
    }

    #[test]
    fn test_build_commit_prompt_with_custom_template() {
        let config = CommitGenerationConfig {
            command: None,
            template: Some("Branch: {{ branch }}\nDiff: {{ git_diff }}".to_string()),
            template_file: None,
            squash_template: None,
            squash_template_file: None,
            template_append: None,
        };
        let context = commit_context("my diff", "feature", None, "repo");
        let result = build_prompt(&config, TemplateType::Commit, &context);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Branch: feature\nDiff: my diff");
    }

    #[test]
    fn test_build_commit_prompt_malformed_jinja() {
        let config = CommitGenerationConfig {
            command: None,
            template: Some("{{ unclosed".to_string()),
            template_file: None,
            squash_template: None,
            squash_template_file: None,
            template_append: None,
        };
        let context = commit_context("diff", "main", None, "repo");
        let result = build_prompt(&config, TemplateType::Commit, &context);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_commit_prompt_empty_template() {
        let config = CommitGenerationConfig {
            command: None,
            template: Some("   ".to_string()),
            template_file: None,
            squash_template: None,
            squash_template_file: None,
            template_append: None,
        };
        let context = commit_context("diff", "main", None, "repo");
        let result = build_prompt(&config, TemplateType::Commit, &context);
        assert_snapshot!(result.unwrap_err().to_string(), @"Template is empty");
    }

    #[test]
    fn test_build_commit_prompt_with_all_variables() {
        let config = CommitGenerationConfig {
            command: None,
            template: Some(
                "Repo: {{ repo }}\nBranch: {{ branch }}\nDiff: {{ git_diff }}\n{% for c in recent_commits %}{{ c }}\n{% endfor %}"
                    .to_string(),
            ),
            template_file: None,
            squash_template: None,
            squash_template_file: None,
            template_append: None,
        };
        let commits = vec!["commit1".to_string(), "commit2".to_string()];
        let context = commit_context("my diff", "feature", Some(&commits), "myrepo");
        let result = build_prompt(&config, TemplateType::Commit, &context);
        assert!(result.is_ok());
        let prompt = result.unwrap();
        assert_eq!(
            prompt,
            "Repo: myrepo\nBranch: feature\nDiff: my diff\ncommit1\ncommit2\n"
        );
    }

    #[test]
    fn test_default_commit_template_renders_project_fragment() {
        let config = CommitGenerationConfig::default();
        let mut context = commit_context("diff content", "main", None, "myrepo");
        context.project_append = Some("- Use conventional commits\n- Reference the issue");
        let prompt = build_prompt(&config, TemplateType::Commit, &context).unwrap();
        // The block lives between `<style>` and `<diffstat>`, only when guidance is set.
        assert_snapshot!(prompt, @r#"
        <task>Write a commit message for the staged changes below.</task>

        <format>
        - Subject line under 50 chars
        - For material changes, add a blank line then a body paragraph explaining the change
        - Output only the commit message, no quotes or code blocks
        </format>

        <style>
        - Imperative mood: "Add feature" not "Added feature"
        - Match recent commit style (conventional commits if used)
        - Describe the change, not the intent or benefit
        </style>

        <project-guidance>
        - Use conventional commits
        - Reference the issue
        </project-guidance>

        <diffstat>

        </diffstat>

        <diff>
        diff content
        </diff>

        <context>
        Branch: main

        </context>
        "#);
    }

    /// The user-level `template-append` (from `CommitGenerationConfig`,
    /// no approval) renders into a `<user-guidance>` block.
    #[test]
    fn test_user_template_append_renders() {
        let config = CommitGenerationConfig {
            template_append: Some("- Personal: explain the why".to_string()),
            ..Default::default()
        };
        let context = commit_context("diff content", "main", None, "myrepo");
        let prompt = build_prompt(&config, TemplateType::Commit, &context).unwrap();
        assert!(
            prompt.contains("<user-guidance>\n- Personal: explain the why\n</user-guidance>"),
            "user append should render in <user-guidance>, got:\n{prompt}"
        );
    }

    /// User and project appends render into separate provenance-labeled
    /// blocks, `<user-guidance>` before `<project-guidance>`.
    #[test]
    fn test_user_and_project_append_combined() {
        let config = CommitGenerationConfig {
            template_append: Some("USER LINE".to_string()),
            ..Default::default()
        };
        let mut context = commit_context("d", "main", None, "repo");
        context.project_append = Some("PROJECT LINE");
        let prompt = build_prompt(&config, TemplateType::Commit, &context).unwrap();
        let user_at = prompt.find("<user-guidance>\nUSER LINE\n</user-guidance>");
        let project_at = prompt.find("<project-guidance>\nPROJECT LINE\n</project-guidance>");
        assert!(
            matches!((user_at, project_at), (Some(u), Some(p)) if u < p),
            "<user-guidance> should precede <project-guidance>, got:\n{prompt}"
        );
    }

    /// A blank user append is treated as unset — no empty block rendered.
    #[test]
    fn test_user_template_append_blank_is_unset() {
        let config = CommitGenerationConfig {
            template_append: Some("   \n\t ".to_string()),
            ..Default::default()
        };
        let context = commit_context("d", "main", None, "repo");
        let prompt = build_prompt(&config, TemplateType::Commit, &context).unwrap();
        assert!(
            !prompt.contains("<user-guidance>"),
            "blank user append must not render a block, got:\n{prompt}"
        );
    }

    /// The user append is itself a minijinja template, rendered against the
    /// same context as the main template.
    #[test]
    fn test_user_template_append_expands_variables() {
        let config = CommitGenerationConfig {
            template_append: Some("Repo {{ repo }} on {{ branch }}".to_string()),
            ..Default::default()
        };
        let context = commit_context("d", "feat/x", None, "myrepo");
        let prompt = build_prompt(&config, TemplateType::Commit, &context).unwrap();
        assert!(
            prompt.contains("Repo myrepo on feat/x"),
            "user append should expand variables, got:\n{prompt}"
        );
        assert!(
            !prompt.contains("{{ repo }}"),
            "user append was not rendered:\n{prompt}"
        );
    }

    /// The project fragment is itself a minijinja template — variables in it
    /// expand against the same context as the main template. Without this
    /// test, the pre-render pass in `build_prompt` could regress to a raw
    /// string injection and nothing else would catch it.
    #[test]
    fn test_project_fragment_expands_template_variables() {
        let config = CommitGenerationConfig::default();
        let mut context = commit_context("diff", "feature/auth", None, "myrepo");
        context.project_append = Some("Branch: {{ branch }} ({{ repo }})");
        let prompt = build_prompt(&config, TemplateType::Commit, &context).unwrap();
        assert!(
            prompt.contains("Branch: feature/auth (myrepo)"),
            "expected minijinja-expanded fragment in prompt, got:\n{prompt}"
        );
        // Make sure the unexpanded form didn't sneak through.
        assert!(
            !prompt.contains("{{ branch }}"),
            "fragment was not rendered:\n{prompt}"
        );
    }

    /// A malformed fragment must surface its render error rather than slipping
    /// into the LLM prompt as literal text.
    #[test]
    fn test_project_fragment_render_error_propagates() {
        let config = CommitGenerationConfig::default();
        let mut context = commit_context("diff", "main", None, "repo");
        context.project_append = Some("Unclosed {{ branch");
        let err = build_prompt(&config, TemplateType::Commit, &context).unwrap_err();
        assert!(
            err.to_string().contains("syntax error")
                || err.to_string().to_lowercase().contains("unexpected"),
            "expected minijinja syntax error, got: {err}"
        );
    }

    #[test]
    fn test_default_squash_template_renders_project_fragment() {
        let config = CommitGenerationConfig::default();
        let commit_details = vec![
            CommitMessageDetail {
                subject: "feat: A".to_string(),
                body: String::new(),
            },
            CommitMessageDetail {
                subject: "fix: B".to_string(),
                body: String::new(),
            },
        ];
        let mut context = squash_context(
            "diff content",
            "feature",
            None,
            "repo",
            &commit_details,
            "main",
        );
        context.project_append = Some("- Reference the related issue");
        let prompt = build_prompt(&config, TemplateType::Squash, &context).unwrap();
        assert_snapshot!(prompt, @r#"
        <task>Write a commit message for the combined effect of these commits.</task>

        <format>
        - Subject line under 50 chars
        - For material changes, add a blank line then a body paragraph explaining the change
        - Output only the commit message, no quotes or code blocks
        </format>

        <style>
        - Imperative mood: "Add feature" not "Added feature"
        - Match the style of commits being squashed (conventional commits if used)
        - Describe the change, not the intent or benefit
        </style>

        <project-guidance>
        - Reference the related issue
        </project-guidance>

        <commits branch="feature" target="main">
        - fix: B
        - feat: A
        </commits>

        <diffstat>

        </diffstat>

        <diff>
        diff content
        </diff>
        "#);
    }

    #[test]
    fn test_build_squash_prompt_with_default_template() {
        let config = CommitGenerationConfig::default();
        let commit_details = vec![
            CommitMessageDetail {
                subject: "feat: A".to_string(),
                body: String::new(),
            },
            CommitMessageDetail {
                subject: "fix: B".to_string(),
                body: String::new(),
            },
        ];
        let context = squash_context(
            "diff content",
            "feature",
            None,
            "repo",
            &commit_details,
            "main",
        );
        let prompt = build_prompt(&config, TemplateType::Squash, &context).unwrap();
        assert_snapshot!(prompt, @r#"
        <task>Write a commit message for the combined effect of these commits.</task>

        <format>
        - Subject line under 50 chars
        - For material changes, add a blank line then a body paragraph explaining the change
        - Output only the commit message, no quotes or code blocks
        </format>

        <style>
        - Imperative mood: "Add feature" not "Added feature"
        - Match the style of commits being squashed (conventional commits if used)
        - Describe the change, not the intent or benefit
        </style>

        <commits branch="feature" target="main">
        - fix: B
        - feat: A
        </commits>

        <diffstat>

        </diffstat>

        <diff>
        diff content
        </diff>
        "#);
    }

    #[test]
    fn test_build_squash_prompt_with_custom_template() {
        let config = CommitGenerationConfig {
            command: None,
            template: None,
            template_file: None,
            squash_template: Some(
                "Target: {{ target_branch }}\n{% for c in commits %}{{ c }}\n{% endfor %}"
                    .to_string(),
            ),
            squash_template_file: None,
            template_append: None,
        };
        let commit_details = vec![
            CommitMessageDetail {
                subject: "A".to_string(),
                body: "body A".to_string(),
            },
            CommitMessageDetail {
                subject: "B".to_string(),
                body: "body B".to_string(),
            },
        ];
        let context = squash_context("diff", "feature", None, "repo", &commit_details, "main");
        let result = build_prompt(&config, TemplateType::Squash, &context);
        assert!(result.is_ok());
        // Commits are reversed, so chronological order is B, A
        assert_eq!(result.unwrap(), "Target: main\nB\nA\n");
    }

    #[test]
    fn test_build_squash_prompt_with_commit_details() {
        let config = CommitGenerationConfig {
            command: None,
            template: None,
            template_file: None,
            squash_template: Some(
                r#"{% for detail in commit_details %}{{ loop.index }}. {{ detail.subject }}
{{ detail.body }}
{% endfor %}"#
                    .to_string(),
            ),
            squash_template_file: None,
            template_append: None,
        };
        let commit_details = vec![
            CommitMessageDetail {
                subject: "newer subject".to_string(),
                body: "newer body".to_string(),
            },
            CommitMessageDetail {
                subject: "older subject".to_string(),
                body: "older body line 1\nolder body line 2".to_string(),
            },
        ];
        let context = squash_context("diff", "feature", None, "repo", &commit_details, "main");

        let prompt = build_prompt(&config, TemplateType::Squash, &context).unwrap();

        assert_eq!(
            prompt,
            "1. older subject\nolder body line 1\nolder body line 2\n2. newer subject\nnewer body\n"
        );
    }

    #[test]
    fn test_build_squash_prompt_empty_commits() {
        let config = CommitGenerationConfig::default();
        let commit_details = vec![];
        let context = squash_context("diff", "feature", None, "repo", &commit_details, "main");
        let result = build_prompt(&config, TemplateType::Squash, &context);
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_squash_prompt_malformed_jinja() {
        let config = CommitGenerationConfig {
            command: None,
            template: None,
            template_file: None,
            squash_template: Some("{% for x in commits %}{{ x }".to_string()),
            squash_template_file: None,
            template_append: None,
        };
        let commit_details = vec![];
        let context = squash_context("diff", "feature", None, "repo", &commit_details, "main");
        let result = build_prompt(&config, TemplateType::Squash, &context);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_squash_prompt_empty_template() {
        let config = CommitGenerationConfig {
            command: None,
            template: None,
            template_file: None,
            squash_template: Some("  \n  ".to_string()),
            squash_template_file: None,
            template_append: None,
        };
        let commit_details = vec![];
        let context = squash_context("diff", "feature", None, "repo", &commit_details, "main");
        let result = build_prompt(&config, TemplateType::Squash, &context);
        assert_snapshot!(result.unwrap_err().to_string(), @"Squash template is empty");
    }

    #[test]
    fn test_build_squash_prompt_with_all_variables() {
        // Test that squash templates now have access to ALL variables including git_diff and recent_commits
        let config = CommitGenerationConfig {
            command: None,
            template: None,
            template_file: None,
            squash_template: Some(
                "Repo: {{ repo }}\nBranch: {{ branch }}\nTarget: {{ target_branch }}\nDiff: {{ git_diff }}\n{% for c in commits %}{{ c }}\n{% endfor %}{% for r in recent_commits %}style: {{ r }}\n{% endfor %}"
                    .to_string(),
            ),
            squash_template_file: None,
            template_append: None,
        };
        let commit_details = vec![
            CommitMessageDetail {
                subject: "A".to_string(),
                body: String::new(),
            },
            CommitMessageDetail {
                subject: "B".to_string(),
                body: String::new(),
            },
        ];
        let recent = vec!["prev1".to_string(), "prev2".to_string()];
        let context = squash_context(
            "the diff",
            "feature",
            Some(&recent),
            "myrepo",
            &commit_details,
            "main",
        );
        let result = build_prompt(&config, TemplateType::Squash, &context);
        assert!(result.is_ok());
        let prompt = result.unwrap();
        assert_eq!(
            prompt,
            "Repo: myrepo\nBranch: feature\nTarget: main\nDiff: the diff\nB\nA\nstyle: prev1\nstyle: prev2\n"
        );
    }

    #[test]
    fn test_build_commit_prompt_with_sophisticated_jinja() {
        // Test advanced jinja features: filters, length, conditionals, whitespace control
        let config = CommitGenerationConfig {
            command: None,
            template: Some(
                r#"=== {{ repo | upper }} ===
Branch: {{ branch }}
{%- if recent_commits %}
Commits: {{ recent_commits | length }}
{%- for c in recent_commits %}
  - {{ loop.index }}. {{ c }}
{%- endfor %}
{%- else %}
No recent commits
{%- endif %}

Diff follows:
{{ git_diff }}"#
                    .to_string(),
            ),
            template_file: None,
            squash_template: None,
            squash_template_file: None,
            template_append: None,
        };

        // With commits — exercises if-branch, filters, loop.index, whitespace control
        let commits = vec![
            "feat: add auth".to_string(),
            "fix: bug".to_string(),
            "docs: update".to_string(),
        ];
        let context = commit_context("my diff content", "feature-x", Some(&commits), "myapp");
        let prompt = build_prompt(&config, TemplateType::Commit, &context).unwrap();
        assert_snapshot!(prompt, @"
        === MYAPP ===
        Branch: feature-x
        Commits: 3
          - 1. feat: add auth
          - 2. fix: bug
          - 3. docs: update

        Diff follows:
        my diff content
        ");

        // Without commits — exercises else-branch
        let context = commit_context("diff", "main", None, "test");
        let prompt = build_prompt(&config, TemplateType::Commit, &context).unwrap();
        assert_snapshot!(prompt, @"
        === TEST ===
        Branch: main
        No recent commits

        Diff follows:
        diff
        ");
    }

    #[test]
    fn test_build_squash_prompt_with_sophisticated_jinja() {
        // Test sophisticated jinja in squash templates
        let config = CommitGenerationConfig {
            command: None,
            template: None,
            template_file: None,
            squash_template: Some(
                r#"Squashing {{ commits | length }} commit(s) from {{ branch }} to {{ target_branch }}
{% if commits | length > 1 -%}
Multiple commits detected:
{%- for c in commits %}
  {{ loop.index }}/{{ loop.length }}: {{ c }}
{%- endfor %}
{%- else -%}
Single commit: {{ commits[0] }}
{%- endif %}"#
                    .to_string(),
            ),
            squash_template_file: None,
            template_append: None,
        };

        // Multiple commits — reversed for chronological order (C, B, A)
        let commit_details = vec![
            CommitMessageDetail {
                subject: "commit A".to_string(),
                body: String::new(),
            },
            CommitMessageDetail {
                subject: "commit B".to_string(),
                body: String::new(),
            },
            CommitMessageDetail {
                subject: "commit C".to_string(),
                body: String::new(),
            },
        ];
        let context = squash_context("diff", "feature", None, "repo", &commit_details, "main");
        let prompt = build_prompt(&config, TemplateType::Squash, &context).unwrap();
        assert_snapshot!(prompt, @"
        Squashing 3 commit(s) from feature to main
        Multiple commits detected:
          1/3: commit C
          2/3: commit B
          3/3: commit A
        ");

        // Single commit — exercises else-branch
        let single_commit = vec![CommitMessageDetail {
            subject: "solo commit".to_string(),
            body: String::new(),
        }];
        let context = squash_context("diff", "feature", None, "repo", &single_commit, "main");
        let prompt = build_prompt(&config, TemplateType::Squash, &context).unwrap();
        assert_snapshot!(prompt, @"
        Squashing 1 commit(s) from feature to main
        Single commit: solo commit
        ");
    }

    #[test]
    fn test_build_commit_prompt_with_template_file() {
        let temp_dir = std::env::temp_dir();
        let template_path = temp_dir.join("test_commit_template.txt");
        std::fs::write(
            &template_path,
            "Branch: {{ branch }}\nRepo: {{ repo }}\nDiff: {{ git_diff }}",
        )
        .unwrap();

        let config = CommitGenerationConfig {
            command: None,
            template: None,
            template_file: Some(template_path.to_string_lossy().to_string()),
            squash_template: None,
            squash_template_file: None,
            template_append: None,
        };
        let context = commit_context("my diff", "feature", None, "myrepo");
        let result = build_prompt(&config, TemplateType::Commit, &context);
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            "Branch: feature\nRepo: myrepo\nDiff: my diff"
        );

        // Cleanup
        std::fs::remove_file(&template_path).ok();
    }

    #[test]
    fn test_build_commit_prompt_with_missing_template_file() {
        let config = CommitGenerationConfig {
            command: None,
            template: None,
            template_file: Some("/nonexistent/path/template.txt".to_string()),
            squash_template: None,
            squash_template_file: None,
            template_append: None,
        };
        let context = commit_context("diff", "main", None, "repo");
        let result = build_prompt(&config, TemplateType::Commit, &context);
        // OS error text varies by platform, so use contains
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Failed to read template-file"), "{err}");
        assert!(err.contains("/nonexistent/path/template.txt"), "{err}");
    }

    #[test]
    fn test_build_squash_prompt_with_template_file() {
        let temp_dir = std::env::temp_dir();
        let template_path = temp_dir.join("test_squash_template.txt");
        std::fs::write(
            &template_path,
            "Target: {{ target_branch }}\nBranch: {{ branch }}\n{% for c in commits %}{{ c }}\n{% endfor %}",
        )
        .unwrap();

        let config = CommitGenerationConfig {
            command: None,
            template: None,
            template_file: None,
            squash_template: None,
            squash_template_file: Some(template_path.to_string_lossy().to_string()),
            template_append: None,
        };
        let commit_details = vec![
            CommitMessageDetail {
                subject: "A".to_string(),
                body: String::new(),
            },
            CommitMessageDetail {
                subject: "B".to_string(),
                body: String::new(),
            },
        ];
        let context = squash_context("diff", "feature", None, "repo", &commit_details, "main");
        let result = build_prompt(&config, TemplateType::Squash, &context);
        assert!(result.is_ok());
        // Commits are reversed for chronological order
        assert_eq!(result.unwrap(), "Target: main\nBranch: feature\nB\nA\n");

        // Cleanup
        std::fs::remove_file(&template_path).ok();
    }

    #[test]
    fn test_build_commit_prompt_with_tilde_expansion() {
        // This test verifies tilde expansion works - it should attempt to read
        // from the expanded home directory path
        let config = CommitGenerationConfig {
            command: None,
            template: None,
            template_file: Some("~/nonexistent_template_for_test.txt".to_string()),
            squash_template: None,
            squash_template_file: None,
            template_append: None,
        };
        let context = commit_context("diff", "main", None, "repo");
        let result = build_prompt(&config, TemplateType::Commit, &context);
        // Should fail because file doesn't exist
        // OS error text varies by platform, so use contains
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Failed to read template-file"), "{err}");
        assert!(err.contains("~/nonexistent_template_for_test.txt"), "{err}");
    }

    #[test]
    fn test_commit_template_can_access_squash_variables() {
        // Verify that commit templates can access squash-specific variables without errors
        // (they're empty/None for regular commits, but shouldn't cause template errors)
        let config = CommitGenerationConfig {
            command: None,
            template: Some(
                "Branch: {{ branch }}\nTarget: {{ target_branch }}\nCommit subjects: {{ commits | length }}\nCommit details: {{ commit_details | length }}"
                    .to_string(),
            ),
            template_file: None,
            squash_template: None,
            squash_template_file: None,
            template_append: None,
        };
        let context = commit_context("diff", "feature", None, "repo");
        let result = build_prompt(&config, TemplateType::Commit, &context);
        assert!(result.is_ok());
        let prompt = result.unwrap();
        // Squash-specific variables are empty for regular commits
        assert_eq!(
            prompt,
            "Branch: feature\nTarget: \nCommit subjects: 0\nCommit details: 0"
        );
    }

    // Tests for diff filtering

    #[test]
    fn test_is_lock_file() {
        // Matches
        assert!(is_lock_file("Cargo.lock"));
        assert!(is_lock_file("package-lock.json"));
        assert!(is_lock_file("pnpm-lock.yaml"));
        assert!(is_lock_file("yarn-lock.yaml"));
        assert!(is_lock_file(".terraform.lock.hcl"));
        assert!(is_lock_file("terraform.lock.hcl"));
        assert!(is_lock_file("path/to/Cargo.lock"));

        // Non-matches
        assert!(!is_lock_file("src/main.rs"));
        assert!(!is_lock_file("README.md"));
        assert!(!is_lock_file("config.toml"));
        assert!(!is_lock_file("lockfile.txt"));
        assert!(!is_lock_file("my.lock.rs")); // Not a standard lock pattern
    }

    #[test]
    fn test_parse_diff_sections() {
        // Empty input
        assert!(parse_diff_sections("").is_empty());

        // Single file
        let diff = "diff --git a/foo.rs b/foo.rs\nsome content\n";
        let sections = parse_diff_sections(diff);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].0, "foo.rs");

        // Multiple files
        let diff = r#"diff --git a/src/foo.rs b/src/foo.rs
index abc..def 100644
--- a/src/foo.rs
+++ b/src/foo.rs
@@ -1,3 +1,4 @@
 fn foo() {}
+fn bar() {}
diff --git a/Cargo.lock b/Cargo.lock
index 111..222 100644
--- a/Cargo.lock
+++ b/Cargo.lock
@@ -1,100 +1,150 @@
 lots of lock content
"#;
        let sections = parse_diff_sections(diff);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].0, "src/foo.rs");
        assert_snapshot!(sections[0].1, @"
        diff --git a/src/foo.rs b/src/foo.rs
        index abc..def 100644
        --- a/src/foo.rs
        +++ b/src/foo.rs
        @@ -1,3 +1,4 @@
         fn foo() {}
        +fn bar() {}
        ");
        assert_eq!(sections[1].0, "Cargo.lock");
        assert_snapshot!(sections[1].1, @"
        diff --git a/Cargo.lock b/Cargo.lock
        index 111..222 100644
        --- a/Cargo.lock
        +++ b/Cargo.lock
        @@ -1,100 +1,150 @@
         lots of lock content
        ");
    }

    #[test]
    fn test_parse_diff_sections_crlf_with_multibyte_utf8() {
        // Regression: CRLF line endings combined with multi-byte UTF-8
        // content used to drift the byte offset (one byte lost per line),
        // eventually slicing inside a char boundary and panicking. See #2355.
        let mut diff = String::from("diff --git a/a b/a\r\n");
        for _ in 0..10 {
            diff.push_str("+测测测\r\n");
        }
        diff.push_str("diff --git a/b b/b\r\n");
        diff.push_str("+more\r\n");

        let sections = parse_diff_sections(&diff);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].0, "a");
        assert_eq!(sections[1].0, "b");
        // Each section must start at the "diff --git" header and together
        // cover the whole input with no bytes dropped or duplicated.
        assert!(sections[0].1.starts_with("diff --git a/a b/a"));
        assert!(sections[1].1.starts_with("diff --git a/b b/b"));
        let combined: String = sections.iter().map(|(_, s)| *s).collect();
        assert_eq!(combined, diff);
    }

    #[test]
    fn test_truncate_diff_section() {
        let section = r#"diff --git a/file.rs b/file.rs
index abc..def 100644
--- a/file.rs
+++ b/file.rs
@@ -1,10 +1,15 @@
 line 1
 line 2
 line 3
 line 4
 line 5
 line 6
 line 7
 line 8
 line 9
 line 10
"#;

        // Truncate to 8 lines (should keep header + first few content lines)
        let truncated = truncate_diff_section(section, 8);
        assert_snapshot!(truncated, @"
        diff --git a/file.rs b/file.rs
        index abc..def 100644
        --- a/file.rs
        +++ b/file.rs
        @@ -1,10 +1,15 @@
         line 1
         line 2
         line 3

        ... (7 lines omitted)
        ");
    }

    #[test]
    fn test_prepare_diff_small_diff_passes_through() {
        let diff = "small diff".to_string();
        let stat = "1 file changed".to_string();

        let prepared = prepare_diff(diff.clone(), stat.clone());
        assert_eq!(prepared.diff, diff);
        assert_eq!(prepared.stat, stat);
    }

    #[test]
    fn test_prepare_diff_filters_lock_files() {
        // Create a diff just over the threshold with a lock file
        let regular_content = "x".repeat(100_000);
        let lock_content = "y".repeat(350_000);

        let diff = format!(
            r#"diff --git a/src/main.rs b/src/main.rs
{}
diff --git a/Cargo.lock b/Cargo.lock
{}
"#,
            regular_content, lock_content
        );
        let stat = "2 files changed".to_string();

        let prepared = prepare_diff(diff, stat);

        // Lock file should be filtered out
        assert!(!prepared.diff.contains("Cargo.lock"));
        assert!(prepared.diff.contains("src/main.rs"));
    }

    #[test]
    fn test_prepare_diff_filters_then_truncates() {
        // Create many non-lock files that exceed threshold even after lock filtering
        let mut diff = String::new();
        for i in 0..100 {
            diff.push_str(&format!(
                "diff --git a/file{}.rs b/file{}.rs\n{}\n",
                i,
                i,
                "x".repeat(5000)
            ));
        }

        let stat = "100 files changed".to_string();
        let prepared = prepare_diff(diff, stat);

        // Should be truncated (max 50 files)
        assert!(prepared.diff.contains("files omitted"));
    }

    #[test]
    fn test_truncate_diff_section_short() {
        // Section shorter than max lines should pass through unchanged
        let section = "line1\nline2\nline3\n";
        let truncated = truncate_diff_section(section, 10);
        assert_eq!(truncated, section);
    }

    #[test]
    fn test_truncate_diff_section_no_header() {
        // Section without @@ marker
        let section = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\n";
        let truncated = truncate_diff_section(section, 3);
        assert_snapshot!(truncated, @"
        line1
        line2
        line3

        ... (5 lines omitted)
        ");
    }

    #[test]
    fn test_format_reproduction_command() {
        // Simple command — no wrapping needed
        let result = format_reproduction_command("git diff", "llm -m haiku");
        assert_snapshot!(result, @"git diff | llm -m haiku");

        // Env var assignment — needs shell wrapping
        let result = format_reproduction_command("git diff", "MAX_THINKING_TOKENS=0 claude -p");
        assert_snapshot!(result, @"git diff | sh -c 'MAX_THINKING_TOKENS=0 claude -p'");

        // Shell metacharacters — needs wrapping
        let result = format_reproduction_command("git diff", "cmd1 && cmd2");
        assert_snapshot!(result, @"git diff | sh -c 'cmd1 && cmd2'");
    }
}
