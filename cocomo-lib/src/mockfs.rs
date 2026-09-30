// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! In-memory `FileSystem`, `NodeFileSystem`, and `WritableFileSystem` double
//! for tests.
//!
//! [`MockFs`] is backed by a `BTreeMap` of tree entries (directory, file, or
//! symlink) plus two error-injection maps. It lets provider-agnostic logic
//! (scanning, comparison, hashing) be tested against per-operation failures
//! such as `PermissionDenied`, which cannot be reproduced reliably on a real
//! filesystem.
//!
//! # Writability
//!
//! Mutating operations mutate the in-memory tree so that later tests can
//! exercise code that expects a mutable backend. There is deliberately no
//! switch to make the double read-only: an operation that must fail is
//! simulated with [`MockFs::with_error`], and an injected error always
//! shadows real behavior (it is checked before any mutation happens).
//!
//! # Tree and node cache
//!
//! The tree is authoritative and keyed by absolute path; entries may be
//! registered without their ancestor directories, unlike on a real
//! filesystem. File content lives in a per-file shared buffer, so reads and
//! handles observe content replaced through `write`, `write_node`, or a
//! handle opened for writing. Mutating operations keep the node cache
//! consistent with the tree: cache entries are tombstoned on removals
//! (yielding `FsError::StaleNode` on later access), re-keyed on renames and
//! moves, and rebuilt when file content was replaced.

use std::{
    collections::{BTreeMap, HashMap},
    ffi::{OsStr, OsString},
    io,
    ops::Range,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
};

use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use futures::Stream;
use parking_lot::{Mutex, RwLock};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::{
    error::{FsError, FsOperation, Result},
    file::FsFile,
    fs::{
        DirEntryMeta, DirStream, FileSystem, NodeFileSystem, OpenMode,
        WritableFileSystem,
    },
    identity::{DirId, FileId, FileSystemId, NodeId},
    meta::Metadata,
    node::{Node, NodeKind, SymlinkTarget},
};

/// Number of bytes `MockFs::read_stream` emits per chunk.
const CHUNK_SIZE: usize = 8192;

/// The shared buffer type backing every mock file. Handles opened for
/// writing append to the same buffer that `read` and `read_stream` serve
/// from, so content written through a handle is immediately visible on the
/// filesystem.
type SharedBuf = Arc<Mutex<Vec<u8>>>;

/// Wrap a byte slice in a fresh shared buffer.
fn shared_buf(data: impl AsRef<[u8]>) -> SharedBuf {
    Arc::new(Mutex::new(data.as_ref().to_vec()))
}

/// Rebase `path` from living under `from` to living under `to`.
fn rebase(path: &Path, from: &Path, to: &Path) -> PathBuf {
    match path.strip_prefix(from) {
        Ok(rel) if rel.as_os_str().is_empty() => to.to_path_buf(),
        Ok(rel) => to.join(rel),
        Err(_) => to.to_path_buf(),
    }
}

// ---------------------------------------------------------------------------
// MockTree
// ---------------------------------------------------------------------------

/// A node of the [`MockTree`] map.
#[derive(Clone, Debug)]
enum MockNode {
    /// A directory (without content).
    Dir,
    /// A regular file with the given content.
    File(SharedBuf),
    /// A symbolic link to the given target path.
    Symlink(PathBuf),
}

/// The predefined contents and injected errors behind a [`MockFs`].
#[derive(Clone, Debug)]
struct MockTree {
    /// Known absolute paths and their kind or content.
    nodes: BTreeMap<PathBuf, MockNode>,
    /// Errors returned instead of performing *any* operation on the key.
    errors: HashMap<PathBuf, FsError>,
    /// Errors emitted as the first item of `read_stream` for the key.
    stream_errors: HashMap<PathBuf, FsError>,
}

