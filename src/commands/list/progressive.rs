use std::io::IsTerminal;

use crate::OutputFormat;

/// Resolved rendering decision for `wt list`. Collapses output format,
/// `--progressive`/`--no-progressive` flags, and stdout TTY detection into
/// a single value the rest of the pipeline can match on.
///
/// `Table { progressive: true }` is only set when stdout is a TTY — an
/// explicit `--progressive` flag on a piped stdout still resolves to
/// `progressive: false` because the in-place updates can't reach the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderTarget {
    /// Caller serializes data themselves (JSON output, picker UI). `collect`
    /// returns data without writing to stdout.
    Json,
    /// Render a table to stdout. `progressive` controls whether intermediate
    /// rows are streamed (`true`) or only the final table is written (`false`).
    Table { progressive: bool },
}

impl RenderTarget {
    /// Resolve the target from CLI inputs and the current stdout TTY state.
    pub fn detect(format: OutputFormat, progressive_flag: Option<bool>) -> Self {
        match format {
            OutputFormat::Json => RenderTarget::Json,
            OutputFormat::Table | OutputFormat::ClaudeCode => {
                let is_tty = std::io::stdout().is_terminal();
                let progressive = match progressive_flag {
                    Some(p) => p && is_tty,
                    None => is_tty,
                };
                RenderTarget::Table { progressive }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_format_always_resolves_to_json() {
        assert_eq!(
            RenderTarget::detect(OutputFormat::Json, None),
            RenderTarget::Json
        );
        assert_eq!(
            RenderTarget::detect(OutputFormat::Json, Some(true)),
            RenderTarget::Json
        );
        assert_eq!(
            RenderTarget::detect(OutputFormat::Json, Some(false)),
            RenderTarget::Json
        );
    }

    #[test]
    fn explicit_no_progressive_disables_progressive() {
        // In test runs stdout isn't a TTY, but we assert the explicit-false
        // branch regardless of TTY state.
        assert_eq!(
            RenderTarget::detect(OutputFormat::Table, Some(false)),
            RenderTarget::Table { progressive: false }
        );
    }
}
