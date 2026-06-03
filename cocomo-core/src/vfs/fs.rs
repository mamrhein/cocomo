// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::io;

use mimetype_detector::MimeKind;

use crate::backends::{DirEntryKind, Metadata};
use crate::vfs::{FSItem, FSItemKind, Vfs, VfsBackend, VfsError};

/// High-level filesystem frontend wrapping a [`VfsBackend`].
///
/// # Example
///
/// ```ignore
/// use cocomo_core::vfs::{VfsImpl, LocalFs};
///
/// let fs = VfsImpl::new(LocalFs);
/// let item = fs.new_item("/tmp".as_ref()).await;
/// assert!(item.is_dir());
/// ```
#[derive(Debug)]
pub struct VfsImpl<FS: VfsBackend> {
    /// The underlying backend.
    pub backend: FS,
}

impl<FS: VfsBackend> VfsImpl<FS> {
    /// Creates a new `VfsImpl` wrapping the given backend.
    pub fn new(backend: FS) -> Self {
        Self { backend }
    }

    fn is_dir(item: &FSItem) -> bool {
        matches!(item.kind, FSItemKind::Directory)
    }

    fn is_file(item: &FSItem) -> bool {
        matches!(item.kind, FSItemKind::File { .. })
    }

    fn is_symlink(item: &FSItem) -> bool {
        matches!(item.kind, FSItemKind::Symlink { .. })
    }
}

impl<FS: VfsBackend> Vfs for VfsImpl<FS> {
    async fn new_item(&self, path: &std::path::Path) -> FSItem {
        match self.backend.symlink_metadata(path).await {
            Ok(meta) => {
                let name = path.file_name().unwrap_or(path.as_os_str()).into();
                let p = path.to_path_buf();
                let kind = match meta.kind {
                    DirEntryKind::Directory => FSItemKind::Directory,
                    DirEntryKind::File => FSItemKind::File {
                        file_type: MimeKind::UNKNOWN,
                    },
                    DirEntryKind::Symlink => {
                        let target = self.backend.read_link(path).await.ok();
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

    async fn read_dir(&self, item: &FSItem) -> Result<Vec<FSItem>, VfsError> {
        if !Self::is_dir(item) {
            return Err(VfsError::NotADirectory(item.path.clone()));
        }
        let entries = self.backend.read_dir_raw(&item.path).await?;
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
                    let target = self.backend.read_link(&p).await.ok();
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

    async fn read_file(&self, item: &FSItem) -> Result<String, VfsError> {
        if !Self::is_file(item) {
            return Err(VfsError::Unsupported(format!(
                "not a file: {}",
                item.path.display()
            )));
        }
        self.backend.read_to_string(&item.path).await
    }

    async fn resolve(&self, item: &FSItem) -> Result<FSItem, VfsError> {
        let mut current = if let Some(target) = item.link_target() {
            if target.is_relative() {
                if let Some(parent) = item.path.parent() {
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
            if let Ok(meta) = self.backend.symlink_metadata(&current).await
                && meta.kind == DirEntryKind::Symlink
                && let Ok(next) = self.backend.read_link(&current).await
            {
                current = current
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new(""))
                    .join(&next);
                hops += 1;
                continue;
            }
            break;
        }
        Ok(self.new_item(&current).await)
    }

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

    async fn copy(&self, src: &FSItem, dst: &FSItem) -> Result<(), VfsError> {
        match &src.kind {
            FSItemKind::Directory => self.backend.copy_raw(&src.path, &dst.path).await,
            FSItemKind::File { .. } => {
                let dst = if Self::is_dir(dst) {
                    dst.path.join(&src.name)
                } else {
                    dst.path.to_path_buf()
                };
                self.backend.copy_raw(&src.path, &dst).await
            }
            _ => Err(VfsError::Unsupported(format!(
                "cannot copy {}",
                src.kind
            ))),
        }
    }

    async fn move_item(&self, src: &FSItem, dst: &FSItem) -> Result<(), VfsError> {
        let dst = if Self::is_dir(dst) {
            dst.path.join(&src.name)
        } else {
            dst.path.to_path_buf()
        };
        self.backend.rename_raw(&src.path, &dst).await
    }

    async fn delete(&self, item: &FSItem) -> Result<(), VfsError> {
        match &item.kind {
            FSItemKind::Directory => self.backend.remove_dir_all(&item.path).await,
            FSItemKind::File { .. } => self.backend.remove_file(&item.path).await,
            _ => Err(VfsError::Unsupported(format!(
                "cannot delete {}",
                item.kind
            ))),
        }
    }

    async fn rename(&self, item: &FSItem, new_name: &str) -> Result<(), VfsError> {
        let mut dst = item.path.to_path_buf();
        dst.set_file_name(new_name);
        self.backend.rename_raw(&item.path, &dst).await
    }
}
