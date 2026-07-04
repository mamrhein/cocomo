// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Structured filesystem errors that carry operation context, paths, and
//! underlying causes so callers can produce actionable diagnostics.

use std::{fmt, io, path::PathBuf, result};

use thiserror::Error;

/// The filesystem operation that was attempted when an error occurred.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum FsOperation {
    /// Opening a file.
    Open,
    /// Reading file content.
    Read,
    /// Writing file content.
    Write,
    /// Removing a file or directory.
    Remove,
    /// Renaming or moving a path.
    Rename,
    /// Copying a path.
    Copy,
    /// Creating a directory.
    CreateDir,
    /// Reading directory entries.
    ReadDir,
    /// Reading a symlink target.
    ReadLink,
    /// Creating a symlink.
    Symlink,
}

impl fmt::Display for FsOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FsOperation::Open => write!(f, "open"),
            FsOperation::Read => write!(f, "read"),
            FsOperation::Write => write!(f, "write"),
            FsOperation::Remove => write!(f, "remove"),
            FsOperation::Rename => write!(f, "rename"),
            FsOperation::Copy => write!(f, "copy"),
            FsOperation::CreateDir => write!(f, "create_dir"),
            FsOperation::ReadDir => write!(f, "read_dir"),
            FsOperation::ReadLink => write!(f, "read_link"),
            FsOperation::Symlink => write!(f, "symlink"),
        }
    }
}

/// A structured error that captures the operation, path, and underlying cause.
#[derive(Clone, Debug, Error)]
pub enum FsError {
    /// A general I/O error from the underlying filesystem.
    #[error("I/O error during {operation} on {path}: {message}")]
    Io {
        operation: FsOperation,
        path: PathBuf,
        message: String,
    },

    /// Access was denied for the requested operation.
    #[error("permission denied: {operation} on {path}")]
    PermissionDenied {
        operation: FsOperation,
        path: PathBuf,
    },

    /// The requested path does not exist.
    #[error("not found: {path}")]
    NotFound { path: PathBuf },
}

// ---------------------------------------------------------------------------
// Helpers to convert io errors into FsError with operation context
// ---------------------------------------------------------------------------

/// Wrap an `io::Error` into an `FsError::Io`, classifying known
/// permission and not-found cases into their dedicated variants.
pub fn wrap(
    io_error: io::Error,
    operation: FsOperation,
    path: PathBuf,
) -> FsError {
    match io_error.kind() {
        io::ErrorKind::PermissionDenied => {
            FsError::PermissionDenied { operation, path }
        }
        io::ErrorKind::NotFound => FsError::NotFound { path },
        _ => FsError::Io {
            operation,
            path,
            message: io_error.to_string(),
        },
    }
}

/// Alias for the `Result` type used throughout the library.
pub type Result<T> = result::Result<T, FsError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_permission_denied() {
        let io =
            io::Error::new(io::ErrorKind::PermissionDenied, "access denied");
        let err = wrap(io, FsOperation::Open, PathBuf::from("/secret"));
        assert!(matches!(err, FsError::PermissionDenied { .. }));
        let FsError::PermissionDenied { operation, path } = err else {
            unreachable!()
        };
        assert_eq!(operation, FsOperation::Open);
        assert_eq!(path, PathBuf::from("/secret"));
    }

    #[test]
    fn wraps_not_found() {
        let io = io::Error::new(io::ErrorKind::NotFound, "no such file");
        let err = wrap(io, FsOperation::Read, PathBuf::from("/missing"));
        assert!(matches!(err, FsError::NotFound { .. }));
    }

    #[test]
    fn wraps_general_io() {
        let io = io::Error::other("disk full");
        let err = wrap(io, FsOperation::Write, PathBuf::from("/full/file"));
        assert!(matches!(err, FsError::Io { .. }));
    }

    #[test]
    fn display_operation() {
        assert_eq!(format!("{}", FsOperation::Read), "read");
        assert_eq!(format!("{}", FsOperation::CreateDir), "create_dir");
    }
}
