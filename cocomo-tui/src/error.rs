// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Typed errors for the TUI service layer.
//!
//! The service modules ([`crate::controller`] and [`crate::session_store`])
//! return [`TuiError`] instead of `anyhow::Error`, so failures carry
//! structured path context on their way to `report_error` and the
//! `AlertModal`. `anyhow` remains only at the binary boundary
//! (`main.rs` / [`crate::run`]).

use std::{io, path::PathBuf, result};

use cocomo_lib::FsError;
use thiserror::Error;

/// Result alias for the TUI service layer.
pub(crate) type Result<T> = result::Result<T, TuiError>;

/// Error type of the TUI service layer.
///
/// Direct I/O errors are classified by [`io::ErrorKind`] at construction
/// time (mirroring `cocomo_lib::error::wrap`) so the UI can distinguish
/// "not found" from "permission denied" without parsing messages.
#[derive(Debug, Error)]
pub(crate) enum TuiError {
    /// An error surfaced by the `cocomo-lib` filesystem layer.
    #[error(transparent)]
    Lib(#[from] FsError),

    /// The targeted path does not exist.
    #[error("not found: {path}")]
    NotFound {
        /// The path that could not be found.
        path: PathBuf,
    },

    /// Access to the targeted path was denied.
    #[error("permission denied: {path}")]
    PermissionDenied {
        /// The path whose access was denied.
        path: PathBuf,
    },

    /// Any other I/O error. The message comes from `fs_err` and already
    /// names the failed operation and the path.
    #[error("{message}")]
    Io {
        /// The `fs_err` context ("failed to … `path`") plus the OS reason.
        message: String,
    },
}

impl TuiError {
    /// Wrap an I/O error from an `fs_err` call operating on `path`,
    /// classifying the common kinds into dedicated variants.
    pub(crate) fn io(path: impl Into<PathBuf>, err: io::Error) -> Self {
        let path = path.into();
        match err.kind() {
            io::ErrorKind::NotFound => Self::NotFound { path },
            io::ErrorKind::PermissionDenied => Self::PermissionDenied { path },
            _ => Self::Io {
                message: err.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_not_found() {
        let err = TuiError::io(
            "/missing/file.txt",
            io::Error::new(io::ErrorKind::NotFound, "no such file"),
        );
        assert!(matches!(err, TuiError::NotFound { .. }));
        assert_eq!(err.to_string(), "not found: /missing/file.txt");
    }

    #[test]
    fn classifies_permission_denied() {
        let err = TuiError::io(
            "/secret/session.toml",
            io::Error::new(io::ErrorKind::PermissionDenied, "access denied"),
        );
        assert!(matches!(err, TuiError::PermissionDenied { .. }));
        assert_eq!(err.to_string(), "permission denied: /secret/session.toml");
    }

    #[test]
    fn keeps_fs_err_message_for_other_kinds() {
        // An `fs_err`-style message: operation and path already embedded.
        let err = TuiError::io(
            "/full/file.txt",
            io::Error::other(
                "failed to write to file `/full/file.txt`: disk full",
            ),
        );
        assert!(matches!(err, TuiError::Io { .. }));
        assert_eq!(
            err.to_string(),
            "failed to write to file `/full/file.txt`: disk full"
        );
    }

    #[test]
    fn converts_lib_fs_error() {
        let fs_err = FsError::NotFound {
            path: PathBuf::from("/gone"),
        };
        let err: TuiError = fs_err.into();
        assert!(matches!(err, TuiError::Lib(_)));
        assert_eq!(err.to_string(), "not found: /gone");
    }
}
