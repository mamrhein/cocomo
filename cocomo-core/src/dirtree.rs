// ---------------------------------------------------------------------------
// Copyright:   (c) 2022 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::{fs, io, path};

use crate::fsitem::FSItem;

type DirTreeItem = (u16, FSItem);
type DirTreeItemList = Vec<DirTreeItem>;

#[derive(Clone, Debug)]
pub(crate) struct FlattenedDirTree {
    root: path::PathBuf,
    items: DirTreeItemList,
}

fn read_dir(level: u16, path: &path::PathBuf) -> io::Result<DirTreeItemList> {
    let mut items = DirTreeItemList::new();
    let mut child_entries: Vec<fs::DirEntry> = fs::read_dir(path)?
        .map(|r| r.expect("Error reading directory entry."))
        .collect();
    child_entries.sort_unstable_by_key(|entry| entry.file_name());
    for entry in child_entries {
        let item = FSItem::try_from(&entry)?;
        let is_dir = item.is_dir();
        let path = item.path().clone();
        items.push((level, item));
        if is_dir {
            items.append(&mut read_dir(level + 1, &path)?);
        }
    }
    Ok(items)
}

impl FlattenedDirTree {
    pub(crate) fn new(root: &path::Path) -> io::Result<Self> {
        let root = root.to_path_buf();
        let items = read_dir(0, &root)?;
        Ok(Self { root, items })
    }
}

impl IntoIterator for FlattenedDirTree {
    type Item = DirTreeItem;
    type IntoIter = std::vec::IntoIter<Self::Item>;
    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dirtree() {
        let tree = FlattenedDirTree::new(path::Path::new("."))
            .expect("Error reading '.'");
        assert!(tree.root.is_dir());
    }
}
