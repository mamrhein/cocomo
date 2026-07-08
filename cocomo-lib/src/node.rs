// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Filesystem node model and classification.
//!
//! Every entry in a filesystem is represented as a [`Node`], which carries
//! metadata, a kind classification, and optional structural links (parent,
//! children). Content is never stored in the node itself — file data is
//! always read on demand through the [`FileSystem`] trait.
//!
//! # Lazy loading
//!
//! Directory children are resolved lazily:
//! - `children: None` means the directory entries have not been enumerated
//!   yet.
//!
//! # Node lifecycle
//!
//! A node can be tombstoned by setting `deleted = true`. Tombstoned nodes are
//! still addressable by their [`NodeId`] but return `NotFound` on access. This
//! allows the provider to detect stale identifiers without relying on weak
//! reference semantics, which do not play well with async trait bounds.

use std::{
    ffi::OsString,
    fmt,
    path::{Path, PathBuf},
};

use bitflags::bitflags;

use crate::{identity::NodeId, meta::Metadata};

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

bitflags! {
    /// Access rights evaluated for the calling user/context.
    ///
    /// Unified across providers: local filesystems map from unix/windows
    /// permissions, remote providers derive these from their own authorization
    /// model.
    #[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
    pub struct UserPermissions: u8 {
        const READ  = 0x04;
        const WRITE = 0x02;
        const EXEC  = 0x01;
    }
}

// ---------------------------------------------------------------------------
// Symlink target
// ---------------------------------------------------------------------------

/// Target of a symbolic link.
///
/// Holds the raw path (always present) and an optional resolved [`NodeId`].
/// The node ID is `None` until the target is resolved via filesystem lookup.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SymlinkTarget {
    /// The raw target path stored on disk.
    path: PathBuf,
    /// Resolved node ID. `None` until resolved.
    #[allow(dead_code)]
    node: Option<NodeId<u64>>,
}

impl SymlinkTarget {
    /// Create a new symlink target with an unresolved path.
    pub fn new(path: PathBuf) -> Self {
        Self { path, node: None }
    }

    /// Create a new symlink target that has been resolved to a node.
    pub fn resolved(path: PathBuf, node: NodeId<u64>) -> Self {
        Self {
            path,
            node: Some(node),
        }
    }

    /// Return the raw target path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return the resolved node ID, if available.
    pub fn node(&self) -> Option<NodeId<u64>> {
        self.node
    }
}

// ---------------------------------------------------------------------------
// Node kind
// ---------------------------------------------------------------------------

/// Classification of a filesystem node.
///
/// Directory children names are populated lazily. File content is never stored
/// in the node — it is always read via the [`FileSystem`] trait.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NodeKind {
    /// A directory. Child names are resolved on demand.
    Directory {
        /// Child entry names. `None` if not yet resolved.
        #[allow(dead_code)]
        children: Option<Vec<String>>,
    },
    /// A regular file.
    File,
    /// A symbolic link. The target may or may not be resolved.
    Symlink {
        /// Target of the symlink.
        target: SymlinkTarget,
    },
    /// A special node (character device, block device, pipe, socket, etc.).
    Special,
}

impl NodeKind {
    /// Return `true` when this node is a directory.
    pub fn is_directory(&self) -> bool {
        matches!(self, Self::Directory { .. })
    }

    /// Return `true` when this node is a regular file.
    pub fn is_file(&self) -> bool {
        matches!(self, Self::File)
    }

    /// Return `true` when this node is a symbolic link.
    pub fn is_symlink(&self) -> bool {
        matches!(self, Self::Symlink { .. })
    }

