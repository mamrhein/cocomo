// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! # File System Operations Module (`fsops`)
//!
//! This module provides functions for basic file system operations: copy,
//! move, delete, and rename.

use std::path::{Path, PathBuf};

use thiserror::Error;
use tokio::fs;

use crate::fsitem::FSItem;

/// Error type for file system operations.
#[derive(Debug, Error)]
pub enum FsError {
    /// Source item not found.
    #[error("source not found: {0}")]
    SourceNotFound(PathBuf),

    /// Destination already exists.
    #[error("destination already exists: {0}")]
    DestinationAlreadyExists(PathBuf),

    /// I/O error occurred.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Operation not supported for the given item type.
    #[error("operation not supported: {0}")]
    Unsupported(String),
}

/// Copies a file or directory from `src` to `dst`.
///
/// If `src` is a directory, it is copied recursively.
pub async fn copy_item(src: &FSItem, dst: &Path) -> Result<(), FsError> {
    if !fs::try_exists(src.path()).await? {
        return Err(FsError::SourceNotFound(src.path().to_path_buf()));
    }

    if src.is_dir() {
        copy_dir_recursive(src.path(), dst).await?;
    } else {
        // Ensure destination parent directory exists
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::copy(src.path(), dst).await?;
    }
    Ok(())
}

/// Recursively copies a directory.
async fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), FsError> {
    fs::create_dir_all(dst).await?;
    let mut entries = fs::read_dir(src).await?;

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

/// Moves a file or directory from `src` to `dst`.
pub async fn move_item(src: &FSItem, dst: &Path) -> Result<(), FsError> {
    if !fs::try_exists(src.path()).await? {
        return Err(FsError::SourceNotFound(src.path().to_path_buf()));
    }

    // Ensure destination parent directory exists
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).await?;
    }

    fs::rename(src.path(), dst).await?;
    Ok(())
}

/// Deletes a file or directory.
pub async fn delete_item(item: &FSItem) -> Result<(), FsError> {
    if !fs::try_exists(item.path()).await? {
        return Err(FsError::SourceNotFound(item.path().to_path_buf()));
    }

    if item.is_dir() {
        fs::remove_dir_all(item.path()).await?;
    } else {
        fs::remove_file(item.path()).await?;
    }
    Ok(())
}

/// Renames a file or directory.
pub async fn rename_item(
    item: &FSItem,
    new_name: &str,
) -> Result<(), FsError> {
    if !fs::try_exists(item.path()).await? {
        return Err(FsError::SourceNotFound(item.path().to_path_buf()));
    }

    let mut dst = item.path().to_path_buf();
    dst.set_file_name(new_name);

    fs::rename(item.path(), &dst).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;
    use tokio::{fs, fs::File, io::AsyncWriteExt};

    use super::*;

    #[tokio::test]
    async fn test_copy_file() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempdir()?;
        let src_path = tmp.path().join("src.txt");
        let dst_path = tmp.path().join("dst.txt");

        let mut file = File::create(&src_path).await?;
        file.write_all(b"Hello world").await?;

        let src_item = FSItem::new(&src_path).await;
        copy_item(&src_item, &dst_path).await?;

        assert!(dst_path.exists());
        let content = fs::read_to_string(&dst_path).await?;
        assert_eq!(content, "Hello world");
        Ok(())
    }

    #[tokio::test]
    async fn test_copy_dir() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempdir()?;
        let src_dir = tmp.path().join("src");
        let dst_dir = tmp.path().join("dst");
        fs::create_dir(&src_dir).await?;

        let src_file = src_dir.join("file.txt");
        let mut file = File::create(&src_file).await?;
        file.write_all(b"Hello from dir").await?;

        let src_item = FSItem::new(&src_dir).await;
        copy_item(&src_item, &dst_dir).await?;

        assert!(dst_dir.exists());
        assert!(dst_dir.join("file.txt").exists());
        let content = fs::read_to_string(dst_dir.join("file.txt")).await?;
        assert_eq!(content, "Hello from dir");
        Ok(())
    }

    #[tokio::test]
    async fn test_move_item() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempdir()?;
        let src_path = tmp.path().join("src.txt");
        let dst_path = tmp.path().join("dst.txt");

        File::create(&src_path).await?;

        let src_item = FSItem::new(&src_path).await;
        move_item(&src_item, &dst_path).await?;

        assert!(!src_path.exists());
        assert!(dst_path.exists());
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_item() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempdir()?;
        let path = tmp.path().join("to_delete.txt");

        File::create(&path).await?;

        let item = FSItem::new(&path).await;
        delete_item(&item).await?;

        assert!(!path.exists());
        Ok(())
    }

    #[tokio::test]
    async fn test_rename_item() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempdir()?;
        let path = tmp.path().join("old.txt");
        let expected = tmp.path().join("new.txt");

        File::create(&path).await?;

        let item = FSItem::new(&path).await;
        rename_item(&item, "new.txt").await?;

        assert!(!path.exists());
        assert!(expected.exists());
        Ok(())
    }
}
