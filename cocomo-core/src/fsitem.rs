// ---------------------------------------------------------------------------
// Copyright:   (c) 2022 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! # File System Item Module (`fsitem`)
//!
//! This module provides a unified abstraction for different types of file
//! system entries, including regular files, directories, and symbolic links.
//! It wraps low-level metadata and extends it with MIME type detection and
//! convenient accessors.
//!
//! ## Overview
//!
//! The primary types are:
//!
//! - [`FSItemType`]: Enum representing the logical type of a file system entry
//!   (`Directory`, `File`, or `SymLink`). Each variant carries additional
//!   information when applicable, such as the detected MIME type for files or
//!   target path for symlinks.
//!
//! - [`FSItem`]: A complete wrapper around a file system entry, including its
//!   path, metadata, name, and logical type. Provides high-level accessors for
//!   inspection and navigation (e.g., dereferencing symlinks).
//!
//! ## Key Features
//!
//! - **Unified handling** of files, directories, and symbolic links.
//! - **MIME type detection** using `mimetype_detector`, with special handling
//!   for directory (`inode/directory`) and symlink (`inode/symlink`) types.
//! - **Symlink resolution**: [`FSItem::unlink()`] resolves symlinks
//!   transitively to the ultimate target, while [`FSItem::final_item_type()`]
//!   yields the resolved logical type, or a placeholder for broken links.
//! - **Comparison support** via [`FSItemType::comparable()`], allowing
//!   comparison only between entries of compatible types (e.g., same MIME type
//!   for files).`

use std::{borrow::Cow, ffi, fmt, fs, path};

use chrono::{DateTime, Local};
use mimetype_detector::{detect_file, MimeKind};
use tokio::{fs as async_fs, io};

pub type FileType = MimeKind;

#[derive(Clone)]
pub enum FSItemType {
    Directory,
    File { file_type: FileType },
    SymLink { target: path::PathBuf },
    Special,
    Invalid { cause: io::ErrorKind },
}

const BROKEN_LINK: FSItemType = FSItemType::SymLink {
    target: path::PathBuf::new(),
};

impl fmt::Debug for FSItemType {
    fn fmt(&self, form: &mut fmt::Formatter) -> fmt::Result {
        write!(form, "FSItemType::{}", self)
    }
}

impl fmt::Display for FSItemType {
    fn fmt(&self, form: &mut fmt::Formatter) -> fmt::Result {
        let s = match self {
            Self::Directory => "Directory".into(),
            Self::File { file_type } => {
                format!("File({})", file_type)
            }
            Self::SymLink { target: path } => {
                format!("SymLink({})", path.display())
            }
            Self::Special => "Special".into(),
            Self::Invalid { cause } => {
                format!("Invalid({})", cause)
            }
        };
        form.write_str(s.as_str())
    }
}

#[derive(Clone, Debug)]
pub struct FSItem {
    item_type: FSItemType,
    name: ffi::OsString,
    path: path::PathBuf,
    metadata: Option<fs::Metadata>,
}

impl FSItem {
    /// Creates a new `FSItem` from the given path.
    ///
    /// Reads metadata for the entry, detects its type (file, directory or
    /// symlink), and determines the MIME type for files. Returns an FSItem
    /// with FSItemType::Invalid if the path does not exist or is of an
    /// unsupported type.
    pub async fn new<P: AsRef<path::Path>>(path: P) -> Self {
        let path = path.as_ref();
        match async_fs::symlink_metadata(path).await {
            Ok(meta) => Self {
                item_type: match &meta {
                    m if m.is_dir() => FSItemType::Directory,
                    m if m.is_file() => FSItemType::File {
                        file_type: detect_file(&path)
                            .map_or_else(|_| MimeKind::UNKNOWN, |t| t.kind()),
                    },
                    m if m.is_symlink() => FSItemType::SymLink {
                        target: async_fs::read_link(&path)
                            .await
                            .unwrap_or(path::PathBuf::new()),
                    },
                    _ => FSItemType::Special,
                },
                name: path.file_name().unwrap_or(path.as_os_str()).into(),
                path: path.to_path_buf(),
                metadata: Some(meta),
            },
            Err(error) => Self {
                item_type: FSItemType::Invalid {
                    cause: error.kind(),
                },
                name: path.file_name().unwrap_or(path.as_os_str()).into(),
                path: path.to_path_buf(),
                metadata: None,
            },
        }
    }

    #[inline(always)]
    /// Returns a reference to the logical type of this file system item.
    pub fn item_type(&self) -> &FSItemType {
        &self.item_type
    }

    #[inline(always)]
    /// Returns the MIME type string for this item.
    pub fn file_type(&self) -> Option<FileType> {
        match self.item_type {
            FSItemType::File { file_type } => Some(file_type),
            _ => None,
        }
    }

    #[inline(always)]
    /// Returns the name of this file system item (basename of its path).
    pub fn name(&self) -> &ffi::OsString {
        &self.name
    }

    #[inline(always)]
    /// Returns a reference to the full path of this file system item.
    pub fn path(&self) -> &path::PathBuf {
        &self.path
    }