    /// Return the child names if this is a resolved directory.
    #[allow(dead_code)]
    pub fn children(&self) -> Option<&[String]> {
        match self {
            NodeKind::Directory { children: Some(c) } => Some(c.as_slice()),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Node
// ---------------------------------------------------------------------------

/// A node in the filesystem graph.
///
/// Every node carries metadata, a kind classification, a full path (for I/O
/// operations), and a parent reference (for navigation). The `deleted` flag
/// implements a tombstone: tombstoned nodes are still addressable but return
/// `NotFound` on access.
///
/// Cached content hashes are stored directly on the node rather than in a
/// separate global cache. This avoids type-erased cache keys and makes
/// invalidation natural — when a node is tombstoned, the cached hash dies
/// with it.
///
/// # Path storage
///
/// The `path` field stores the absolute filesystem path. This is needed for
/// the provider to perform actual I/O (open, read, write) through the OS.
/// The node graph provides an indexed view; the path provides the anchor to
/// the real filesystem.
///
/// # Parent navigation
///
/// The `parent` field is a strong index (not an [`std::sync::Weak`]). The
/// ownership cycle is broken at the cache level — the provider owns all
/// [`std::sync::Arc`] references, while `parent` is just an index into that
/// cache. Path reconstruction walks parent indices through the cache,
/// collecting names.
#[derive(Clone, Debug)]
pub struct Node {
    /// Node name (empty for a root directory).
    name: OsString,
    /// Absolute path of this node on the underlying filesystem.
    path: PathBuf,
    /// Node metadata.
    metadata: Metadata,
    /// Node kind classification.
    kind: NodeKind,
    /// Parent directory. `None` for root nodes.
    parent: Option<u64>,
    /// Tombstone flag. Set by delete operations to mark this node as removed.
    pub(crate) deleted: bool,
    /// Cached content hash (hex-encoded blake3). Only meaningful for files;
    /// `None` means the file has not been hashed yet.
    cached_hash: Option<String>,
}

impl Node {
    /// Create a new directory node.
    pub fn directory(
        name: OsString,
        path: PathBuf,
        metadata: Metadata,
    ) -> Self {
        Self {
            name,
            path,
            metadata,
            kind: NodeKind::Directory { children: None },
            parent: None,
            deleted: false,
            cached_hash: None,
        }
    }

    /// Create a new file node.
    pub fn file(name: OsString, path: PathBuf, metadata: Metadata) -> Self {
        Self {
            name,
            path,
            metadata,
            kind: NodeKind::File,
            parent: None,
            deleted: false,
            cached_hash: None,
        }
    }

    /// Create a new symlink node.
    pub fn symlink(
        name: OsString,
        path: PathBuf,
        metadata: Metadata,
        target: SymlinkTarget,
    ) -> Self {
        Self {
            name,
            path,
            metadata,
            kind: NodeKind::Symlink { target },
            parent: None,
            deleted: false,
            cached_hash: None,
        }
    }

    /// Create a new special node.
    pub fn special(name: OsString, path: PathBuf, metadata: Metadata) -> Self {
        Self {
            name,
            path,
            metadata,
            kind: NodeKind::Special,
            parent: None,
            deleted: false,
            cached_hash: None,
        }
    }

    /// Return the node name.
    pub fn name(&self) -> &OsString {
        &self.name
    }

    /// Return the absolute path of this node.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Return a reference to the node metadata.
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    /// Return the node kind.
    pub fn kind(&self) -> &NodeKind {
        &self.kind
    }

    /// Return `true` if the node is tombstoned.
    pub fn is_deleted(&self) -> bool {
        self.deleted
    }

    /// Mark this node as deleted (tombstone).
    pub(crate) fn set_deleted(&mut self) {
        self.deleted = true;
    }

    /// Return a copy of this node with the parent field set.
    pub(crate) fn with_parent(mut self, parent: Option<u64>) -> Self {
        self.parent = parent;
        self
    }

    /// Update the node name.
    pub(crate) fn set_name(&mut self, name: OsString) {
        self.name = name;
    }

    /// Update the absolute path.
    pub(crate) fn set_path(&mut self, path: PathBuf) {
        self.path = path;
    }

    /// Set the child names for a directory node. Does nothing if this is not
    /// a directory.
    pub(crate) fn set_children(&mut self, children: Vec<String>) {
        if let NodeKind::Directory { children: c } = &mut self.kind {
            *c = Some(children);
        }
    }

    /// Return the parent node ID, if any.
    #[allow(dead_code)]
    pub fn parent(&self) -> Option<u64> {
        self.parent
    }

    /// Return the cached content hash, if this node has been hashed.
    ///
    /// Only meaningful for `NodeKind::File`. Returns `None` for directories
    /// and if the file has not been hashed yet.
    pub fn cached_hash(&self) -> Option<&str> {
        self.cached_hash.as_deref()
    }

    /// Set the cached content hash.
    ///
    /// Used after computing a file hash to avoid re-hashing on subsequent
    /// accesses. Only meaningful for files.
    pub(crate) fn set_cached_hash(&mut self, hash: String) {
        self.cached_hash = Some(hash);
    }
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind_str = match &self.kind {
            NodeKind::Directory { .. } => "dir",
            NodeKind::File => "file",
            NodeKind::Symlink { .. } => "symlink",
            NodeKind::Special => "special",
        };
        write!(
            f,
            "{} ({}, {} bytes)",
            self.name.to_string_lossy(),
            kind_str,
            self.metadata.size
        )
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    fn dummy_meta() -> Metadata {
        Metadata::file(100, Utc::now())
    }

    #[test]
    fn directory_node_kind() {
        let node = Node::directory(
            OsString::from("src"),
            PathBuf::from("/home/src"),
            Metadata::dir(Utc::now()),
        );
        assert!(node.kind().is_directory());
        assert!(!node.kind().is_file());
        assert!(!node.kind().is_symlink());
    }

    #[test]
    fn file_node_kind() {
        let node = Node::file(
            OsString::from("main.rs"),
            PathBuf::from("/home/main.rs"),
            dummy_meta(),
        );
        assert!(node.kind().is_file());
        assert!(!node.kind().is_directory());
    }

    #[test]
    fn symlink_node_kind() {
        let target = SymlinkTarget::new(PathBuf::from("/tmp/link"));
        let node = Node::symlink(
            OsString::from("link"),
            PathBuf::from("/home/link"),
            dummy_meta(),
            target,
        );
        assert!(node.kind().is_symlink());
        assert!(!node.kind().is_file());
    }

    #[test]
    fn symlink_target_unresolved() {
        let target = SymlinkTarget::new(PathBuf::from("../other"));
        assert_eq!(target.path(), Path::new("../other"));
        assert!(target.node().is_none());
    }

    #[test]
    fn symlink_target_resolved() {
        let target = SymlinkTarget::resolved(
            PathBuf::from("../other"),
            NodeId::new(42),
        );
        assert_eq!(target.node(), Some(NodeId::new(42)));
    }

    #[test]
    fn node_not_deleted_by_default() {
        let node = Node::file(
            OsString::from("test.txt"),
            PathBuf::from("/test.txt"),
            dummy_meta(),
        );
        assert!(!node.is_deleted());
    }

    #[test]
    fn permissions_bitflags() {
        let mut perms = UserPermissions::READ;
        perms |= UserPermissions::WRITE;
        assert!(perms.contains(UserPermissions::READ));
        assert!(perms.contains(UserPermissions::WRITE));
        assert!(!perms.contains(UserPermissions::EXEC));
    }

    #[test]
    fn node_display() {
        let node = Node::file(
            OsString::from("hello.txt"),
            PathBuf::from("/hello.txt"),
            Metadata::file(50, Utc::now()),
        );
        let display = format!("{node}");
        assert!(display.contains("hello.txt"));
        assert!(display.contains("file"));
        assert!(display.contains("50"));
    }

    #[test]
    fn node_path_is_stored() {
        let p = PathBuf::from("/some/deep/path/file.txt");
        let node =
            Node::file(OsString::from("file.txt"), p.clone(), dummy_meta());
        assert_eq!(node.path(), p.as_path());
    }

    #[test]
    fn directory_children_lazy() {
        let node = Node::directory(
            OsString::from("dir"),
            PathBuf::from("/dir"),
            Metadata::dir(Utc::now()),
        );
        assert!(node.kind().children().is_none());
    }

    #[test]
    fn cached_hash_none_by_default() {
        let node = Node::file(
            OsString::from("test.txt"),
            PathBuf::from("/test.txt"),
            dummy_meta(),
        );
        assert!(node.cached_hash().is_none());
    }
}
