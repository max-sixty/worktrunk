use worktrunk::shell;
use worktrunk::styling::println;

pub fn handle_init(shell: shell::Shell) -> Result<(), String> {
    let init = shell::ShellInit::new(shell);

    // Generate shell integration code (includes dynamic completion registration)
    let integration_output = init
        .generate()
        .map_err(|e| format!("Failed to generate shell code: {}", e))?;

    println!("{}", integration_output);

    Ok(())
}