impl MockTree {
    fn new() -> Self {
        Self {
            nodes: BTreeMap::new(),
            errors: HashMap::new(),
            stream_errors: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// MockFs
// ---------------------------------------------------------------------------

/// A counter handing out unique filesystem identifiers to each `MockFs`
/// instance, so that two mocks are never treated as the same filesystem.
static NEXT_FS_ID: AtomicU64 = AtomicU64::new(1);

/// A writable, in-memory `FileSystem` for tests.
///
/// Build the tree with the `with_*` builder methods, then pass the instance
/// to anything that takes a `&Arc<dyn FileSystem>`:
///
/// ```text
/// let fs: Arc<dyn FileSystem> = Arc::new(
///     MockFs::new("mock")
///         .with_dir("/root")
///         .with_file("/root/a.txt", "alpha")
///         .with_error("/root/locked", FsError::PermissionDenied { .. }),
/// );
/// ```
#[derive(Debug)]
pub struct MockFs {
    label: String,
    /// Filesystem instance identifier (unique per process, not a device ID).
    fs_id: FileSystemId<u64>,
    /// The authoritative tree, behind a lock so that mutating operations can
    /// take `&self`.
    tree: RwLock<MockTree>,
    /// Node cache: node ID → node.
    nodes: RwLock<HashMap<u64, Arc<Node>>>,
    /// Reverse lookup: absolute path → node ID.
    path_to_id: RwLock<HashMap<PathBuf, u64>>,
    /// Monotonically increasing counter for node ID generation.
    next_id: AtomicU64,
}

impl MockFs {
    /// Create a new, empty mock filesystem with the given label.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            fs_id: FileSystemId::new(
                NEXT_FS_ID.fetch_add(1, Ordering::Relaxed),
            ),
            tree: RwLock::new(MockTree::new()),
            nodes: RwLock::new(HashMap::new()),
            path_to_id: RwLock::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    /// Add a directory node at `path`.
    #[must_use]
    pub fn with_dir(self, path: impl AsRef<Path>) -> Self {
        self.with_node(path, MockNode::Dir)
    }

    /// Add a file node with the given content at `path`.
    #[must_use]
    pub fn with_file(
        self,
        path: impl AsRef<Path>,
        content: impl AsRef<[u8]>,
    ) -> Self {
        self.with_node(path, MockNode::File(shared_buf(content)))
    }

    /// Make *every* operation on `path` return `error` instead of succeeding.
    #[must_use]
    pub fn with_error(
        mut self,
        path: impl AsRef<Path>,
        error: FsError,
    ) -> Self {
        self.tree
            .get_mut()
            .errors
            .insert(path.as_ref().to_path_buf(), error);
        self
    }

    /// Make `read_stream` on `path` yield `error` as its first stream item.
    #[must_use]
    pub fn with_stream_error(
        mut self,
        path: impl AsRef<Path>,
        error: FsError,
    ) -> Self {
        self.tree
            .get_mut()
            .stream_errors
            .insert(path.as_ref().to_path_buf(), error);
        self
    }

    fn with_node(mut self, path: impl AsRef<Path>, node: MockNode) -> Self {
        self.tree
            .get_mut()
            .nodes
            .insert(path.as_ref().to_path_buf(), node);
        self
    }

    /// The injected error for `path`, if one was registered.
    fn error_at(&self, path: &Path) -> Option<FsError> {
        self.tree.read().errors.get(path).cloned()
    }

    /// The tree entry at `path`, if one was registered.
    fn node_at(&self, path: &Path) -> Option<MockNode> {
        self.tree.read().nodes.get(path).cloned()
    }

    /// `true` when any strict descendant of `path` is registered.
    fn has_children(&self, path: &Path) -> bool {
        let tree = self.tree.read();
        tree.nodes
            .keys()
            .any(|p| p.starts_with(path) && p.as_path() != path)
    }

    /// `true` when `path` itself or any descendant of it is registered.
    fn is_occupied(&self, path: &Path) -> bool {
        let tree = self.tree.read();
        tree.nodes.keys().any(|p| p.starts_with(path))
    }

    /// Collect `path` and all its descendants with their entries, in path
    /// order.
    fn subtree(&self, path: &Path) -> Vec<(PathBuf, MockNode)> {
        let tree = self.tree.read();
        tree.nodes
            .iter()
            .filter(|(p, _)| p.as_path() == path || p.starts_with(path))
            .map(|(p, node)| (p.clone(), node.clone()))
            .collect()
    }

    /// The direct children of `dir` with their entries, in path order.
    fn children_of(&self, dir: &Path) -> Vec<(PathBuf, MockNode)> {
        let tree = self.tree.read();
        tree.nodes
            .iter()
            .filter(|(path, _)| path.parent() == Some(dir))
            .map(|(path, node)| (path.clone(), node.clone()))
            .collect()
    }

    /// The content of the file at `path`, if one was registered.
    fn content(&self, path: &Path) -> Option<Bytes> {
        match self.node_at(path) {
            Some(MockNode::File(buf)) => {
                Some(Bytes::copy_from_slice(buf.lock().as_slice()))
            }
            _ => None,
        }
    }

    /// Metadata of the direct children of `dir`, in path order.
    fn child_meta(&self, dir: &Path) -> Vec<DirEntryMeta> {
        self.children_of(dir)
            .into_iter()
            .map(|(path, node)| {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let meta = match node {
                    MockNode::Dir => Metadata::dir(Utc::now()),
                    MockNode::File(buf) => {
                        Metadata::file(buf.lock().len() as u64, Utc::now())
                    }
                    MockNode::Symlink(_) => {
                        let mut meta = Metadata::file(0, Utc::now());
                        meta.is_symlink = true;
                        meta
                    }
                };
                DirEntryMeta { name, meta }
            })
            .collect()
    }

    /// Allocate a new unique node ID.
    fn alloc_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Cache `node` under its path and return its ID. Replaces a cached node
    /// for the same path, so rebuilt nodes (fresh metadata, re-keyed paths)
    /// keep their identity.
    fn upsert_node(&self, node: Node) -> u64 {
        let path = node.path().to_path_buf();
        // `parking_lot` locks are not reentrant: acquiring the write lock
        // while the same thread still holds a read guard on the same lock
        // parks forever in `wait_for_readers`. The lookup therefore releases
        // its read guard before the write lock is taken.
        let known = {
            let ids = self.path_to_id.read();
            ids.get(&path).copied()
        };
        let id = match known {
            Some(id) => id,
            None => {
                let id = self.alloc_id();
                self.path_to_id.write().insert(path, id);
                id
            }
        };
        self.nodes.write().insert(id, Arc::new(node));
        id
    }

    /// Cache `node` under its path and return its ID, keeping the cached
    /// node for an already-known path (used by `resolve_path` and
    /// `read_dir_node`, which must not discard a node that is already
    /// addressable).
    fn cache_node(&self, node: Node) -> u64 {
        let path = node.path().to_path_buf();
        if let Some(id) = self.lookup_path(&path) {
            return id;
        }
        self.upsert_node(node)
    }

    /// Lookup a node ID from a path in the cache. Returns `None` if the path
    /// is not cached.
    fn lookup_path(&self, path: &Path) -> Option<u64> {
        self.path_to_id.read().get(path).copied()
    }

    /// Resolve the parent directory ID for `path`, if the parent is cached.
    fn resolve_parent_id(&self, path: &Path) -> Option<u64> {
        path.parent().and_then(|p| self.lookup_path(p))
    }

    /// Rebuild the cached file node for `path` with the given content size,
    /// so that metadata and reads served from the cache observe a content
    /// write.
    fn replace_file_node(&self, path: &Path, size: usize) {
        let Some(id) = self.lookup_path(path) else {
            return;
        };
        let Some(node) = self.nodes.read().get(&id).cloned() else {
            return;
        };
        let fresh = Node::file(
            node.name().clone(),
            path.to_path_buf(),
            Metadata::file(size as u64, Utc::now()),
        )
        .with_parent(node.parent());
        self.nodes.write().insert(id, Arc::new(fresh));
    }

    /// Tombstone the cached node for `path`, if one exists. Tombstoned nodes
    /// stay addressable and answer with `FsError::StaleNode`, which is how
    /// the real providers detect stale identifiers after a removal.
    fn tombstone_path(&self, path: &Path) {
        if let Some(id) = self.lookup_path(path)
            && let Some(node) = self.nodes.write().get_mut(&id)
        {
            Arc::make_mut(node).set_deleted();
        }
    }

    /// Re-key cached nodes that were moved to a new prefix: update the
    /// stored path (and name) of every cached node under `old_prefix` and
    /// clear the resolved child lists under `new_prefix`, so that a
    /// re-listing observes the new structure.
    fn fixup_cache_after_move(&self, old_prefix: &Path, new_prefix: &Path) {
        let mut nodes = self.nodes.write();
        let mut ids = self.path_to_id.write();
        // A directory that was moved (or that gained moved children) has a
        // stale child list; clearing it forces `read_dir_node` to rebuild.
        let stale_children: Vec<u64> = nodes
            .iter()
            .filter(|(_, node)| {
                node.kind().is_directory()
                    && (node.path().starts_with(old_prefix)
                        || node.path().starts_with(new_prefix))
                    && node
                        .kind()
                        .children()
                        .is_some_and(|names| !names.is_empty())
            })
            .map(|(id, _)| *id)
            .collect();
        for id in stale_children {
            if let Some(node) = nodes.get_mut(&id) {
                Arc::make_mut(node).set_children(Vec::new());
            }
        }
        let rekeyed: Vec<(PathBuf, u64)> = ids
            .iter()
            .filter(|(path, _)| path.starts_with(old_prefix))
            .map(|(path, &id)| (path.clone(), id))
            .collect();
        for (old_path, id) in rekeyed {
            let rel = match old_path.strip_prefix(old_prefix) {
                Ok(rel) => rel.to_path_buf(),
                Err(_) => continue,
            };
            let new_path = if rel.as_os_str().is_empty() {
                new_prefix.to_path_buf()
            } else {
                new_prefix.join(rel)
            };
            if let Some(node) = nodes.get_mut(&id) {
                let node = Arc::make_mut(node);
                node.set_name(
                    new_path
                        .file_name()
                        .map(OsStr::to_owned)
                        .unwrap_or_default(),
                );
                node.set_path(new_path.clone());
            }
            ids.insert(new_path, id);
        }
    }

    /// Remove the subtree rooted at `path` (the entry itself plus every
    /// descendant) from the tree and tombstone the cached nodes that
    /// addressed it.
    fn remove_subtree(&self, path: &Path) -> Result<()> {
        if self.node_at(path).is_none() {
            return Err(FsError::NotFound {
                path: path.to_path_buf(),
            });
        }
        let doomed = self.subtree(path);
        {
            let mut tree = self.tree.write();
            for doomed_path in doomed.iter().map(|(p, _)| p) {
                tree.nodes.remove(doomed_path);
            }
        }
        for doomed_path in doomed.iter().map(|(p, _)| p) {
            self.tombstone_path(doomed_path);
        }
        Ok(())
    }

    /// Move the subtree rooted at `src` to `dst`, re-keying the tree and
    /// keeping the node cache consistent (see
    /// [`MockFs::fixup_cache_after_move`]).
    fn move_subtree(&self, src: &Path, dst: &Path) -> Result<()> {
        if self.node_at(src).is_none() {
            return Err(FsError::NotFound {
                path: src.to_path_buf(),
            });
        }
        if self.is_occupied(dst) {
            return Err(FsError::InvalidArgument {
                operation: FsOperation::Move,
                path: dst.to_path_buf(),
                message: "a move onto an occupied destination is not \
                          supported on MockFs"
                    .to_string(),
            });
        }
        let subtree = self.subtree(src);
        {
            let mut tree = self.tree.write();
            for (old_path, node) in subtree {
                tree.nodes.insert(rebase(&old_path, src, dst), node);
            }
            tree.nodes.remove(src);
        }
        self.fixup_cache_after_move(src, dst);
        // A move in or out of a directory changes its child list, so any
        // cached listing of the source or destination parent becomes stale.
        for parent_id in
            [self.resolve_parent_id(src), self.resolve_parent_id(dst)]
        {
            if let Some(id) = parent_id
                && let Some(node) = self.nodes.write().get_mut(&id)
            {
                Arc::make_mut(node).set_children(Vec::new());
            }
        }
        Ok(())
    }

    /// Copy the subtree rooted at `src` (content only) to `dst` and cache a
    /// node for the copied root, mirroring what `resolve_path` would
    /// produce for the new path.
    fn copy_subtree(&self, src: &Path, dst: &Path) -> Result<NodeId<u64>> {
        if self.node_at(src).is_none() {
            return Err(FsError::NotFound {
                path: src.to_path_buf(),
            });
        }
        if self.is_occupied(dst) {
            return Err(FsError::InvalidArgument {
                operation: FsOperation::Copy,
                path: dst.to_path_buf(),
                message: "a copy onto an occupied destination is not \
                          supported on MockFs"
                    .to_string(),
            });
        }
        let subtree = self.subtree(src);
        {
            let mut tree = self.tree.write();
            for (old_path, node) in subtree {
                tree.nodes.insert(rebase(&old_path, src, dst), node);
            }
        }
        let id = match self.node_at(dst) {
            Some(MockNode::Dir) => {
                let name: OsString =
                    dst.file_name().map(OsStr::to_owned).unwrap_or_default();
                let node = Node::directory(
                    name,
                    dst.to_path_buf(),
                    Metadata::dir(Utc::now()),
                )
                .with_parent(self.resolve_parent_id(dst));
                self.upsert_node(node)
            }
            Some(MockNode::File(buf)) => {
                let name: OsString =
                    dst.file_name().map(OsStr::to_owned).unwrap_or_default();
                let size = buf.lock().len();
                let node = Node::file(
                    name,
                    dst.to_path_buf(),
                    Metadata::file(size as u64, Utc::now()),
                )
                .with_parent(self.resolve_parent_id(dst));
                self.upsert_node(node)
            }
            Some(MockNode::Symlink(target)) => {
                let name: OsString =
                    dst.file_name().map(OsStr::to_owned).unwrap_or_default();
                let node = Node::symlink(
                    name,
                    dst.to_path_buf(),
                    Metadata::file(0, Utc::now()),
                    SymlinkTarget::new(target),
                )
                .with_parent(self.resolve_parent_id(dst));
                self.upsert_node(node)
            }
            None => {
                return Err(FsError::NotFound {
                    path: dst.to_path_buf(),
                });
            }
        };
        Ok(NodeId::new(id))
    }

    /// Replace the content behind `path` (creating a file entry if the path
    /// is unknown) and rebuild the cached node so that metadata and reads
    /// observe the new content.
    fn set_content(&self, path: &Path, data: &[u8]) -> Result<()> {
        let size = data.len();
        match self.node_at(path) {
            Some(MockNode::File(buf)) => {
                *buf.lock() = data.to_vec();
            }
            Some(MockNode::Dir) => {
                return Err(FsError::WrongKind {
                    expected: "file",
                    actual: "directory",
                });
            }
            Some(MockNode::Symlink(_)) => {
                return Err(FsError::WrongKind {
                    expected: "file",
                    actual: "symlink",
                });
            }
            None => {
                self.tree.write().nodes.insert(
                    path.to_path_buf(),
                    MockNode::File(shared_buf(data)),
                );
            }
        }
        self.replace_file_node(path, size);
        Ok(())
    }
}

impl Clone for MockFs {
    fn clone(&self) -> Self {
        Self {
            label: self.label.clone(),
            fs_id: self.fs_id,
            tree: RwLock::new(self.tree.read().clone()),
            nodes: RwLock::new(self.nodes.read().clone()),
            path_to_id: RwLock::new(self.path_to_id.read().clone()),
            next_id: AtomicU64::new(self.next_id.load(Ordering::Relaxed)),
        }
    }
}

#[async_trait]
impl FileSystem for MockFs {
    async fn metadata(&self, path: &Path) -> Result<Metadata> {
        if let Some(err) = self.error_at(path) {
            return Err(err);
        }
        match self.node_at(path) {
            Some(MockNode::Dir) => Ok(Metadata::dir(Utc::now())),
            Some(MockNode::File(buf)) => {
                Ok(Metadata::file(buf.lock().len() as u64, Utc::now()))
            }
            Some(MockNode::Symlink(_)) => {
                let mut meta = Metadata::file(0, Utc::now());
                meta.is_symlink = true;
                Ok(meta)
            }
            None => Err(FsError::NotFound {
                path: path.to_path_buf(),
            }),
        }
    }

    async fn read_dir(&self, path: &Path) -> Result<DirStream<'_>> {
        if let Some(err) = self.error_at(path) {
            return Err(err);
        }
        match self.node_at(path) {
            Some(MockNode::Dir) => {
                let entries: Vec<_> =
                    self.child_meta(path).into_iter().map(Ok).collect();
                let stream: DirStream<'_> =
                    Box::pin(futures::stream::iter(entries));
                Ok(stream)
            }
            Some(MockNode::File(_)) => Err(FsError::WrongKind {
                expected: "directory",
                actual: "file",
            }),
            Some(MockNode::Symlink(_)) => Err(FsError::WrongKind {
                expected: "directory",
                actual: "symlink",
            }),
            None => Err(FsError::NotFound {
                path: path.to_path_buf(),
            }),
        }
    }

    async fn open(
        &self,
        path: &Path,
        mode: OpenMode,
    ) -> Result<Box<dyn FsFile>> {
        if let Some(err) = self.error_at(path) {
            return Err(err);
        }
        match mode {
            OpenMode::Read => match self.node_at(path) {
                Some(MockNode::File(buf)) => {
                    Ok(Box::new(MockFile::new(buf.lock().as_slice())))
                }
                _ => Err(FsError::NotFound {
                    path: path.to_path_buf(),
                }),
            },
            // Writing through a handle appends to the file's shared
            // buffer, so a later `read` on the same path observes what was
            // written.
            OpenMode::Write | OpenMode::Append => match self.node_at(path) {
                Some(MockNode::File(buf)) => {
                    Ok(Box::new(MockWritableFile::new(Arc::clone(&buf))))
                }
                _ => Err(FsError::NotFound {
                    path: path.to_path_buf(),
                }),
            },
        }
    }

    async fn read(
        &self,
        path: &Path,
        range: Option<Range<u64>>,
    ) -> Result<Bytes> {
        if let Some(err) = self.error_at(path) {
            return Err(err);
        }
        let Some(data) = self.content(path) else {
            return Err(FsError::NotFound {
                path: path.to_path_buf(),
            });
        };
        let Some(r) = range else {
            return Ok(data);
        };
        // Mirror `LocalFs::read`: bounds are clamped to the content length
        // and inverted or empty ranges are rejected before slicing.
        if r.start >= r.end {
            return Err(FsError::InvalidArgument {
                operation: FsOperation::Read,
                path: path.to_path_buf(),
                message: format!(
                    "invalid range: start ({}) must be less than end ({})",
                    r.start, r.end
                ),
            });
        }
        let len = data.len() as u64;
        if r.start >= len {
            return Ok(Bytes::new());
        }
        let end = usize::try_from(std::cmp::min(r.end, len)).unwrap_or(0);
        Ok(data.slice(r.start as usize..end))
    }

    async fn read_stream(
        &self,
        path: &Path,
        _range: Option<Range<u64>>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        if let Some(err) = self.error_at(path) {
            return Err(err);
        }
        if let Some(err) = self.tree.read().stream_errors.get(path) {
            let stream: Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>> =
                Box::pin(futures::stream::iter([Err(err.clone())]));
            return Ok(stream);
        }
        let Some(buf) = self.content(path) else {
            return Err(FsError::NotFound {
                path: path.to_path_buf(),
            });
        };
        let mut data = buf;
        // Chunk the content like a real provider so that streaming consumers
        // observe more than one item for larger files.
        let mut chunks = Vec::new();
        while !data.is_empty() {
            let n = usize::min(CHUNK_SIZE, data.len());
            chunks.push(Ok(data.split_to(n)));
        }
        let stream: Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>> =
            Box::pin(futures::stream::iter(chunks));
        Ok(stream)
    }

    async fn write(&self, path: &Path, data: Bytes) -> Result<()> {
        if let Some(err) = self.error_at(path) {
            return Err(err);
        }
        self.set_content(path, &data)
    }

    async fn create_dir(&self, path: &Path) -> Result<()> {
        if let Some(err) = self.error_at(path) {
            return Err(err);
        }
        {
            let mut tree = self.tree.write();
            if tree.nodes.contains_key(path) {
                return Err(FsError::InvalidArgument {
                    operation: FsOperation::CreateDir,
                    path: path.to_path_buf(),
                    message: "entry already exists".to_string(),
                });
            }
            tree.nodes.insert(path.to_path_buf(), MockNode::Dir);
        }
        let name: OsString =
            path.file_name().map(OsStr::to_owned).unwrap_or_default();
        let node = Node::directory(
            name,
            path.to_path_buf(),
            Metadata::dir(Utc::now()),
        )
        .with_parent(self.resolve_parent_id(path));
        self.upsert_node(node);
        Ok(())
    }

    async fn remove(&self, path: &Path) -> Result<()> {
        if let Some(err) = self.error_at(path) {
            return Err(err);
        }
        if self.node_at(path).is_none() {
            return Err(FsError::NotFound {
                path: path.to_path_buf(),
            });
        }
        if self.has_children(path) {
            return Err(FsError::InvalidArgument {
                operation: FsOperation::Remove,
                path: path.to_path_buf(),
                message: "directory is not empty".to_string(),
            });
        }
        self.tree.write().nodes.remove(path);
        self.tombstone_path(path);
        Ok(())
    }

    async fn remove_all(&self, path: &Path) -> Result<()> {
        if let Some(err) = self.error_at(path) {
            return Err(err);
        }
        self.remove_subtree(path)
    }

    async fn rename(&self, src: &Path, dst: &Path) -> Result<()> {
        if let Some(err) = self.error_at(src) {
            return Err(err);
        }
        if let Some(err) = self.error_at(dst) {
            return Err(err);
        }
        self.move_subtree(src, dst)
    }

    async fn copy(&self, src: &Path, dst: &Path) -> Result<()> {
        if let Some(err) = self.error_at(src) {
            return Err(err);
        }
        if let Some(err) = self.error_at(dst) {
            return Err(err);
        }
        self.copy_subtree(src, dst).map(|_| ())
    }

    async fn read_link(&self, path: &Path) -> Result<PathBuf> {
        if let Some(err) = self.error_at(path) {
            return Err(err);
        }
        match self.node_at(path) {
            Some(MockNode::Symlink(target)) => Ok(target),
            Some(_) => Err(FsError::InvalidArgument {
                operation: FsOperation::ReadLink,
                path: path.to_path_buf(),
                message: "entry is not a symlink".to_string(),
            }),
            None => Err(FsError::NotFound {
                path: path.to_path_buf(),
            }),
        }
    }

    async fn symlink(&self, target: &Path, link: &Path) -> Result<()> {
        if let Some(err) = self.error_at(link) {
            return Err(err);
        }
        self.tree.write().nodes.insert(
            link.to_path_buf(),
            MockNode::Symlink(target.to_path_buf()),
        );
        Ok(())
    }

    fn label(&self) -> &str {
        &self.label
    }
}

// ---------------------------------------------------------------------------
// NodeFileSystem implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl NodeFileSystem for MockFs {
    type FsId = u64;
    type Nid = u64;
    type Error = FsError;

