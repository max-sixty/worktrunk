//! Reusable prompt utilities for interactive CLI prompts.

use std::io::{self, Write};

use color_print::cformat;
use worktrunk::styling::{PROMPT_SYMBOL, eprint};

/// Response from a `[y/N/?]` prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptResponse {
    /// User accepted (y/yes)
    Accepted,
    /// User declined (n/no/empty/other)
    Declined,
}

/// Prompt with `[y/N/?]` options. Loops on `?` to show preview.
///
/// Emits no leading blank line: the separator belongs to the narration it
/// separates from, and most prompts are the first thing their command prints
/// (`wt config shell install`, `wt config plugins claude install`, the
/// commit-generation setup offer at the top of `wt merge`), where a leading
/// blank is exactly the leading blank /writing-user-outputs forbids. A caller
/// that has already printed adds its own `eprintln!()` first — see
/// `handle_config_update` and `prompt_shell_integration`.
///
/// # Arguments
/// * `prompt_text` - The question to ask (without the `[y/N/?]` suffix)
/// * `show_preview` - Closure called when user enters `?`
///
/// # Returns
/// * `Ok(Accepted)` if user enters `y` or `yes`
/// * `Ok(Declined)` if user enters anything else (including empty)
///
/// # Example
/// ```ignore
/// match prompt_yes_no_preview(
///     &cformat!("Configure <bold>{tool}</>?"),
///     || {
///         eprintln!("{}", info_message("Would add:"));
///         eprintln!("{}", format_with_gutter(&preview, None));
///     },
/// )? {
///     PromptResponse::Accepted => { /* do the thing */ }
///     PromptResponse::Declined => { /* skip */ }
/// }
/// ```
pub fn prompt_yes_no_preview(
    prompt_text: &str,
    show_preview: impl Fn(),
) -> io::Result<PromptResponse> {
    loop {
        eprint!(
            "{}",
            cformat!("{PROMPT_SYMBOL} {prompt_text} <bold>[y/N/?]</> ")
        );
        io::stderr().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        let response = input.trim().to_lowercase();
        match response.as_str() {
            "y" | "yes" => {
                return Ok(PromptResponse::Accepted);
            }
            "?" => {
                show_preview();
                // Loop back to prompt again
            }
            _ => {
                return Ok(PromptResponse::Declined);
            }
        }
    }
}
