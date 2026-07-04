// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! cocomo_lib — Unified async filesystem library for file/folder comparison,
//! copying, and moving.
//!
//! # Architecture
//!
//! All filesystem I/O is async behind the [`FileSystem`] trait. Built-in
//! providers include [`LocalFs`] for the local filesystem. Front-ends (TUI,
//! GUI, CLI) await on the same abstractions, keeping the library as the
//! single source of business logic.
//!
//! # Key modules
//!
//! - [`fs`] — Core [`FileSystem`] trait and related types.
//! - [`local`] — [`LocalFs`], the local filesystem provider.
//! - [`hash`] — Content hashing (blake3) and LRU [`ContentCache`].
//! - [`scan`] — Directory scanning that produces a tree of [`ScanEntry`]
//!   nodes.
//! - [`error`] — Structured [`FsError`] with operation context.
//! - [`meta`] — [`Metadata`] for files and directories.
//! - [`file`] — [`FsFile`] trait for opened file handles.

pub mod error;
pub mod file;
pub mod fs;
pub mod hash;
pub mod local;
pub mod meta;
pub mod scan;

// Re-exports for convenience.
pub use error::{FsError, FsOperation, Result};
pub use file::FsFile;
pub use fs::{DirEntryMeta, FileSystem, OpenMode};
pub use hash::{
    ContentCache, ContentCacheConfig, ContentId, hash_bytes, hash_file,
};
pub use local::LocalFs;
pub use meta::Metadata;
pub use scan::{ScanConfig, ScanEntry, ScanResult, scan_directory};
