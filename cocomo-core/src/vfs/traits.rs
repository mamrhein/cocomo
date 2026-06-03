// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

// The traits use native `async fn`; the returned futures are guaranteed
// `Send` because `Self: Send + Sync` and all captured data is `Send`.
#![allow(async_fn_in_trait)]

use std::{ffi, path};

use crate::backends::{DirEntry, Metadata};
use crate::vfs::fsitem::{FSItem, FSItemKind};
use crate::vfs::error::VfsError;

// ===========================================================================
// VfsItem — base trait for every filesystem item
// ===========================================================================

/// Common interface for all filesystem items.
pub trait VfsItem: std::fmt::Debug + Send + Sync {
    /// The basename of this entry.
    fn name(&self) -> &ffi::OsString;

    /// The full path of this entry.
    fn path(&self) -> &path::PathBuf;

    /// Metadata of this entry.
    fn metadata(&self) -> &Metadata;

    /// A lightweight discriminator for this item.
    fn item_kind(&self) -> FSItemKind;

    /// Last modification time, if available.
    fn modified(&self) -> Option<std::time::SystemTime> {
        self.metadata().modified
    }

    /// Returns `true` if this item is a directory.
    fn is_dir(&self) -> bool;

    /// Returns `true` if this item is a regular file.
    fn is_file(&self) -> bool;

    /// Returns `true` if this item is a symbolic link.
    fn is_link(&self) -> bool;
}

// ===========================================================================
// VfsBackend — low-level, path-based backend interface
// ===========================================================================

/// Low-level, path-based filesystem operations.
///
/// Implement this trait to provide a concrete backend (local, SSH, S3, …).
/// Wrap a backend in [`VfsImpl`](crate::vfs::VfsImpl) to get the high-level
/// [`Vfs`] frontend.
pub trait VfsBackend: std::fmt::Debug + Send + Sync {
    /// Returns metadata for the given path, without following symlinks.
    async fn symlink_metadata(&self, path: &path::Path) -> Result<Metadata, VfsError>;

    /// Returns metadata for the given path, following symlinks.
    async fn metadata(&self, path: &path::Path) -> Result<Metadata, VfsError>;

    /// Reads the target of a symbolic link.
    async fn read_link(&self, path: &path::Path) -> Result<path::PathBuf, VfsError>;

    /// Reads the raw entries of a directory (one level, no recursion).
    async fn read_dir_raw(&self, path: &path::Path) -> Result<Vec<DirEntry>, VfsError>;

    /// Reads the contents of a file as a string.
    async fn read_to_string(&self, path: &path::Path) -> Result<String, VfsError>;

    /// Reads up to `n` bytes from the beginning of a file.
    async fn read_head(&self, path: &path::Path, n: usize) -> Result<Vec<u8>, VfsError>;

    /// Resolves a path to its canonical absolute form.
    async fn canonicalize(&self, path: &path::Path) -> Result<path::PathBuf, VfsError>;

    /// Copies a file or directory from `from` to `to`.
    async fn copy_raw(&self, from: &path::Path, to: &path::Path) -> Result<(), VfsError>;

    /// Renames (moves) a file or directory from `from` to `to`.
    async fn rename_raw(&self, from: &path::Path, to: &path::Path) -> Result<(), VfsError>;

    /// Deletes a file.
    async fn remove_file(&self, path: &path::Path) -> Result<(), VfsError>;

    /// Deletes a directory and all its contents.
    async fn remove_dir_all(&self, path: &path::Path) -> Result<(), VfsError>;

    /// Creates a directory and all parent directories.
    async fn create_dir_all(&self, path: &path::Path) -> Result<(), VfsError>;
}

// ===========================================================================
// Vfs — high-level, FSItem-based frontend
// ===========================================================================

/// High-level filesystem operations operating on [`FSItem`] values.
///
/// Use [`VfsImpl`](crate::vfs::VfsImpl) to materialise a [`VfsBackend`] into
/// a [`Vfs`].
pub trait Vfs: std::fmt::Debug + Send + Sync {
    /// Creates an [`FSItem`] from a path by probing metadata, symlink target
    /// and MIME type.
    async fn new_item(&self, path: &path::Path) -> FSItem;

    /// Reads the contents of a directory and returns fully typed [`FSItem`]
    /// values.
    async fn read_dir(&self, item: &FSItem) -> Result<Vec<FSItem>, VfsError>;

    /// Reads the full contents of a file as a string.
    async fn read_file(&self, item: &FSItem) -> Result<String, VfsError>;

    /// Resolves a symlink chain transitively (up to 32 hops) and returns
    /// the final target as an [`FSItem`].
    async fn resolve(&self, item: &FSItem) -> Result<FSItem, VfsError>;

    /// Returns `true` if two items have compatible types for comparison.
    async fn comparable(&self, a: &FSItem, b: &FSItem) -> bool;

    /// Copies an item into a destination.
    async fn copy(&self, src: &FSItem, dst: &FSItem) -> Result<(), VfsError>;

    /// Moves (renames) an item into another directory.
    async fn move_item(&self, src: &FSItem, dst: &FSItem) -> Result<(), VfsError>;

    /// Deletes a file or directory.
    async fn delete(&self, item: &FSItem) -> Result<(), VfsError>;

    /// Renames an item in-place.
    async fn rename(&self, item: &FSItem, new_name: &str) -> Result<(), VfsError>;
}
