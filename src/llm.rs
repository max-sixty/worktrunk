use std::process;
use worktrunk::config::LlmConfig;
use worktrunk::styling::{WARNING, WARNING_EMOJI, eprintln};

pub fn generate_squash_message(
    target_branch: &str,
    subjects: &[String],
    llm_config: &LlmConfig,
) -> String {
    // Try LLM generation if configured
    if let Some(ref command) = llm_config.command {
        if let Ok(llm_message) =
            try_generate_llm_message(target_branch, subjects, command, &llm_config.args)
        {
            return llm_message;
        }
        // If LLM fails, fall through to deterministic approach
        eprintln!(
            "{WARNING_EMOJI} {WARNING}LLM generation failed, using deterministic message{WARNING:#}"
        );
    }

    // Fallback: deterministic commit message
    let mut commit_message = format!("Squash commits from {}\n\n", target_branch);
    commit_message.push_str("Combined commits:\n");
    for subject in subjects.iter().rev() {
        // Reverse so they're in chronological order
        commit_message.push_str(&format!("- {}\n", subject));
    }
    commit_message
}

fn try_generate_llm_message(
    target_branch: &str,
    subjects: &[String],
    command: &str,
    args: &[String],
) -> Result<String, Box<dyn std::error::Error>> {
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

    // Execute LLM command
    let output = process::Command::new(command)
        .args(args)
        .arg(&full_prompt)
        .output()?;

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
