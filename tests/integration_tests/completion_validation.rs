use std::collections::HashSet;
use std::process::Command;
use worktrunk::styling::SUCCESS_EMOJI;

/// Issue found during validation
#[derive(Debug)]
struct Issue {
    shell: String,
    severity: Severity,
    category: Category,
    message: String,
}

#[derive(Debug, PartialEq)]
enum Severity {
    Error,
    Warning,
}

#[derive(Debug)]
enum Category {
    HiddenFlag,
    Consistency,
}

impl Issue {
    fn error(shell: impl Into<String>, category: Category, message: impl Into<String>) -> Self {
        Self {
            shell: shell.into(),
            severity: Severity::Error,
            category,
            message: message.into(),
        }
    }

    fn warning(shell: impl Into<String>, category: Category, message: impl Into<String>) -> Self {
        Self {
            shell: shell.into(),
            severity: Severity::Warning,
            category,
            message: message.into(),
        }
    }
}

/// Validate fish shell completions
fn validate_fish(content: &str) -> Vec<Issue> {
    let mut issues = Vec::new();

    // Check for hidden flags
    for line in content.lines() {
        if line.contains("complete -c wt") && line.contains("-l internal") {
            issues.push(Issue::error(
                "fish",
                Category::HiddenFlag,
                "Hidden flag --internal appears in completions",
            ));
        }
    }

    issues
}

/// Validate bash shell completions
fn validate_bash(content: &str) -> Vec<Issue> {
    let mut issues = Vec::new();

    // Check for hidden flags in opts strings
    // Note: We don't validate bash syntax here. If you need that, use shellcheck.
    // We only check that our filtering logic removed hidden flags.
    for line in content.lines() {
        if line.contains("opts=") && line.contains("--internal") {
            issues.push(Issue::error(
                "bash",
                Category::HiddenFlag,
                "Hidden flag --internal appears in opts",
            ));
        }
    }

    issues
}

/// Validate zsh shell completions
fn validate_zsh(content: &str) -> Vec<Issue> {
    let mut issues = Vec::new();

    // Check for hidden flags in argument specs
    for line in content.lines() {
        if line.contains("'--internal[") {
            issues.push(Issue::error(
                "zsh",
                Category::HiddenFlag,
                "Hidden flag --internal appears in completions",
            ));
        }
    }

    issues
}

/// Extract flags from shell completion content
fn extract_flags(content: &str, shell: &str) -> HashSet<String> {
    let mut flags = HashSet::new();

    match shell {
        "fish" => {
            // Only from 'complete -c wt' lines, not local variables
            for line in content.lines() {
                if line.contains("complete -c wt")
                    && let Some(captures) = line.split("-l ").nth(1)
                    && let Some(flag) = captures.split_whitespace().next()
                {
                    flags.insert(flag.to_string());
                }
            }
        }
        "bash" => {
            // From opts= lines
            for line in content.lines() {
                if let Some(opts_start) = line.find("opts=\"") {
                    // Find the closing quote
                    let search_from = opts_start + 6;
                    if let Some(rel_end) = line[search_from..].find('"') {
                        let opts_end = search_from + rel_end;
                        let opts_str = &line[search_from..opts_end];
                        for word in opts_str.split_whitespace() {
                            if let Some(flag) = word.strip_prefix("--") {
                                flags.insert(flag.to_string());
                            }
                        }
                    }
                }
            }
            // Also from case statements
            for line in content.lines() {
                if let Some(stripped) = line
                    .trim()
                    .strip_prefix("--")
                    .and_then(|s| s.strip_suffix(')'))
                {
                    flags.insert(stripped.to_string());
                }
            }
        }
        "zsh" => {
            // From _arguments lines
            for line in content.lines() {
                if let Some(start) = line.find("'--")
                    && let Some(rest) = line[start + 3..].split(&['[', '='][..]).next()
                {
                    flags.insert(rest.to_string());
                }
            }
        }
        _ => {}
    }

    flags
}

