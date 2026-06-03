// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::{io, path};

use thiserror::Error;

/// Error type for virtual file system operations.
#[derive(Debug, Error)]
pub enum VfsError {
    /// An I/O error occurred.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// The given path is not a directory.
    #[error("not a directory: {0}")]
    NotADirectory(path::PathBuf),

    /// Operation not supported by this backend.
    #[error("{0}")]
    Unsupported(String),

    /// A backend-specific error.
    #[error("{0}")]
    Other(String),
}
