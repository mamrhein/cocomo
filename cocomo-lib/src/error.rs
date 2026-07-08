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
    /// Flushing buffered writes.
    Flush,
    /// Removing a file or directory.
    Remove,
    /// Renaming or moving a path.
    Rename,
    /// Moving a node to a different parent.
    Move,
    /// Copying a path.
    Copy,
    /// Creating a directory.
    CreateDir,
    /// Creating a file.
    CreateFile,
    /// Creating a symlink.
    CreateSymlink,
    /// Reading directory entries.
    ReadDir,
    /// Reading a symlink target.
    ReadLink,
    /// Creating a symlink.
    Symlink,
    /// Resolving a path to a node identifier.
    Resolve,
}

impl fmt::Display for FsOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FsOperation::Open => write!(f, "open"),
            FsOperation::Read => write!(f, "read"),
            FsOperation::Write => write!(f, "write"),
            FsOperation::Flush => write!(f, "flush"),
            FsOperation::Remove => write!(f, "remove"),
            FsOperation::Rename => write!(f, "rename"),
            FsOperation::Move => write!(f, "move"),
            FsOperation::Copy => write!(f, "copy"),
            FsOperation::CreateDir => write!(f, "create_dir"),
            FsOperation::CreateFile => write!(f, "create_file"),
            FsOperation::CreateSymlink => write!(f, "create_symlink"),
            FsOperation::ReadDir => write!(f, "read_dir"),
            FsOperation::ReadLink => write!(f, "read_link"),
            FsOperation::Symlink => write!(f, "symlink"),
            FsOperation::Resolve => write!(f, "resolve"),
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

    /// The requested path or node does not exist.
    #[error("not found: {path}")]
    NotFound { path: PathBuf },

    /// An invalid argument was provided (e.g., an empty or inverted range).
    #[error("invalid argument during {operation} on {path}: {message}")]
    InvalidArgument {
        operation: FsOperation,
        path: PathBuf,
        message: String,
    },

    /// A node identifier is no longer valid because the node was deleted or
    /// evicted from the provider cache.
    #[error("stale node reference: node no longer exists")]
    StaleNode,

    /// A node field has not been resolved yet (e.g., directory children or
    /// symlink target). Callers should trigger resolution and retry.
    #[error("field \"{field}\" has not been resolved yet")]
    NotResolved { field: &'static str },

    /// A node was accessed with the wrong kind (e.g., a file identifier where
    /// a directory was expected).
    #[error("expected {expected}, found {actual}")]
    WrongKind {
        expected: &'static str,
        actual: &'static str,
    },
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
        assert_eq!(format!("{}", FsOperation::Flush), "flush");
        assert_eq!(format!("{}", FsOperation::Move), "move");
        assert_eq!(format!("{}", FsOperation::Resolve), "resolve");
    }

    #[test]
    fn invalid_argument_error() {
        let err = FsError::InvalidArgument {
            operation: FsOperation::Read,
            path: PathBuf::from("/test"),
            message: "start (100) must be less than end (50)".into(),
        };
        assert!(matches!(err, FsError::InvalidArgument { .. }));
        let FsError::InvalidArgument {
            operation,
            path,
            message,
        } = err
        else {
            unreachable!()
        };
        assert_eq!(operation, FsOperation::Read);
        assert_eq!(path, PathBuf::from("/test"));
        assert!(message.contains("100"));
    }

    #[test]
    fn stale_node_error() {
        let err = FsError::StaleNode;
        let msg = format!("{err}");
        assert!(msg.contains("stale"));
    }

    #[test]
    fn not_resolved_error() {
        let err = FsError::NotResolved { field: "children" };
        let msg = format!("{err}");
        assert!(msg.contains("children"));
    }

    #[test]
    fn wrong_kind_error() {
        let err = FsError::WrongKind {
            expected: "directory",
            actual: "file",
        };
        let msg = format!("{err}");
        assert!(msg.contains("directory"));
        assert!(msg.contains("file"));
    }
}
