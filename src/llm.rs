use anyhow::Context;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{self, Stdio};
use worktrunk::config::CommitGenerationConfig;
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;

use minijinja::Environment;

/// Maximum diff size in characters before filtering kicks in
const DIFF_SIZE_THRESHOLD: usize = 400_000;

/// Maximum lines per file after truncation
const MAX_LINES_PER_FILE: usize = 50;

/// Maximum number of files to include after truncation
const MAX_FILES: usize = 50;

/// Lock file patterns that are filtered out when diff is too large
const LOCK_FILE_PATTERNS: &[&str] = &[".lock", "-lock.json", "-lock.yaml", ".lock.hcl"];

/// Prepared diff output with optional filtering applied
struct PreparedDiff {
    /// The diff content (possibly filtered/truncated)
    diff: String,
    /// The diffstat output
    stat: String,
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

    for line in diff.lines() {
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
        current_byte += line.len() + 1; // +1 for newline
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
fn prepare_diff(diff: String, stat: String) -> PreparedDiff {
    // If under threshold, pass through unchanged
    if diff.len() < DIFF_SIZE_THRESHOLD {
        return PreparedDiff { diff, stat };
    }

    log::debug!(
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
        log::debug!("Filtered out {} lock file(s)", lock_files_removed);
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
    log::debug!(
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
/// Squash-specific fields (`commits`, `target_branch`) are empty/None for regular commits.
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
    /// Commits being squashed (squash only)
    commits: &'a [String],
    /// Target branch for merge (squash only)
    target_branch: Option<&'a str>,
}

/// Format a command and its arguments into a display string
fn format_command_display(command: &str, args: &[String]) -> String {
    if args.is_empty() {
        command.to_string()
    } else {
        format!("{} {}", command, args.join(" "))
    }
}

/// Get recent commit subjects for style reference.
///
/// If `start_ref` is provided, gets commits starting from that ref (exclusive).
/// Otherwise gets commits from HEAD.
fn get_recent_commits(repo: &Repository, start_ref: Option<&str>) -> Option<Vec<String>> {
    let mut args = vec!["log", "--pretty=format:%s", "-n", "5", "--no-merges"];
    if let Some(ref_name) = start_ref {
        args.push(ref_name);
    }
    repo.run_command(&args).ok().and_then(|output| {
        if output.trim().is_empty() {
            None
        } else {
            Some(output.lines().map(String::from).collect())
        }
    })
}

/// Default template for commit message prompts
///
/// Synced to dev/config.example.toml by `cargo test readme_sync`
const DEFAULT_TEMPLATE: &str = r#"Write a commit message for the staged changes below.

<format>
- Subject under 50 chars, blank line, then optional body
- Output only the commit message, no quotes or code blocks
</format>

<style>
- Imperative mood: "Add feature" not "Added feature"
- Match recent commit style (conventional commits if used)
- Describe the change, not the intent or benefit
</style>

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
const DEFAULT_SQUASH_TEMPLATE: &str = r#"Combine these commits into a single commit message.

<format>
- Subject under 50 chars, blank line, then optional body
- Output only the commit message, no quotes or code blocks
</format>

<style>
- Imperative mood: "Add feature" not "Added feature"
- Match the style of commits being squashed (conventional commits if used)
- Describe the change, not the intent or benefit
</style>

<commits branch="{{ branch }}" target="{{ target_branch }}">
{% for commit in commits %}- {{ commit }}
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
/// This is the canonical way to execute LLM commands in this codebase.
/// All LLM execution should go through this function to maintain consistency.
fn execute_llm_command(command: &str, args: &[String], prompt: &str) -> anyhow::Result<String> {
    // Build command args
    let mut cmd = process::Command::new(command);
    cmd.args(args);

    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Log execution
    log::debug!("$ {} {}", command, args.join(" "));
    log::debug!("  Prompt (stdin):");
    for line in prompt.lines() {
        log::debug!("    {}", line);
    }

    let mut child = cmd.spawn().context("Failed to spawn LLM command")?;

    // Write prompt to stdin
    // Note: We ignore BrokenPipe errors because some commands (like `echo`) exit
    // immediately without reading stdin. This is fine as long as they produce output.
    if let Some(mut stdin) = child.stdin.take()
        && let Err(e) = stdin.write_all(prompt.as_bytes())
        && e.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(e).context("Failed to write prompt to LLM stdin");
    }
    // stdin is dropped here, closing the pipe

    let output = child
        .wait_with_output()
        .context("Failed to wait for LLM output")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{}", stderr.trim());
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

/// Build prompt from template using minijinja
///
/// Template variables available to both commit and squash templates:
/// - `git_diff`: The diff to describe
/// - `branch`: Current branch name
/// - `recent_commits`: Recent commit subjects for style reference
/// - `repo`: Repository name
///
/// Squash-specific variables (empty for regular commits):
/// - `commits`: Commits being squashed
/// - `target_branch`: Target branch for merge
fn build_prompt(
    config: &CommitGenerationConfig,
    template_type: TemplateType,
    context: &TemplateContext<'_>,
) -> anyhow::Result<String> {
    // Get template source based on type
    let (template, type_name) = match template_type {
        TemplateType::Commit => {
            let tmpl = match (&config.template, &config.template_file) {
                (Some(inline), None) => inline.clone(),
                (None, Some(path)) => {
                    let expanded_path = PathBuf::from(shellexpand::tilde(path).as_ref());
                    std::fs::read_to_string(&expanded_path).map_err(|e| {
                        anyhow::Error::from(worktrunk::git::GitError::Other {
                            message: format!(
                                "Failed to read template-file '{}': {}",
                                format_path_for_display(&expanded_path),
                                e
                            ),
                        })
                    })?
                }
                (None, None) => DEFAULT_TEMPLATE.to_string(),
                (Some(_), Some(_)) => {
                    unreachable!("Config validation should prevent both template and template-file")
                }
            };
            (tmpl, "Template")
        }
        TemplateType::Squash => {
            let tmpl = match (&config.squash_template, &config.squash_template_file) {
                (Some(inline), None) => inline.clone(),
                (None, Some(path)) => {
                    let expanded_path = PathBuf::from(shellexpand::tilde(path).as_ref());
                    std::fs::read_to_string(&expanded_path).map_err(|e| {
                        anyhow::Error::from(worktrunk::git::GitError::Other {
                            message: format!(
                                "Failed to read squash-template-file '{}': {}",
                                format_path_for_display(&expanded_path),
                                e
                            ),
                        })
                    })?
                }
                (None, None) => DEFAULT_SQUASH_TEMPLATE.to_string(),
                (Some(_), Some(_)) => {
                    unreachable!(
                        "Config validation should prevent both squash-template and squash-template-file"
                    )
                }
            };
            (tmpl, "Squash template")
        }
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

    // Reverse commits so they're in chronological order (oldest first)
    let commits_chronological: Vec<&String> = context.commits.iter().rev().collect();

    let rendered = tmpl.render(minijinja::context! {
        git_diff => context.git_diff,
        git_diff_stat => context.git_diff_stat,
        branch => context.branch,
        recent_commits => context.recent_commits.unwrap_or(&vec![]),
        repo => context.repo_name,
        commits => commits_chronological,
        target_branch => context.target_branch.unwrap_or(""),
    })?;

    Ok(rendered)
}

pub fn generate_commit_message(
    commit_generation_config: &CommitGenerationConfig,
) -> anyhow::Result<String> {
    // Check if commit generation is configured (non-empty command)
    if commit_generation_config.is_configured() {
        let command = commit_generation_config.command.as_ref().unwrap();
        let args = &commit_generation_config.args;
        // Commit generation is explicitly configured - fail if it doesn't work
        return try_generate_commit_message(command, args, commit_generation_config).map_err(|e| {
            worktrunk::git::GitError::LlmCommandFailed {
                command: format_command_display(command, args),
                error: e.to_string(),
            }
            .into()
        });
    }

    // Fallback: generate a descriptive commit message based on changed files
    let repo = Repository::current();
    let file_list = repo.run_command(&["diff", "--staged", "--name-only"])?;
    let staged_files = file_list
        .lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
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
    args: &[String],
    config: &CommitGenerationConfig,
) -> anyhow::Result<String> {
    let prompt = build_commit_prompt(config)?;
    execute_llm_command(command, args, &prompt)
}

/// Build the commit prompt from staged changes.
///
/// Gathers the staged diff, branch name, repo name, and recent commits, then renders
/// the prompt template. Used by both normal commit generation and `--show-prompt`.
pub fn build_commit_prompt(config: &CommitGenerationConfig) -> anyhow::Result<String> {
    let repo = Repository::current();

    // Get staged diff and diffstat
    // Use -c flags to ensure consistent format regardless of user's git config
    // (diff.noprefix, diff.mnemonicPrefix, etc. could break our parsing)
    let diff_output = repo.run_command(&[
        "-c",
        "diff.noprefix=false",
        "-c",
        "diff.mnemonicPrefix=false",
        "--no-pager",
        "diff",
        "--staged",
    ])?;
    let diff_stat = repo.run_command(&["--no-pager", "diff", "--staged", "--stat"])?;

    // Prepare diff (may filter if too large)
    let prepared = prepare_diff(diff_output, diff_stat);

    // Get current branch
    let current_branch = repo.current_branch()?.unwrap_or_else(|| "HEAD".to_string());

    // Get repo name from directory
    let repo_root = repo.worktree_root()?;
    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");

    let recent_commits = get_recent_commits(&repo, None);

    let context = TemplateContext {
        git_diff: &prepared.diff,
        git_diff_stat: &prepared.stat,
        branch: &current_branch,
        recent_commits: recent_commits.as_ref(),
        repo_name,
        commits: &[],
        target_branch: None,
    };
    build_prompt(config, TemplateType::Commit, &context)
}

pub fn generate_squash_message(
    target_branch: &str,
    merge_base: &str,
    subjects: &[String],
    current_branch: &str,
    repo_name: &str,
    commit_generation_config: &CommitGenerationConfig,
) -> anyhow::Result<String> {
    // Check if commit generation is configured (non-empty command)
    if commit_generation_config.is_configured() {
        let command = commit_generation_config.command.as_ref().unwrap();
        let args = &commit_generation_config.args;

        let prompt = build_squash_prompt(
            target_branch,
            merge_base,
            subjects,
            current_branch,
            repo_name,
            commit_generation_config,
        )?;

        return execute_llm_command(command, args, &prompt).map_err(|e| {
            worktrunk::git::GitError::LlmCommandFailed {
                command: format_command_display(command, args),
                error: e.to_string(),
            }
            .into()
        });
    }

    // Fallback: deterministic commit message (only when not configured)
    let mut commit_message = format!("Squash commits from {}\n\n", target_branch);
    commit_message.push_str("Combined commits:\n");
    for subject in subjects.iter().rev() {
        // Reverse so they're in chronological order
        commit_message.push_str(&format!("- {}\n", subject));
    }
    Ok(commit_message)
}

/// Build the squash prompt from commits being squashed.
///
/// Gathers the combined diff, commit subjects, branch names, and recent commits, then
/// renders the prompt template. Used by both normal squash generation and `--show-prompt`.
pub fn build_squash_prompt(
    target_branch: &str,
    merge_base: &str,
    subjects: &[String],
    current_branch: &str,
    repo_name: &str,
    config: &CommitGenerationConfig,
) -> anyhow::Result<String> {
    let repo = Repository::current();

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

    let recent_commits = get_recent_commits(&repo, Some(merge_base));
    let context = TemplateContext {
        git_diff: &prepared.diff,
        git_diff_stat: &prepared.stat,
        branch: current_branch,
        recent_commits: recent_commits.as_ref(),
        repo_name,
        commits: subjects,
        target_branch: Some(target_branch),
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
pub fn test_commit_generation(
    commit_generation_config: &CommitGenerationConfig,
) -> anyhow::Result<String> {
    if !commit_generation_config.is_configured() {
        anyhow::bail!(
            "Commit generation is not configured. Add [commit-generation] to your config."
        );
    }

    let command = commit_generation_config.command.as_ref().unwrap();
    let args = &commit_generation_config.args;

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
        commits: &[],
        target_branch: None,
    };
    let prompt = build_prompt(commit_generation_config, TemplateType::Commit, &context)?;

    execute_llm_command(command, args, &prompt).map_err(|e| {
        worktrunk::git::GitError::LlmCommandFailed {
            command: format_command_display(command, args),
            error: e.to_string(),
        }
        .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
            commits: &[],
            target_branch: None,
        }
    }

    /// Helper to create a squash context (all fields)
    fn squash_context<'a>(
        git_diff: &'a str,
        branch: &'a str,
        recent_commits: Option<&'a Vec<String>>,
        repo_name: &'a str,
        commits: &'a [String],
        target_branch: &'a str,
    ) -> TemplateContext<'a> {
        TemplateContext {
            git_diff,
            git_diff_stat: "",
            branch,
            recent_commits,
            repo_name,
            commits,
            target_branch: Some(target_branch),
        }
    }

    #[test]
    fn test_build_commit_prompt_with_default_template() {
        let config = CommitGenerationConfig::default();
        let context = commit_context("diff content", "main", None, "myrepo");
        let result = build_prompt(&config, TemplateType::Commit, &context);
        assert!(result.is_ok());
        let prompt = result.unwrap();
        assert!(prompt.contains("diff content"));
        assert!(prompt.contains("main"));
    }

    #[test]
    fn test_build_commit_prompt_with_recent_commits() {
        let config = CommitGenerationConfig::default();
        let commits = vec!["feat: add feature".to_string(), "fix: bug".to_string()];
        let context = commit_context("diff", "main", Some(&commits), "repo");
        let result = build_prompt(&config, TemplateType::Commit, &context);
        assert!(result.is_ok());
        let prompt = result.unwrap();
        assert!(prompt.contains("feat: add feature"));
        assert!(prompt.contains("fix: bug"));
        assert!(prompt.contains("<recent_commits>"));
    }

    #[test]
    fn test_build_commit_prompt_empty_recent_commits() {
        let config = CommitGenerationConfig::default();
        let commits = vec![];
        let context = commit_context("diff", "main", Some(&commits), "repo");
        let result = build_prompt(&config, TemplateType::Commit, &context);
        assert!(result.is_ok());
        // Should not render the recent commits data section if empty
        // Note: <recent_commits> is mentioned in the style text, but the actual
        // data section (with commit list) should not be rendered
        let prompt = result.unwrap();
        // The context section should have the branch but no recent_commits content
        assert!(prompt.contains("Branch: main"));
        assert!(!prompt.contains("- feat:"));
        assert!(!prompt.contains("- fix:"));
    }

    #[test]
    fn test_build_commit_prompt_with_custom_template() {
        let config = CommitGenerationConfig {
            command: None,
            args: vec![],
            template: Some("Branch: {{ branch }}\nDiff: {{ git_diff }}".to_string()),
            template_file: None,
            squash_template: None,
            squash_template_file: None,
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
            args: vec![],
            template: Some("{{ unclosed".to_string()),
            template_file: None,
            squash_template: None,
            squash_template_file: None,
        };
        let context = commit_context("diff", "main", None, "repo");
        let result = build_prompt(&config, TemplateType::Commit, &context);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_commit_prompt_empty_template() {
        let config = CommitGenerationConfig {
            command: None,
            args: vec![],
            template: Some("   ".to_string()),
            template_file: None,
            squash_template: None,
            squash_template_file: None,
        };
        let context = commit_context("diff", "main", None, "repo");
        let result = build_prompt(&config, TemplateType::Commit, &context);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Template is empty")
        );
    }

    #[test]
    fn test_build_commit_prompt_with_all_variables() {
        let config = CommitGenerationConfig {
            command: None,
            args: vec![],
            template: Some(
                "Repo: {{ repo }}\nBranch: {{ branch }}\nDiff: {{ git_diff }}\n{% for c in recent_commits %}{{ c }}\n{% endfor %}"
                    .to_string(),
            ),
            template_file: None,
            squash_template: None,
            squash_template_file: None,
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
    fn test_build_squash_prompt_with_default_template() {
        let config = CommitGenerationConfig::default();
        let commits = vec!["feat: A".to_string(), "fix: B".to_string()];
        let context = squash_context("diff content", "feature", None, "repo", &commits, "main");
        let result = build_prompt(&config, TemplateType::Squash, &context);
        assert!(result.is_ok());
        let prompt = result.unwrap();
        // Commits should be reversed (chronological order: B first, then A)
        assert!(prompt.contains("fix: B"));
        assert!(prompt.contains("feat: A"));
        assert!(prompt.contains("main"));
        // Default squash template now includes the diff
        assert!(prompt.contains("diff content"));
    }

    #[test]
    fn test_build_squash_prompt_with_custom_template() {
        let config = CommitGenerationConfig {
            command: None,
            args: vec![],
            template: None,
            template_file: None,
            squash_template: Some(
                "Target: {{ target_branch }}\n{% for c in commits %}{{ c }}\n{% endfor %}"
                    .to_string(),
            ),
            squash_template_file: None,
        };
        let commits = vec!["A".to_string(), "B".to_string()];
        let context = squash_context("diff", "feature", None, "repo", &commits, "main");
        let result = build_prompt(&config, TemplateType::Squash, &context);
        assert!(result.is_ok());
        // Commits are reversed, so chronological order is B, A
        assert_eq!(result.unwrap(), "Target: main\nB\nA\n");
    }

    #[test]
    fn test_build_squash_prompt_empty_commits() {
        let config = CommitGenerationConfig::default();
        let commits: Vec<String> = vec![];
        let context = squash_context("diff", "feature", None, "repo", &commits, "main");
        let result = build_prompt(&config, TemplateType::Squash, &context);
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_squash_prompt_malformed_jinja() {
        let config = CommitGenerationConfig {
            command: None,
            args: vec![],
            template: None,
            template_file: None,
            squash_template: Some("{% for x in commits %}{{ x }".to_string()),
            squash_template_file: None,
        };
        let commits: Vec<String> = vec![];
        let context = squash_context("diff", "feature", None, "repo", &commits, "main");
        let result = build_prompt(&config, TemplateType::Squash, &context);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_squash_prompt_empty_template() {
        let config = CommitGenerationConfig {
            command: None,
            args: vec![],
            template: None,
            template_file: None,
            squash_template: Some("  \n  ".to_string()),
            squash_template_file: None,
        };
        let commits: Vec<String> = vec![];
        let context = squash_context("diff", "feature", None, "repo", &commits, "main");
        let result = build_prompt(&config, TemplateType::Squash, &context);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Squash template is empty")
        );
    }

    #[test]
    fn test_build_squash_prompt_with_all_variables() {
        // Test that squash templates now have access to ALL variables including git_diff and recent_commits
        let config = CommitGenerationConfig {
            command: None,
            args: vec![],
            template: None,
            template_file: None,
            squash_template: Some(
                "Repo: {{ repo }}\nBranch: {{ branch }}\nTarget: {{ target_branch }}\nDiff: {{ git_diff }}\n{% for c in commits %}{{ c }}\n{% endfor %}{% for r in recent_commits %}style: {{ r }}\n{% endfor %}"
                    .to_string(),
            ),
            squash_template_file: None,
        };
        let commits = vec!["A".to_string(), "B".to_string()];
        let recent = vec!["prev1".to_string(), "prev2".to_string()];
        let context = squash_context(
            "the diff",
            "feature",
            Some(&recent),
            "myrepo",
            &commits,
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
            args: vec![],
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
        };
        let commits = vec![
            "feat: add auth".to_string(),
            "fix: bug".to_string(),
            "docs: update".to_string(),
        ];
        let context = commit_context("my diff content", "feature-x", Some(&commits), "myapp");
        let result = build_prompt(&config, TemplateType::Commit, &context);
        assert!(result.is_ok());
        let prompt = result.unwrap();

        // Verify filters work (upper)
        assert!(prompt.contains("=== MYAPP ==="));

        // Verify length filter
        assert!(prompt.contains("Commits: 3"));

        // Verify loop.index
        assert!(prompt.contains("  - 1. feat: add auth"));
        assert!(prompt.contains("  - 2. fix: bug"));
        assert!(prompt.contains("  - 3. docs: update"));

        // Verify whitespace control (no blank lines after "Branch:")
        assert!(prompt.contains("Branch: feature-x\nCommits: 3"));

        // Verify diff is included
        assert!(prompt.contains("Diff follows:\nmy diff content"));
    }

    #[test]
    fn test_build_commit_prompt_with_sophisticated_jinja_no_commits() {
        // Test the else branch of conditionals
        let config = CommitGenerationConfig {
            command: None,
            args: vec![],
            template: Some(
                r#"Repo: {{ repo | upper }}
{%- if recent_commits %}
Has commits: {{ recent_commits | length }}
{%- else %}
No recent commits
{%- endif %}"#
                    .to_string(),
            ),
            template_file: None,
            squash_template: None,
            squash_template_file: None,
        };
        let context = commit_context("diff", "main", None, "test");
        let result = build_prompt(&config, TemplateType::Commit, &context);
        assert!(result.is_ok());
        let prompt = result.unwrap();

        assert!(prompt.contains("Repo: TEST"));
        assert!(prompt.contains("No recent commits"));
        assert!(!prompt.contains("Has commits"));
    }

    #[test]
    fn test_build_squash_prompt_with_sophisticated_jinja() {
        // Test sophisticated jinja in squash templates
        let config = CommitGenerationConfig {
            command: None,
            args: vec![],
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
        };

        // Test with multiple commits
        let commits = vec![
            "commit A".to_string(),
            "commit B".to_string(),
            "commit C".to_string(),
        ];
        let context = squash_context("diff", "feature", None, "repo", &commits, "main");
        let result = build_prompt(&config, TemplateType::Squash, &context);
        assert!(result.is_ok());
        let prompt = result.unwrap();

        // Commits are reversed for chronological order, so we expect C, B, A
        assert!(prompt.contains("Squashing 3 commit(s) from feature to main"));
        assert!(prompt.contains("Multiple commits detected:"));
        assert!(prompt.contains("1/3: commit C")); // First in chronological order
        assert!(prompt.contains("2/3: commit B"));
        assert!(prompt.contains("3/3: commit A")); // Last in chronological order

        // Test with single commit
        let single_commit = vec!["solo commit".to_string()];
        let context = squash_context("diff", "feature", None, "repo", &single_commit, "main");
        let result = build_prompt(&config, TemplateType::Squash, &context);
        assert!(result.is_ok());
        let prompt = result.unwrap();

        assert!(prompt.contains("Squashing 1 commit(s)"));
        assert!(prompt.contains("Single commit: solo commit"));
        assert!(!prompt.contains("Multiple commits detected"));
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
            args: vec![],
            template: None,
            template_file: Some(template_path.to_string_lossy().to_string()),
            squash_template: None,
            squash_template_file: None,
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
            args: vec![],
            template: None,
            template_file: Some("/nonexistent/path/template.txt".to_string()),
            squash_template: None,
            squash_template_file: None,
        };
        let context = commit_context("diff", "main", None, "repo");
        let result = build_prompt(&config, TemplateType::Commit, &context);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Failed to read"));
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
            args: vec![],
            template: None,
            template_file: None,
            squash_template: None,
            squash_template_file: Some(template_path.to_string_lossy().to_string()),
        };
        let commits = vec!["A".to_string(), "B".to_string()];
        let context = squash_context("diff", "feature", None, "repo", &commits, "main");
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
            args: vec![],
            template: None,
            template_file: Some("~/nonexistent_template_for_test.txt".to_string()),
            squash_template: None,
            squash_template_file: None,
        };
        let context = commit_context("diff", "main", None, "repo");
        let result = build_prompt(&config, TemplateType::Commit, &context);
        // Should fail because file doesn't exist
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Failed to read"));
        // Error message may display ~ for readability, but the actual file read
        // should have used the expanded path (verified by the error occurring)
    }

    #[test]
    fn test_commit_template_can_access_squash_variables() {
        // Verify that commit templates can access squash-specific variables without errors
        // (they're empty/None for regular commits, but shouldn't cause template errors)
        let config = CommitGenerationConfig {
            command: None,
            args: vec![],
            template: Some(
                "Branch: {{ branch }}\nTarget: {{ target_branch }}\nCommits: {{ commits | length }}"
                    .to_string(),
            ),
            template_file: None,
            squash_template: None,
            squash_template_file: None,
        };
        let context = commit_context("diff", "feature", None, "repo");
        let result = build_prompt(&config, TemplateType::Commit, &context);
        assert!(result.is_ok());
        let prompt = result.unwrap();
        // Squash-specific variables are empty for regular commits
        assert_eq!(prompt, "Branch: feature\nTarget: \nCommits: 0");
    }

    // Tests for diff filtering

    #[test]
    fn test_is_lock_file() {
        assert!(is_lock_file("Cargo.lock"));
        assert!(is_lock_file("package-lock.json"));
        assert!(is_lock_file("pnpm-lock.yaml"));
        assert!(is_lock_file(".terraform.lock.hcl"));
        assert!(is_lock_file("path/to/Cargo.lock"));

        assert!(!is_lock_file("src/main.rs"));
        assert!(!is_lock_file("lockfile.txt"));
        assert!(!is_lock_file("my.lock.rs")); // Not a standard lock pattern
    }

    #[test]
    fn test_parse_diff_sections() {
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
        assert!(sections[0].1.contains("fn foo()"));
        assert_eq!(sections[1].0, "Cargo.lock");
        assert!(sections[1].1.contains("lots of lock content"));
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
        assert!(truncated.contains("diff --git"));
        assert!(truncated.contains("@@"));
        assert!(truncated.contains("... ("));
        assert!(truncated.contains("lines omitted)"));
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
}
