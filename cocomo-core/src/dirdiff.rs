// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! # Directory Comparison Module (`dirdiff`)
//!
//! This module provides the logic for comparing two directories. It computes
//! a list of differences between the files and subdirectories found in each.

use std::{cmp, ffi, io};

use crate::{fsitem::FSItem, readdir::read_dir};

const EMPTY: &ffi::OsString = &ffi::OsString::new();

/// Identifies which side of a comparison an item belongs to or is newer on.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum DiffSide {
    /// The left side (typically the first directory).
    Left,
    /// The right side (typically the second directory).
    Right,
}

/// The criteria used to determine that two items are the same.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum By {
    /// Metadata (size and modification time) match.
    Metadata,
    /// Full content match.
    Content,
}

/// The type of difference found between two items with the same name.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum DiffItemType {
    /// Item exists only on the left side.
    LeftOnly,
    /// Item exists only on the right side.
    RightOnly,
    /// Item exists on both sides but is different.
    Different {
        /// Which side has the newer version, if determined by metadata.
        newer: Option<DiffSide>,
    },
    /// Item exists on both sides and is considered the same.
    Same {
        /// How the equality was determined.
        by: By,
    },
}

/// A single entry in a directory comparison result.
#[derive(Clone, Debug)]
pub struct DiffItem {
    /// The result of the comparison for this item.
    pub diff_item_type: DiffItemType,
    /// The file system item from the left side, if it exists.
    pub left_item: Option<FSItem>,
    /// The file system item from the right side, if it exists.
    pub right_item: Option<FSItem>,
}

impl DiffItem {
    /// Compares two optional [`FSItem`]s and returns a `DiffItem`.
    pub fn new(
        left_item: &Option<FSItem>,
        right_item: &Option<FSItem>,
    ) -> io::Result<Self> {
        match (left_item, right_item) {
            (Some(left), Some(right)) => Ok(Self {
                diff_item_type: match (left.metadata(), right.metadata()) {
                    (None, _) | (_, None) => {
                        DiffItemType::Different { newer: None }
                    }
                    (Some(left_meta), Some(right_meta)) => {
                        match (left_meta.modified(), right_meta.modified()) {
                            (Ok(left_time), Ok(right_time)) => match left_time
                                .cmp(&right_time)
                            {
                                cmp::Ordering::Less => {
                                    DiffItemType::Different {
                                        newer: Some(DiffSide::Right),
                                    }
                                }
                                cmp::Ordering::Greater => {
                                    DiffItemType::Different {
                                        newer: Some(DiffSide::Left),
                                    }
                                }
                                _ => {
                                    if left_meta.len() == right_meta.len() {
                                        DiffItemType::Same { by: By::Metadata }
                                    } else {
                                        DiffItemType::Different { newer: None }
                                    }
                                }
                            },
                            _ => DiffItemType::Different { newer: None },
                        }
                    }
                },
                left_item: left_item.clone(),
                right_item: right_item.clone(),
            }),
            (Some(..), None) => Ok(Self {
                diff_item_type: DiffItemType::LeftOnly,
                left_item: left_item.clone(),
                right_item: right_item.clone(),
            }),
            (None, Some(..)) => Ok(Self {
                diff_item_type: DiffItemType::RightOnly,
                left_item: left_item.clone(),
                right_item: right_item.clone(),
            }),
            _ => Err(io::Error::other(
                "Internal error: both sides of diff item empty.",
            )),
        }
    }

    /// Returns the name of the item.
    pub fn name(&self) -> &ffi::OsString {
        if let Some(left_item) = &self.left_item {
            return left_item.name();
        }
        if let Some(right_item) = &self.right_item {
            return right_item.name();
        }
        // should never happen
        EMPTY
    }

    /// Returns `true` if the item exists on both sides and the left one is
    /// newer.
    pub fn left_newer(&self) -> bool {
        matches!(
            self.diff_item_type,
            DiffItemType::Different {
                newer: Some(DiffSide::Left)
            }
        )
    }

    /// Returns `true` if the item exists on both sides and the right one is
    /// newer.
    pub fn right_newer(&self) -> bool {
        matches!(
            self.diff_item_type,
            DiffItemType::Different {
                newer: Some(DiffSide::Right)
            }
        )
    }
}

/// A complete result of a comparison between two directories.
#[derive(Clone, Debug)]
pub struct DirDiff {
    /// The source directory on the left side.
    pub left_dir: FSItem,
    /// The source directory on the right side.
    pub right_dir: FSItem,
    /// The list of compared entries within these directories.
    pub items: Vec<DiffItem>,
}

#[inline]
fn cmp_items(a: &FSItem, b: &FSItem) -> cmp::Ordering {
    (!a.is_dir(), a.name()).cmp(&(!b.is_dir(), b.name()))
}

impl DirDiff {
    /// Compares the contents of two directories.
    pub async fn new(
        left_dir: Option<&FSItem>,
        right_dir: Option<&FSItem>,
    ) -> io::Result<Self> {
        let mut left_items = if let Some(dir) = left_dir {
            read_dir(dir).await?
        } else {
            Vec::new()
        };
        let mut right_items = if let Some(dir) = right_dir {
            read_dir(dir).await?
        } else {
            Vec::new()
        };
        left_items.sort_by(|a, b| cmp_items(b, a));
        right_items.sort_by(|a, b| cmp_items(b, a));
        let mut diff_items: Vec<DiffItem> = Vec::new();
        let mut left_item = left_items.pop();
        let mut right_item = right_items.pop();
        loop {
            match (&left_item, &right_item) {
                (Some(left), Some(right)) => match cmp_items(left, right) {
                    cmp::Ordering::Equal => {
                        diff_items
                            .push(DiffItem::new(&left_item, &right_item)?);
                        left_item = left_items.pop();
                        right_item = right_items.pop();
                    }
                    cmp::Ordering::Less => {
                        diff_items.push(DiffItem::new(&left_item, &None)?);
                        left_item = left_items.pop();
                    }
                    cmp::Ordering::Greater => {
                        diff_items.push(DiffItem::new(&None, &right_item)?);
                        right_item = right_items.pop();
                    }
                },
                (Some(..), None) => {
                    diff_items.push(DiffItem::new(&left_item, &right_item)?);
                    left_item = left_items.pop();
                }
                (None, Some(..)) => {
                    diff_items.push(DiffItem::new(&left_item, &right_item)?);
                    right_item = right_items.pop();
                }
                _ => {
                    break;
                }
            }
        }
        Ok(Self {
            left_dir: left_dir.cloned().unwrap_or_else(FSItem::default),
            right_dir: right_dir.cloned().unwrap_or_else(FSItem::default),
            items: diff_items,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path;

    use super::*;

    #[tokio::test]
    async fn test_dirdiff() {
        let path1 = path::Path::new("../cocomo-core");
        let dir1 = FSItem::new(path1).await;
        let path2 = path::Path::new("../cocomo-tui");
        let dir2 = FSItem::new(path2).await;
        let diff = DirDiff::new(Some(&dir1), Some(&dir2))
            .await
            .expect("Error creating diff");
        assert!(!diff.items.is_empty());
    }
}
