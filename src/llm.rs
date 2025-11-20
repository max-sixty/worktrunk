use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{self, Stdio};
use worktrunk::config::CommitGenerationConfig;
use worktrunk::git::{GitError, Repository};
use worktrunk::path::format_path_for_display;

use minijinja::Environment;

/// Context data for building LLM prompts
struct PromptContext<'a> {
    branch: &'a str,
    repo_name: &'a str,
}

/// Default template for commit message prompts
///
/// TODO: Generate config.example.toml dynamically to keep templates in sync.
/// Currently this constant must be manually copied to config.example.toml (lines 41-76).
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
/// TODO: Generate config.example.toml dynamically to keep templates in sync.
/// Currently this constant must be manually copied to config.example.toml (lines 109-118).
const DEFAULT_SQUASH_TEMPLATE: &str = r#"Generate a conventional commit message (feat/fix/docs/style/refactor) that combines these changes into one cohesive message. Output only the commit message without any explanation.

Squashing commits on current branch since branching from {{ target_branch }}

Commits being combined:
{% for commit in commits %}
- {{ commit }}
{% endfor %}
"#;

/// Execute an LLM command with the given prompt via stdin.
///
/// This is the canonical way to execute LLM commands in this codebase.
/// All LLM execution should go through this function to maintain consistency.
fn execute_llm_command(
    command: &str,
    args: &[String],
    prompt: &str,
) -> Result<String, Box<dyn std::error::Error>> {
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

    let mut child = cmd.spawn()?;

    // Write prompt to stdin
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(prompt.as_bytes())?;
        // stdin is dropped here, closing the pipe
    }

    let output = child.wait_with_output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("LLM command failed: {}", stderr).into());
    }

    let message = String::from_utf8_lossy(&output.stdout).trim().to_owned();

    if message.is_empty() {
        return Err("LLM returned empty message".into());
    }

    Ok(message)
}

/// Build the commit prompt from config template or default using minijinja
fn build_commit_prompt(
    config: &CommitGenerationConfig,
    diff: &str,
    recent_commits: Option<&Vec<String>>,
    context: &PromptContext<'_>,
) -> Result<String, Box<dyn std::error::Error>> {
    // Get template source
    let template = match (&config.template, &config.template_file) {
        (Some(inline), None) => inline.clone(),
        (None, Some(path)) => {
            let expanded_path = PathBuf::from(shellexpand::tilde(path).as_ref());
            std::fs::read_to_string(&expanded_path).map_err(|e| {
                format!(
                    "Failed to read template-file '{}': {}",
                    format_path_for_display(&expanded_path),
                    e
                )
            })?
        }
        (None, None) => DEFAULT_TEMPLATE.to_string(),
        (Some(_), Some(_)) => {
            unreachable!("Config validation should prevent both template and template-file")
        }
    };

    // Validate non-empty
    if template.trim().is_empty() {
        return Err("Template is empty".into());
    }

    // Render template with minijinja
    let env = Environment::new();
    let tmpl = env.template_from_str(&template)?;

    let rendered = tmpl.render(minijinja::context! {
        git_diff => diff,
        branch => context.branch,
        recent_commits => recent_commits.unwrap_or(&vec![]),
        repo => context.repo_name,
    })?;

    Ok(rendered)
}

/// Build the squash commit prompt from config template or default using minijinja
fn build_squash_prompt(
    config: &CommitGenerationConfig,
    target_branch: &str,
    commits: &[String],
    context: &PromptContext<'_>,
) -> Result<String, Box<dyn std::error::Error>> {
    // Get template source
    let template = match (&config.squash_template, &config.squash_template_file) {
        (Some(inline), None) => inline.clone(),
        (None, Some(path)) => {
            let expanded_path = PathBuf::from(shellexpand::tilde(path).as_ref());
            std::fs::read_to_string(&expanded_path).map_err(|e| {
                format!(
                    "Failed to read squash-template-file '{}': {}",
                    format_path_for_display(&expanded_path),
                    e
                )
            })?
        }
        (None, None) => DEFAULT_SQUASH_TEMPLATE.to_string(),
        (Some(_), Some(_)) => {
            unreachable!(
                "Config validation should prevent both squash-template and squash-template-file"
            )
        }
    };

    // Validate non-empty
    if template.trim().is_empty() {
        return Err("Squash template is empty".into());
    }

    // Render template with minijinja
    let env = Environment::new();
    let tmpl = env.template_from_str(&template)?;

    // Reverse commits so they're in chronological order
    let commits_reversed: Vec<&String> = commits.iter().rev().collect();

    let rendered = tmpl.render(minijinja::context! {
        target_branch => target_branch,
        commits => commits_reversed,
        branch => context.branch,
        repo => context.repo_name,
    })?;

    Ok(rendered)
}

