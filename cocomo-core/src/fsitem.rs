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
//! system entries, including regular files, directories, and symbolic links. It
//! wraps low-level metadata and extends it with MIME type detection and
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
//!   for files).
//!
//! ## Usage Example
//!
//! ```
//! use std::path::Path;
//!
//! use fsitem::{FSItem, FSItemType};
//!
//! let item = FSItem::new(Path::new("example.txt"))?;
//! if item.is_file() {
//!     println!("MIME: {}", item.mime());
//! }
//! ```

use std::{fmt, fs, io, path};

use mimetype_detector::{detect_file, MimeType};

pub type FileType = MimeType;

// MIME types not supported by mimetype_detector
const INODE_DIR: &str = "inode/directory";
const INODE_SYMLINK: &str = "inode/symlink";

#[derive(Clone)]
pub enum FSItemType {
    Directory,
    File { file_type: &'static FileType },
    SymLink { path: path::PathBuf },
}

const BROKEN_LINK: FSItemType = FSItemType::SymLink {
    path: path::PathBuf::new(),
};

impl FSItemType {
    /// Returns the MIME type string representing this item type.
    pub fn mime(&self) -> &'static str {
        match self {
            FSItemType::Directory => INODE_DIR,
            FSItemType::File { file_type } => file_type.mime(),
            FSItemType::SymLink { .. } => INODE_SYMLINK,
        }
    }

    /// Returns `true` if this item type is comparable with the other item type.
    ///
    /// Two items are comparable if they are both directories, or both files
    /// of the same MIME type. Symbolic links and mixed types are not
    /// comparable.
    pub fn comparable(&self, other: &FSItemType) -> bool {
        match (self, other) {
            (FSItemType::Directory, FSItemType::Directory) => true,
            (
                FSItemType::File {
                    file_type: left_file_type,
                },
                FSItemType::File {
                    file_type: right_file_type,
                },
            ) => left_file_type.mime() == right_file_type.mime(),
            _ => false,
        }
    }
}

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
            Self::SymLink { path } => {
                format!("SymLink({})", path.display())
            }
        };
        form.write_str(s.as_str())
    }
}

#[derive(Clone, Debug)]
pub struct FSItem {
    item_type: FSItemType,
    name: String,
    path: path::PathBuf,
    metadata: fs::Metadata,
}

impl FSItem {
    /// Creates a new `FSItem` from the given path.
    ///
    /// Reads metadata for the entry, detects its type (file, directory or
    /// symlink), and determines the MIME type for files. Returns an error
    /// if the path does not exist or is of an unsupported type.
    pub fn new<P: AsRef<path::Path>>(path: P) -> io::Result<Self> {
        let path = path.as_ref();
        let meta = path.symlink_metadata()?;
        Ok(Self {
            item_type: match &meta {
                m if m.is_dir() => FSItemType::Directory,
                m if m.is_file() => FSItemType::File {
                    file_type: detect_file(&path)?,
                },
                m if m.is_symlink() => FSItemType::SymLink {
                    path: fs::read_link(&path)?,
                },
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "Unknown directory entry",
                    ));
                }
            },
            name: path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .into(),
            path: path.to_path_buf(),
            metadata: meta,
        })
    }

    #[inline(always)]
    /// Returns a reference to the logical type of this file system item.
    pub fn item_type(&self) -> &FSItemType {
        &self.item_type
    }

    #[inline(always)]
    /// Returns the MIME type string for this item.
    pub fn mime(&self) -> &'static str {
        self.item_type.mime()
    }

    #[inline(always)]
    /// Returns the name of this file system item (basename of its path).
    pub fn name(&self) -> &str {
        &self.name
    }

    #[inline(always)]
    /// Returns a reference to the full path of this file system item.
    pub fn path(&self) -> &path::PathBuf {
        &self.path
    }

    #[inline(always)]
    /// Returns a reference to the raw `fs::Metadata` for this item.
    pub fn metadata(&self) -> &fs::Metadata {
        &self.metadata
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
    pub fn unlink(&self) -> io::Result<FSItem> {
        match self.item_type() {
            FSItemType::SymLink { path } => {
                let mut current_path = path.to_path_buf();
                // Follow symlinks until we reach a non-symlink
                while let Ok(link_target) = fs::read_link(&current_path) {
                    current_path = link_target;
                }
                FSItem::new(&current_path)
            }
            _ => Ok(self.clone()),
        }
    }

    /// Returns the resolved logical type of this item.
    ///
    /// For directories and files, returns their type directly. For symbolic
    /// links, follows the chain transitively to determine the final
    /// target's type; for broken links returns a placeholder representing a
    /// symlink with empty path.
    pub fn final_item_type(&self) -> FSItemType {
        match self.item_type() {
            FSItemType::Directory => self.item_type.clone(),
            FSItemType::File { .. } => self.item_type.clone(),
            FSItemType::SymLink { .. } => match self.unlink() {
                Ok(item) => item.item_type,
                Err(_) => BROKEN_LINK,
            },
        }
    }
}

/// Creates an `FSItem` from a directory entry obtained via `fs::ReadDir`.
impl TryFrom<&fs::DirEntry> for FSItem {
    type Error = io::Error;

    fn try_from(item: &fs::DirEntry) -> Result<Self, Self::Error> {
        Self::new(&item.path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dir() {
        let dir = FSItem::new(".").unwrap();
        assert!(dir.is_dir());
        assert_eq!(dir.name(), ".");
        assert_eq!(dir.mime(), INODE_DIR);
    }

    #[test]
    fn test_file() {
        let file = FSItem::new("./Cargo.toml").unwrap();
        assert!(file.is_file());
        assert_eq!(file.name(), "Cargo.toml");
        assert_eq!(file.mime(), "application/toml");
    }

    #[cfg(target_family = "unix")]
    #[test]
    fn test_symlink() {
        let link = FSItem::new("/usr/lib/libzstd.so").unwrap();
        assert!(link.is_link());
        assert_eq!(link.name(), "libzstd.so");
        assert_eq!(link.mime(), INODE_SYMLINK);
    }
}
