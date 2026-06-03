// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::{ffi, io, path};

use mimetype_detector::MimeKind;

use crate::vfs::types::{FSItemKind, FileKind, Metadata};
use crate::vfs::{Vfs, VfsDirectory, VfsFile, VfsItem, VfsSpecial, VfsSymlink, VfsError};

// ---------------------------------------------------------------------------
// Concrete item types
// ---------------------------------------------------------------------------

/// A directory entry.
#[derive(Clone, Debug)]
pub struct DirItem {
    /// The basename of the entry.
    pub name: ffi::OsString,
    /// The full path of the entry.
    pub path: path::PathBuf,
    /// Metadata of the entry.
    pub metadata: Metadata,
}

/// A regular file entry.
#[derive(Clone, Debug)]
pub struct FileItem {
    /// The basename of the entry.
    pub name: ffi::OsString,
    /// The full path of the entry.
    pub path: path::PathBuf,
    /// Metadata of the entry.
    pub metadata: Metadata,
    /// The detected MIME type of the file.
    pub file_type: MimeKind,
}

/// A symbolic link entry.
#[derive(Clone, Debug)]
pub struct SymlinkItem {
    /// The basename of the entry.
    pub name: ffi::OsString,
    /// The full path of the entry.
    pub path: path::PathBuf,
    /// Metadata of the entry.
    pub metadata: Metadata,
    /// The target of the symbolic link.
    pub target: path::PathBuf,
}

/// A special filesystem entry (socket, pipe, device, etc.).
#[derive(Clone, Debug)]
pub struct SpecialItem {
    /// The basename of the entry.
    pub name: ffi::OsString,
    /// The full path of the entry.
    pub path: path::PathBuf,
    /// Metadata of the entry.
    pub metadata: Metadata,
}

/// An invalid / inaccessible entry.
#[derive(Clone, Debug)]
pub struct InvalidItem {
    /// The basename of the entry.
    pub name: ffi::OsString,
    /// The full path of the entry.
    pub path: path::PathBuf,
    /// The reason the entry could not be accessed.
    pub cause: io::ErrorKind,
}

// ---------------------------------------------------------------------------
// VfsItem implementations
// ---------------------------------------------------------------------------

impl VfsItem for DirItem {
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
        FSItemKind::Directory
    }
    fn is_dir(&self) -> bool {
        true
    }
    fn is_file(&self) -> bool {
        false
    }
    fn is_link(&self) -> bool {
        false
    }
}

impl VfsItem for FileItem {
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
        FSItemKind::File {
            file_type: self.file_type,
        }
    }
    fn is_dir(&self) -> bool {
        false
    }
    fn is_file(&self) -> bool {
        true
    }
    fn is_link(&self) -> bool {
        false
    }
}

impl VfsItem for SymlinkItem {
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
        FSItemKind::Symlink
    }
    fn is_dir(&self) -> bool {
        false
    }
    fn is_file(&self) -> bool {
        false
    }
    fn is_link(&self) -> bool {
        true
    }
}

impl VfsItem for SpecialItem {
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
        FSItemKind::Special
    }
    fn is_dir(&self) -> bool {
        false
    }
    fn is_file(&self) -> bool {
        false
    }
    fn is_link(&self) -> bool {
        false
    }
}

impl VfsItem for InvalidItem {
    fn name(&self) -> &ffi::OsString {
        &self.name
    }
    fn path(&self) -> &path::PathBuf {
        &self.path
    }
    fn metadata(&self) -> &Metadata {
        const INVALID_META: &Metadata = &Metadata {
            kind: FileKind::Other,
            len: 0,
            modified: None,
            readonly: false,
        };
        INVALID_META
    }
    fn item_kind(&self) -> FSItemKind {
        FSItemKind::Invalid {
            cause: self.cause,
        }
    }
    fn is_dir(&self) -> bool {
        false
    }
    fn is_file(&self) -> bool {
        false
    }
    fn is_link(&self) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Subtrait implementations for concrete types
// ---------------------------------------------------------------------------

impl VfsDirectory for DirItem {
    async fn entries<V: Vfs + ?Sized>(&self, vfs: &V) -> Result<Vec<FSItem>, VfsError> {
        vfs.read_dir(self).await
    }
}

impl VfsFile for FileItem {
    fn file_type(&self) -> MimeKind {
        self.file_type
    }

