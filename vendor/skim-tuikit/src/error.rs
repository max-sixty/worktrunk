use std::error::Error;
use std::fmt::{Display, Formatter};
use std::string::FromUtf8Error;
use std::time::Duration;

#[derive(Debug)]
pub enum TuikitError {
    UnknownSequence(String),
    NoCursorReportResponse,
    IndexOutOfBound(usize, usize),
    Timeout(Duration),
    Interrupted,
    TerminalNotStarted,
    DrawError(Box<dyn std::error::Error + Send + Sync>),
    SendEventError(String),
    FromUtf8Error(std::string::FromUtf8Error),
    ParseIntError(std::num::ParseIntError),
    IOError(std::io::Error),
    NixError(nix::Error),
    ChannelReceiveError(std::sync::mpsc::RecvError),
}

impl Display for TuikitError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TuikitError::UnknownSequence(sequence) => {
                write!(f, "unsupported esc sequence: {sequence}")
            }
            TuikitError::NoCursorReportResponse => {
                write!(f, "buffer did not contain cursor position response")
            }
            TuikitError::IndexOutOfBound(row, col) => {
                write!(f, "({row}, {col}) is out of bound")
            }
            TuikitError::Timeout(duration) => write!(f, "timeout with duration: {duration:?}"),
            TuikitError::Interrupted => write!(f, "interrupted"),
            TuikitError::TerminalNotStarted => {
                write!(f, "terminal not started, call `restart` to start it")
            }
            TuikitError::DrawError(error) => write!(f, "draw error: {error}"),
            TuikitError::SendEventError(error) => write!(f, "send event error: {error}"),
            TuikitError::FromUtf8Error(error) => write!(f, "{error}"),
            TuikitError::ParseIntError(error) => write!(f, "{error}"),
            TuikitError::IOError(error) => write!(f, "{error}"),
            TuikitError::NixError(error) => write!(f, "{error}"),
            TuikitError::ChannelReceiveError(error) => write!(f, "{error}"),
        }
    }
}

impl Error for TuikitError {}

impl From<std::string::FromUtf8Error> for TuikitError {
    fn from(error: FromUtf8Error) -> Self {
        TuikitError::FromUtf8Error(error)
    }
}

impl From<std::num::ParseIntError> for TuikitError {
    fn from(error: std::num::ParseIntError) -> Self {
        TuikitError::ParseIntError(error)
    }
}

impl From<nix::Error> for TuikitError {
    fn from(error: nix::Error) -> Self {
        TuikitError::NixError(error)
    }
}

impl From<std::io::Error> for TuikitError {
    fn from(error: std::io::Error) -> Self {
        TuikitError::IOError(error)
    }
}

impl From<std::sync::mpsc::RecvError> for TuikitError {
    fn from(error: std::sync::mpsc::RecvError) -> Self {
        TuikitError::ChannelReceiveError(error)
    }
}
