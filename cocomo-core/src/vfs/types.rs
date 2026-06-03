// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::{fmt, io};

use mimetype_detector::MimeKind;

pub use crate::backends::{DirEntry, DirEntryKind, Metadata};

/// Lightweight discriminator for an [`FSItem`](crate::vfs::FSItem).
///
/// Carries enough info for quick pattern matching (e.g. TUI validation)
/// without the full data of each variant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FSItemKind {
    /// A directory.
    Directory,
    /// A regular file with its detected MIME type.
    File { file_type: MimeKind },
    /// A symbolic link.
    Symlink,
    /// A special entry (socket, pipe, device, etc.).
    Special,
    /// An entry that could not be processed.
    Invalid { cause: io::ErrorKind },
}

impl fmt::Display for FSItemKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Directory => write!(f, "Directory"),
            Self::File { file_type } => write!(f, "File({})", file_type),
            Self::Symlink => write!(f, "Symlink"),
            Self::Special => write!(f, "Special"),
            Self::Invalid { cause } => write!(f, "Invalid({})", cause),
        }
    }
}
