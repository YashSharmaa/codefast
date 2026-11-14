use std::fmt;

/// Every error the library reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The patch text is malformed. `line` is 1-based.
    Parse { line: usize, message: String },
    /// Hunk number `0` (1-based) could not be placed.
    HunkFailed(usize),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Parse { line, message } => write!(f, "line {line}: {message}"),
            Error::HunkFailed(n) => write!(f, "hunk {n} failed"),
        }
    }
}

impl std::error::Error for Error {}
