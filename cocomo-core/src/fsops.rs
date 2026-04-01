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
    if src.is_dir() {
        copy_dir_recursive(src.path(), dst).await?;
    } else {
        let mut dst = dst.to_path_buf();
        if dst.is_symlink() {
            dst = dst.canonicalize()?;
        }
        if dst.is_dir() {
            dst = dst.join(src.name());
        }
        fs::copy(src.path(), dst).await?;
    }
    Ok(())
}

/// Recursively copies a directory.
async fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), FsError> {
    let mut dst_path = dst.join(src.file_name().get_or_insert_default());
    fs::create_dir(&dst_path).await.unwrap_or(());
    let mut entries = fs::read_dir(src).await?;
    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        let src_path = entry.path();
        dst_path = dst_path.join(entry.file_name());
        if file_type.is_dir() {
            fs::create_dir(&dst_path).await.unwrap_or(());
            Box::pin(copy_dir_recursive(&src_path, &dst_path)).await?;
        } else {
            fs::copy(&src_path, &dst_path).await?;
        }
    }
    Ok(())
}

/// Moves a file or directory from `src` to `dst`.
pub async fn move_item(src: &FSItem, dst: &Path) -> Result<(), FsError> {
    let mut dst = dst.to_path_buf();
    if dst.is_dir() {
        dst = dst.join(src.name());
    }
    fs::rename(src.path(), &dst).await?;
    Ok(())
}

