// ---------------------------------------------------------------------------
// Copyright:   (c) 2022 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::{fmt, fs, io, path};

// TODO: replace by struct from extern file type matcher (maybe 'infer').
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FileType {}

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
    fn new(path: &path::PathBuf, meta: &fs::Metadata) -> io::Result<Self> {
        let path = path.canonicalize()?;
        // TODO: examine file type
        let file_type = FileType {};
        Ok(Self {
            item_type: match meta {
                m if m.is_dir() => FSItemType::Directory,
                m if m.is_file() => FSItemType::File {
                    file_type: file_type,
                },
                m if m.is_symlink() => FSItemType::SymLink {
                    path: fs::read_link(&path)?,
                },
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "Unknown directory entry",
                    ))
                }
            },
            name: path.file_name().unwrap().to_string_lossy().into(),
            path: path.clone(),
            metadata: meta.clone(),
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
}

impl TryFrom<&String> for FSItem {
    type Error = io::Error;

    fn try_from(s: &String) -> Result<Self, Self::Error> {
        let path = path::PathBuf::from(s);
        let meta = fs::metadata(&path)?;
        Self::new(&path, &meta)
    }
}

impl TryFrom<&fs::DirEntry> for FSItem {
    type Error = io::Error;

    fn try_from(item: &fs::DirEntry) -> Result<Self, Self::Error> {
        Self::new(&item.path(), &item.metadata()?)
    }
}
