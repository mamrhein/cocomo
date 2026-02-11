// ---------------------------------------------------------------------------
// Copyright:   (c) 2022 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::{fmt, fs, io, path};

use infer::get_from_path;

#[derive(Clone, Debug, PartialEq, Default)]
pub enum FileType {
    #[default]
    Unknown,
    Inferred {
        mime: &'static str,
        ext: &'static str,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum FSItemType {
    Directory,
    File { file_type: FileType },
    SymLink { path: path::PathBuf },
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
                m if m.is_file() => match get_from_path(&path) {
                    Ok(Some(guess)) => FSItemType::File {
                        file_type: FileType::Inferred {
                            mime: guess.mime_type(),
                            ext: guess.extension(),
                        },
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
