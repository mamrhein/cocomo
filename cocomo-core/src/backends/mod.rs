// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::{ffi, path, time};

use mimetype_detector::MimeKind;

pub(crate) mod localfs;
pub(crate) mod traits;

pub use traits::VfsBackend;

/// Kind of a directory entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirEntryKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link.
    Symlink,
    /// Special (socket, pipe, device, etc.).
    Special,
}

/// Metadata of a file system entry.
#[derive(Clone, Debug)]
pub struct Metadata {
    /// The kind of entry.
    pub kind: DirEntryKind,
    /// Size of the entry in bytes.
    pub len: u64,
    /// Last modification time, if available.
    pub modified: Option<time::SystemTime>,
    /// Whether the entry is read-only.
    pub readonly: bool,
}

/// A raw directory entry returned by [`VfsBackend::read_dir`](VfsBackend::read_dir).
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
