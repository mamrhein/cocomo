// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::{ffi, fmt, io, path};

use mimetype_detector::MimeKind;

use crate::backends::{DirEntryKind, Metadata};
use crate::vfs::VfsItem;

// ---------------------------------------------------------------------------
// FSItemKind — lightweight discriminator
// ---------------------------------------------------------------------------

/// Lightweight discriminator for an [`FSItem`].
///
/// Carries enough info for quick pattern matching (e.g. TUI validation)
/// without requiring access to all fields of [`FSItem`].
#[derive(Clone, Debug, PartialEq)]
pub enum FSItemKind {
    /// A directory.
    Directory,
    /// A regular file with its detected MIME type.
    File { file_type: MimeKind },
    /// A symbolic link with its target path.
    Symlink { target: Option<path::PathBuf> },
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
            Self::Symlink { .. } => write!(f, "Symlink"),
            Self::Special => write!(f, "Special"),
            Self::Invalid { cause } => write!(f, "Invalid({})", cause),
        }
    }
}

// ---------------------------------------------------------------------------
// FSItem — unified item type
// ---------------------------------------------------------------------------

/// A filesystem item backed by a [`VfsBackend`](crate::vfs::VfsBackend).
///
/// The [`kind`](Self::kind) field discriminates between directories, files,
/// symlinks, special entries and invalid entries.
#[derive(Clone, Debug)]
pub struct FSItem {
    /// The basename of the entry.
    pub name: ffi::OsString,
    /// The full path of the entry.
    pub path: path::PathBuf,
    /// Metadata of the entry.
    pub metadata: Metadata,
    /// Kind discriminator with type-specific data.
    pub kind: FSItemKind,
}

impl FSItem {
    /// Returns the detected MIME type, if this is a file.
    pub fn file_type(&self) -> Option<MimeKind> {
        match &self.kind {
            FSItemKind::File { file_type } => Some(*file_type),
            _ => None,
        }
    }

    /// Returns the symlink target, if this is a symlink.
    pub fn link_target(&self) -> Option<&path::Path> {
        match &self.kind {
            FSItemKind::Symlink { target } => target.as_deref(),
            _ => None,
        }
    }

    /// Returns a placeholder invalid item.
    ///
    /// Useful as a fallback sentinel.
    pub fn default_invalid() -> Self {
        FSItem {
            name: ffi::OsString::new(),
            path: path::PathBuf::new(),
            metadata: Metadata {
                kind: DirEntryKind::Special,
                len: 0,
                modified: None,
                readonly: false,
            },
            kind: FSItemKind::Invalid {
                cause: io::ErrorKind::NotFound,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// VfsItem implementation
// ---------------------------------------------------------------------------

impl VfsItem for FSItem {
    fn name(&self) -> &ffi::OsString {
        &self.name
    }

    fn path(&self) -> &path::PathBuf {
        &self.path
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn item_kind(&self) -> FSItemKind {
        self.kind.clone()
    }

    fn is_dir(&self) -> bool {
        matches!(self.kind, FSItemKind::Directory)
    }

    fn is_file(&self) -> bool {
        matches!(self.kind, FSItemKind::File { .. })
    }

    fn is_link(&self) -> bool {
        matches!(self.kind, FSItemKind::Symlink { .. })
    }
}
