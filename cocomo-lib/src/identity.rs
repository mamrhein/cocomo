// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Opaque identifiers for filesystems and their nodes.
//!
//! Every filesystem backend assigns its own [`NodeId`] values internally. The
//! IDs are opaque to callers — they are not paths, not inodes, not memory
//! addresses. Callers receive them from the [`FileSystem`] trait and pass them
//! back for subsequent operations. This eliminates TOCTOU races and path
//! construction bugs.
//!
//! Type-safe wrappers [`DirId`] and [`FileId`] encode the node kind at
//! compile time, preventing accidental misuse (e.g., calling `create_dir`
//! with a file identifier). Conversion from the raw [`NodeId`] is unchecked —
//! callers must verify the node kind before narrowing the type.

use std::fmt::Debug;

// ---------------------------------------------------------------------------
// FileSystem identifier
// ---------------------------------------------------------------------------

/// Opaque identifier for a filesystem instance.
///
/// Each backend chooses a concrete inner type. For local filesystems this is
/// typically a device ID; for remote providers it could be an endpoint URI
/// hash or any value that uniquely identifies the provider instance.
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct FileSystemId<F> {
    inner: F,
}

impl<F> FileSystemId<F> {
    /// Create a new filesystem identifier from a raw value.
    pub fn new(inner: F) -> Self {
        Self { inner }
    }

    /// Extract the raw identifier value.
    pub fn get(&self) -> &F {
        &self.inner
    }
}

impl<F: Debug> Debug for FileSystemId<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileSystemId")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<F: Default> Default for FileSystemId<F> {
    fn default() -> Self {
        Self::new(F::default())
    }
}

// ---------------------------------------------------------------------------
// Node identifier
// ---------------------------------------------------------------------------

/// Opaque identifier for a node within a filesystem.
///
/// Backends assign node IDs at resolution time (`resolve_path`). The ID is
/// valid as long as the node exists in the provider's cache. Accessing a
/// deleted or evicted node returns an error.
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct NodeId<N> {
    inner: N,
}

impl<N> NodeId<N> {
    /// Create a new node identifier from a raw value.
    pub fn new(inner: N) -> Self {
        Self { inner }
    }

    /// Extract the raw identifier value.
    pub fn get(&self) -> &N {
        &self.inner
    }
}

impl<N: Debug> Debug for NodeId<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeId")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<N: Default> Default for NodeId<N> {
    fn default() -> Self {
        Self::new(N::default())
    }
}

// ---------------------------------------------------------------------------
// Directory identifier
// ---------------------------------------------------------------------------

/// Type-safe wrapper around [`NodeId`] that encodes a directory node.
///
/// Conversion from [`NodeId`] is unchecked — callers must verify the node
/// kind before constructing a [`DirId`].
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct DirId<N>(NodeId<N>);

impl<N> DirId<N> {
    /// Create a new directory identifier.
    ///
    /// # Panics
    ///
    /// Callers must ensure the underlying node is actually a directory. This
    /// constructor does not validate the node kind.
    pub fn new(inner: N) -> Self {
        Self(NodeId::new(inner))
    }

    /// Downcast to the raw [`NodeId`].
    pub fn as_node_id(self) -> NodeId<N> {
        self.0
    }
}

impl<N: Debug> Debug for DirId<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("DirId").field(&self.0).finish()
    }
}

impl<N: Default> Default for DirId<N> {
    fn default() -> Self {
        Self::new(N::default())
    }
}

impl<N> From<DirId<N>> for NodeId<N> {
    fn from(id: DirId<N>) -> Self {
        id.as_node_id()
    }
}

// ---------------------------------------------------------------------------
// File identifier
// ---------------------------------------------------------------------------

/// Type-safe wrapper around [`NodeId`] that encodes a file node.
///
/// Conversion from [`NodeId`] is unchecked — callers must verify the node
/// kind before constructing a [`FileId`].
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct FileId<N>(NodeId<N>);

impl<N> FileId<N> {
    /// Create a new file identifier.
    ///
    /// # Panics
    ///
    /// Callers must ensure the underlying node is actually a file. This
    /// constructor does not validate the node kind.
    pub fn new(inner: N) -> Self {
        Self(NodeId::new(inner))
    }

    /// Downcast to the raw [`NodeId`].
    pub fn as_node_id(self) -> NodeId<N> {
        self.0
    }
}

impl<N: Debug> Debug for FileId<N> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("FileId").field(&self.0).finish()
    }
}

impl<N: Default> Default for FileId<N> {
    fn default() -> Self {
        Self::new(N::default())
    }
}

impl<N> From<FileId<N>> for NodeId<N> {
    fn from(id: FileId<N>) -> Self {
        id.as_node_id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_id_new_and_get() {
        let id = FileSystemId::new(42u64);
        assert_eq!(*id.get(), 42);
    }

    #[test]
    fn node_id_new_and_get() {
        let id = NodeId::new(123u64);
        assert_eq!(*id.get(), 123);
    }

    #[test]
    fn dir_id_downcasts_to_node_id() {
        let dir = DirId::<u64>::new(1);
        let node: NodeId<u64> = dir.as_node_id();
        assert_eq!(*node.get(), 1);
    }

    #[test]
    fn file_id_downcasts_to_node_id() {
        let file = FileId::<u64>::new(2);
        let node: NodeId<u64> = file.as_node_id();
        assert_eq!(*node.get(), 2);
    }

    #[test]
    fn dir_id_from_trait() {
        let dir = DirId::<u64>::new(5);
        let node: NodeId<u64> = dir.into();
        assert_eq!(*node.get(), 5);
    }

    #[test]
    fn file_id_from_trait() {
        let file = FileId::<u64>::new(6);
        let node: NodeId<u64> = file.into();
        assert_eq!(*node.get(), 6);
    }

    #[test]
    fn ids_are_hashable() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(NodeId::new(1u64));
        set.insert(NodeId::new(2u64));
        assert!(!set.contains(&NodeId::new(3u64)));
    }

    #[test]
    fn debug_output_contains_inner() {
        let id = FileSystemId::new(99u64);
        let debug = format!("{id:?}");
        assert!(debug.contains("99"));
    }
}
