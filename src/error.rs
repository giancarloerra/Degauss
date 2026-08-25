//! Error type.
//!
//! Deliberately loud: every failure carries the path or device that caused it.
//! Nothing in this crate converts a failure into an empty result.

use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum DegaussError {
    /// An OS call failed on a specific path or device node.
    Io {
        what: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    /// A file exists but its contents are not what the format requires.
    Malformed {
        what: &'static str,
        path: PathBuf,
        detail: String,
    },
    /// The environment cannot support the operation (wrong pixel format,
    /// missing device, unsupported geometry). Never silently worked around.
    Unsupported { what: &'static str, detail: String },
}

impl DegaussError {
    pub fn io(what: &'static str, path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        DegaussError::Io {
            what,
            path: path.into(),
            source,
        }
    }

    pub fn malformed(
        what: &'static str,
        path: impl Into<PathBuf>,
        detail: impl Into<String>,
    ) -> Self {
        DegaussError::Malformed {
            what,
            path: path.into(),
            detail: detail.into(),
        }
    }

    pub fn unsupported(what: &'static str, detail: impl Into<String>) -> Self {
        DegaussError::Unsupported {
            what,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for DegaussError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DegaussError::Io { what, path, source } => {
                write!(f, "{what} failed for {}: {source}", path.display())
            }
            DegaussError::Malformed { what, path, detail } => {
                write!(f, "{what} is malformed at {}: {detail}", path.display())
            }
            DegaussError::Unsupported { what, detail } => {
                write!(f, "{what} unsupported: {detail}")
            }
        }
    }
}

impl std::error::Error for DegaussError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            DegaussError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, DegaussError>;
