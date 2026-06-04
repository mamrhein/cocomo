// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

#![allow(async_fn_in_trait)]

use std::{ffi, path};

use crate::backends::Metadata;
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