    async fn contents<V: Vfs + ?Sized>(&self, vfs: &V) -> Result<String, VfsError> {
        vfs.read_file(self).await
    }
}

impl VfsSymlink for SymlinkItem {
    fn target(&self) -> Option<&path::Path> {
        if self.target.as_os_str().is_empty() {
            None
        } else {
            Some(&self.target)
        }
    }

    async fn resolve<V: Vfs + ?Sized>(&self, vfs: &V) -> Result<FSItem, VfsError> {
        vfs.resolve(self).await
    }
}

impl VfsSpecial for SpecialItem {}

// ---------------------------------------------------------------------------
// FSItem enum — concrete wrapper for storage
// ---------------------------------------------------------------------------

/// A concrete, type-dispatched filesystem item.
///
/// Each variant wraps the corresponding concrete struct.  Use the
/// [`VfsItem`] trait for common accessors, and `as_dir()` / `as_file()` /
/// `as_symlink()` to get a typed reference with additional methods.
#[derive(Clone, Debug)]
pub enum FSItem {
    /// A directory.
    Dir(DirItem),
    /// A regular file with a known MIME type.
    File(FileItem),
    /// A symbolic link.
    Symlink(SymlinkItem),
    /// A special entry (socket, pipe, device, etc.).
    Special(SpecialItem),
    /// An invalid / inaccessible entry.
    Invalid(InvalidItem),
}

impl FSItem {
    /// Returns the kind of this item.
    pub fn item_kind(&self) -> FSItemKind {
        match self {
            Self::Dir(_) => FSItemKind::Directory,
            Self::File(f) => FSItemKind::File {
                file_type: f.file_type,
            },
            Self::Symlink(_) => FSItemKind::Symlink,
            Self::Special(_) => FSItemKind::Special,
            Self::Invalid(i) => FSItemKind::Invalid { cause: i.cause },
        }
    }

    /// If this is a directory, returns a reference to the inner [`DirItem`].
    pub fn as_dir(&self) -> Option<&DirItem> {
        match self {
            Self::Dir(d) => Some(d),
            _ => None,
        }
    }

    /// If this is a file, returns a reference to the inner [`FileItem`].
    pub fn as_file(&self) -> Option<&FileItem> {
        match self {
            Self::File(f) => Some(f),
            _ => None,
        }
    }

    /// If this is a symlink, returns a reference to the inner [`SymlinkItem`].
    pub fn as_symlink(&self) -> Option<&SymlinkItem> {
        match self {
            Self::Symlink(s) => Some(s),
            _ => None,
        }
    }

    /// If this is a special entry, returns a reference to the inner
    /// [`SpecialItem`].
    pub fn as_special(&self) -> Option<&SpecialItem> {
        match self {
            Self::Special(s) => Some(s),
            _ => None,
        }
    }

    /// If this is an invalid entry, returns a reference to the inner
    /// [`InvalidItem`].
    pub fn as_invalid(&self) -> Option<&InvalidItem> {
        match self {
            Self::Invalid(i) => Some(i),
            _ => None,
        }
    }

    /// Returns a placeholder invalid item.
    ///
    /// Useful as a fallback sentinel (replaces `Default`).
    pub fn default_invalid() -> Self {
        Self::Invalid(InvalidItem {
            name: ffi::OsString::new(),
            path: path::PathBuf::new(),
            cause: io::ErrorKind::NotFound,
        })
    }
}

impl VfsItem for FSItem {
    fn name(&self) -> &ffi::OsString {
        match self {
            Self::Dir(i) => i.name(),
            Self::File(i) => i.name(),
            Self::Symlink(i) => i.name(),
            Self::Special(i) => i.name(),
            Self::Invalid(i) => i.name(),
        }
    }

    fn path(&self) -> &path::PathBuf {
        match self {
            Self::Dir(i) => i.path(),
            Self::File(i) => i.path(),
            Self::Symlink(i) => i.path(),
            Self::Special(i) => i.path(),
            Self::Invalid(i) => i.path(),
        }
    }

    fn metadata(&self) -> &Metadata {
        match self {
            Self::Dir(i) => i.metadata(),
            Self::File(i) => i.metadata(),
            Self::Symlink(i) => i.metadata(),
            Self::Special(i) => i.metadata(),
            Self::Invalid(i) => i.metadata(),
        }
    }

    fn item_kind(&self) -> FSItemKind {
        self.item_kind()
    }

    fn is_dir(&self) -> bool {
        matches!(self, Self::Dir(_))
    }

    fn is_file(&self) -> bool {
        matches!(self, Self::File(_))
    }

    fn is_link(&self) -> bool {
        matches!(self, Self::Symlink(_))
    }
}
