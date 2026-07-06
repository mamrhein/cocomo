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
//! - [`compare`] — Directory comparison engine that merges two scanned trees.
//! - [`filter`] — Name, display, and content filters for comparison results.
//! - [`text`] — Line-based text comparison engine.
//! - [`grammar`] — Syntax-aware grammar rules for smart diffing.
//! - [`format`] — File format registry and format-specific settings.
//! - [`error`] — Structured [`FsError`] with operation context.
//! - [`meta`] — [`Metadata`] for files and directories.
//! - [`file`] — [`FsFile`] trait for opened file handles.

#![forbid(unsafe_code)]

pub mod compare;
pub mod error;
pub mod file;
pub mod filter;
pub mod format;
pub mod fs;
pub mod grammar;
pub mod hash;
pub mod local;
pub mod meta;
pub mod scan;
pub mod text;

// Re-exports for convenience.
pub use compare::{
    CompareConfig, DirComparison, DirEntry, DirEntryStatus, EntryInfo,
    compare_directories,
};
pub use error::{FsError, FsOperation, Result};
pub use file::FsFile;
pub use format::{
    FileFormat, FormatRegistry, FormatSettings, FormatType, LineEnding,
    TableParser, TextEncoding,
};
pub use fs::{DirEntryMeta, FileSystem, OpenMode};
pub use grammar::{Grammar, GrammarRule, Importance};
pub use hash::{
    ContentCache, ContentCacheConfig, ContentId, hash_bytes, hash_file,
};
pub use local::LocalFs;
pub use meta::Metadata;
pub use scan::{ScanConfig, ScanEntry, ScanResult, scan_directory};
pub use text::{
    AlignmentMode, LineInfo, TextCompareSettings, TextDiff, TextDifference,
    Token, WhitespaceMode, compare_texts, lines_equal,
};
