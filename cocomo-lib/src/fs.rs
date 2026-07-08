// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Core filesystem traits that every backend implements.
//!
//! # Path-based API (current)
//!
//! The [`FileSystem`] trait uses path-based operations. This is the current
//! API used by all downstream modules (scanning, comparison, etc.).
//!
//! # Node-based API (new)
//!
//! The [`NodeFileSystem`] trait operates on opaque [`NodeId`] identifiers.
//! Paths are resolved exactly once at entry time via `resolve_path()`, and all
//! subsequent I/O uses node identifiers to avoid TOCTOU races.
//!
//! Both traits are implemented by [`LocalFs`](crate::local::LocalFs). The
//! node-based trait is the target for future migration; the path-based trait
//! remains for backward compatibility.
//!
//! # Read / write split
//!
//! [`NodeFileSystem`] provides read-only operations. [`WritableFileSystem`]
//! extends it with mutations. Read-only providers (archives, Git commits,
//! S3 in read-only mode) implement only the base trait.

use std::{
    ffi::OsStr,
    fmt::Debug,
    hash::Hash,
    ops::Range,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;

use crate::{
    error::{FsError, Result},
    file::FsFile,
    identity::{DirId, FileId, FileSystemId, NodeId},
    meta::Metadata,
    node::Node,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Path-based FileSystem trait (current API)
// ---------------------------------------------------------------------------

/// The unified filesystem abstraction. Every backend (local, FTP, S3, etc.)
/// implements this trait so that higher-level logic is provider-agnostic.
///
/// This trait uses path-based operations and is the current stable API.
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
        range: Option<Range<u64>>,
    ) -> Result<Bytes>;

    /// Read file content as a stream. Suitable for large files and
    /// streaming comparison or hashing.
    async fn read_stream(
        &self,
        path: &Path,
        range: Option<Range<u64>>,
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

// ---------------------------------------------------------------------------
// Node-based FileSystem trait (new API)
// ---------------------------------------------------------------------------

/// A node-oriented filesystem abstraction.
///
/// All operations use opaque [`NodeId`] identifiers. Paths are resolved at
/// entry time via [`NodeFileSystem::resolve_path`], and all subsequent I/O
/// uses node identifiers to avoid TOCTOU races.
///
/// This trait is the target for future migration of downstream modules.
#[async_trait]
pub trait NodeFileSystem: Send + Sync {
    /// Concrete type used to identify this filesystem instance.
    type FsId: Copy + Eq + Hash + Debug + Default;

    /// Concrete type used to identify nodes within this filesystem.
    type Nid: Copy + Eq + Hash + Debug + Default;

    /// Provider-specific error type, convertible into [`FsError`].
    type Error: Into<FsError>;

    /// Return the filesystem instance identifier.
    fn id(&self) -> FileSystemId<Self::FsId>;

    /// Return a human-readable label for this provider instance.
    fn label_node(&self) -> &str;

    // ── Resolution (path -> node, the entry point) ──

    /// Resolve an absolute path to a node identifier, creating a cached
    /// [`Node`] entry if it does not exist yet.
    async fn resolve_path(&self, path: &Path) -> Result<NodeId<Self::Nid>>;

    /// Resolve the symlink target to a node identifier. Returns the node
    /// ID of the target (following the link), not the symlink itself.
    async fn resolve_symlink(
        &self,
        id: NodeId<Self::Nid>,
    ) -> Result<NodeId<Self::Nid>>;

    // ── Node access ──

    /// Return the cached node for the given identifier.
    ///
    /// Returns [`FsError::NotFound`] if the node does not exist in the cache,
    /// and [`FsError::StaleNode`] if the node has been tombstoned.
    fn get_node(&self, id: NodeId<Self::Nid>) -> Result<Arc<Node>>;

    /// Return the cached metadata for a node without resolving a path.
    fn node_metadata(&self, id: NodeId<Self::Nid>) -> Result<Metadata>;

    // ── Directory traversal ──

    /// Populate the children of a directory node. If the children have
    /// already been resolved, this is a no-op.
    async fn read_dir_node(&self, id: DirId<Self::Nid>) -> Result<()>;

    // ── File I/O ──

    /// Open a file for I/O. The returned handle owns the open descriptor.
    async fn open_node(
        &self,
        id: FileId<Self::Nid>,
        mode: OpenMode,
    ) -> Result<Box<dyn FsFile>>;

    /// Read file content in a single call. `range` is optional for sparse
    /// reads. For large files prefer [`NodeFileSystem::read_stream_node`]
    /// instead.
    async fn read_node(
        &self,
        id: FileId<Self::Nid>,
        range: Option<Range<u64>>,
    ) -> Result<Bytes>;

    /// Read file content as a stream. Suitable for large files and
    /// streaming comparison or hashing.
    async fn read_stream_node(
        &self,
        id: FileId<Self::Nid>,
        range: Option<Range<u64>>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>>;
}

// ---------------------------------------------------------------------------
// WritableFileSystem trait (node-based, write operations)
// ---------------------------------------------------------------------------

/// Extension trait for filesystems that support write operations.
///
/// Providers that are read-only (archives, Git commits) implement only
/// [`NodeFileSystem`].
#[async_trait]
pub trait WritableFileSystem: NodeFileSystem {
    // ── Node creation ──

    /// Create a new empty file in the given parent directory.
    async fn create_file(
        &self,
        parent: DirId<Self::Nid>,
        name: &OsStr,
    ) -> Result<FileId<Self::Nid>>;

    /// Create a new directory in the given parent directory.
    async fn create_dir_node(
        &self,
        parent: DirId<Self::Nid>,
        name: &OsStr,
    ) -> Result<DirId<Self::Nid>>;

    /// Create a new symbolic link in the given parent directory.
    async fn create_symlink(
        &self,
        parent: DirId<Self::Nid>,
        name: &OsStr,
        target: &Path,
    ) -> Result<NodeId<Self::Nid>>;

    // ── Write I/O ──

    /// Write data to a file.
    async fn write_node(
        &self,
        id: FileId<Self::Nid>,
        data: Bytes,
    ) -> Result<()>;

    /// Flush any buffered writes for the given file.
    async fn flush_node(&self, id: FileId<Self::Nid>) -> Result<()>;

    // ── High-level operations ──

    /// Delete a file or empty directory.
    async fn remove_node(&self, id: NodeId<Self::Nid>) -> Result<()>;

    /// Delete a file or directory recursively.
    async fn remove_all_node(&self, id: NodeId<Self::Nid>) -> Result<()>;

    /// Rename a node within its parent directory.
    async fn rename_node(
        &self,
        id: NodeId<Self::Nid>,
        new_name: &OsStr,
    ) -> Result<()>;

    /// Copy a node into a different directory within the same filesystem.
    async fn copy_node(
        &self,
        src: NodeId<Self::Nid>,
        dst: DirId<Self::Nid>,
    ) -> Result<NodeId<Self::Nid>>;

    /// Move a node into a different directory within the same filesystem.
    async fn move_node(
        &self,
        src: NodeId<Self::Nid>,
        dst: DirId<Self::Nid>,
    ) -> Result<NodeId<Self::Nid>>;
}
