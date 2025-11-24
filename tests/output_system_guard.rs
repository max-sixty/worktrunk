use std::fs;
use std::path::Path;

#[test]
fn check_output_system_usage() {
    let project_root = env!("CARGO_MANIFEST_DIR");
    let restricted_files = [
        "src/commands/worktree.rs",
        "src/commands/merge.rs",
        "src/commands/list/mod.rs",
        "src/commands/list/collect.rs",
        "src/commands/list/collect_progressive_impl.rs",
    ];
    let forbidden_tokens = ["print!", "println!", "eprint!", "eprintln!"];
    let allowed_substrings = ["spacing_test.rs", "command_approval.rs"];

    let mut violations = Vec::new();

    for relative_path in restricted_files {
        let full_path = Path::new(project_root).join(relative_path);
        if !full_path.exists() {
            continue;
        }

        let contents = fs::read_to_string(&full_path)
            .unwrap_or_else(|err| panic!("failed to read {}: {err}", relative_path));

        // Check if file imports the correct anstream-wrapped macros from worktrunk::styling
        let has_styling_import = contents.contains("use worktrunk::styling::")
            && (contents.contains("eprintln") || contents.contains("println"));

        'line: for (idx, line) in contents.lines().enumerate() {
            for token in forbidden_tokens {
                if let Some(pos) = line.find(token) {
                    if allowed_substrings
                        .iter()
                        .any(|pattern| line.contains(pattern))
                    {
                        continue 'line;
                    }

                    if line
                        .find("//")
                        .map(|comment_pos| comment_pos <= pos)
                        .unwrap_or(false)
                    {
                        continue 'line;
                    }

                    // Allow if file imports the correct macro from worktrunk::styling
                    if has_styling_import && (token == "eprintln!" || token == "println!") {
                        continue 'line;
                    }

                    violations.push(format!("{}:{}: {}", relative_path, idx + 1, line.trim()));
                    continue 'line;
                }
            }
        }
    }

    if !violations.is_empty() {
        panic!(
            "Direct output macros found in commands using --internal:\n{}",
            violations.join("\n")
        );
    }
}