    fn id(&self) -> FileSystemId<Self::FsId> {
        self.fs_id
    }

    fn label_node(&self) -> &str {
        &self.label
    }

    async fn resolve_path(&self, path: &Path) -> Result<NodeId<Self::Nid>> {
        // Injections replace *every* operation on the key, resolution
        // included, so callers cannot smuggle an injected path past the
        // entry point.
        if let Some(err) = self.error_at(path) {
            return Err(err);
        }
        // Mock paths are registered verbatim, so there is no base directory
        // to resolve relative paths against and no canonicalization to do.
        if let Some(id) = self.lookup_path(path) {
            let node = self.nodes.read().get(&id).cloned();
            return match node {
                Some(n) if n.is_deleted() => Err(FsError::StaleNode),
                Some(_) => Ok(NodeId::new(id)),
                None => Err(FsError::NotFound {
                    path: path.to_path_buf(),
                }),
            };
        }
        let name = path.file_name().map(OsStr::to_owned).unwrap_or_default();
        let parent_id = self.resolve_parent_id(path);
        let node = match self.node_at(path) {
            Some(MockNode::Dir) => Node::directory(
                name,
                path.to_path_buf(),
                Metadata::dir(Utc::now()),
            )
            .with_parent(parent_id),
            Some(MockNode::File(buf)) => Node::file(
                name,
                path.to_path_buf(),
                Metadata::file(buf.lock().len() as u64, Utc::now()),
            )
            .with_parent(parent_id),
            Some(MockNode::Symlink(target)) => Node::symlink(
                name,
                path.to_path_buf(),
                Metadata::file(0, Utc::now()),
                SymlinkTarget::new(target),
            )
            .with_parent(parent_id),
            None => {
                return Err(FsError::NotFound {
                    path: path.to_path_buf(),
                });
            }
        };
        let id = self.cache_node(node);
        Ok(NodeId::new(id))
    }

