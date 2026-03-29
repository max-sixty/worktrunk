//! Documentation text transformations.
//!
//! Converts `$ `‐prefixed `console` code blocks into Zola `{% terminal() %}`
//! shortcodes. Used by both the `--help-page` generator (CLI source → web docs)
//! and the doc sync test (hand-written docs → web docs).

/// Convert `$ `‐prefixed console blocks into `{% terminal() %}` shortcodes.
///
/// All shell commands in `console` blocks use `$ ` prefix. This function detects
/// them and emits the appropriate shortcode form:
///
/// - Single command, no `{{ }}`: `{{ terminal(cmd="...") }}` (Syntect highlighting)
/// - Single command + output, no `{{ }}`: `{% terminal(cmd="...") %}output{% end %}`
/// - Multiple commands or `{{ }}`: `{% terminal() %}` with `<span class="cmd">` body
///
/// Blocks without `$ ` are left unchanged for the `console` → `bash` replacement.
pub fn convert_dollar_console_to_terminal(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut lines = text.lines().peekable();

    while let Some(line) = lines.next() {
        if line.trim_start() == "```console" {
            // Collect the block, then decide whether to convert
            let mut block_lines = Vec::new();
            for content_line in lines.by_ref() {
                let stripped = content_line.trim_start();
                if stripped.starts_with("```")
                    && (stripped.len() == 3 || !stripped.as_bytes()[3].is_ascii_alphabetic())
                {
                    break;
                }
                block_lines.push(content_line);
            }

            // Only convert if the block contains $ commands
            let commands: Vec<_> = block_lines
                .iter()
                .filter_map(|l| l.strip_prefix("$ "))
                .collect();

            if commands.is_empty() {
                // No $ lines — emit unchanged as console block
                result.push_str(line);
                result.push('\n');
                for bl in &block_lines {
                    result.push_str(bl);
                    result.push('\n');
                }
                result.push_str("```\n");
                continue;
            }

            let has_template_syntax = commands.iter().any(|c| c.contains("{{"));

            if has_template_syntax {
                // {{ }} in commands — must use body approach (Tera would interpret
                // template syntax in cmd parameter). Accent color only.
                result.push_str("{% terminal() %}\n");
                for bl in &block_lines {
                    if let Some(cmd) = bl.strip_prefix("$ ") {
                        result.push_str("<span class=\"cmd\">");
                        for ch in cmd.chars() {
                            match ch {
                                '<' => result.push_str("&lt;"),
                                '>' => result.push_str("&gt;"),
                                '&' => result.push_str("&amp;"),
                                _ => result.push(ch),
                            }
                        }
                        result.push_str("</span>\n");
                    } else {
                        result.push_str(bl);
                        result.push('\n');
                    }
                }
                result.push_str("{% end %}\n");
            } else {
                // No {{ }} — use cmd parameter for Syntect highlighting.
                // Multiple commands/comments joined with ||| delimiter;
                // the template splits and highlights each line individually.
                let cmd_value: Vec<_> = block_lines
                    .iter()
                    .filter_map(|l| {
                        if let Some(cmd) = l.strip_prefix("$ ") {
                            Some(cmd.replace('"', "&quot;"))
                        } else if l.starts_with('#') || l.is_empty() {
                            Some(l.to_string())
                        } else {
                            None // output lines go in body
                        }
                    })
                    .collect();
                let body_lines: Vec<_> = block_lines
                    .iter()
                    .filter(|l| !l.starts_with("$ ") && !l.starts_with('#') && !l.is_empty())
                    .collect();

                let cmd_str = cmd_value.join("|||");
                if body_lines.is_empty() {
                    result.push_str(&format!("{{{{ terminal(cmd=\"{cmd_str}\") }}}}\n"));
                } else {
                    result.push_str(&format!("{{% terminal(cmd=\"{cmd_str}\") %}}\n"));
                    for bl in &body_lines {
                        result.push_str(bl);
                        result.push('\n');
                    }
                    result.push_str("{% end %}\n");
                }
            }
            continue;
        }
        result.push_str(line);
        result.push('\n');
    }

    // .lines() strips the trailing newline; match original
    if !text.ends_with('\n') {
        result.pop();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    #[test]
    fn test_convert_dollar_console_to_terminal() {
        // Command+output with {{ }} → body approach
        assert_snapshot!(convert_dollar_console_to_terminal(
            "```console\n$ wt step eval '{{ branch | hash_port }}'\n16066\n```"
        ), @r#"
        {% terminal() %}
        <span class="cmd">wt step eval '{{ branch | hash_port }}'</span>
        16066
        {% end %}
        "#);

        // Command only (no $) → unchanged
        assert_snapshot!(convert_dollar_console_to_terminal(
            "```console\nwt step commit --stage=tracked\n```"
        ), @r"
        ```console
        wt step commit --stage=tracked
        ```
        ");

        // Multi-line output, no {{ }} → cmd parameter
        assert_snapshot!(convert_dollar_console_to_terminal(
            "```console\n$ wt step eval --dry-run 'test'\nbranch=feature/auth\nResult: feature/auth\n```"
        ), @r#"
        {% terminal(cmd="wt step eval --dry-run 'test'") %}
        branch=feature/auth
        Result: feature/auth
        {% end %}
        "#);

        // Single command with output, no {{ }} → cmd parameter
        assert_snapshot!(convert_dollar_console_to_terminal(
            "```console\n$ echo 'PORT=8080' > .env\noutput\n```"
        ), @r#"
        {% terminal(cmd="echo 'PORT=8080' > .env") %}
        output
        {% end %}
        "#);

        // Command only (single, no output) → self-closing shortcode
        assert_snapshot!(convert_dollar_console_to_terminal(
            "```console\n$ wt remove\n```"
        ), @r#"{{ terminal(cmd="wt remove") }}
        "#);

        // Multiple commands → |||‐delimited cmd with Syntect highlighting
        assert_snapshot!(convert_dollar_console_to_terminal(
            "```console\n$ wt step push\n$ wt step push develop\n```"
        ), @r#"{{ terminal(cmd="wt step push|||wt step push develop") }}
        "#);

        // Comment before $ commands → included in cmd, Syntect highlights as comment
        assert_snapshot!(convert_dollar_console_to_terminal(
            "```console\n# Recent commands\n$ tail -5 log.jsonl | jq .\n\n# Failed\n$ jq 'select(.exit != 0)' log.jsonl\n```"
        ), @r##"{{ terminal(cmd="# Recent commands|||tail -5 log.jsonl | jq .||||||# Failed|||jq 'select(.exit != 0)' log.jsonl") }}
        "##);
    }
}