pub fn generate_commit_message(
    commit_generation_config: &CommitGenerationConfig,
) -> Result<String, GitError> {
    // Check if commit generation is configured (non-empty command)
    if commit_generation_config.is_configured() {
        let command = commit_generation_config.command.as_ref().unwrap();
        // Commit generation is explicitly configured - fail if it doesn't work
        return try_generate_commit_message(
            command,
            &commit_generation_config.args,
            commit_generation_config,
        )
        .map_err(|e| {
            GitError::CommandFailed(format!(
                "Commit generation command '{}' failed: {}",
                command, e
            ))
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
) -> Result<String, Box<dyn std::error::Error>> {
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
    let context = PromptContext {
        branch: &current_branch,
        repo_name,
    };
    let prompt = build_commit_prompt(config, &diff_output, recent_commits.as_ref(), &context)?;

    execute_llm_command(command, args, &prompt)
}

pub fn generate_squash_message(
    target_branch: &str,
    subjects: &[String],
    current_branch: &str,
    repo_name: &str,
    commit_generation_config: &CommitGenerationConfig,
) -> Result<String, Box<dyn std::error::Error>> {
    // Check if commit generation is configured (non-empty command)
    if commit_generation_config.is_configured() {
        let command = commit_generation_config.command.as_ref().unwrap();
        // Commit generation is explicitly configured - fail if it doesn't work
        let context = PromptContext {
            branch: current_branch,
            repo_name,
        };
        let prompt =
            build_squash_prompt(commit_generation_config, target_branch, subjects, &context)?;
        return execute_llm_command(command, &commit_generation_config.args, &prompt);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_commit_prompt_with_default_template() {
        let config = CommitGenerationConfig::default();
        let context = PromptContext {
            branch: "main",
            repo_name: "myrepo",
        };
        let result = build_commit_prompt(&config, "diff content", None, &context);
        assert!(result.is_ok());
        let prompt = result.unwrap();
        assert!(prompt.contains("diff content"));
        assert!(prompt.contains("main"));
        // Default template doesn't directly show repo name in output
    }

    #[test]
    fn test_build_commit_prompt_with_recent_commits() {
        let config = CommitGenerationConfig::default();
        let commits = vec!["feat: add feature".to_string(), "fix: bug".to_string()];
        let context = PromptContext {
            branch: "main",
            repo_name: "repo",
        };
        let result = build_commit_prompt(&config, "diff", Some(&commits), &context);
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
        let context = PromptContext {
            branch: "main",
            repo_name: "repo",
        };
        let result = build_commit_prompt(&config, "diff", Some(&commits), &context);
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
        let context = PromptContext {
            branch: "feature",
            repo_name: "repo",
        };
        let result = build_commit_prompt(&config, "my diff", None, &context);
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
        let context = PromptContext {
            branch: "main",
            repo_name: "repo",
        };
        let result = build_commit_prompt(&config, "diff", None, &context);
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
        let context = PromptContext {
            branch: "main",
            repo_name: "repo",
        };
        let result = build_commit_prompt(&config, "diff", None, &context);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "Template is empty");
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
        let context = PromptContext {
            branch: "feature",
            repo_name: "myrepo",
        };
        let result = build_commit_prompt(&config, "my diff", Some(&commits), &context);
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
        let context = PromptContext {
            branch: "feature",
            repo_name: "repo",
        };
        let result = build_squash_prompt(&config, "main", &commits, &context);
        assert!(result.is_ok());
        let prompt = result.unwrap();
        // Commits should be reversed (chronological order: B first, then A)
        assert!(prompt.contains("fix: B"));
        assert!(prompt.contains("feat: A"));
        assert!(prompt.contains("main"));
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
        let context = PromptContext {
            branch: "feature",
            repo_name: "repo",
        };
        let result = build_squash_prompt(&config, "main", &commits, &context);
        assert!(result.is_ok());
        // Commits are reversed, so chronological order is B, A
        assert_eq!(result.unwrap(), "Target: main\nB\nA\n");
    }

    #[test]
    fn test_build_squash_prompt_empty_commits() {
        let config = CommitGenerationConfig::default();
        let commits = vec![];
        let context = PromptContext {
            branch: "feature",
            repo_name: "repo",
        };
        let result = build_squash_prompt(&config, "main", &commits, &context);
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
        let context = PromptContext {
            branch: "feature",
            repo_name: "repo",
        };
        let result = build_squash_prompt(&config, "main", &[], &context);
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
        let context = PromptContext {
            branch: "feature",
            repo_name: "repo",
        };
        let result = build_squash_prompt(&config, "main", &[], &context);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "Squash template is empty");
    }

    #[test]
    fn test_build_squash_prompt_with_all_variables() {
        let config = CommitGenerationConfig {
            command: None,
            args: vec![],
            template: None,
            template_file: None,
            squash_template: Some(
                "Repo: {{ repo }}\nBranch: {{ branch }}\nTarget: {{ target_branch }}\n{% for c in commits %}{{ c }}\n{% endfor %}"
                    .to_string(),
            ),
            squash_template_file: None,
        };
        let commits = vec!["A".to_string(), "B".to_string()];
        let context = PromptContext {
            branch: "feature",
            repo_name: "myrepo",
        };
        let result = build_squash_prompt(&config, "main", &commits, &context);
        assert!(result.is_ok());
        let prompt = result.unwrap();
        assert_eq!(
            prompt,
            "Repo: myrepo\nBranch: feature\nTarget: main\nB\nA\n"
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
        let context = PromptContext {
            branch: "feature-x",
            repo_name: "myapp",
        };
        let result = build_commit_prompt(&config, "my diff content", Some(&commits), &context);
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
        let context = PromptContext {
            branch: "main",
            repo_name: "test",
        };
        let result = build_commit_prompt(&config, "diff", None, &context);
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
        let context = PromptContext {
            branch: "feature",
            repo_name: "repo",
        };
        let result = build_squash_prompt(&config, "main", &commits, &context);
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
        let result = build_squash_prompt(&config, "main", &single_commit, &context);
        assert!(result.is_ok());
        let prompt = result.unwrap();

        assert!(prompt.contains("Squashing 1 commit(s)"));
        assert!(prompt.contains("Single commit: solo commit"));
        assert!(!prompt.contains("Multiple commits detected"));
    }
}
