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
///
/// The `PermissionDenied` and `NotFound` variants carry the original OS /
/// provider message (possibly empty when no message was available) so the
/// rendered text names the underlying cause, not just the classification.
#[derive(Clone, Debug)]
pub enum FsError {
    /// A general I/O error from the underlying filesystem.
    Io {
        operation: FsOperation,
        path: PathBuf,
        message: String,
    },

    /// Access was denied for the requested operation.
    PermissionDenied {
        operation: FsOperation,
        path: PathBuf,
        /// Original OS / provider message, empty when none was available.
        message: String,
    },

    /// The requested path or node does not exist.
    NotFound {
        path: PathBuf,
        /// Original OS / provider message, empty when none was available.
        message: String,
    },

    /// An invalid argument was provided (e.g., an empty or inverted range).
    InvalidArgument {
        operation: FsOperation,
        path: PathBuf,
        message: String,
    },

    /// A node identifier is no longer valid because the node was deleted or
    /// evicted from the provider cache.
    StaleNode,

    /// A node field has not been resolved yet (e.g., directory children or
    /// symlink target). Callers should trigger resolution and retry.
    NotResolved { field: &'static str },

    /// A node was accessed with the wrong kind (e.g., a file identifier where
    /// a directory was expected).
    WrongKind {
        expected: &'static str,
        actual: &'static str,
    },

    /// The operation could not be completed because one or more filesystem
    /// errors occurred while walking the tree. The individual errors are
    /// preserved so callers can report them, and the rendered text includes
    /// them so frontends never show a bare error count.
    Incomplete { errors: Vec<FsError> },

    /// The operation was aborted because its cancellation token was
    /// triggered by the caller.
    Cancelled,
}

impl FsError {
    /// `NotFound` without extra detail beyond the path.
    pub fn not_found(path: impl Into<PathBuf>) -> Self {
        Self::NotFound {
            path: path.into(),
            message: String::new(),
        }
    }

    /// `NotFound` carrying the original OS / provider message.
    pub fn not_found_with_message(
        path: impl Into<PathBuf>,
        message: impl Into<String>,
    ) -> Self {
        Self::NotFound {
            path: path.into(),
            message: message.into(),
        }
    }

    /// `PermissionDenied` without extra detail beyond operation and path.
    pub fn permission_denied(
        operation: FsOperation,
        path: impl Into<PathBuf>,
    ) -> Self {
        Self::PermissionDenied {
            operation,
            path: path.into(),
            message: String::new(),
        }
    }

    /// `PermissionDenied` carrying the original OS / provider message.
    pub fn permission_denied_with_message(
        operation: FsOperation,
        path: impl Into<PathBuf>,
        message: impl Into<String>,
    ) -> Self {
        Self::PermissionDenied {
            operation,
            path: path.into(),
            message: message.into(),
        }
    }
}

/// Append `": {message}"` when the message is non-empty, so classification
/// variants without an original error text render exactly as before.
fn fmt_optional_message(
    f: &mut fmt::Formatter<'_>,
    message: &str,
) -> fmt::Result {
    if message.is_empty() {
        Ok(())
    } else {
        write!(f, ": {message}")
    }
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                message,
            } => {
                write!(
                    f,
                    "I/O error during {operation} on {}: {message}",
                    path.display()
                )
            }
            Self::PermissionDenied {
                operation,
                path,
                message,
            } => {
                write!(
                    f,
                    "permission denied: {operation} on {}",
                    path.display()
                )?;
                fmt_optional_message(f, message)
            }
            Self::NotFound { path, message } => {
                write!(f, "not found: {}", path.display())?;
                fmt_optional_message(f, message)
            }
            Self::InvalidArgument {
                operation,
                path,
                message,
            } => {
                write!(
                    f,
                    "invalid argument during {operation} on {}: {message}",
                    path.display()
                )
            }
            Self::StaleNode => {
                write!(f, "stale node reference: node no longer exists")
            }
            Self::NotResolved { field } => {
                write!(f, "field \"{field}\" has not been resolved yet")
            }
            Self::WrongKind { expected, actual } => {
                write!(f, "expected {expected}, found {actual}")
            }
            Self::Incomplete { errors } => {
                write!(
                    f,
                    "operation incomplete due to {} filesystem error(s): ",
                    errors.len(),
                )?;
                for (i, err) in errors.iter().enumerate() {
                    if i > 0 {
                        write!(f, "; ")?;
                    }
                    write!(f, "{err}")?;
                }
                Ok(())
            }
            Self::Cancelled => write!(f, "operation cancelled"),
        }
    }
}

impl std::error::Error for FsError {}

// ---------------------------------------------------------------------------
// Helpers to convert io errors into FsError with operation context
// ---------------------------------------------------------------------------

/// Wrap an `io::Error` into an `FsError::Io`, classifying known
/// permission and not-found cases into their dedicated variants.
///
/// The original error message is always preserved, so the rendered text
/// names the underlying OS cause (e.g. `Permission denied (os error 13)`).
pub fn wrap(
    io_error: io::Error,
    operation: FsOperation,
    path: PathBuf,
) -> FsError {
    let message = io_error.to_string();
    match io_error.kind() {
        io::ErrorKind::PermissionDenied => {
            FsError::permission_denied_with_message(operation, path, message)
        }
        io::ErrorKind::NotFound => {
            FsError::not_found_with_message(path, message)
        }
        _ => FsError::Io {
            operation,
            path,
            message,
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
        let FsError::PermissionDenied {
            operation,
            path,
            message,
        } = err
        else {
            unreachable!()
        };
        assert_eq!(operation, FsOperation::Open);
        assert_eq!(path, PathBuf::from("/secret"));
        assert_eq!(message, "access denied");
    }

    #[test]
    fn wraps_permission_denied_keeps_os_message_in_display() {
        let io = io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Permission denied (os error 13)",
        );
        let err = wrap(io, FsOperation::Write, PathBuf::from("/right/a.txt"));
        assert_eq!(
            err.to_string(),
            "permission denied: write on /right/a.txt: Permission denied (os \
             error 13)"
        );
    }

    #[test]
    fn wraps_not_found() {
        let io = io::Error::new(io::ErrorKind::NotFound, "no such file");
        let err = wrap(io, FsOperation::Read, PathBuf::from("/missing"));
        assert!(matches!(err, FsError::NotFound { .. }));
        let FsError::NotFound { path, message } = err else {
            unreachable!()
        };
        assert_eq!(path, PathBuf::from("/missing"));
        assert_eq!(message, "no such file");
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

    #[test]
    fn incomplete_display_lists_nested_errors() {
        let err = FsError::Incomplete {
            errors: vec![
                FsError::permission_denied_with_message(
                    FsOperation::ReadDir,
                    "/left/blocked",
                    "Permission denied (os error 13)",
                ),
                FsError::not_found("/right/gone"),
            ],
        };
        let msg = err.to_string();
        assert!(msg.contains("2 filesystem error(s)"));
        assert!(msg.contains("read_dir on /left/blocked"));
        assert!(msg.contains("Permission denied (os error 13)"));
        assert!(msg.contains("not found: /right/gone"));
    }

    #[test]
    fn classification_variants_render_without_message() {
        // An empty message keeps the pre-existing rendering intact.
        let err = FsError::permission_denied(FsOperation::Read, "/secret");
        assert_eq!(err.to_string(), "permission denied: read on /secret");
        let err = FsError::not_found("/gone");
        assert_eq!(err.to_string(), "not found: /gone");
    }
}
