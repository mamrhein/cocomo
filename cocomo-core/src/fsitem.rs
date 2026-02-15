// ---------------------------------------------------------------------------
// Copyright:   (c) 2022 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use mimetype_detector::{detect_file, MimeType};
use std::{fmt, fs, io, path};

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
    pub fn mime(&self) -> &'static str {
        match self {
            FSItemType::Directory => INODE_DIR,
            FSItemType::File { file_type } => file_type.mime(),
            FSItemType::SymLink { .. } => INODE_SYMLINK,
        }
    }
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
    pub fn item_type(&self) -> &FSItemType {
        &self.item_type
    }

    #[inline(always)]
    pub fn mime(&self) -> &'static str {
        self.item_type.mime()
    }

    #[inline(always)]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[inline(always)]
    pub fn path(&self) -> &path::PathBuf {
        &self.path
    }

    #[inline(always)]
    pub fn metadata(&self) -> &fs::Metadata {
        &self.metadata
    }

    #[inline(always)]
    pub fn is_dir(&self) -> bool {
        matches!(self.item_type, FSItemType::Directory)
    }

    #[inline(always)]
    pub fn is_file(&self) -> bool {
        matches!(self.item_type, FSItemType::File { .. })
    }

    #[inline(always)]
    pub fn is_link(&self) -> bool {
        matches!(self.item_type, FSItemType::SymLink { .. })
    }

    /// Follows symbolic links until a non-link is reached and returns that path as a FSItem.
    /// For files and directories, it returns the item itself.
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

impl TryFrom<&str> for FSItem {
    type Error = io::Error;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        let path = path::PathBuf::from(s);
        Self::new(&path)
    }
}

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
        let dir = FSItem::try_from(".").unwrap();
        assert!(dir.is_dir());
        assert_eq!(dir.name(), "cocomo-core");
    }

    #[test]
    fn test_file() {
        let file = FSItem::new("./Cargo.toml").unwrap();
        assert!(file.is_file());
        assert_eq!(file.name(), "Cargo.toml");
    }

    #[test]
    fn test_symlink() {
        let link = FSItem::new("/usr/lib/libzstd.so").unwrap();
        assert!(link.is_link());
        assert_eq!(link.name(), "libz.so");
    }
}
