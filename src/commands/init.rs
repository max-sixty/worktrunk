use worktrunk::shell;
use worktrunk::styling::println;

pub fn handle_init(shell: shell::Shell, cmd: String) -> Result<(), String> {
    let init = shell::ShellInit::with_prefix(shell, cmd);

    // Generate shell integration code (includes dynamic completion registration)
    let integration_output = init
        .generate()
        .map_err(|e| format!("Failed to generate shell code: {}", e))?;

    println!("{}", integration_output);

    Ok(())
}
