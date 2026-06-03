// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::{ffi, fmt, io, path, time};

use mimetype_detector::MimeKind;

/// Kind of a file system entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link.
    Symlink,
    /// Other (socket, pipe, device, etc.).
    Other,
}

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

/// Metadata of a file system entry.
#[derive(Clone, Debug)]
pub struct Metadata {
    /// The kind of entry.
    pub kind: FileKind,
    /// Size of the entry in bytes.
    pub len: u64,
    /// Last modification time, if available.
    pub modified: Option<time::SystemTime>,
    /// Whether the entry is read-only.
    pub readonly: bool,
}

/// A raw directory entry returned by [`VfsBackend::read_dir_raw`](crate::vfs::VfsBackend).
///
/// Backends may set [`mime_type`] to [`Some`] if they can cheaply detect it;
/// otherwise the frontend [`Vfs::read_dir`](crate::vfs::Vfs) fills it in.
#[derive(Clone, Debug)]
pub struct DirEntry {
    /// The name of the entry.
    pub name: ffi::OsString,
    /// The full path of the entry.
    pub path: path::PathBuf,
    /// Metadata of the entry.
    pub metadata: Metadata,
    /// Detected MIME type, if available.
    pub mime_type: Option<MimeKind>,
}
