use anyhow::Context;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{self, Stdio};
use worktrunk::config::CommitGenerationConfig;
use worktrunk::git::Repository;
use worktrunk::path::format_path_for_display;

use minijinja::Environment;

/// Context data for building LLM prompts
///
/// All fields are available to both commit and squash templates.
/// Squash-specific fields (`commits`, `target_branch`) are empty/None for regular commits.
struct TemplateContext<'a> {
    /// The diff to describe (staged changes for commit, combined diff for squash)
    git_diff: &'a str,
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

/// Default template for commit message prompts
///
/// Synced to dev/config.example.toml by `cargo test readme_sync`
const DEFAULT_TEMPLATE: &str = r#"Format
- First line: <50 chars, present tense, describes WHAT and WHY (not HOW).
- Blank line after first line.
- Optional details with proper line breaks explaining context. Commits with more substantial changes should have more details.
- Return ONLY the formatted message without quotes, code blocks, or preamble.

Style
- Do not give normative statements or otherwise speculate on why the change was made.
- Broadly match the style of the previous commit messages.
  - For example, if they're in conventional commit format, use conventional commits; if they're not, don't use conventional commits.

The context contains:
- <git-diff> with the staged changes. This is the ONLY content you should base your message on.
- <git-info> with branch name and recent commit message titles for style reference ONLY. DO NOT use their content to inform your message.

---
The following is the context for your task:
---
<git-diff>
```
{{ git_diff }}
```
</git-diff>

<git-info>
  <current-branch>{{ branch }}</current-branch>
{% if recent_commits %}
  <previous-commit-message-titles>
{% for commit in recent_commits %}
    <previous-commit-message-title>{{ commit }}</previous-commit-message-title>
{% endfor %}
  </previous-commit-message-titles>
{% endif %}
</git-info>
"#;

/// Default template for squash commit message prompts
///
/// Synced to dev/config.example.toml by `cargo test readme_sync`
const DEFAULT_SQUASH_TEMPLATE: &str = r#"Generate a commit message that combines these changes into one cohesive message. Output only the commit message without any explanation.

Format
- First line: <50 chars, present tense, describes WHAT and WHY (not HOW).
- Blank line after first line.
- Optional details with proper line breaks explaining context.
- Match the style of the existing commits being squashed (e.g., if they use conventional commits, use conventional commits).

Squashing commits from {{ branch }} to merge into {{ target_branch }}

Commits being combined:
{% for commit in commits %}
- {{ commit }}
{% endfor %}

<git-diff>
```
{{ git_diff }}
```
</git-diff>
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
    let repo = Repository::current();

    // Get staged diff
    let diff_output = repo.run_command(&["--no-pager", "diff", "--staged"])?;

    // Get current branch
    let current_branch = repo.current_branch()?.unwrap_or_else(|| "HEAD".to_string());

    // Get repo name from directory
    let repo_root = repo.worktree_root()?;
    let repo_name = repo_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo");

    // Get recent commit messages for style reference
    let recent_commits = repo
        .run_command(&["log", "--pretty=format:%s", "-n", "5", "--no-merges"])
        .ok()
        .and_then(|output| {
            if output.trim().is_empty() {
                None
            } else {
                Some(output.lines().map(String::from).collect::<Vec<_>>())
            }
        });

    // Build prompt from template
    let context = TemplateContext {
        git_diff: &diff_output,
        branch: &current_branch,
        recent_commits: recent_commits.as_ref(),
        repo_name,
        commits: &[],
        target_branch: None,
    };
    let prompt = build_prompt(config, TemplateType::Commit, &context)?;

    execute_llm_command(command, args, &prompt)
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

        // Get the combined diff for all commits being squashed
        let repo = Repository::current();
        let diff_output = repo.run_command(&["--no-pager", "diff", merge_base, "HEAD"])?;

        // Get recent commit messages for style reference (from before the commits being squashed)
        let recent_commits = repo
            .run_command(&[
                "log",
                "--pretty=format:%s",
                "-n",
                "5",
                "--no-merges",
                merge_base,
            ])
            .ok()
            .and_then(|output| {
                if output.trim().is_empty() {
                    None
                } else {
                    Some(output.lines().map(String::from).collect::<Vec<_>>())
                }
            });

        // Build prompt from template with all variables
        let context = TemplateContext {
            git_diff: &diff_output,
            branch: current_branch,
            recent_commits: recent_commits.as_ref(),
            repo_name,
            commits: subjects,
            target_branch: Some(target_branch),
        };
        let prompt = build_prompt(commit_generation_config, TemplateType::Squash, &context)?;

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
        assert!(prompt.contains("<previous-commit-message-titles>"));
    }

    #[test]
    fn test_build_commit_prompt_empty_recent_commits() {
        let config = CommitGenerationConfig::default();
        let commits = vec![];
        let context = commit_context("diff", "main", Some(&commits), "repo");
        let result = build_prompt(&config, TemplateType::Commit, &context);
        assert!(result.is_ok());
        // Should not render the recent commits section if empty
        let prompt = result.unwrap();
        assert!(!prompt.contains("<previous-commit-message-titles>"));
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
}