/// Deletes a file or directory.
pub async fn delete_item(item: &FSItem) -> Result<(), FsError> {
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
    async fn test_copy_file_to_file() -> Result<(), Box<dyn std::error::Error>>
    {
        let txt = "Hello world";
        let tmp = tempdir()?;
        let tmp_dir = tmp.path();
        let src_dir = tmp_dir.join("src");
        let src_file = src_dir.join("src.txt");
        let dst_dir = tmp_dir.join("dst");
        let dst_file = dst_dir.join("dst.txt");
        // Create src dir / file
        fs::create_dir(&src_dir).await?;
        let mut file = File::create(&src_file).await?;
        file.write_all(txt.as_bytes()).await?;
        let src_item = FSItem::new(&src_file).await;
        // dst dir does not exist => copy should fail
        assert!(copy_item(&src_item, &dst_file).await.is_err());
        // Create dst dir
        fs::create_dir(&dst_dir).await?;
        // dst file does not exist, but dst dir does => copy should succeed
        assert!(copy_item(&src_item, &dst_file).await.is_ok());
        assert!(&dst_file.exists());
        // Modify dst file
        let mut file = File::open(&dst_file).await?;
        file.write_all(b"Huhu baloo").await?;
        // dst file exists => copy should succeed (overwrite)
        assert!(copy_item(&src_item, &dst_file).await.is_ok());
        assert!(&dst_file.exists());
        let content = fs::read_to_string(&dst_file).await?;
        assert_eq!(content, txt);
        Ok(())
    }

    #[tokio::test]
    async fn test_copy_file_to_dir() -> Result<(), Box<dyn std::error::Error>>
    {
        let txt = "Hello world";
        let tmp = tempdir()?;
        let tmp_dir = tmp.path();
        let src_dir = tmp_dir.join("src");
        let src_file = src_dir.join("src.txt");
        // Create src dir / file
        fs::create_dir(&src_dir).await?;
        let mut file = File::create(&src_file).await?;
        file.write_all(txt.as_bytes()).await?;
        let src_item = FSItem::new(&src_file).await;
        assert!(src_item.is_file());
        // Create dst dir
        let dst_dir = tmp_dir.join("d1").join("d2").join("dst");
        fs::create_dir_all(&dst_dir).await?;
        assert!(copy_item(&src_item, &dst_dir).await.is_ok());
        let dst_file = dst_dir.join(src_item.name());
        assert!(&dst_file.exists());
        // Modify dst file
        let mut file = File::open(&dst_file).await?;
        file.write_all(b"Huhu baloo").await?;
        // dst file exists => copy should succeed (overwrite)
        assert!(copy_item(&src_item, &dst_dir).await.is_ok());
        assert!(&dst_file.exists());
        let content = fs::read_to_string(&dst_file).await?;
        assert_eq!(content, txt);
        Ok(())
    }

    #[tokio::test]
    async fn test_copy_file_to_link() -> Result<(), Box<dyn std::error::Error>>
    {
        let txt = "Hello world";
        let tmp = tempdir()?;
        let tmp_dir = tmp.path();
        let src_dir = tmp_dir.join("src");
        let src_file = src_dir.join("src.txt");
        // Create src dir / file
        fs::create_dir(&src_dir).await?;
        let mut file = File::create(&src_file).await?;
        file.write_all(txt.as_bytes()).await?;
        let src_item = FSItem::new(&src_file).await;
        assert!(src_item.is_file());
        // Create dst dir and link to it
        let dst_dir = tmp_dir.join("d1").join("d2").join("dst");
        fs::create_dir_all(&dst_dir).await?;
        fs::symlink(&dst_dir, &tmp_dir.join("dst")).await?;
        let dst_link = tmp_dir.join(dst_dir.file_name().unwrap());
        assert!(&dst_link.exists());
        assert!(copy_item(&src_item, &dst_link).await.is_ok());
        let dst_file = dst_link.canonicalize().unwrap().join(src_item.name());
        assert!(&dst_file.exists());
        // Set link to dst file
        fs::remove_file(&dst_link).await?;
        fs::symlink(&dst_file, &tmp_dir.join("dst")).await?;
        assert!(&dst_link.exists());
        // Modify dst file
        let mut file = File::open(&dst_file).await?;
        file.write_all(b"Huhu baloo").await?;
        // dst file exists => copy should succeed (overwrite)
        assert!(copy_item(&src_item, &dst_link).await.is_ok());
        assert!(&dst_file.exists());
        let content = fs::read_to_string(&dst_file).await?;
        assert_eq!(content, txt);
        Ok(())
    }

    #[tokio::test]
    async fn test_copy_dir() -> Result<(), Box<dyn std::error::Error>> {
        let txt = "Hello world";
        let tmp = tempdir()?;
        let tmp_dir = tmp.path();
        let src_dir = tmp_dir.join("src");
        let src_file = src_dir.join("file.txt");
        let parent_dir = tmp_dir.join("parent");
        let dst_dir = parent_dir.join("dst");
        let dst_file = dst_dir.join("src").join("file.txt");
        // Create src dir / file
        fs::create_dir(&src_dir).await?;
        let src_item = FSItem::new(&src_dir).await;
        let mut file = File::create(&src_file).await?;
        file.write_all(txt.as_bytes()).await?;
        // parent dir does not exist => copy should fail
        assert!(copy_item(&src_item, &dst_dir).await.is_err());
        // Create parent dir
        fs::create_dir(&parent_dir).await?;
        // dst dir does not exist => copy should fail
        assert!(copy_item(&src_item, &dst_dir).await.is_err());
        // Create dst dir
        fs::create_dir(&dst_dir).await?;
        // dst dir exists => copy should succeed
        assert!(copy_item(&src_item, &dst_dir).await.is_ok());
        assert!(&dst_dir.exists());
        assert!(&dst_file.exists());
        let content = fs::read_to_string(&dst_file).await?;
        assert_eq!(content, txt);
        // Modify dst file
        let mut file = File::open(&dst_file).await?;
        file.write_all(b"Huhu baloo").await?;
        // dst file exists => copy should succeed (overwrite)
        assert!(copy_item(&src_item, &dst_dir).await.is_ok());
        assert!(&dst_file.exists());
        let content = fs::read_to_string(&dst_file).await?;
        assert_eq!(content, txt);
        // Copying a dir to a file should fail
        assert!(copy_item(&src_item, &dst_file).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_move_file_to_file() -> Result<(), Box<dyn std::error::Error>>
    {
        let txt = "Hello world";
        let tmp = tempdir()?;
        let tmp_dir = tmp.path();
        let src_file = tmp_dir.join("src.txt");
        let dst_dir = tmp_dir.join("dst");
        let dst_file = dst_dir.join("dst.txt");
        // Create src file
        File::create(&src_file).await?;
        let mut file = File::create(&src_file).await?;
        file.write_all(txt.as_bytes()).await?;
        let src_item = FSItem::new(&src_file).await;
        // dst dir does not exist => move should fail
        assert!(move_item(&src_item, &dst_file).await.is_err());
        // Create dst dir
        fs::create_dir(&dst_dir).await?;
        // dst file does not exist, but dst dir does => move should succeed
        assert!(move_item(&src_item, &dst_file).await.is_ok());
        assert!(!&src_file.exists());
        assert!(&dst_file.exists());
        // Recreate src file
        File::create(&src_file).await?;
        let mut file = File::create(&src_file).await?;
        file.write_all(txt.as_bytes()).await?;
        let src_item = FSItem::new(&src_file).await;
        // Modify dst file
        let mut file = File::open(&dst_file).await?;
        file.write_all(b"Huhu baloo").await?;
        // dst file exists => move should succeed (overwrite)
        assert!(move_item(&src_item, &dst_file).await.is_ok());
        assert!(!&src_file.exists());
        assert!(&dst_file.exists());
        let content = fs::read_to_string(&dst_file).await?;
        assert_eq!(content, txt);
        Ok(())
    }

    #[tokio::test]
    async fn test_move_file_to_dir() -> Result<(), Box<dyn std::error::Error>>
    {
        let txt = "Hello world";
        let tmp = tempdir()?;
        let tmp_dir = tmp.path();
        let src_file = tmp_dir.join("file.txt");
        let dst_dir = tmp_dir.join("d1").join("dst");
        let dst_file = dst_dir.join("file.txt");
        // Create src file
        File::create(&src_file).await?;
        let mut file = File::create(&src_file).await?;
        file.write_all(txt.as_bytes()).await?;
        let src_item = FSItem::new(&src_file).await;
        // dst dir and its parent do not exist => move should fail
        assert!(move_item(&src_item, &dst_dir).await.is_err());
        // Create dst dir
        fs::create_dir_all(&dst_dir).await?;
        // dst file does not exist, but dst dir does => move should succeed
        move_item(&src_item, &dst_dir).await?;
        // assert!(move_item(&src_item, &dst_dir).await.is_ok());
        assert!(!&src_file.exists());
        assert!(&dst_file.exists());
        // Recreate src file
        File::create(&src_file).await?;
        let mut file = File::create(&src_file).await?;
        file.write_all(txt.as_bytes()).await?;
        let src_item = FSItem::new(&src_file).await;
        // Modify dst file
        let mut file = File::open(&dst_file).await?;
        file.write_all(b"Huhu baloo").await?;
        // dst file exists => move should succeed (overwrite)
        assert!(move_item(&src_item, &dst_dir).await.is_ok());
        assert!(!&src_file.exists());
        assert!(&dst_file.exists());
        let content = fs::read_to_string(&dst_file).await?;
        assert_eq!(content, txt);
        Ok(())
    }

    #[tokio::test]
    async fn test_move_file_to_link() -> Result<(), Box<dyn std::error::Error>>
    {
        let txt = "Hello world";
        let tmp = tempdir()?;
        let tmp_dir = tmp.path();
        let src_dir = tmp_dir.join("src");
        let src_file = src_dir.join("src.txt");
        // Create src dir / file
        fs::create_dir(&src_dir).await?;
        let mut file = File::create(&src_file).await?;
        file.write_all(txt.as_bytes()).await?;
        let src_item = FSItem::new(&src_file).await;
        assert!(src_item.is_file());
        // Create dst dir and link to it
        let dst_dir = tmp_dir.join("d1").join("d2").join("dst");
        fs::create_dir_all(&dst_dir).await?;
        fs::symlink(&dst_dir, &tmp_dir.join("dst")).await?;
        let dst_link = tmp_dir.join(dst_dir.file_name().unwrap());
        assert!(&dst_link.exists());
        assert!(move_item(&src_item, &dst_link).await.is_ok());
        assert!(!&src_file.exists());
        let dst_file = dst_link.canonicalize().unwrap().join(src_item.name());
        assert!(&dst_file.exists());
        // Recreate src file
        File::create(&src_file).await?;
        let mut file = File::create(&src_file).await?;
        file.write_all(txt.as_bytes()).await?;
        let src_item = FSItem::new(&src_file).await;
        // Set link to dst file
        fs::remove_file(&dst_link).await?;
        fs::symlink(&dst_file, &tmp_dir.join("dst")).await?;
        assert!(&dst_link.exists());
        // Modify dst file
        let mut file = File::open(&dst_file).await?;
        file.write_all(b"Huhu baloo").await?;
        // dst file exists => move should succeed (overwrite)
        assert!(move_item(&src_item, &dst_link).await.is_ok());
        assert!(&dst_file.exists());
        let content = fs::read_to_string(&dst_file).await?;
        assert_eq!(content, txt);
        Ok(())
    }

    #[tokio::test]
    async fn test_move_dir() -> Result<(), Box<dyn std::error::Error>> {
        let txt = "Hello world";
        let tmp = tempdir()?;
        let tmp_dir = tmp.path();
        let src_dir = tmp_dir.join("src");
        let src_file = src_dir.join("file.txt");
        let parent_dir = tmp_dir.join("parent");
        let dst_dir = parent_dir.join("dst");
        let dst_file = dst_dir.join("src").join("file.txt");
        // Create src dir / file
        fs::create_dir(&src_dir).await?;
        let src_item = FSItem::new(&src_dir).await;
        let mut file = File::create(&src_file).await?;
        file.write_all(txt.as_bytes()).await?;
        // parent dir does not exist => move should fail
        assert!(move_item(&src_item, &dst_dir).await.is_err());
        // Create dst dir
        fs::create_dir_all(&dst_dir).await?;
        // dst dir exists => move should succeed
        assert!(move_item(&src_item, &dst_dir).await.is_ok());
        assert!(!&src_dir.exists());
        assert!(&dst_dir.exists());
        assert!(&dst_file.exists());
        let content = fs::read_to_string(&dst_file).await?;
        assert_eq!(content, txt);
        // Recreate src dir / file
        fs::create_dir(&src_dir).await?;
        let src_item = FSItem::new(&src_dir).await;
        File::create(&src_file).await?;
        // dst dir not empty => move should fail
        assert!(move_item(&src_item, &dst_dir).await.is_err());
        // Moving a dir to a file should fail
        assert!(move_item(&src_item, &dst_file).await.is_err());
        // Moving a dir into itself should fail
        let src_item = FSItem::new(&parent_dir).await;
        assert!(move_item(&src_item, &dst_dir).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_item() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempdir()?;
        let tmp_dir = tmp.path();
        let path = tmp_dir.join("to_delete.txt");

        File::create(&path).await?;

        let item = FSItem::new(&path).await;
        delete_item(&item).await?;

        assert!(!path.exists());
        Ok(())
    }

    #[tokio::test]
    async fn test_rename_item() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempdir()?;
        let tmp_dir = tmp.path();
        let path = tmp_dir.join("old.txt");
        let expected = tmp_dir.join("new.txt");

        File::create(&path).await?;

        let item = FSItem::new(&path).await;
        rename_item(&item, "new.txt").await?;

        assert!(!path.exists());
        assert!(expected.exists());
        Ok(())
    }
}
