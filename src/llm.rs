use std::process;
use worktrunk::config::CommitGenerationConfig;
use worktrunk::git::{GitError, Repository};

pub fn generate_commit_message(
    custom_instruction: Option<&str>,
    commit_generation_config: &CommitGenerationConfig,
) -> Result<String, GitError> {
    // Check if commit generation is configured (non-empty command)
    if let Some(ref command) = commit_generation_config.command
        && !command.trim().is_empty()
    {
        // Commit generation is explicitly configured - fail if it doesn't work
        return try_generate_commit_message(
            custom_instruction,
            command,
            &commit_generation_config.args,
        )
        .map_err(|e| {
            GitError::CommandFailed(format!(
                "Commit generation command '{}' failed: {}",
                command, e
            ))
        });
    }

    // Fallback: simple deterministic commit message (only when not configured)
    Ok("WIP: Auto-commit before merge".to_string())
}

fn try_generate_commit_message(
    custom_instruction: Option<&str>,
    command: &str,
    args: &[String],
) -> Result<String, Box<dyn std::error::Error>> {
    use std::io::Write;
    use std::process::Stdio;

    let repo = Repository::current();

    // Get staged diff
    let diff_output = repo.run_command(&["--no-pager", "diff", "--staged"])?;

    // Get current branch
    let current_branch = repo.current_branch()?.unwrap_or_else(|| "HEAD".to_string());

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

    // Build the prompt following the Fish function format exactly
    let user_instruction = custom_instruction
        .unwrap_or("Write a concise, clear git commit message based on the provided diff.");

    let mut prompt = String::new();

    // Format section
    prompt.push_str("Format\n");
    prompt.push_str("- First line: <50 chars, present tense, describes WHAT and WHY (not HOW).\n");
    prompt.push_str("- Blank line after first line.\n");
    prompt.push_str("- Optional details with proper line breaks explaining context. Commits with more substantial changes should have more details.\n");
    prompt.push_str(
        "- Return ONLY the formatted message without quotes, code blocks, or preamble.\n",
    );
    prompt.push('\n');

    // Style section
    prompt.push_str("Style\n");
    prompt.push_str(
        "- Do not give normative statements or otherwise speculate on why the change was made.\n",
    );
    prompt.push_str("- Broadly match the style of the previous commit messages.\n");
    prompt.push_str("  - For example, if they're in conventional commit format, use conventional commits; if they're not, don't use conventional commits.\n");
    prompt.push('\n');

    // Context description
    prompt.push_str("The context contains:\n");
    prompt.push_str("- <git-diff> with the staged changes. This is the ONLY content you should base your message on.\n");
    prompt.push_str("- <git-info> with branch name and recent commit message titles for style reference ONLY. DO NOT use their content to inform your message.\n");
    prompt.push('\n');
    prompt.push_str("---\n");
    prompt.push_str("The following is the context for your task:\n");
    prompt.push_str("---\n");

    // Git diff section
    prompt.push_str("<git-diff>\n```\n");
    prompt.push_str(&diff_output);
    prompt.push_str("\n```\n</git-diff>\n\n");

    // Git info section
    prompt.push_str("<git-info>\n");
    prompt.push_str(&format!(
        "  <current-branch>{}</current-branch>\n",
        current_branch
    ));

    if let Some(commits) = recent_commits {
        prompt.push_str("  <previous-commit-message-titles>\n");
        for commit in commits {
            prompt.push_str(&format!(
                "    <previous-commit-message-title>{}</previous-commit-message-title>\n",
                commit
            ));
        }
        prompt.push_str("  </previous-commit-message-titles>\n");
    }

    prompt.push_str("</git-info>\n\n");

    // Execute LLM command with prompt via stdin
    log::debug!("$ {} {}", command, args.join(" "));
    log::debug!("  System: {}", user_instruction);
    log::debug!("  Prompt (stdin):");
    for line in prompt.lines() {
        log::debug!("    {}", line);
    }

    let mut child = process::Command::new(command)
        .args(args)
        .arg("--system")
        .arg(user_instruction)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

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

pub fn generate_squash_message(
    target_branch: &str,
    subjects: &[String],
    commit_generation_config: &CommitGenerationConfig,
) -> Result<String, Box<dyn std::error::Error>> {
    // Check if commit generation is configured (non-empty command)
    if let Some(ref command) = commit_generation_config.command
        && !command.trim().is_empty()
    {
        // Commit generation is explicitly configured - fail if it doesn't work
        return try_generate_llm_message(
            target_branch,
            subjects,
            command,
            &commit_generation_config.args,
        );
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

fn try_generate_llm_message(
    target_branch: &str,
    subjects: &[String],
    command: &str,
    args: &[String],
) -> Result<String, Box<dyn std::error::Error>> {
    use std::io::Write;
    use std::process::Stdio;

    // Build context prompt
    let mut context = format!(
        "Squashing commits on current branch since branching from {}\n\n",
        target_branch
    );
    context.push_str("Commits being combined:\n");
    for subject in subjects.iter().rev() {
        context.push_str(&format!("- {}\n", subject));
    }

    let prompt = "Generate a conventional commit message (feat/fix/docs/style/refactor) that combines these changes into one cohesive message. Output only the commit message without any explanation.";
    let full_prompt = format!("{}\n\n{}", context, prompt);

    // Execute LLM command with prompt via stdin
    log::debug!("$ {} {}", command, args.join(" "));
    log::debug!("  Prompt (stdin):");
    for line in full_prompt.lines() {
        log::debug!("  {}", line);
    }

    let mut child = process::Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // Write prompt to stdin
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(full_prompt.as_bytes())?;
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
