// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::path::Path;

use fs_err::tokio as fs;
use mimetype_detector::MimeKind;
use tokio::io::AsyncReadExt;

use crate::{
    backends::{DirEntry, DirEntryKind, Metadata, VfsBackend},
    vfs::VfsError,
};

/// Virtual file system implementation backed by the local OS filesystem.
#[derive(Debug)]
pub struct LocalFs;

impl VfsBackend for LocalFs {
    async fn symlink_metadata(
        &self,
        path: &Path,
    ) -> Result<Metadata, VfsError> {
        let meta = fs::symlink_metadata(path).await?;
        Ok(metadata_from_std(&meta))
    }

    async fn metadata(&self, path: &Path) -> Result<Metadata, VfsError> {
        let meta = fs::metadata(path).await?;
        Ok(metadata_from_std(&meta))
    }

    async fn read_link(
        &self,
        path: &Path,
    ) -> Result<std::path::PathBuf, VfsError> {
        Ok(fs::read_link(path).await?)
    }

    async fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, VfsError> {
        let mut entries = fs::read_dir(path).await?;
        let mut result = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let meta = fs::symlink_metadata(entry.path()).await?;
            let mime_type = if meta.is_file() {
                Some(MimeKind::UNKNOWN)
            } else {
                None
            };
            result.push(DirEntry {
                name: entry.file_name(),
                path: entry.path(),
                metadata: metadata_from_std(&meta),
                mime_type,
            });
        }
        Ok(result)
    }

    async fn read_to_string(&self, path: &Path) -> Result<String, VfsError> {
        Ok(fs::read_to_string(path).await?)
    }

    async fn read_head(
        &self,
        path: &Path,
        n: usize,
    ) -> Result<Vec<u8>, VfsError> {
        let file = fs::File::open(path).await?;
        let mut buf = Vec::with_capacity(n);
        file.take(n as u64).read_to_end(&mut buf).await?;
        Ok(buf)
    }

    async fn canonicalize(
        &self,
        path: &Path,
    ) -> Result<std::path::PathBuf, VfsError> {
        Ok(fs::canonicalize(path).await?)
    }

    async fn copy(&self, from: &Path, to: &Path) -> Result<(), VfsError> {
        let src_meta = self.symlink_metadata(from).await?;
        match src_meta.kind {
            DirEntryKind::Directory => {
                let dst = if self
                    .symlink_metadata(to)
                    .await
                    .is_ok_and(|m| m.kind == DirEntryKind::Directory)
                {
                    to.join(from.file_name().unwrap_or_default())
                } else {
                    to.to_path_buf()
                };
                copy_dir_recursive(from, &dst).await
            }
            _ => {
                let dst = resolve_dst(from, to).await?;
                fs::copy(from, &dst).await?;
                Ok(())
            }
        }
    }

    async fn rename(&self, from: &Path, to: &Path) -> Result<(), VfsError> {
        let dst = if self
            .metadata(to)
            .await
            .is_ok_and(|m| m.kind == DirEntryKind::Directory)
        {
            to.join(from.file_name().unwrap_or_default())
        } else {
            to.to_path_buf()
        };
        fs::rename(from, &dst).await?;
        Ok(())
    }

    async fn remove_file(&self, path: &Path) -> Result<(), VfsError> {
        Ok(fs::remove_file(path).await?)
    }

    async fn remove_dir_all(&self, path: &Path) -> Result<(), VfsError> {
        Ok(fs::remove_dir_all(path).await?)
    }

    async fn create_dir_all(&self, path: &Path) -> Result<(), VfsError> {
        Ok(fs::create_dir_all(path).await?)
    }

    #[cfg(unix)]
    async fn is_mountpoint(&self, path: &Path) -> Result<bool, VfsError> {
        use std::os::unix::fs::MetadataExt;
        let target_meta = fs::metadata(path).await?;
        if let Some(parent) = path.parent() {
            let parent_meta = fs::metadata(parent).await?;
            Ok(target_meta.dev() != parent_meta.dev())
        } else {
            Ok(true) // Root "/" is a mountpoint
        }
    }

    #[cfg(windows)]
    async fn is_mountpoint(&self, path: &Path) -> Result<bool, VfsError> {
        use std::os::windows::fs::MetadataExt;

        // Windows folders use Reparse Points for volume mounts / junctions
        let meta = fs::symlink_metadata(path).await?;
        let file_attributes = meta.file_attributes();

        // Check if the FILE_ATTRIBUTE_REPARSE_POINT (0x400) flag is present
        Ok((file_attributes & 0x400) != 0)
    }
}

fn metadata_from_std(meta: &std::fs::Metadata) -> Metadata {
    Metadata {
        kind: if meta.is_dir() {
            DirEntryKind::Directory
        } else if meta.is_file() {
            DirEntryKind::File
        } else if meta.is_symlink() {
            DirEntryKind::Symlink
        } else {
            DirEntryKind::Special
        },
        len: meta.len(),
        modified: meta.modified().ok(),
        readonly: meta.permissions().readonly(),
    }
}

async fn resolve_dst(
    from: &Path,
    to: &Path,
) -> Result<std::path::PathBuf, VfsError> {
    let mut dst = to.to_path_buf();
    if fs::symlink_metadata(to).await.is_ok_and(|m| m.is_symlink()) {
        dst = fs::canonicalize(to).await?;
    }
    if fs::metadata(&dst).await.is_ok_and(|m| m.is_dir()) {
        dst = dst.join(from.file_name().unwrap_or_default());
    }
    Ok(dst)
}

async fn copy_dir_recursive(from: &Path, to: &Path) -> Result<(), VfsError> {
    let dst = to.join(from.file_name().unwrap_or_default());
    fs::create_dir(&dst).await.unwrap_or(());
    let mut entries = fs::read_dir(from).await?;
    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            Box::pin(copy_dir_recursive(&src_path, &dst_path)).await?;
        } else {
            fs::copy(&src_path, &dst_path).await?;
        }
    }
    Ok(())
}