    async fn resolve_symlink(
        &self,
        id: NodeId<Self::Nid>,
    ) -> Result<NodeId<Self::Nid>> {
        let node = self.get_node(id)?;
        match node.kind() {
            NodeKind::Symlink { target } => {
                self.resolve_path(target.path()).await
            }
            _ => Err(FsError::InvalidArgument {
                operation: FsOperation::ReadLink,
                path: node.path().to_path_buf(),
                message: "node is not a symlink".to_string(),
            }),
        }
    }

    fn get_node(&self, id: NodeId<Self::Nid>) -> Result<Arc<Node>> {
        let nodes = self.nodes.read();
        match nodes.get(id.get()).cloned() {
            Some(node) if node.is_deleted() => Err(FsError::StaleNode),
            Some(node) => Ok(node),
            None => Err(FsError::NotFound {
                path: PathBuf::from("(unknown node)"),
            }),
        }
    }

    fn node_metadata(&self, id: NodeId<Self::Nid>) -> Result<Metadata> {
        let node = self.get_node(id)?;
        Ok(node.metadata().clone())
    }

    fn set_node_hash(
        &self,
        id: NodeId<Self::Nid>,
        hash: String,
    ) -> Result<()> {
        let mut nodes = self.nodes.write();
        let Some(arc) = nodes.get_mut(id.get()) else {
            return Err(FsError::NotFound {
                path: PathBuf::from("(unknown node)"),
            });
        };
        // Like the real providers, the mock caches hashes in memory only;
        // the cached hash dies with the node.
        Arc::make_mut(arc).set_cached_hash(hash);
        Ok(())
    }

