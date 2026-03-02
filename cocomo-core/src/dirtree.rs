// ---------------------------------------------------------------------------
// Copyright:   (c) 2022 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

use std::path;

use tokio::{fs, io};

use crate::fsitem::FSItem;

type DirTreeItem = (u16, FSItem);
type DirTreeItemList = Vec<DirTreeItem>;

#[derive(Clone, Debug)]
pub(crate) struct FlattenedDirTree {
    root: path::PathBuf,
    items: DirTreeItemList,
}

async fn read_dir<P: AsRef<path::Path>>(
    level: u16,
    path: P,
) -> io::Result<DirTreeItemList> {
    let path = path.as_ref();
    let mut items = DirTreeItemList::new();
    let mut rd = fs::read_dir(path).await?;
    while let Some(entry) = rd.next_entry().await? {
        let item = FSItem::new(entry.path()).await;
        items.push((level, item));
    }
    Ok(items)
}

impl FlattenedDirTree {
    pub(crate) async fn new(root: &path::Path) -> io::Result<Self> {
        let root = root.to_path_buf();
        let items = read_dir(0, &root).await?;
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

    #[tokio::test]
    async fn test_dirtree() {
        let tree = FlattenedDirTree::new(path::Path::new(".."))
            .await
            .expect("Error reading '.'");
        assert!(tree.root.is_dir());
    }
}
