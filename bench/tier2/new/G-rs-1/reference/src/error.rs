use std::fmt;

/// Every error the library reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The text given to `Version::parse` is not a valid version.
    InvalidVersion(String),
    /// The text given to `Constraint::parse` is not a valid constraint.
    InvalidConstraint(String),
    /// A registry file could not be parsed. `line` is 1-based.
    Parse { line: usize, message: String },
    /// A root requirement names a package the registry does not contain.
    UnknownPackage(String),
    /// No assignment of versions satisfies every requirement.
    NoSolution,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidVersion(s) => write!(f, "invalid version: {s}"),
            Error::InvalidConstraint(s) => write!(f, "invalid constraint: {s}"),
            Error::Parse { line, message } => write!(f, "line {line}: {message}"),
            Error::UnknownPackage(name) => write!(f, "unknown package: {name}"),
            Error::NoSolution => write!(f, "no solution"),
        }
    }
}

impl std::error::Error for Error {}
