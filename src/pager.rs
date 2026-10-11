//! Git owns pager selection, including configuration outside a repository and
//! explicit disabled values. Optional detection failures leave output unpaged.

use worktrunk::git::base_path;
use worktrunk::shell_exec::Cmd;

/// Normalize Git's pager output, treating `cat` or whitespace as unpaged.
fn parse_pager_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty() && trimmed != "cat").then(|| trimmed.to_string())
}

/// Ask Git for its pager. Previews suppress PAGER/default fallback, since an
/// interactive default pager cannot run inside their captured output window.
pub(crate) fn git_pager(allow_fallback: bool) -> Option<String> {
    let mut command = Cmd::new("git")
        .args(["var", "GIT_PAGER"])
        .current_dir(base_path());
    if !allow_fallback {
        command = command.env("PAGER", "cat");
    }
    let output = command
        .run()
        .ok()
        .filter(|output| output.status.success())?;
    std::str::from_utf8(&output.stdout)
        .ok()
        .and_then(parse_pager_value)
}

#[cfg(test)]
mod tests {
    use super::parse_pager_value;

    #[test]
    fn parse_pager_output() {
        for (input, expected) in [
            ("cat", None),
            ("  cat  ", None),
            ("", None),
            ("  ", None),
            ("less", Some("less")),
            ("  less  ", Some("less")),
            ("delta", Some("delta")),
            ("less -R", Some("less -R")),
        ] {
            assert_eq!(parse_pager_value(input).as_deref(), expected, "{input:?}");
        }
    }
}
