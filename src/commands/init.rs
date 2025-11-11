use clap::Command;
use clap_complete::{Shell as CompletionShell, generate};
use worktrunk::shell;
use worktrunk::styling::{ERROR, ERROR_EMOJI, println};

pub fn handle_init(shell: shell::Shell, cli_cmd: &mut Command) -> Result<(), String> {
    let init = shell::ShellInit::new(shell, "wt".to_string());

    // Generate shell integration code
    let integration_output = init
        .generate(cli_cmd)
        .map_err(|e| format!("Failed to generate shell code: {}", e))?;

    println!("{}", integration_output);

    // Generate and append static completions
    println!();
    println!("# Static completions (commands and flags)");

    // Check if shell supports completion
    if !shell.supports_completion() {
        println!("{ERROR_EMOJI} {ERROR}Completion not yet supported for {shell}{ERROR:#}");
        std::process::exit(1);
    }

    // Generate completions to a string so we can filter out hidden commands
    let mut completion_output = Vec::new();
    let completion_shell = match shell {
        shell::Shell::Bash | shell::Shell::Oil => CompletionShell::Bash,
        shell::Shell::Fish => CompletionShell::Fish,
        shell::Shell::Zsh => CompletionShell::Zsh,
        _ => unreachable!(
            "supports_completion() check above ensures we only reach this for supported shells"
        ),
    };
    generate(completion_shell, cli_cmd, "wt", &mut completion_output);

    // Filter out lines for hidden commands (completion, complete) and hidden flags (--internal)
    let completion_str = String::from_utf8_lossy(&completion_output);

    for line in completion_str.lines() {
        // Skip lines that complete the hidden commands
        if line.contains("\"completion\"")
            || line.contains("\"complete\"")
            || line.contains("-a \"completion\"")
            || line.contains("-a \"complete\"")
        {
            continue;
        }

        // Skip lines that are specifically for completing --internal flag
        // But DON'T skip opts= lines that just mention --internal in the list
        if line.contains("-l internal") && !line.contains("opts=") {
            // Fish: complete -c wt ... -l internal ...
            continue;
        }
        if line.contains("'--internal[") {
            // Zsh: '--internal[Use internal mode]'
            continue;
        }

        // For bash opts= lines and zsh argument specs, remove --internal from the string
        let line = if line.contains("--internal") {
            // Bash: opts="-c -b --create --base --internal --help"
            // Remove --internal and -internal from opts strings
            line.replace(" --internal", "").replace("--internal ", "")
        } else {
            line.to_string()
        };

        // For Fish: Add --source to global optspecs so argparse recognizes it
        // --source is shell-wrapper-only but Fish's argparse needs to know about it
        // for completion to work when users type "wt --source <TAB>"
        let line = if matches!(shell, shell::Shell::Fish)
            && line.contains("string join \\n v/verbose internal")
        {
            line.replace(
                "string join \\n v/verbose internal",
                "string join \\n source v/verbose internal",
            )
        } else {
            line
        };

        // For Zsh: Guard the final compdef call to avoid errors when completion system isn't loaded
        // Not all users have compinit in their .zshrc, and --no-rcs mode never loads it
        let line = if matches!(shell, shell::Shell::Zsh) && line.trim() == "compdef _wt wt" {
            // Replace with guarded version that checks if compdef exists
            "    if (( $+functions[compdef] )); then compdef _wt wt; fi".to_string()
        } else {
            line
        };

        println!("{}", line);
    }

    Ok(())
}
