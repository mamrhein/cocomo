// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

#![allow(async_fn_in_trait)]

use std::path;

use crate::{
    backends::{DirEntry, Metadata},
    vfs::VfsError,
};

/// Low-level, path-based filesystem operations.
///
/// Implement this trait to provide a concrete backend (local, SSH, S3, …).
/// Wrap a backend in [`VfsImpl`](crate::vfs::VfsImpl) to get the high-level
/// [`Vfs`](crate::vfs::Vfs) frontend.
pub trait VfsBackend: std::fmt::Debug + Send + Sync {
    /// Returns metadata for the given path, without following symlinks.
    async fn symlink_metadata(
        &self,
        path: &path::Path,
    ) -> Result<Metadata, VfsError>;

    /// Returns metadata for the given path, following symlinks.
    async fn metadata(&self, path: &path::Path) -> Result<Metadata, VfsError>;

    /// Reads the target of a symbolic link.
    async fn read_link(
        &self,
        path: &path::Path,
    ) -> Result<path::PathBuf, VfsError>;

    /// Reads the raw entries of a directory (one level, no recursion).
    async fn read_dir(
        &self,
        path: &path::Path,
    ) -> Result<Vec<DirEntry>, VfsError>;

    /// Reads the contents of a file as a string.
    async fn read_to_string(
        &self,
        path: &path::Path,
    ) -> Result<String, VfsError>;

    /// Reads up to `n` bytes from the beginning of a file.
    async fn read_head(
        &self,
        path: &path::Path,
        n: usize,
    ) -> Result<Vec<u8>, VfsError>;

    /// Resolves a path to its canonical absolute form.
    async fn canonicalize(
        &self,
        path: &path::Path,
    ) -> Result<path::PathBuf, VfsError>;

    /// Copies a file or directory from `from` to `to`.
    async fn copy(
        &self,
        from: &path::Path,
        to: &path::Path,
    ) -> Result<(), VfsError>;

    /// Renames (moves) a file or directory from `from` to `to`.
    async fn rename(
        &self,
        from: &path::Path,
        to: &path::Path,
    ) -> Result<(), VfsError>;

    /// Deletes a file.
    async fn remove_file(&self, path: &path::Path) -> Result<(), VfsError>;

    /// Deletes a directory and all its contents.
    async fn remove_dir_all(&self, path: &path::Path) -> Result<(), VfsError>;

    /// Creates a directory and all parent directories.
    async fn create_dir_all(&self, path: &path::Path) -> Result<(), VfsError>;

    /// Returns `true` if the given path is a mount point.
    ///
    /// The default implementation always returns `false`. Backends that can
    /// detect mount points (e.g. `LocalFs`) should override this.
    async fn is_mountpoint(
        &self,
        _path: &path::Path,
    ) -> Result<bool, VfsError> {
        Ok(false)
    }
}
