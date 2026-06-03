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

use std::{ffi, io, path};

use mimetype_detector::MimeKind;

use crate::vfs::error::VfsError;
use crate::vfs::items::{
    DirItem, FileItem, FSItem, InvalidItem, SpecialItem, SymlinkItem,
};
use crate::vfs::types::{DirEntry, DirEntryKind, FSItemKind, Metadata};

// ===========================================================================
// VfsItem — base trait for every filesystem item
// ===========================================================================

/// Common interface for all filesystem items.
///
/// Every concrete item struct (`DirItem`, `FileItem`, …) implements this
/// trait, as does the [`FSItem`] enum that wraps them.
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
// VfsItem subtraits
// ===========================================================================

/// A directory with the ability to list its contents.
pub trait VfsDirectory: VfsItem {
    /// Reads the contents of this directory, returning fully typed
    /// [`FSItem`] values.
    async fn entries<V: Vfs + ?Sized>(&self, vfs: &V) -> Result<Vec<FSItem>, VfsError>;
}

/// A regular file.
pub trait VfsFile: VfsItem {
    /// The detected MIME type of this file.
    fn file_type(&self) -> MimeKind;

    /// Reads the full contents of this file as a string.
    async fn contents<V: Vfs + ?Sized>(&self, vfs: &V) -> Result<String, VfsError>;
}

/// A symbolic link.
pub trait VfsSymlink: VfsItem {
    /// The target path of this symlink (may be relative or absolute).
    fn target(&self) -> Option<&path::Path>;

    /// Resolves the symlink chain transitively (up to 32 hops) and returns
    /// the final target as an [`FSItem`].
    async fn resolve<V: Vfs + ?Sized>(&self, vfs: &V) -> Result<FSItem, VfsError>;
}

/// A special filesystem entry (socket, pipe, device, etc.).
///
/// This is a marker trait — no additional methods beyond [`VfsItem`].
pub trait VfsSpecial: VfsItem {}

// ===========================================================================
// VfsBackend — low-level, path-based backend interface
// ===========================================================================

/// Low-level, path-based filesystem operations.
///
/// Implement this trait to provide a concrete backend (local, SSH, S3, …).
/// Every implementor automatically receives the higher-level [`Vfs`] trait
/// via blanket impl.
pub trait VfsBackend: std::fmt::Debug + Send + Sync {
    /// Returns metadata for the given path, without following symlinks.
    async fn symlink_metadata(&self, path: &path::Path) -> Result<Metadata, VfsError>;

    /// Returns metadata for the given path, following symlinks.
    async fn metadata(&self, path: &path::Path) -> Result<Metadata, VfsError>;

    /// Reads the target of a symbolic link.
    async fn read_link(&self, path: &path::Path) -> Result<path::PathBuf, VfsError>;

    /// Reads the raw entries of a directory (one level, no recursion).
    ///
    /// Backends may set [`DirEntry::mime_type`] if they can cheaply do so;
    /// the default [`Vfs::read_dir`] fills it in otherwise.
    async fn read_dir_raw(&self, path: &path::Path) -> Result<Vec<DirEntry>, VfsError>;

    /// Reads the contents of a file as a string.
    async fn read_to_string(&self, path: &path::Path) -> Result<String, VfsError>;

    /// Reads up to `n` bytes from the beginning of a file.
    async fn read_head(&self, path: &path::Path, n: usize) -> Result<Vec<u8>, VfsError>;

    /// Resolves a path to its canonical absolute form.
    async fn canonicalize(&self, path: &path::Path) -> Result<path::PathBuf, VfsError>;

    /// Copies a file or directory from `from` to `to`.
    ///
    /// If `from` is a directory, its contents are copied recursively. If `to`
    /// is an existing directory, `from` is placed inside it.
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
/// Every [`VfsBackend`] implementor automatically receives a default impl of
/// this trait.
pub trait Vfs: VfsBackend {
    /// Creates an [`FSItem`] from a path by probing metadata, symlink target
    /// and MIME type.
    async fn new_item(&self, path: &path::Path) -> FSItem {
        match self.symlink_metadata(path).await {
            Ok(meta) => {
                let name = path.file_name().unwrap_or(path.as_os_str()).into();
                let p = path.to_path_buf();
                match meta.kind {
                    DirEntryKind::Directory => FSItem::Dir(DirItem {
                        name,
                        path: p,
                        metadata: meta,
                    }),
                    DirEntryKind::File => FSItem::File(FileItem {
                        name,
                        path: p,
                        metadata: meta,
                        file_type: MimeKind::UNKNOWN,
                    }),
                    DirEntryKind::Symlink => {
                        let target = self.read_link(path).await.unwrap_or_default();
                        FSItem::Symlink(SymlinkItem {
                            name,
                            path: p,
                            metadata: meta,
                            target,
                        })
                    }
                    DirEntryKind::Special => FSItem::Special(SpecialItem {
                        name,
                        path: p,
                        metadata: meta,
                    }),
                }
            }
            Err(e) => {
                let cause = match &e {
                    VfsError::Io(ioe) => ioe.kind(),
                    _ => io::ErrorKind::Other,
                };
                FSItem::Invalid(InvalidItem {
                    name: path.file_name().unwrap_or(path.as_os_str()).into(),
                    path: path.to_path_buf(),
                    cause,
                })
            }
        }
    }

