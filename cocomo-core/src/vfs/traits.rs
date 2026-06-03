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

use crate::backends::{DirEntry, DirEntryKind, Metadata};
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
                let kind = match meta.kind {
                    DirEntryKind::Directory => FSItemKind::Directory,
                    DirEntryKind::File => FSItemKind::File {
                        file_type: MimeKind::UNKNOWN,
                    },
                    DirEntryKind::Symlink => {
                        let target = self.read_link(path).await.ok();
                        FSItemKind::Symlink { target }
                    }
                    DirEntryKind::Special => FSItemKind::Special,
                };
                FSItem {
                    name,
                    path: p,
                    metadata: meta,
                    kind,
                }
            }
            Err(e) => {
                let cause = match &e {
                    VfsError::Io(ioe) => ioe.kind(),
                    _ => io::ErrorKind::Other,
                };
                FSItem {
                    name: path.file_name().unwrap_or(path.as_os_str()).into(),
                    path: path.to_path_buf(),
                    metadata: Metadata {
                        kind: DirEntryKind::Special,
                        len: 0,
                        modified: None,
                        readonly: false,
                    },
                    kind: FSItemKind::Invalid { cause },
                }
            }
        }
    }

    /// Reads the contents of a directory and returns fully typed [`FSItem`]
    /// values.
    async fn read_dir(&self, item: &FSItem) -> Result<Vec<FSItem>, VfsError> {
        if !item.is_dir() {
            return Err(VfsError::NotADirectory(item.path().clone()));
        }
        let entries = self.read_dir_raw(item.path()).await?;
        let mut result = Vec::with_capacity(entries.len());
        for entry in entries {
            let name = entry.name;
            let p = entry.path;
            let meta = entry.metadata;
            let kind = match meta.kind {
                DirEntryKind::Directory => FSItemKind::Directory,
                DirEntryKind::File => FSItemKind::File {
                    file_type: entry.mime_type.unwrap_or(MimeKind::UNKNOWN),
                },
                DirEntryKind::Symlink => {
                    let target = self.read_link(&p).await.ok();
                    FSItemKind::Symlink { target }
                }
                DirEntryKind::Special => FSItemKind::Special,
            };
            result.push(FSItem {
                name,
                path: p,
                metadata: meta,
                kind,
            });
        }
        Ok(result)
    }

    /// Reads the full contents of a file as a string.
    async fn read_file(&self, item: &FSItem) -> Result<String, VfsError> {
        if !item.is_file() {
            return Err(VfsError::Unsupported(format!(
                "not a file: {}",
                item.path().display()
            )));
        }
        self.read_to_string(item.path()).await
    }

    /// Resolves a symlink chain transitively (up to 32 hops) and returns
    /// the final target as an [`FSItem`].
    async fn resolve(&self, item: &FSItem) -> Result<FSItem, VfsError> {
        let mut current = if let Some(target) = item.link_target() {
            if target.is_relative() {
                if let Some(parent) = item.path().parent() {
                    parent.join(target)
                } else {
                    target.to_path_buf()
                }
            } else {
                target.to_path_buf()
            }
        } else {
            return Ok(item.clone());
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
        match (&a.kind, &b.kind) {
            (FSItemKind::Directory, FSItemKind::Directory) => true,
            (
                FSItemKind::File { file_type: fa },
                FSItemKind::File { file_type: fb },
            ) => fa == fb,
            _ => false,
        }
    }

    /// Copies a directory into another directory.
    async fn copy(&self, src: &FSItem, dst: &FSItem) -> Result<(), VfsError> {
        match &src.kind {
            FSItemKind::Directory => self.copy_raw(src.path(), dst.path()).await,
            FSItemKind::File { .. } => {
                let dst = if dst.is_dir() {
                    dst.path().join(src.name())
                } else {
                    dst.path().to_path_buf()
                };
                self.copy_raw(src.path(), &dst).await
            }
            _ => Err(VfsError::Unsupported(format!(
                "cannot copy {}",
                src.kind
            ))),
        }
    }

    /// Moves (renames) an item into another directory.
    async fn move_item(&self, src: &FSItem, dst: &FSItem) -> Result<(), VfsError> {
        let dst = if dst.is_dir() {
            dst.path().join(src.name())
        } else {
            dst.path().to_path_buf()
        };
        self.rename_raw(src.path(), &dst).await
    }

    /// Deletes a file or directory.
    async fn delete(&self, item: &FSItem) -> Result<(), VfsError> {
        match &item.kind {
            FSItemKind::Directory => self.remove_dir_all(item.path()).await,
            FSItemKind::File { .. } => self.remove_file(item.path()).await,
            _ => Err(VfsError::Unsupported(format!(
                "cannot delete {}",
                item.kind
            ))),
        }
    }

    /// Renames an item in-place.
    async fn rename(&self, item: &FSItem, new_name: &str) -> Result<(), VfsError> {
        let mut dst = item.path().to_path_buf();
        dst.set_file_name(new_name);
        self.rename_raw(item.path(), &dst).await
    }
}

// Blanket impl: every VfsBackend is automatically a Vfs.
impl<T: VfsBackend> Vfs for T {}
