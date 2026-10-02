//! Config failures retain the parser and its source through command context.
//!
//! A parse error owns its file attribution and quoted parser output. Loaders,
//! config reports, and tolerant-load warnings share that presentation; an
//! outer "failed to load" context must not replace the source's diagnosis.

use std::error::Error;
use std::path::{Path, PathBuf};

use color_print::cformat;

use crate::git::Diagnostic;
use crate::path::format_path_for_display;
use crate::styling::{error_message, format_with_gutter, warning_message};

/// A configuration operation failed, preserving typed load and parse causes.
#[derive(Debug)]
pub enum ConfigError {
    Message(String),
    Load(super::LoadError),
    Parse(ConfigParseError),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Message(message) => f.write_str(message),
            Self::Load(error) => error.fmt(f),
            Self::Parse(error) => error.fmt(f),
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Message(_) => None,
            Self::Load(error) => Some(error),
            Self::Parse(error) => Some(error),
        }
    }
}

/// A parser's failure and the config source it read.
#[derive(Debug)]
pub struct ConfigParseError {
    source: ConfigParseSource,
    path: PathBuf,
    error: Box<dyn Error + Send + Sync>,
}

/// Approval data is separate from the config layers that carry settings/hooks.
#[derive(Debug)]
enum ConfigParseSource {
    Config(super::ConfigFileKind),
    Approvals,
}

impl ConfigParseSource {
    fn label(&self) -> &'static str {
        match self {
            Self::Config(kind) => kind.label(),
            Self::Approvals => "Approvals",
        }
    }
}

impl ConfigParseError {
    pub fn new(
        kind: super::ConfigFileKind,
        path: &Path,
        error: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self::from_source(ConfigParseSource::Config(kind), path, error)
    }

    pub fn approvals(path: &Path, error: impl Error + Send + Sync + 'static) -> Self {
        Self::from_source(ConfigParseSource::Approvals, path, error)
    }

    fn from_source(
        source: ConfigParseSource,
        path: &Path,
        error: impl Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            source,
            path: path.to_path_buf(),
            error: Box::new(error),
        }
    }

    fn title(&self) -> String {
        let path = format_path_for_display(&self.path);
        let label = self.source.label();
        cformat!("{label} @ <bold>{path}</> failed to parse")
    }

    /// Tolerant loading skips the broken source, retaining valid lower layers.
    pub(crate) fn render_warning(&self) -> String {
        self.render_with(warning_message(format!("{}, skipping", self.title())).to_string())
    }

    fn render_with(&self, header: String) -> String {
        format!(
            "{header}\n{}",
            format_with_gutter(&self.error.to_string(), None)
        )
    }
}

impl std::fmt::Display for ConfigParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} @ {} failed to parse",
            self.source.label(),
            format_path_for_display(&self.path)
        )
    }
}

impl Error for ConfigParseError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.error.as_ref())
    }
}

impl Diagnostic for ConfigParseError {
    fn render(&self) -> String {
        self.render_with(error_message(self.title()).to_string())
    }
}
