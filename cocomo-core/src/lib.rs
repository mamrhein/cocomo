// ---------------------------------------------------------------------------
// Copyright:   (c) 2022 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

#![allow(dead_code)]

//! # Cocomo Core
//!
//! `cocomo-core` provides the core functionality for the Cocomo directory
//! comparison tool. It includes types for representing file system items,
//! reading directory contents, and computing differences between directories.

pub mod backends;
pub mod dirdiff;
mod fsitem;
pub mod fsops;
mod readdir;
pub mod textdiff;
pub mod vfs;

pub use backends::{DirEntry, DirEntryKind, Metadata};
pub use dirdiff::{By, DiffItem, DiffItemType, DiffSide, DirDiff};
pub use fsitem::{FSItem, FSItemType};
pub use fsops::{FsError, copy_item, delete_item, move_item, rename_item};
pub use textdiff::{DiffLine, LineDiffType, TextDiff};
pub use vfs::{FSItemKind, Vfs, VfsBackend, VfsError, VfsImpl, VfsItem};