    async fn read_dir_node(&self, id: DirId<Self::Nid>) -> Result<()> {
        let dir_node = self.get_node(id.as_node_id())?;
        if !dir_node.kind().is_directory() {
            return Err(FsError::WrongKind {
                expected: "directory",
                actual: "file",
            });
        }
        // If the children were resolved while the directory was located
        // elsewhere (or the child list was cleared by a move), rebuild it.
        if dir_node
            .kind()
            .children()
            .is_some_and(|names| !names.is_empty())
        {
            return Ok(());
        }
        let dir_path = dir_node.path().to_path_buf();
        let children = self.children_of(&dir_path);
        let mut child_names = Vec::with_capacity(children.len());
        for (child_path, node) in children {
            let name = child_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            child_names.push(name.clone());
            let name_os = OsString::from(&name);
            let child = match node {
                MockNode::Dir => Node::directory(
                    name_os,
                    child_path,
                    Metadata::dir(Utc::now()),
                ),
                MockNode::File(buf) => Node::file(
                    name_os,
                    child_path,
                    Metadata::file(buf.lock().len() as u64, Utc::now()),
                ),
                MockNode::Symlink(target) => Node::symlink(
                    name_os,
                    child_path,
                    Metadata::file(0, Utc::now()),
                    SymlinkTarget::new(target),
                ),
            };
            self.cache_node(child.with_parent(Some(*id.get())));
        }

        // Mark the directory node as resolved.
        let mut nodes = self.nodes.write();
        if let Some(arc) = nodes.get_mut(id.get()) {
            Arc::make_mut(arc).set_children(child_names);
        }
        Ok(())
    }

    async fn open_node(
        &self,
        id: FileId<Self::Nid>,
        mode: OpenMode,
    ) -> Result<Box<dyn FsFile>> {
        let node = self.get_node(id.as_node_id())?;
        // Delegate to the path-based API, which honours the error
        // injections.
        self.open(node.path(), mode).await
    }

    async fn read_node(
        &self,
        id: FileId<Self::Nid>,
        range: Option<Range<u64>>,
    ) -> Result<Bytes> {
        let node = self.get_node(id.as_node_id())?;
        // Delegate to the path-based API, which honours the error
        // injections.
        self.read(node.path(), range).await
    }

    async fn read_stream_node(
        &self,
        id: FileId<Self::Nid>,
        range: Option<Range<u64>>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        let node = self.get_node(id.as_node_id())?;
        // Delegate to the path-based API, which honours the stream error
        // injections.
        self.read_stream(node.path(), range).await
    }
}

// ---------------------------------------------------------------------------
// WritableFileSystem implementation
// ---------------------------------------------------------------------------

impl MockFs {
    /// Register a file entry at `path` and cache a node for it.
    fn create_file_entry(
        &self,
        path: &Path,
        parent_id: u64,
        name: &OsStr,
    ) -> Result<u64> {
        if self.node_at(path).is_some() {
            return Err(FsError::InvalidArgument {
                operation: FsOperation::CreateFile,
                path: path.to_path_buf(),
                message: "entry already exists".to_string(),
            });
        }
        self.tree
            .write()
            .nodes
            .insert(path.to_path_buf(), MockNode::File(shared_buf(b"")));
        let node = Node::file(
            name.to_os_string(),
            path.to_path_buf(),
            Metadata::file(0, Utc::now()),
        )
        .with_parent(Some(parent_id));
        Ok(self.upsert_node(node))
    }

    /// Register a directory entry at `path` and cache a node for it.
    fn create_dir_entry(
        &self,
        path: &Path,
        parent_id: u64,
        name: &OsStr,
    ) -> Result<u64> {
        if self.node_at(path).is_some() {
            return Err(FsError::InvalidArgument {
                operation: FsOperation::CreateDir,
                path: path.to_path_buf(),
                message: "entry already exists".to_string(),
            });
        }
        self.tree
            .write()
            .nodes
            .insert(path.to_path_buf(), MockNode::Dir);
        let node = Node::directory(
            name.to_os_string(),
            path.to_path_buf(),
            Metadata::dir(Utc::now()),
        )
        .with_parent(Some(parent_id));
        Ok(self.upsert_node(node))
    }
}

#[async_trait]
impl WritableFileSystem for MockFs {
    async fn create_file(
        &self,
        parent: DirId<Self::Nid>,
        name: &OsStr,
    ) -> Result<FileId<Self::Nid>> {
        // Un-resolved parents cannot host children, just like on a real
        // filesystem.
        let parent_node = self.get_node(parent.as_node_id())?;
        let path = parent_node.path().join(name);
        // Injections shadow the creation, too.
        if let Some(err) = self.error_at(&path) {
            return Err(err);
        }
        let id = self.create_file_entry(&path, *parent.get(), name)?;
        Ok(FileId::new(id))
    }

    async fn create_dir_node(
        &self,
        parent: DirId<Self::Nid>,
        name: &OsStr,
    ) -> Result<DirId<Self::Nid>> {
        let parent_node = self.get_node(parent.as_node_id())?;
        let path = parent_node.path().join(name);
        if let Some(err) = self.error_at(&path) {
            return Err(err);
        }
        let id = self.create_dir_entry(&path, *parent.get(), name)?;
        Ok(DirId::new(id))
    }

    async fn create_symlink(
        &self,
        parent: DirId<Self::Nid>,
        name: &OsStr,
        target: &Path,
    ) -> Result<NodeId<Self::Nid>> {
        let parent_node = self.get_node(parent.as_node_id())?;
        let path = parent_node.path().join(name);
        if let Some(err) = self.error_at(&path) {
            return Err(err);
        }
        if self.node_at(&path).is_some() {
            return Err(FsError::InvalidArgument {
                operation: FsOperation::CreateSymlink,
                path: path.clone(),
                message: "entry already exists".to_string(),
            });
        }
        self.tree
            .write()
            .nodes
            .insert(path.clone(), MockNode::Symlink(target.to_path_buf()));
        let node = Node::symlink(
            name.to_os_string(),
            path,
            Metadata::file(0, Utc::now()),
            SymlinkTarget::new(target.to_path_buf()),
        )
        .with_parent(Some(*parent.get()));
        let id = self.upsert_node(node);
        Ok(NodeId::new(id))
    }

    async fn write_node(
        &self,
        id: FileId<Self::Nid>,
        data: Bytes,
    ) -> Result<()> {
        let node = self.get_node(id.as_node_id())?;
        if let Some(err) = self.error_at(node.path()) {
            return Err(err);
        }
        self.set_content(node.path(), &data)
    }

    async fn flush_node(&self, _id: FileId<Self::Nid>) -> Result<()> {
        // Nothing is ever buffered, so flushing is trivially successful.
        Ok(())
    }

    async fn remove_node(&self, id: NodeId<Self::Nid>) -> Result<()> {
        // Delegate to the path-based API, which honours the error
        // injections and rejects non-empty directories.
        let node = self.get_node(id)?;
        self.remove(node.path()).await
    }

    async fn remove_all_node(&self, id: NodeId<Self::Nid>) -> Result<()> {
        let node = self.get_node(id)?;
        self.remove_subtree(node.path())
    }

    async fn rename_node(
        &self,
        id: NodeId<Self::Nid>,
        new_name: &OsStr,
    ) -> Result<()> {
        let node = self.get_node(id)?;
        let old_path = node.path().to_path_buf();
        let Some(parent) = old_path.parent() else {
            return Err(FsError::NotFound { path: old_path });
        };
        let new_path = parent.join(new_name);
        if new_path == old_path {
            return Ok(());
        }
        // Delegate to the path-based rename, which honours the error
        // injections and re-keys the node cache.
        self.rename(&old_path, &new_path).await
    }

