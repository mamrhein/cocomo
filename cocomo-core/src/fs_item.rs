// ---------------------------------------------------------------------------
// Copyright:   (c) 2022 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ItemType {
    Directory,
    File,
}

impl Default for ItemType {
    fn default() -> Self {
        Self::Directory
    }
}

#[derive(Debug)]
pub struct FSItem {
    pub path: PathBuf,
    pub item_type: ItemType,
    pub metadata: fs::Metadata,
}

impl TryFrom<&String> for FSItem {
    type Error = io::Error;

    fn try_from(s: &String) -> Result<Self, Self::Error> {
        let path = Path::new(&s).canonicalize()?;
        let metadata = fs::metadata(&path)?;
        Ok(Self {
            path,
            item_type: if metadata.is_dir() {
                ItemType::Directory
            } else {
                ItemType::File
            },
            metadata,
        })
    }
}
