// ---------------------------------------------------------------------------
// Copyright:   (c) 2022 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! # Directory Reading Module (`readdir`)
//!
//! This internal module provides a helper for asynchronously reading the
//! contents of a directory and wrapping them into [`FSItem`] objects.

use tokio::{fs, io};

use crate::fsitem::FSItem;

/// Reads the contents of a directory and returns a vector of [`FSItem`]s.
///
/// If `dir` is a symbolic link, it is first resolved to its target directory.
pub(crate) async fn read_dir(dir: &FSItem) -> io::Result<Vec<FSItem>> {
    let dir = dir.unlink().await.into_owned();
    let path = dir.path();
    let mut items = Vec::new();
    let mut rd = fs::read_dir(path).await?;
    while let Some(entry) = rd.next_entry().await? {
        let item = FSItem::new(entry.path()).await;
        items.push(item);
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_readdir() {
        let content = read_dir(&FSItem::new("..").await)
            .await
            .expect("Error reading '..'");
        assert!(content.len() > 0);
    }
}