    #[inline(always)]
    /// Returns a reference to the raw `fs::Metadata` for this item.
    pub fn metadata(&self) -> &Option<fs::Metadata> {
        &self.metadata
    }

    pub fn modified(&self) -> Option<DateTime<Local>> {
        Some(self.metadata().as_ref()?.modified().ok()?.into())
    }

    #[inline(always)]
    /// Returns `true` if this item is a directory.
    pub fn is_dir(&self) -> bool {
        matches!(self.item_type, FSItemType::Directory)
    }

    #[inline(always)]
    /// Returns `true` if this item is a regular file.
    pub fn is_file(&self) -> bool {
        matches!(self.item_type, FSItemType::File { .. })
    }

    #[inline(always)]
    /// Returns `true` if this item is a symbolic link.
    pub fn is_link(&self) -> bool {
        matches!(self.item_type, FSItemType::SymLink { .. })
    }

    /// Follows symbolic links transitively until a non-link target is reached
    /// and returns it as an `FSItem`. For files and directories, returns the
    /// item itself.
    ///
    /// Note: This method does not check if the ultimate target exists; for
    /// broken symlinks it will try to access the nonexistent path and fail
    /// with an error.
    pub async fn unlink(&self) -> Cow<'_, Self> {
        match self.item_type() {
            FSItemType::SymLink { target } => {
                let mut current_path = self
                    .path()
                    .parent()
                    .unwrap_or_else(|| path::Path::new(""))
                    .join(target);
                // Follow symlinks until we reach a non-symlink or a broken
                // link, with a limit to avoid infinite loops
                let mut hops = 0;
                while hops < 32 {
                    if let Ok(link_target) =
                        async_fs::read_link(&current_path).await
                    {
                        current_path = current_path
                            .parent()
                            .unwrap_or_else(|| path::Path::new(""))
                            .join(link_target);
                        hops += 1;
                    } else {
                        break;
                    }
                }
                Cow::Owned(FSItem::new(&current_path).await)
            }
            _ => Cow::Borrowed(&self),
        }
    }

    /// Returns the resolved logical type of this item.
    ///
    /// For directories and files, returns their type directly. For symbolic
    /// links, follows the chain transitively to determine the final
    /// target's type; for broken links returns a placeholder representing a
    /// symlink with empty path.
    pub async fn final_item_type(&self) -> Cow<'_, FSItemType> {
        match self.item_type() {
            FSItemType::SymLink { .. } => {
                Cow::Owned(self.unlink().await.item_type.clone())
            }
            _ => Cow::Borrowed(&self.item_type),
        }
    }

    /// Returns `true` if this item is comparable with the other item.
    ///
    /// Two items are comparable if they are both directories, or both files
    /// of the same MIME kind. Symbolic links are compared based on their
    /// resolved target types. Broken links and special files are never
    /// comparable.
    pub async fn comparable(&self, other: &FSItem) -> bool {
        let self_final_item_type = self.final_item_type().await;
        let other_final_item_type = other.final_item_type().await;
        match (
            self_final_item_type.as_ref(),
            other_final_item_type.as_ref(),
        ) {
            (FSItemType::Directory, FSItemType::Directory) => true,
            (
                FSItemType::File {
                    file_type: left_file_type,
                },
                FSItemType::File {
                    file_type: right_file_type,
                },
            ) => left_file_type == right_file_type,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_dir() {
        let dir = FSItem::new("../target").await;
        assert!(dir.is_dir());
        assert_eq!(dir.name(), "target");
        assert!(dir.file_type().is_none());
    }

    #[tokio::test]
    async fn test_file() {
        let file = FSItem::new("./Cargo.toml").await;
        assert!(file.is_file());
        assert_eq!(file.name(), "Cargo.toml");
        assert_eq!(file.file_type().unwrap(), FileType::TEXT);
    }

    #[cfg(target_family = "unix")]
    #[tokio::test]
    async fn test_symlink() {
        let link = FSItem::new("/usr/lib/libzstd.so").await;
        assert!(link.is_link());
        assert_eq!(link.name(), "libzstd.so");
        let file = link.unlink().await;
        assert!(file.is_file());
        assert_eq!(file.file_type().unwrap(), FileType::EXECUTABLE);
    }

    #[tokio::test]
    async fn test_comparable() {
        let dir1 = FSItem::new("../cocomo-tui").await;
        let dir2 = FSItem::new(".").await;
        let file1 = FSItem::new("./Cargo.toml").await;
        let file2 = FSItem::new("./Cargo.lock").await;
        let file3 = FSItem::new("../cocomo-tui/Cargo.toml").await;
        let invalid = FSItem::new("./coc").await;
        assert!(dir1.comparable(&dir1).await);
        assert!(dir1.comparable(&dir2).await);
        assert!(!dir1.comparable(&file2).await);
        assert!(!dir2.comparable(&invalid).await);
        assert!(file1.comparable(&file3).await);
        assert!(!file1.comparable(&file2).await);
        assert!(!file1.comparable(&invalid).await);
    }
}