    /// Reads the contents of a directory returned by [`VfsBackend::read_dir_raw`]
    /// and wraps them into fully typed [`FSItem`] values.
    async fn read_dir(&self, dir: &DirItem) -> Result<Vec<FSItem>, VfsError> {
        let entries = self.read_dir_raw(dir.path()).await?;
        let mut result = Vec::with_capacity(entries.len());
        for entry in entries {
            let name = entry.name;
            let p = entry.path;
            let meta = entry.metadata;
            match meta.kind {
                DirEntryKind::Directory => {
                    result.push(FSItem::Dir(DirItem {
                        name,
                        path: p,
                        metadata: meta,
                    }));
                }
                DirEntryKind::File => {
                    let file_type = entry.mime_type.unwrap_or(MimeKind::UNKNOWN);
                    result.push(FSItem::File(FileItem {
                        name,
                        path: p,
                        metadata: meta,
                        file_type,
                    }));
                }
                DirEntryKind::Symlink => {
                    let target = self.read_link(&p).await.unwrap_or_default();
                    result.push(FSItem::Symlink(SymlinkItem {
                        name,
                        path: p,
                        metadata: meta,
                        target,
                    }));
                }
                DirEntryKind::Special => {
                    result.push(FSItem::Special(SpecialItem {
                        name,
                        path: p,
                        metadata: meta,
                    }));
                }
            }
        }
        Ok(result)
    }

    /// Reads the full contents of a file as a string.
    async fn read_file(&self, file: &FileItem) -> Result<String, VfsError> {
        self.read_to_string(file.path()).await
    }

    /// Resolves a symlink chain transitively (up to 32 hops) and returns
    /// the final target as an [`FSItem`].
    async fn resolve(&self, symlink: &SymlinkItem) -> Result<FSItem, VfsError> {
        let mut current = if let Some(target) = symlink.target() {
            if target.is_relative() {
                if let Some(parent) = symlink.path().parent() {
                    parent.join(target)
                } else {
                    target.to_path_buf()
                }
            } else {
                target.to_path_buf()
            }
        } else {
            return Ok(self.new_item(symlink.path()).await);
        };

        let mut hops = 0;
        while hops < 32 {
            if let Ok(meta) = self.symlink_metadata(&current).await
                && meta.kind == DirEntryKind::Symlink
                && let Ok(next) = self.read_link(&current).await
            {
                current = current
                    .parent()
                    .unwrap_or_else(|| path::Path::new(""))
                    .join(&next);
                hops += 1;
                continue;
            }
            break;
        }
        Ok(self.new_item(&current).await)
    }

    /// Returns `true` if two items have compatible types for comparison.
    async fn comparable(&self, a: &FSItem, b: &FSItem) -> bool {
        match (a, b) {
            (FSItem::Dir(_), FSItem::Dir(_)) => true,
            (FSItem::File(fa), FSItem::File(fb)) => fa.file_type == fb.file_type,
            _ => false,
        }
    }

    /// Copies a directory into another directory.
    async fn copy(&self, src: &DirItem, dst: &DirItem) -> Result<(), VfsError> {
        self.copy_raw(src.path(), dst.path()).await
    }

    /// Copies a file into a directory.
    async fn copy_file(&self, src: &FileItem, dst: &DirItem) -> Result<(), VfsError> {
        let dst = dst.path().join(src.name());
        self.copy_raw(src.path(), &dst).await
    }

    /// Moves a directory into another directory.
    async fn move_item(&self, src: &DirItem, dst: &DirItem) -> Result<(), VfsError> {
        self.rename_raw(src.path(), dst.path()).await
    }

    /// Moves a file into a directory.
    async fn move_file(&self, src: &FileItem, dst: &DirItem) -> Result<(), VfsError> {
        let dst = dst.path().join(src.name());
        self.rename_raw(src.path(), &dst).await
    }

    /// Deletes a directory.
    async fn delete(&self, item: &DirItem) -> Result<(), VfsError> {
        self.remove_dir_all(item.path()).await
    }

    /// Deletes a file.
    async fn delete_file(&self, item: &FileItem) -> Result<(), VfsError> {
        self.remove_file(item.path()).await
    }

    /// Renames a directory in-place.
    async fn rename(&self, item: &DirItem, new_name: &str) -> Result<(), VfsError> {
        let mut dst = item.path().to_path_buf();
        dst.set_file_name(new_name);
        self.rename_raw(item.path(), &dst).await
    }

    /// Renames a file in-place.
    async fn rename_file(&self, item: &FileItem, new_name: &str) -> Result<(), VfsError> {
        let mut dst = item.path().to_path_buf();
        dst.set_file_name(new_name);
        self.rename_raw(item.path(), &dst).await
    }
}

// Blanket impl: every VfsBackend is automatically a Vfs.
impl<T: VfsBackend> Vfs for T {}