    async fn copy_node(
        &self,
        src: NodeId<Self::Nid>,
        dst: DirId<Self::Nid>,
    ) -> Result<NodeId<Self::Nid>> {
        let src_node = self.get_node(src)?;
        let dst_node = self.get_node(dst.as_node_id())?;
        let src_path = src_node.path().to_path_buf();
        if let Some(err) = self.error_at(&src_path) {
            return Err(err);
        }
        let name = src_path
            .file_name()
            .map(OsStr::to_owned)
            .unwrap_or_default();
        let dst_path = dst_node.path().join(name);
        if let Some(err) = self.error_at(&dst_path) {
            return Err(err);
        }
        self.copy_subtree(&src_path, &dst_path)
    }

    async fn move_node(
        &self,
        src: NodeId<Self::Nid>,
        dst: DirId<Self::Nid>,
    ) -> Result<NodeId<Self::Nid>> {
        let src_node = self.get_node(src)?;
        let dst_node = self.get_node(dst.as_node_id())?;
        let src_path = src_node.path().to_path_buf();
        if let Some(err) = self.error_at(&src_path) {
            return Err(err);
        }
        let name = src_path
            .file_name()
            .map(OsStr::to_owned)
            .unwrap_or_default();
        let dst_path = dst_node.path().join(name);
        if let Some(err) = self.error_at(&dst_path) {
            return Err(err);
        }
        if dst_path == src_path {
            return Ok(src);
        }
        self.move_subtree(&src_path, &dst_path)?;
        // The result is the moved node under its new path.
        self.resolve_path(&dst_path).await
    }
}

// ---------------------------------------------------------------------------
// File handles
// ---------------------------------------------------------------------------

/// A read-only `FsFile` handle over an in-memory buffer.
#[derive(Clone, Debug)]
pub struct MockFile {
    meta: Metadata,
    data: Bytes,
    /// Read cursor. Kept separate from `data` on purpose: wrapping a
    /// `std::io::Cursor` would make `read` return `WriteZero` when polled
    /// with a full buffer, which hangs `read_to_end`.
    pos: usize,
}

impl MockFile {
    /// Create a new handle on the given file content.
    pub fn new(data: impl AsRef<[u8]>) -> Self {
        let data = Bytes::copy_from_slice(data.as_ref());
        Self {
            meta: Metadata::file(data.len() as u64, Utc::now()),
            data,
            pos: 0,
        }
    }
}

#[async_trait]
impl FsFile for MockFile {
    fn metadata(&self) -> &Metadata {
        &self.meta
    }

    async fn flush(&mut self) -> Result<()> {
        // Nothing is ever buffered, so flushing is trivially successful.
        Ok(())
    }
}

impl AsyncRead for MockFile {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        let rest = &me.data[me.pos..];
        let n = usize::min(rest.len(), buf.remaining());
        buf.put_slice(&rest[..n]);
        me.pos += n;
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for MockFile {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(io::Error::other("MockFile is read-only")))
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// A writable `FsFile` handle over a shared in-memory buffer. Writes append
/// to the same buffer that the filesystem reads from, so content written
/// through the handle is observable via `MockFs::read` and the other
/// read-only operations.
#[derive(Debug)]
pub struct MockWritableFile {
    meta: Metadata,
    data: SharedBuf,
    /// Read cursor, semantics as in [`MockFile`].
    pos: usize,
}

impl MockWritableFile {
    /// Create a new handle sharing the given buffer.
    pub fn new(data: SharedBuf) -> Self {
        let size = data.lock().len();
        Self {
            meta: Metadata::file(size as u64, Utc::now()),
            data,
            pos: 0,
        }
    }
}

#[async_trait]
impl FsFile for MockWritableFile {
    fn metadata(&self) -> &Metadata {
        &self.meta
    }