/// Validate cross-shell consistency
fn validate_cross_shell(fish_content: &str, bash_content: &str, zsh_content: &str) -> Vec<Issue> {
    let mut issues = Vec::new();

    let fish_flags = extract_flags(fish_content, "fish");
    let bash_flags = extract_flags(bash_content, "bash");
    let zsh_flags = extract_flags(zsh_content, "zsh");

    // Flags that should be hidden
    let hidden_flags: HashSet<String> = ["internal"].iter().map(|s| s.to_string()).collect();

    // Check if hidden flags appear anywhere
    for flag in &hidden_flags {
        let mut appears_in = Vec::new();
        if fish_flags.contains(flag) {
            appears_in.push("fish");
        }
        if bash_flags.contains(flag) {
            appears_in.push("bash");
        }
        if zsh_flags.contains(flag) {
            appears_in.push("zsh");
        }

        if !appears_in.is_empty() {
            issues.push(Issue::error(
                "cross-shell",
                Category::HiddenFlag,
                format!(
                    "Hidden flag --{} appears in: {}",
                    flag,
                    appears_in.join(", ")
                ),
            ));
        }
    }

    // Check for flags missing from some shells
    // (only report if missing from multiple shells - single shell might be intentional)
    let all_flags: HashSet<_> = fish_flags
        .union(&bash_flags)
        .chain(zsh_flags.iter())
        .filter(|f| !hidden_flags.contains(*f))
        .collect();

    for flag in all_flags {
        let in_fish = fish_flags.contains(flag);
        let in_bash = bash_flags.contains(flag);
        let in_zsh = zsh_flags.contains(flag);

        let present_count = [in_fish, in_bash, in_zsh].iter().filter(|&&x| x).count();

        // Only warn if flag is in exactly one shell (likely a bug)
        // If it's in 2 shells, might be intentional
        if present_count == 1 {
            let mut missing = Vec::new();
            if !in_fish {
                missing.push("fish");
            }
            if !in_bash {
                missing.push("bash");
            }
            if !in_zsh {
                missing.push("zsh");
            }

            issues.push(Issue::warning(
                "cross-shell",
                Category::Consistency,
                format!("Flag --{} missing from: {}", flag, missing.join(", ")),
            ));
        }
    }

    issues
}

#[test]
fn test_completion_validation() {
    // Generate completions
    let fish_output = Command::new(assert_cmd::cargo::cargo_bin!("wt"))
        .arg("init")
        .arg("fish")
        .output()
        .expect("Failed to generate fish completion");

    let bash_output = Command::new(assert_cmd::cargo::cargo_bin!("wt"))
        .arg("init")
        .arg("bash")
        .output()
        .expect("Failed to generate bash completion");

    let zsh_output = Command::new(assert_cmd::cargo::cargo_bin!("wt"))
        .arg("init")
        .arg("zsh")
        .output()
        .expect("Failed to generate zsh completion");

    assert!(fish_output.status.success());
    assert!(bash_output.status.success());
    assert!(zsh_output.status.success());

    let fish_content = String::from_utf8_lossy(&fish_output.stdout);
    let bash_content = String::from_utf8_lossy(&bash_output.stdout);
    let zsh_content = String::from_utf8_lossy(&zsh_output.stdout);

    // Run all validators
    let mut all_issues = Vec::new();
    all_issues.extend(validate_fish(&fish_content));
    all_issues.extend(validate_bash(&bash_content));
    all_issues.extend(validate_zsh(&zsh_content));
    all_issues.extend(validate_cross_shell(
        &fish_content,
        &bash_content,
        &zsh_content,
    ));

    // Separate errors and warnings
    let errors: Vec<_> = all_issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .collect();
    let warnings: Vec<_> = all_issues
        .iter()
        .filter(|i| i.severity == Severity::Warning)
        .collect();

    // Report issues
    if !errors.is_empty() {
        eprintln!("\n{}", "=".repeat(80));
        eprintln!("COMPLETION VALIDATION ERRORS ({})", errors.len());
        eprintln!("{}", "=".repeat(80));
        for issue in &errors {
            eprintln!(
                "❌ [{}] {:?}: {}",
                issue.shell, issue.category, issue.message
            );
        }
    }

    if !warnings.is_empty() {
        eprintln!("\n{}", "=".repeat(80));
        eprintln!("COMPLETION VALIDATION WARNINGS ({})", warnings.len());
        eprintln!("{}", "=".repeat(80));
        for issue in &warnings {
            eprintln!(
                "⚠️  [{}] {:?}: {}",
                issue.shell, issue.category, issue.message
            );
        }
    }

    // Fail on errors only
    if !errors.is_empty() {
        panic!(
            "\n{} completion validation error(s) found - see output above",
            errors.len()
        );
    }

    if errors.is_empty() && warnings.is_empty() {
        println!("{SUCCESS_EMOJI} All shell completions validated successfully!");
    }
}
