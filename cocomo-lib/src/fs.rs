// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Core `FileSystem` trait that every filesystem backend implements.
//!
//! The design follows two principles derived from robust filesystem practices:
//! paths are resolved exactly once at `open()` time, and all subsequent I/O
//! operates on owned file handles rather than paths. This avoids TOCTOU races.

use std::{
    ops::Range,
    path::{Path, PathBuf},
    pin::Pin,
};

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;

use crate::{error::Result, file::FsFile, meta::Metadata};

/// Mode for opening a file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OpenMode {
    /// Open for reading.
    Read,
    /// Open for writing. Creates the file or truncates if it exists.
    Write,
    /// Open for appending. Creates the file if it does not exist.
    Append,
}

/// A single directory entry returned by `read_dir`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirEntryMeta {
    /// File or directory name (not the full path).
    pub name: String,
    /// Metadata for this entry.
    pub meta: Metadata,
}

/// A stream of directory entries. Used by `read_dir` to handle large
/// directories without loading all entries into memory at once.
pub type DirStream<'a> =
    Pin<Box<dyn Stream<Item = Result<DirEntryMeta>> + Send + 'a>>;

/// The unified filesystem abstraction. Every backend (local, FTP, S3, etc.)
/// implements this trait so that higher-level logic is provider-agnostic.
#[async_trait]
pub trait FileSystem: Send + Sync {
    /// Resolve a path to metadata. Does not open the file.
    async fn metadata(&self, path: &Path) -> Result<Metadata>;

    /// List directory entries. Returns a stream for large directories.
    async fn read_dir(&self, path: &Path) -> Result<DirStream<'_>>;

    /// Open a file for I/O. The returned handle owns the open descriptor;
    /// the caller should discard `path` after this call.
    async fn open(
        &self,
        path: &Path,
        mode: OpenMode,
    ) -> Result<Box<dyn FsFile>>;

    /// Read file content in a single call. `range` is optional for sparse
    /// reads. For large files prefer `read_stream()` instead.
    async fn read(
        &self,
        path: &Path,
        range: Option<Range<usize>>,
    ) -> Result<Bytes>;

    /// Read file content as a stream. Suitable for large files and
    /// streaming comparison or hashing.
    async fn read_stream(
        &self,
        path: &Path,
        range: Option<Range<usize>>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>>;

    /// Write file content.
    async fn write(&self, path: &Path, data: Bytes) -> Result<()>;

    /// Create directory (parents as needed).
    async fn create_dir(&self, path: &Path) -> Result<()>;

    /// Delete a file or empty directory.
    async fn remove(&self, path: &Path) -> Result<()>;

    /// Delete recursively.
    async fn remove_all(&self, path: &Path) -> Result<()>;

    /// Rename / move within the same provider.
    async fn rename(&self, src: &Path, dst: &Path) -> Result<()>;

    /// Copy within the same provider.
    async fn copy(&self, src: &Path, dst: &Path) -> Result<()>;

    /// Symlink metadata (target path, if applicable).
    async fn read_link(&self, path: &Path) -> Result<PathBuf>;

    /// Create a symlink.
    async fn symlink(&self, target: &Path, link: &Path) -> Result<()>;

    /// Human-readable label for this provider instance.
    fn label(&self) -> &str;
}
