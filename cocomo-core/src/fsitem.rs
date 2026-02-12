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

#[derive(Clone, Default)]
pub enum FileType {
    #[default]
    Unknown,
    Inferred(&'static MimeType),
}

impl FileType {
    pub fn mime(&self) -> &'static str {
        match self {
            FileType::Unknown => "<unknown>",
            FileType::Inferred(mime_type) => mime_type.mime(),
        }
    }
}

impl PartialEq for FileType {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                FileType::Inferred(self_type),
                FileType::Inferred(other_type),
            ) => self_type.mime() == other_type.mime(),
            (FileType::Unknown, _) | (_, FileType::Unknown) => false,
        }
    }
}

impl fmt::Debug for FileType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileType::Unknown => write!(f, "FileType: <unknown>"),
            FileType::Inferred(mime_type) => {
                write!(f, "FileType: {}", mime_type.mime())
            }
        }
    }
}

impl fmt::Display for FileType {
    fn fmt(&self, form: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(form, "{}", self.mime())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum FSItemType {
    Directory,
    File { file_type: FileType },
    SymLink { path: path::PathBuf },
}

const BROKEN_LINK: FSItemType = FSItemType::SymLink {
    path: path::PathBuf::new(),
};

impl FSItemType {
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

impl fmt::Display for FSItemType {
    fn fmt(&self, form: &mut fmt::Formatter) -> fmt::Result {
        write!(
            form,
            "{}",
            match self {
                Self::Directory => "Directory",
                Self::File { .. } => "File",
                Self::SymLink { .. } => "SymLink",
            }
        )
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
    pub fn new(path: &path::Path) -> io::Result<Self> {
        let path = path.canonicalize()?;
        let meta = path.metadata()?;
        Ok(Self {
            item_type: match &meta {
                m if m.is_dir() => FSItemType::Directory,
                m if m.is_file() => match detect_file(&path) {
                    Ok(guess) => FSItemType::File {
                        file_type: FileType::Inferred(guess),
                    },
                    _ => FSItemType::File {
                        file_type: FileType::default(),
                    },
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
            name: path.file_name().unwrap().to_string_lossy().into(),
            path,
            metadata: meta,
        })
    }

    #[inline(always)]
    pub fn item_type(&self) -> &FSItemType {
        &self.item_type
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
        self.item_type == FSItemType::Directory
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
        let file = FSItem::try_from("./Cargo.toml").unwrap();
        assert!(file.is_file());
        assert_eq!(file.name(), "Cargo.toml");
    }
}
