//! Reusable prompt utilities for interactive CLI prompts.

use std::io::{self, Write};

use color_print::cformat;
use worktrunk::styling::PROMPT_SYMBOL;

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
    // Blank line before first prompt for visual separation
    worktrunk::styling::eprintln!();

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