    async fn flush(&mut self) -> Result<()> {
        // Nothing is ever buffered, so flushing is trivially successful.
        Ok(())
    }
}

impl AsyncRead for MockWritableFile {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        let data = me.data.lock();
        let rest = &data[me.pos..];
        let n = usize::min(rest.len(), buf.remaining());
        buf.put_slice(&rest[..n]);
        me.pos += n;
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for MockWritableFile {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let me = self.get_mut();
        me.data.lock().extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use futures::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::hash::{hash_bytes, hash_file};

    #[tokio::test]
    async fn metadata_reflects_tree() {
        let fs = MockFs::new("mock")
            .with_dir("/root")
            .with_file("/root/a.txt", "alpha");
        assert!(fs.metadata(Path::new("/root")).await.unwrap().is_dir);
        let file = fs.metadata(Path::new("/root/a.txt")).await.unwrap();
        assert!(file.is_file());
        assert_eq!(file.size, 5);
        assert!(matches!(
            fs.metadata(Path::new("/root/missing.txt")).await,
            Err(FsError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn injected_error_surfaces_on_all_operations() {
        let err = FsError::PermissionDenied {
            operation: FsOperation::Read,
            path: "/secret".into(),
        };
        let fs = MockFs::new("mock")
            .with_file("/secret", "x")
            .with_error("/secret", err);
        assert!(matches!(
            fs.metadata(Path::new("/secret")).await,
            Err(FsError::PermissionDenied { .. })
        ));
        assert!(fs.read_dir(Path::new("/secret")).await.is_err());
        assert!(fs.read(Path::new("/secret"), None).await.is_err());
        assert!(fs.read_stream(Path::new("/secret"), None).await.is_err());
        // Injections shadow the write operations as well.
        assert!(matches!(
            fs.write(Path::new("/secret"), Bytes::from_static(b"y"))
                .await,
            Err(FsError::PermissionDenied { .. })
        ));
    }

    #[tokio::test]
    async fn read_dir_lists_direct_children() {
        let fs = MockFs::new("mock")
            .with_dir("/root")
            .with_dir("/root/sub")
            .with_file("/root/sub/b.txt", "bravo")
            .with_file("/root/a.txt", "alpha");
        let mut stream = fs.read_dir(Path::new("/root")).await.unwrap();
        let mut names = Vec::new();
        while let Some(entry) = stream.next().await {
            names.push(entry.unwrap().name);
        }
        assert_eq!(names, ["a.txt", "sub"]);
    }

    #[tokio::test]
    async fn read_clamps_and_rejects_ranges() {
        let fs = MockFs::new("mock").with_file("/data.txt", "hello world");
        // Clamped to the content length.
        assert_eq!(
            fs.read(Path::new("/data.txt"), Some(8..100)).await.unwrap(),
            Bytes::from_static(b"rld")
        );
        // Entirely past EOF.
        assert!(
            fs.read(Path::new("/data.txt"), Some(100..200))
                .await
                .unwrap()
                .is_empty()
        );
        // Inverted range is rejected, mirroring `LocalFs::read`.
        #[allow(clippy::reversed_empty_ranges)]
        let err = fs
            .read(Path::new("/data.txt"), Some(10..5))
            .await
            .unwrap_err();
        assert!(matches!(err, FsError::InvalidArgument { .. }));
    }

    #[tokio::test]
    async fn stream_error_surfaces_in_hash_file() {
        let fs = MockFs::new("mock")
            .with_file("/broken.bin", vec![0u8; CHUNK_SIZE * 2 + 3])
            .with_stream_error(
                "/broken.bin",
                FsError::Io {
                    operation: FsOperation::Read,
                    path: "/broken.bin".into(),
                    message: "injected".to_string(),
                },
            );
        let err = hash_file(&fs, Path::new("/broken.bin")).await.unwrap_err();
        assert!(matches!(err, FsError::Io { .. }));
    }

    #[tokio::test]
    async fn hash_file_reads_every_chunk() {
        let data = Bytes::from_static(b"hello world");
        let fs = MockFs::new("mock").with_file("/f.txt", &data);
        let hash = hash_file(&fs, Path::new("/f.txt")).await.unwrap();
        assert_eq!(hash, hash_bytes(&data));
    }

    #[tokio::test]
    async fn file_handle_reads_to_end() {
        let fs = MockFs::new("mock").with_file("/f.txt", "hello");
        let mut handle =
            fs.open(Path::new("/f.txt"), OpenMode::Read).await.unwrap();
        let mut buf = Vec::new();
        handle.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, b"hello");
        // Writing through a read-only handle fails.
        assert!(handle.write_all(b"more").await.is_err());
    }

    #[tokio::test]
    async fn writable_handle_appends_to_the_tree() {
        let fs = MockFs::new("mock").with_file("/f.txt", "hello");
        let mut handle =
            fs.open(Path::new("/f.txt"), OpenMode::Write).await.unwrap();
        // Writing through a write-mode handle succeeds and is visible on
        // the filesystem because the handle shares the file buffer.
        handle.write_all(b"!").await.unwrap();
        handle.flush().await.unwrap();
        drop(handle);
        assert_eq!(
            fs.read(Path::new("/f.txt"), None).await.unwrap(),
            Bytes::from_static(b"hello!")
        );
    }

    #[tokio::test]
    async fn resolve_path_caches_nodes() {
        let fs = MockFs::new("mock")
            .with_dir("/root")
            .with_dir("/root/sub")
            .with_file("/root/sub/b.txt", "bravo");
        let root = fs.resolve_path(Path::new("/root")).await.unwrap();
        // Re-resolving the same path yields the cached node ID.
        assert_eq!(fs.resolve_path(Path::new("/root")).await.unwrap(), root);
        let sub = fs.resolve_path(Path::new("/root/sub")).await.unwrap();
        let b = fs.resolve_path(Path::new("/root/sub/b.txt")).await.unwrap();
        let node = fs.get_node(b).unwrap();
        assert!(node.kind().is_file());
        // Resolving a child before its cached parent links the two nodes.
        assert_eq!(node.parent(), Some(*sub.get()));
        assert!(matches!(
            fs.resolve_path(Path::new("/root/missing")).await,
            Err(FsError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn resolve_path_honours_injected_errors() {
        let err = FsError::PermissionDenied {
            operation: FsOperation::Resolve,
            path: "/secret".into(),
        };
        let fs = MockFs::new("mock")
            .with_file("/secret", "x")
            .with_error("/secret", err);
        assert!(matches!(
            fs.resolve_path(Path::new("/secret")).await,
            Err(FsError::PermissionDenied { .. })
        ));
    }

    #[tokio::test]
    async fn read_dir_node_resolves_children() {
        let fs = MockFs::new("mock")
            .with_dir("/root")
            .with_dir("/root/sub")
            .with_file("/root/a.txt", "alpha");
        let root = fs.resolve_path(Path::new("/root")).await.unwrap();
        fs.read_dir_node(DirId::new(*root.get())).await.unwrap();
        let root_node = fs.get_node(root).unwrap();
        assert_eq!(root_node.kind().children().unwrap(), ["a.txt", "sub"]);
        // Children are cached as nodes, linked to their parent, and
        // addressable by ID.
        let sub = fs.resolve_path(Path::new("/root/sub")).await.unwrap();
        assert!(fs.get_node(sub).unwrap().kind().is_directory());
        assert_eq!(fs.get_node(sub).unwrap().parent(), Some(*root.get()));
        // A second call is a no-op.
        assert!(fs.read_dir_node(DirId::new(*root.get())).await.is_ok());
        // Directories must not be listed as files.
        let file = fs.resolve_path(Path::new("/root/a.txt")).await.unwrap();
        assert!(matches!(
            fs.read_dir_node(DirId::new(*file.get())).await,
            Err(FsError::WrongKind { .. })
        ));
    }

    #[tokio::test]
    async fn node_reads_delegate_to_path_reads() {
        let fs = MockFs::new("mock")
            .with_file("/data.txt", "hello world")
            .with_file("/stream.bin", vec![0u8; CHUNK_SIZE + 1])
            .with_stream_error(
                "/stream.bin",
                FsError::Io {
                    operation: FsOperation::Read,
                    path: "/stream.bin".into(),
                    message: "injected".to_string(),
                },
            );
        let data = fs.resolve_path(Path::new("/data.txt")).await.unwrap();
        let data_id = FileId::new(*data.get());
        assert_eq!(
            fs.read_node(data_id, Some(8..100)).await.unwrap(),
            Bytes::from_static(b"rld")
        );
        let mut handle = fs.open_node(data_id, OpenMode::Read).await.unwrap();
        let mut buf = Vec::new();
        handle.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, b"hello world");
        // The stream error injected for the path surfaces in the node-based
        // stream as well.
        let stream = fs.resolve_path(Path::new("/stream.bin")).await.unwrap();
        let mut stream = fs
            .read_stream_node(FileId::new(*stream.get()), None)
            .await
            .unwrap();
        assert!(matches!(stream.next().await, Some(Err(FsError::Io { .. }))));
    }

    #[tokio::test]
    async fn write_replaces_content_and_refreshes_metadata() {
        let fs = MockFs::new("mock").with_file("/f.txt", "hello");
        fs.write(Path::new("/f.txt"), Bytes::from_static(b"world!"))
            .await
            .unwrap();
        assert_eq!(
            fs.read(Path::new("/f.txt"), None).await.unwrap(),
            Bytes::from_static(b"world!")
        );
        // Writing to an unknown path creates the file, like `std::fs::write`
        // and unlike the write methods of the real providers.
        fs.write(Path::new("/g.bin"), Bytes::from_static(b"bin"))
            .await
            .unwrap();
        assert_eq!(fs.metadata(Path::new("/g.bin")).await.unwrap().size, 3);
    }

    #[tokio::test]
    async fn write_node_replaces_content_and_refreshes_metadata() {
        let fs = MockFs::new("mock").with_file("/f.txt", "hello");
        let f = fs.resolve_path(Path::new("/f.txt")).await.unwrap();
        let f_id = FileId::new(*f.get());
        fs.write_node(f_id, Bytes::from_static(b"world!"))
            .await
            .unwrap();
        assert_eq!(fs.node_metadata(f).unwrap().size, 6);
        // An injected error on the path shadows the node-based write.
        // The node must be resolved before the error is injected, since the
        // injection shadows `resolve_path` as well.
        let locked = MockFs::new("mock").with_file("/locked.txt", "hello");
        let locked_id = FileId::new(
            *locked
                .resolve_path(Path::new("/locked.txt"))
                .await
                .unwrap()
                .get(),
        );
        let locked = locked.with_error(
            "/locked.txt",
            FsError::PermissionDenied {
                operation: FsOperation::Write,
                path: "/locked.txt".into(),
            },
        );
        assert!(matches!(
            locked.write_node(locked_id, Bytes::from_static(b"x")).await,
            Err(FsError::PermissionDenied { .. })
        ));
    }

    #[tokio::test]
    async fn remove_tombstones_cached_nodes() {
        let fs = MockFs::new("mock")
            .with_dir("/root")
            .with_dir("/root/sub")
            .with_file("/root/sub/b.txt", "bravo");
        let b = fs.resolve_path(Path::new("/root/sub/b.txt")).await.unwrap();
        // A non-empty directory cannot be removed with the single-entry
        // operation.
        assert!(matches!(
            fs.remove(Path::new("/root/sub")).await,
            Err(FsError::InvalidArgument { .. })
        ));
        fs.remove(Path::new("/root/sub/b.txt")).await.unwrap();
        // Removals keep the node addressable but mark it as stale, like the
        // real providers do.
        assert!(matches!(
            fs.resolve_path(Path::new("/root/sub/b.txt")).await,
            Err(FsError::StaleNode)
        ));
        assert!(matches!(fs.get_node(b), Err(FsError::StaleNode)));
        // `remove_all` clears the subtree and tombstones the cached children
        // and subdirectories.
        fs.remove_all(Path::new("/root/sub")).await.unwrap();
        assert!(matches!(
            fs.resolve_path(Path::new("/root/sub/b.txt")).await,
            Err(FsError::StaleNode)
        ));
        assert!(matches!(
            fs.metadata(Path::new("/root/sub")).await,
            Err(FsError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn rename_moves_subtrees() {
        let fs = MockFs::new("mock")
            .with_dir("/root")
            .with_dir("/root/sub")
            .with_file("/root/sub/b.txt", "bravo")
            .with_file("/root/a.txt", "alpha");
        let root = fs.resolve_path(Path::new("/root")).await.unwrap();
        fs.read_dir_node(DirId::new(*root.get())).await.unwrap();
        fs.rename(Path::new("/root/sub"), Path::new("/root/moved"))
            .await
            .unwrap();
        // The child list of the parent is cleared so that a re-listing
        // observes the moved subtree...
        assert!(fs.read_dir_node(DirId::new(*root.get())).await.is_ok());
        let root_node = fs.get_node(root).unwrap();
        assert_eq!(root_node.kind().children().unwrap(), ["a.txt", "moved"]);
        // ...and the moved content is addressable under the new path.
        assert_eq!(
            fs.read(Path::new("/root/moved/b.txt"), None).await.unwrap(),
            Bytes::from_static(b"bravo")
        );
        // A rename onto an occupied target is rejected (and would leak the
        // subtree if it were not).
        assert!(matches!(
            fs.rename(Path::new("/root/a.txt"), Path::new("/root/moved"))
                .await,
            Err(FsError::InvalidArgument { .. })
        ));
    }

    #[tokio::test]
    async fn copy_leaves_source_intact() {
        let fs = MockFs::new("mock")
            .with_dir("/root")
            .with_dir("/root/src")
            .with_file("/root/src/b.txt", "bravo");
        fs.copy(Path::new("/root/src"), Path::new("/root/dst"))
            .await
            .unwrap();
        assert_eq!(
            fs.read(Path::new("/root/dst/b.txt"), None).await.unwrap(),
            Bytes::from_static(b"bravo")
        );
        assert_eq!(
            fs.read(Path::new("/root/src/b.txt"), None).await.unwrap(),
            Bytes::from_static(b"bravo")
        );
        // Copies of unknown sources and onto occupied targets are rejected.
        assert!(matches!(
            fs.copy(Path::new("/root/gone"), Path::new("/root/x")).await,
            Err(FsError::NotFound { .. })
        ));
        assert!(matches!(
            fs.copy(Path::new("/root/src"), Path::new("/root/dst"))
                .await,
            Err(FsError::InvalidArgument { .. })
        ));
    }

    #[tokio::test]
    async fn symlink_roundtrip() {
        // A symlink to an unknown path is dangling and must not resolve.
        let fs = MockFs::new("mock")
            .with_dir("/root")
            .with_file("/root/a.txt", "alpha")
            .with_node("/root/link", MockNode::Symlink("/outside".into()))
            .with_node("/root/alias", MockNode::Symlink("/root/a.txt".into()));
        let link = fs.resolve_path(Path::new("/root/link")).await.unwrap();
        assert!(matches!(
            fs.resolve_symlink(link).await,
            Err(FsError::NotFound { .. })
        ));
        let alias = fs.resolve_path(Path::new("/root/alias")).await.unwrap();
        let target = fs.resolve_symlink(alias).await.unwrap();
        assert!(fs.get_node(target).unwrap().kind().is_file());
    }

    #[tokio::test]
    async fn node_write_ops_roundtrip() {
        let fs = MockFs::new("mock").with_dir("/root");
        let root = fs.resolve_path(Path::new("/root")).await.unwrap();
        let root_id = DirId::new(*root.get());
        let file = fs
            .create_file(root_id, OsStr::new("new.txt"))
            .await
            .unwrap();
        assert!(fs.get_node(file.as_node_id()).unwrap().kind().is_file());
        fs.write_node(file, Bytes::from_static(b"data"))
            .await
            .unwrap();
        assert_eq!(fs.node_metadata(file.as_node_id()).unwrap().size, 4);
        let sub = fs
            .create_dir_node(root_id, OsStr::new("sub"))
            .await
            .unwrap();
        assert!(fs.get_node(sub.as_node_id()).unwrap().kind().is_directory());
        // Creating twice under the same name fails.
        assert!(matches!(
            fs.create_dir_node(root_id, OsStr::new("sub")).await,
            Err(FsError::InvalidArgument { .. })
        ));
        // Node-based removal delegates to the path-based one.
        fs.remove_node(file.as_node_id()).await.unwrap();
        assert!(matches!(
            fs.get_node(file.as_node_id()),
            Err(FsError::StaleNode)
        ));
        // A directory with children can only be removed recursively.
        fs.create_file(sub, OsStr::new("x.txt")).await.unwrap();
        assert!(matches!(
            fs.remove_node(sub.as_node_id()).await,
            Err(FsError::InvalidArgument { .. })
        ));
        fs.remove_all_node(sub.as_node_id()).await.unwrap();
        assert!(matches!(
            fs.get_node(sub.as_node_id()),
            Err(FsError::StaleNode)
        ));
    }

    #[tokio::test]
    async fn node_copy_and_move_roundtrip() {
        let fs = MockFs::new("mock")
            .with_dir("/root")
            .with_dir("/root/src")
            .with_file("/root/src/b.txt", "bravo")
            .with_dir("/root/dst");
        let src = fs.resolve_path(Path::new("/root/src")).await.unwrap();
        let dst = fs.resolve_path(Path::new("/root/dst")).await.unwrap();
        // A cross-directory copy leaves the source intact and yields a
        // node for the copied root.
        let copied = fs.copy_node(src, DirId::new(*dst.get())).await.unwrap();
        assert!(fs.get_node(copied).unwrap().kind().is_directory());
        let b = fs.resolve_path(Path::new("/root/src/b.txt")).await.unwrap();
        // A rename within a directory re-keys the cache.
        fs.rename_node(b, OsStr::new("c.txt")).await.unwrap();
        assert_eq!(
            fs.read(Path::new("/root/src/c.txt"), None).await.unwrap(),
            Bytes::from_static(b"bravo")
        );
        // A cross-directory move keeps the node identity (its cache entry is
        // re-keyed rather than tombstoned) and moves the content.
        let c = fs.resolve_path(Path::new("/root/src/c.txt")).await.unwrap();
        fs.move_node(c, DirId::new(*dst.get())).await.unwrap();
        assert_eq!(
            fs.read(Path::new("/root/dst/c.txt"), None).await.unwrap(),
            Bytes::from_static(b"bravo")
        );
        assert!(matches!(
            fs.metadata(Path::new("/root/src/c.txt")).await,
            Err(FsError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn symlink_create_and_resolve() {
        let fs = MockFs::new("mock")
            .with_dir("/root")
            .with_file("/root/a.txt", "alpha");
        let root = fs.resolve_path(Path::new("/root")).await.unwrap();
        let root_id = DirId::new(*root.get());
        let link = fs
            .create_symlink(
                root_id,
                OsStr::new("link"),
                Path::new("/root/a.txt"),
            )
            .await
            .unwrap();
        assert!(fs.get_node(link).unwrap().kind().is_symlink());
        let target = fs.resolve_symlink(link).await.unwrap();
        assert!(fs.get_node(target).unwrap().kind().is_file());
    }
}
