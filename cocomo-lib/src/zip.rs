// ---------------------------------------------------------------------------
// Copyright:   (c) 2026 ff. Michael Amrhein (michael@adrhinum.de)
// License:     This program is part of a larger application. For license
//              details please read the file LICENSE.TXT provided together
//              with the application.
// ---------------------------------------------------------------------------
// $Source$
// $Revision$

//! Read-only ZIP archive provider.
//!
//! [`ZipFs`] exposes a single `.zip` archive as a filesystem that can serve
//! one side of a folder comparison. The archive is opened eagerly at
//! construction: the central directory is read once and an in-memory tree of
//! directories and files is built from the entry names. File contents are
//! decompressed on demand, per read.
//!
//! # Read-only
//!
//! ZIP archives have no in-place mutation API, so every mutating
//! [`WritableFileSystem`](crate::fs::WritableFileSystem) method fails with an
//! [`FsError::Io`] whose message states that the provider is read-only.
//!
//! # Paths
//!
//! Paths resolved through [`NodeFileSystem::resolve_path`] address locations
//! *inside* the archive: the archive root is `/`, and entries are addressed
//! as `/dir/file.txt`. Directories that a ZIP lacks an explicit entry for
//! are synthesized from the path prefixes of the file entries.
//!
//! # Node cache
//!
//! Every resolved path is cached as a [`Node`] keyed by a monotonically
//! increasing [`u64`] identifier, mirroring
//! [`LocalFs`](crate::local::LocalFs). Since the archive never changes,
//! nodes are never tombstoned.

use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    ffi::{OsStr, OsString},
    fmt,
    hash::{Hash, Hasher},
    io::{self, Read},
    ops::Range,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    task::{Context, Poll},
};

// The crate module is also named `zip`, so the dependency is addressed
// through the extern prelude with a leading `::` to avoid ambiguity.
use ::zip::{DateTime as ZipDateTime, ZipArchive, result::ZipError};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, NaiveDate, Utc};
use futures::{Stream, stream};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::{
    error::{FsError, FsOperation, Result, wrap},
    file::FsFile,
    fs::{NodeFileSystem, OpenMode, WritableFileSystem},
    identity::{DirId, FileId, FileSystemId, NodeId},
    meta::Metadata,
    node::{Node, NodeKind, UserPermissions},
};

// ---------------------------------------------------------------------------
// Type aliases
// ---------------------------------------------------------------------------

/// Concrete node ID type for the ZIP provider.
pub type ZipNodeId = NodeId<u64>;

/// Concrete directory ID type for the ZIP provider.
pub type ZipDirId = DirId<u64>;

/// Concrete file ID type for the ZIP provider.
pub type ZipFileId = FileId<u64>;

// ---------------------------------------------------------------------------
// Archive tree
// ---------------------------------------------------------------------------

/// One node of the in-memory archive tree, keyed by normalized in-archive
/// path (the archive root is the empty key).
#[derive(Clone, Debug)]
struct ZipEntryInfo {
    /// Entry name (final path component; empty for the archive root).
    name: String,
    /// `true` for directories, whether explicit or synthesized.
    is_dir: bool,
    /// Index of the entry in the archive's central directory. `None` for
    /// directories.
    entry_index: Option<usize>,
    /// Uncompressed size in bytes (0 for directories).
    size: u64,
    /// Last modification time.
    modified: DateTime<Utc>,
    /// Keys of the child entries (directories only), sorted.
    children: Vec<String>,
}

/// Normalize an entry name or caller-supplied path to a tree key:
/// `/`-separated, without leading or trailing separators and without `.`
/// components. The archive root normalizes to the empty string.
fn normalize_key(raw: &str) -> String {
    raw.replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// Return the parent key of a tree key (the archive root for top-level
/// entries).
fn parent_of(key: &str) -> &str {
    match key.rfind('/') {
        Some(idx) => &key[..idx],
        None => "",
    }
}

/// Return the entry name (final path component) of a tree key.
fn name_of(key: &str) -> String {
    match key.rfind('/') {
        Some(idx) => key[idx + 1..].to_owned(),
        None => key.to_owned(),
    }
}

/// Return the node path (an absolute, `/`-rooted path) for a tree key.
fn node_path(key: &str) -> PathBuf {
    if key.is_empty() {
        PathBuf::from("/")
    } else {
        PathBuf::from(format!("/{key}"))
    }
}

/// Convert a ZIP timestamp to UTC. Returns `None` for out-of-range values.
fn to_utc(dt: ZipDateTime) -> Option<DateTime<Utc>> {
    let date = NaiveDate::from_ymd_opt(
        i32::from(dt.year()),
        u32::from(dt.month()),
        u32::from(dt.day()),
    )?;
    let naive = date.and_hms_opt(
        u32::from(dt.hour()),
        u32::from(dt.minute()),
        u32::from(dt.second()),
    )?;
    Some(DateTime::from_naive_utc_and_offset(naive, Utc))
}

/// Convert a [`ZipError`] into an [`FsError`] with operation context.
fn zip_error(err: ZipError, operation: FsOperation, path: &Path) -> FsError {
    match err {
        ZipError::FileNotFound => FsError::NotFound {
            path: path.to_path_buf(),
        },
        ZipError::Io(io_err) => wrap(io_err, operation, path.to_path_buf()),
        other => FsError::Io {
            operation,
            path: path.to_path_buf(),
            message: other.to_string(),
        },
    }
}

/// Read the central directory once and build the in-memory tree of
/// directories and files from the entry names.
///
/// `fallback` is the archive file's own modification time, used for entries
/// without a timestamp and for synthesized directories.
fn build_tree(
    zip: &mut ZipArchive<fs_err::File>,
    fallback: DateTime<Utc>,
) -> HashMap<String, ZipEntryInfo> {
    let mut tree: HashMap<String, ZipEntryInfo> = HashMap::new();
    tree.insert(
        String::new(),
        ZipEntryInfo {
            name: String::new(),
            is_dir: true,
            entry_index: None,
            size: 0,
            modified: fallback,
            children: Vec::new(),
        },
    );
    for index in 0..zip.len() {
        // Skip entries that cannot be read from the central directory.
        let Ok(entry) = zip.by_index(index) else {
            continue;
        };
        // Skip entries whose name cannot be decoded.
        let Ok(name) = entry.name() else {
            continue;
        };
        let key = normalize_key(&name);
        if key.is_empty() {
            continue;
        }
        let modified =
            entry.last_modified().and_then(to_utc).unwrap_or(fallback);
        if entry.is_dir() {
            match tree.get_mut(&key) {
                // An explicit directory entry refines a synthesized one.
                Some(info) if info.is_dir => info.modified = modified,
                // A file with the same name wins; such archives are
                // pathological and not worth special-casing further.
                Some(_) => {}
                None => {
                    tree.insert(
                        key.clone(),
                        ZipEntryInfo {
                            name: name_of(&key),
                            is_dir: true,
                            entry_index: None,
                            size: 0,
                            modified,
                            children: Vec::new(),
                        },
                    );
                }
            }
        } else {
            tree.insert(
                key.clone(),
                ZipEntryInfo {
                    name: name_of(&key),
                    is_dir: false,
                    entry_index: Some(index),
                    size: entry.size(),
                    modified,
                    children: Vec::new(),
                },
            );
            // Synthesize missing ancestor directories from the prefix.
            let mut parent = parent_of(&key);
            while !parent.is_empty() && !tree.contains_key(parent) {
                tree.insert(
                    parent.to_owned(),
                    ZipEntryInfo {
                        name: name_of(parent),
                        is_dir: true,
                        entry_index: None,
                        size: 0,
                        modified: fallback,
                        children: Vec::new(),
                    },
                );
                parent = parent_of(parent);
            }
        }
    }
    // Link every entry into its parent's child list, then sort.
    let keys: Vec<String> =
        tree.keys().filter(|key| !key.is_empty()).cloned().collect();
    for key in keys {
        let parent = parent_of(&key);
        if let Some(dir) = tree.get_mut(parent) {
            dir.children.push(key);
        }
    }
    for info in tree.values_mut() {
        info.children.sort();
    }
    tree
}

// ---------------------------------------------------------------------------
// ZipFile (read-only file handle)
// ---------------------------------------------------------------------------

/// A read-only file handle over the decompressed content of one archive
/// entry.
pub struct ZipFile {
    meta: Metadata,
    data: Bytes,
    /// Read cursor. Kept separate from `data` on purpose: wrapping a
    /// `std::io::Cursor` would make `read` return `WriteZero` when polled
    /// with a full buffer, which hangs `read_to_end`.
    pos: usize,
}

#[async_trait]
impl FsFile for ZipFile {
    fn metadata(&self) -> &Metadata {
        &self.meta
    }

    async fn flush(&mut self) -> Result<()> {
        // Nothing is ever buffered, so flushing is trivially successful.
        Ok(())
    }
}

impl AsyncRead for ZipFile {
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

impl AsyncWrite for ZipFile {
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(io::Error::other("ZipFile is read-only")))
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

// ---------------------------------------------------------------------------
// ZipFs
// ---------------------------------------------------------------------------

/// Read-only ZIP archive provider.
///
/// The archive is opened eagerly at construction and its central directory
/// is turned into an in-memory tree, so path resolution and directory
/// listing never touch the archive again. Only file reads decompress, per
/// read.
///
/// # Node cache
///
/// Every resolved path is cached as a [`Node`] keyed by a monotonically
/// increasing [`u64`] identifier, mirroring
/// [`LocalFs`](crate::local::LocalFs). A reverse lookup map maintains node
/// path \u{2192} ID mappings. The archive is immutable, so nodes are never
/// tombstoned.
pub struct ZipFs {
    /// Human-readable label for this provider instance.
    label: String,
    /// Filesystem instance identifier (hash of the canonical archive path).
    fs_id: FileSystemId<u64>,
    /// Canonical absolute path of the backing archive.
    archive_path: PathBuf,
    /// The opened archive. `ZipArchive::by_index` needs `&mut self`, so
    /// content reads go through a mutex.
    archive: parking_lot::Mutex<ZipArchive<fs_err::File>>,
    /// In-memory tree of all archive entries, keyed by normalized
    /// in-archive path (the archive root is the empty key).
    tree: HashMap<String, ZipEntryInfo>,
    /// Node cache: node ID \u{2192} node.
    nodes: parking_lot::RwLock<HashMap<u64, Arc<Node>>>,
    /// Reverse lookup: node path \u{2192} node ID.
    path_to_id: parking_lot::RwLock<HashMap<PathBuf, u64>>,
    /// Monotonically increasing counter for node ID generation.
    next_id: AtomicU64,
}

impl ZipFs {
    /// Open the ZIP archive at `archive` as a read-only provider, using the
    /// archive path as the provider label.
    ///
    /// # Errors
    ///
    /// Returns [`FsError`] if the archive cannot be opened or its central
    /// directory cannot be read.
    pub fn open(archive: impl AsRef<Path>) -> Result<Self> {
        Self::with_label(archive.as_ref().display().to_string(), archive)
    }

    /// Open the ZIP archive at `archive` with an explicit `label`.
    ///
    /// # Errors
    ///
    /// Returns [`FsError`] if the archive cannot be opened or its central
    /// directory cannot be read.
    pub fn with_label(
        label: impl Into<String>,
        archive: impl AsRef<Path>,
    ) -> Result<Self> {
        let path = archive.as_ref();
        let file = fs_err::File::open(path)
            .map_err(|e| wrap(e, FsOperation::Open, path.to_path_buf()))?;
        // The archive's own mtime is the fallback for entries without a
        // timestamp and for synthesized directories.
        let archive_mtime = file
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .map(DateTime::<Utc>::from)
            .unwrap_or_default();
        let mut zip = ZipArchive::new(file)
            .map_err(|e| zip_error(e, FsOperation::Open, path))?;
        let tree = build_tree(&mut zip, archive_mtime);
        // The canonical path identifies the archive instance regardless of
        // how it was addressed.
        let canonical =
            fs_err::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let mut hasher = DefaultHasher::new();
        canonical.hash(&mut hasher);
        Ok(Self {
            label: label.into(),
            fs_id: FileSystemId::new(hasher.finish()),
            archive_path: canonical,
            archive: parking_lot::Mutex::new(zip),
            tree,
            nodes: parking_lot::RwLock::new(HashMap::new()),
            path_to_id: parking_lot::RwLock::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        })
    }

    /// Return the canonical path of the backing archive.
    pub fn archive_path(&self) -> &Path {
        &self.archive_path
    }

    /// Build a [`Node`] for a tree entry.
    fn build_node(
        key: &str,
        info: &ZipEntryInfo,
        parent_id: Option<u64>,
    ) -> Node {
        let path = node_path(key);
        let name = OsString::from(&info.name);
        let metadata = Metadata {
            size: if info.is_dir { 0 } else { info.size },
            created: None,
            modified: info.modified,
            is_dir: info.is_dir,
            is_symlink: false,
            inode: None,
            device_id: None,
            // The provider is read-only, so no write permission is ever
            // reported.
            permissions: if info.is_dir {
                UserPermissions::READ | UserPermissions::EXEC
            } else {
                UserPermissions::READ
            },
        };
        let node = if info.is_dir {
            Node::directory(name, path, metadata)
        } else {
            Node::file(name, path, metadata)
        };
        node.with_parent(parent_id)
    }

    /// Cache a node and return its ID. If the path is already cached, return
    /// the existing ID without replacing the node.
    fn cache_node(&self, node: Node) -> u64 {
        let path = node.path().to_path_buf();
        {
            let ptid = self.path_to_id.read();
            if let Some(&id) = ptid.get(&path) {
                return id;
            }
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let arc_node = Arc::new(node);
        self.nodes.write().insert(id, arc_node);
        self.path_to_id.write().insert(path, id);
        id
    }

    /// Lookup a node ID from a node path in the cache.
    fn lookup_path(&self, path: &Path) -> Option<u64> {
        self.path_to_id.read().get(path).copied()
    }

    /// Resolve the parent directory ID for a node path, if cached.
    fn resolve_parent_id(&self, path: &Path) -> Option<u64> {
        path.parent().and_then(|p| self.lookup_path(p))
    }

    /// Decompress the archive entry with the given central-directory index.
    fn read_entry_bytes(&self, index: usize, path: &Path) -> Result<Bytes> {
        let mut archive = self.archive.lock();
        let mut entry = archive
            .by_index(index)
            .map_err(|e| zip_error(e, FsOperation::Read, path))?;
        // Bound the initial reservation so a huge declared size cannot
        // pre-allocate unboundedly before the first read.
        let reserve = usize::try_from(entry.size())
            .unwrap_or(0)
            .min(8 * 1024 * 1024);
        let mut buf = Vec::with_capacity(reserve);
        entry
            .read_to_end(&mut buf)
            .map_err(|e| wrap(e, FsOperation::Read, path.to_path_buf()))?;
        Ok(Bytes::from(buf))
    }

    /// Return the node path for a node ID, for error context.
    fn node_path_of(&self, id: NodeId<u64>) -> Result<PathBuf> {
        Ok(self.get_node(id)?.path().to_path_buf())
    }

    /// Build the error every mutating operation fails with.
    fn read_only_error(&self, operation: FsOperation, path: &Path) -> FsError {
        FsError::Io {
            operation,
            path: path.to_path_buf(),
            message: format!("{} is read-only", self.label),
        }
    }
}

impl fmt::Debug for ZipFs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ZipFs")
            .field("label", &self.label)
            .field("archive_path", &self.archive_path)
            .field("entries", &self.tree.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// NodeFileSystem implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl NodeFileSystem for ZipFs {
    type FsId = u64;
    type Nid = u64;
    type Error = FsError;

    fn id(&self) -> FileSystemId<Self::FsId> {
        self.fs_id
    }

    fn label(&self) -> &str {
        &self.label
    }

    async fn resolve_path(&self, path: &Path) -> Result<NodeId<Self::Nid>> {
        // Paths address locations inside the archive; the archive root is
        // the empty key.
        let key = normalize_key(&path.to_string_lossy());
        let npath = node_path(&key);

        // Check cache first.
        if let Some(id) = self.lookup_path(&npath) {
            let node = self.nodes.read().get(&id).cloned();
            return match node {
                Some(n) if n.is_deleted() => Err(FsError::StaleNode),
                Some(_) => Ok(NodeId::new(id)),
                None => Err(FsError::NotFound { path: npath }),
            };
        }

        let info = self.tree.get(&key).ok_or(FsError::NotFound {
            path: npath.clone(),
        })?;
        let parent_id = self.resolve_parent_id(&npath);
        let node = Self::build_node(&key, info, parent_id);
        Ok(NodeId::new(self.cache_node(node)))
    }

    async fn resolve_symlink(
        &self,
        id: NodeId<Self::Nid>,
    ) -> Result<NodeId<Self::Nid>> {
        // ZIP entries are never exposed as symlinks, so there is no target
        // to resolve.
        let node = self.get_node(id)?;
        Err(FsError::WrongKind {
            expected: "symlink",
            actual: match node.kind() {
                NodeKind::Directory { .. } => "directory",
                NodeKind::File => "file",
                NodeKind::Symlink { .. } => "symlink",
                NodeKind::Special => "special",
            },
        })
    }

    fn get_node(&self, id: NodeId<Self::Nid>) -> Result<Arc<Node>> {
        let nodes = self.nodes.read();
        match nodes.get(id.get()).cloned() {
            Some(node) if node.is_deleted() => Err(FsError::StaleNode),
            Some(node) => Ok(node),
            None => {
                drop(nodes);
                Err(FsError::NotFound {
                    path: PathBuf::from("(unknown node)"),
                })
            }
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
        // Metadata-only cache update, allowed on a read-only provider.
        let mut nodes = self.nodes.write();
        let Some(arc) = nodes.get_mut(id.get()) else {
            return Err(FsError::NotFound {
                path: PathBuf::from("(unknown node)"),
            });
        };
        Arc::make_mut(arc).set_cached_hash(hash);
        Ok(())
    }

    async fn read_dir_node(&self, dir_id: DirId<Self::Nid>) -> Result<()> {
        let dir_node = self.get_node(dir_id.as_node_id())?;
        if !dir_node.kind().is_directory() {
            return Err(FsError::WrongKind {
                expected: "directory",
                actual: "file",
            });
        }
        let key = normalize_key(&dir_node.path().to_string_lossy());
        let info = self.tree.get(&key).ok_or(FsError::StaleNode)?;

        let mut child_names = Vec::with_capacity(info.children.len());
        for child_key in &info.children {
            let Some(child_info) = self.tree.get(child_key) else {
                continue;
            };
            child_names.push(child_info.name.clone());
            let child_node =
                Self::build_node(child_key, child_info, Some(*dir_id.get()));
            self.cache_node(child_node);
        }

        // Update the directory node's children.
        {
            let mut nodes = self.nodes.write();
            if let Some(arc) = nodes.get_mut(dir_id.get()) {
                Arc::make_mut(arc).set_children(child_names);
            }
        }
        Ok(())
    }

    async fn open_node(
        &self,
        id: FileId<Self::Nid>,
        mode: OpenMode,
    ) -> Result<Box<dyn FsFile>> {
        let node = self.get_node(id.as_node_id())?;
        if !matches!(mode, OpenMode::Read) {
            return Err(self.read_only_error(FsOperation::Open, node.path()));
        }
        let path = node.path().to_path_buf();
        let key = normalize_key(&path.to_string_lossy());
        let info = self.tree.get(&key).ok_or(FsError::StaleNode)?;
        if info.is_dir {
            return Err(FsError::WrongKind {
                expected: "file",
                actual: "directory",
            });
        }
        let index = info.entry_index.ok_or(FsError::StaleNode)?;
        let data = self.read_entry_bytes(index, &path)?;
        Ok(Box::new(ZipFile {
            meta: node.metadata().clone(),
            data,
            pos: 0,
        }))
    }

    async fn read_node(
        &self,
        id: FileId<Self::Nid>,
        range: Option<Range<u64>>,
    ) -> Result<Bytes> {
        let node = self.get_node(id.as_node_id())?;
        let path = node.path().to_path_buf();
        let key = normalize_key(&path.to_string_lossy());
        let info = self.tree.get(&key).ok_or(FsError::StaleNode)?;
        if info.is_dir {
            return Err(FsError::WrongKind {
                expected: "file",
                actual: "directory",
            });
        }
        let index = info.entry_index.ok_or(FsError::StaleNode)?;
        let data = self.read_entry_bytes(index, &path)?;
        let Some(r) = range else {
            return Ok(data);
        };
        // Validate and clamp the range like LocalFs does.
        if r.start >= r.end {
            return Err(FsError::InvalidArgument {
                operation: FsOperation::Read,
                path,
                message: format!(
                    "invalid range: start ({}) must be less than end ({})",
                    r.start, r.end
                ),
            });
        }
        let size = data.len() as u64;
        if r.start >= size {
            return Ok(Bytes::new());
        }
        let end = r.end.min(size);
        Ok(data.slice(r.start as usize..end as usize))
    }

    async fn read_stream_node(
        &self,
        id: FileId<Self::Nid>,
        range: Option<Range<u64>>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Bytes>> + Send>>> {
        // The whole entry is decompressed in one go, so the stream is a
        // single chunk.
        let data = self.read_node(id, range).await?;
        Ok(Box::pin(stream::once(async move { Ok(data) })))
    }
}

// ---------------------------------------------------------------------------
// WritableFileSystem implementation (all operations refused)
// ---------------------------------------------------------------------------

#[async_trait]
impl WritableFileSystem for ZipFs {
    async fn create_file(
        &self,
        parent: DirId<Self::Nid>,
        name: &OsStr,
    ) -> Result<FileId<Self::Nid>> {
        let path = self.node_path_of(parent.as_node_id())?.join(name);
        Err(self.read_only_error(FsOperation::CreateFile, &path))
    }

    async fn create_dir_node(
        &self,
        parent: DirId<Self::Nid>,
        name: &OsStr,
    ) -> Result<DirId<Self::Nid>> {
        let path = self.node_path_of(parent.as_node_id())?.join(name);
        Err(self.read_only_error(FsOperation::CreateDir, &path))
    }

    async fn create_symlink(
        &self,
        parent: DirId<Self::Nid>,
        name: &OsStr,
        _target: &Path,
    ) -> Result<NodeId<Self::Nid>> {
        let path = self.node_path_of(parent.as_node_id())?.join(name);
        Err(self.read_only_error(FsOperation::CreateSymlink, &path))
    }

    async fn write_node(
        &self,
        id: FileId<Self::Nid>,
        _data: Bytes,
    ) -> Result<()> {
        let path = self.node_path_of(id.as_node_id())?;
        Err(self.read_only_error(FsOperation::Write, &path))
    }

    async fn flush_node(&self, id: FileId<Self::Nid>) -> Result<()> {
        let path = self.node_path_of(id.as_node_id())?;
        Err(self.read_only_error(FsOperation::Flush, &path))
    }

    async fn remove_node(&self, id: NodeId<Self::Nid>) -> Result<()> {
        let path = self.node_path_of(id)?;
        Err(self.read_only_error(FsOperation::Remove, &path))
    }

    async fn remove_all_node(&self, id: NodeId<Self::Nid>) -> Result<()> {
        let path = self.node_path_of(id)?;
        Err(self.read_only_error(FsOperation::Remove, &path))
    }

    async fn rename_node(
        &self,
        id: NodeId<Self::Nid>,
        _new_name: &OsStr,
    ) -> Result<()> {
        let path = self.node_path_of(id)?;
        Err(self.read_only_error(FsOperation::Rename, &path))
    }

    async fn copy_node(
        &self,
        src: NodeId<Self::Nid>,
        _dst: DirId<Self::Nid>,
    ) -> Result<NodeId<Self::Nid>> {
        let path = self.node_path_of(src)?;
        Err(self.read_only_error(FsOperation::Copy, &path))
    }

    async fn move_node(
        &self,
        src: NodeId<Self::Nid>,
        _dst: DirId<Self::Nid>,
    ) -> Result<NodeId<Self::Nid>> {
        let path = self.node_path_of(src)?;
        Err(self.read_only_error(FsOperation::Move, &path))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use ::zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};
    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    /// The timestamp every sample entry carries.
    fn sample_time() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 6).unwrap()
    }

    /// Build a small archive: two top-level files, a nested directory, and
    /// an explicit empty directory entry.
    fn sample_zip(dir: &Path) -> PathBuf {
        let path = dir.join("archive.zip");
        let file = fs_err::File::create(&path).unwrap();
        let mut zw = ZipWriter::new(file);
        let opts = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(
                ::zip::DateTime::from_date_and_time(2026, 1, 2, 3, 4, 6)
                    .unwrap(),
            );
        zw.start_file("hello.txt", opts).unwrap();
        zw.write_all(b"hello zip").unwrap();
        zw.start_file("docs/readme.md", opts).unwrap();
        zw.write_all(b"# readme").unwrap();
        zw.start_file("docs/nested/deep.txt", opts).unwrap();
        zw.write_all(b"deep content").unwrap();
        zw.add_directory("empty/", opts).unwrap();
        zw.finish().unwrap();
        path
    }

    fn sample_fs(dir: &Path) -> ZipFs {
        ZipFs::open(sample_zip(dir)).unwrap()
    }

    async fn file_id(fs: &ZipFs, path: &str) -> FileId<u64> {
        let id = fs.resolve_path(Path::new(path)).await.unwrap();
        FileId::new(*id.get())
    }

    #[tokio::test]
    async fn resolve_and_read_file() {
        let dir = tempdir().unwrap();
        let fs = sample_fs(dir.path());
        let id = file_id(&fs, "/hello.txt").await;
        let data = fs.read_node(id, None).await.unwrap();
        assert_eq!(data.as_ref(), b"hello zip");
    }

    #[tokio::test]
    async fn metadata_size_is_dir_and_modified() {
        let dir = tempdir().unwrap();
        let fs = sample_fs(dir.path());

        let id = fs.resolve_path(Path::new("/hello.txt")).await.unwrap();
        let meta = fs.node_metadata(id).unwrap();
        assert_eq!(meta.size, 9);
        assert!(!meta.is_dir);
        assert_eq!(meta.modified, sample_time());

        // `docs` has no explicit entry: it is synthesized from prefixes.
        let id = fs.resolve_path(Path::new("/docs")).await.unwrap();
        let meta = fs.node_metadata(id).unwrap();
        assert!(meta.is_dir);

        // `empty` is an explicit directory entry.
        let id = fs.resolve_path(Path::new("/empty")).await.unwrap();
        let meta = fs.node_metadata(id).unwrap();
        assert!(meta.is_dir);
        assert_eq!(meta.size, 0);
        assert_eq!(meta.modified, sample_time());
    }

    #[tokio::test]
    async fn read_dir_root_lists_top_level_entries() {
        let dir = tempdir().unwrap();
        let fs = sample_fs(dir.path());
        let id = fs.resolve_path(Path::new("/")).await.unwrap();
        let dir_id = DirId::new(*id.get());
        fs.read_dir_node(dir_id).await.unwrap();
        let node = fs.get_node(id).unwrap();
        let NodeKind::Directory { children } = node.kind() else {
            panic!("root must be a directory");
        };
        let children = children.as_deref().unwrap();
        assert_eq!(children, ["docs", "empty", "hello.txt"]);
    }

    #[tokio::test]
    async fn read_nested_dir_and_file() {
        let dir = tempdir().unwrap();
        let fs = sample_fs(dir.path());
        let id = fs.resolve_path(Path::new("/docs")).await.unwrap();
        fs.read_dir_node(DirId::new(*id.get())).await.unwrap();
        let node = fs.get_node(id).unwrap();
        let NodeKind::Directory { children } = node.kind() else {
            panic!("docs must be a directory");
        };
        assert_eq!(children.as_deref().unwrap(), ["nested", "readme.md"]);

        let id = fs.resolve_path(Path::new("/docs/nested/deep.txt")).await;
        let data = fs
            .read_node(FileId::new(*id.unwrap().get()), None)
            .await
            .unwrap();
        assert_eq!(data.as_ref(), b"deep content");
    }

    #[tokio::test]
    async fn resolve_relative_paths_works() {
        let dir = tempdir().unwrap();
        let fs = sample_fs(dir.path());
        // Both `docs/readme.md` and `/docs/readme.md` address the entry.
        let rel = fs.resolve_path(Path::new("docs/readme.md")).await.unwrap();
        let abs = fs.resolve_path(Path::new("/docs/readme.md")).await.unwrap();
        assert_eq!(rel, abs);
    }

    #[tokio::test]
    async fn missing_entry_is_not_found() {
        let dir = tempdir().unwrap();
        let fs = sample_fs(dir.path());
        let err = fs
            .resolve_path(Path::new("/missing.txt"))
            .await
            .unwrap_err();
        assert!(matches!(err, FsError::NotFound { .. }));
    }

    #[tokio::test]
    async fn read_range_is_clamped_and_validated() {
        let dir = tempdir().unwrap();
        let fs = sample_fs(dir.path());
        let id = file_id(&fs, "/hello.txt").await;

        let data = fs.read_node(id, Some(6..9)).await.unwrap();
        assert_eq!(data.as_ref(), b"zip");
        // End beyond EOF is clamped.
        let data = fs.read_node(id, Some(6..1000)).await.unwrap();
        assert_eq!(data.as_ref(), b"zip");
        // Start beyond EOF yields an empty read.
        let data = fs.read_node(id, Some(100..200)).await.unwrap();
        assert!(data.is_empty());
        // Inverted ranges are rejected.
        let err = fs.read_node(id, Some(5..5)).await.unwrap_err();
        assert!(matches!(err, FsError::InvalidArgument { .. }));
    }

    #[tokio::test]
    async fn open_node_reads_through_handle() {
        let dir = tempdir().unwrap();
        let fs = sample_fs(dir.path());
        let id = file_id(&fs, "/docs/readme.md").await;
        let mut handle = fs.open_node(id, OpenMode::Read).await.unwrap();
        assert_eq!(handle.metadata().size, 8);
        let mut buf = Vec::new();
        handle.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, b"# readme");
        // Writing through the handle fails.
        let err = match handle.write_all(b"nope").await {
            Ok(()) => panic!("writing to a read-only handle must fail"),
            Err(e) => e,
        };
        assert!(err.to_string().contains("read-only"));
    }

    #[tokio::test]
    async fn open_node_write_mode_is_refused() {
        let dir = tempdir().unwrap();
        let fs = sample_fs(dir.path());
        let id = file_id(&fs, "/hello.txt").await;
        let err = match fs.open_node(id, OpenMode::Write).await {
            Ok(_) => panic!("write mode must be refused"),
            Err(e) => e,
        };
        match err {
            FsError::Io { message, .. } => {
                assert!(message.contains("read-only"));
            }
            other => panic!("expected an Io read-only error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_stream_yields_content() {
        use futures::StreamExt;
        let dir = tempdir().unwrap();
        let fs = sample_fs(dir.path());
        let id = file_id(&fs, "/hello.txt").await;
        let mut s = fs.read_stream_node(id, None).await.unwrap();
        let mut out = Vec::new();
        while let Some(chunk) = s.next().await {
            out.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(out, b"hello zip");
    }

    #[tokio::test]
    async fn mutating_ops_are_refused() {
        let dir = tempdir().unwrap();
        let fs = sample_fs(dir.path());
        let root = fs.resolve_path(Path::new("/")).await.unwrap();
        let root_dir = DirId::new(*root.get());

        let err = fs
            .create_file(root_dir, OsStr::new("new.txt"))
            .await
            .unwrap_err();
        match err {
            FsError::Io {
                operation, message, ..
            } => {
                assert_eq!(operation, FsOperation::CreateFile);
                assert!(message.contains("read-only"));
            }
            other => panic!("expected an Io read-only error, got {other:?}"),
        }
        let err = fs.remove_node(root).await.unwrap_err();
        assert!(matches!(err, FsError::Io { .. }));
        let hello = fs.resolve_path(Path::new("/hello.txt")).await.unwrap();
        let err = fs
            .rename_node(hello, OsStr::new("x.txt"))
            .await
            .unwrap_err();
        assert!(matches!(err, FsError::Io { .. }));
    }

    #[tokio::test]
    async fn set_node_hash_caches_on_node() {
        let dir = tempdir().unwrap();
        let fs = sample_fs(dir.path());
        let id = fs.resolve_path(Path::new("/hello.txt")).await.unwrap();
        fs.set_node_hash(id, "abc123".into()).unwrap();
        assert_eq!(fs.get_node(id).unwrap().cached_hash(), Some("abc123"));
    }

    #[tokio::test]
    async fn resolve_symlink_is_wrong_kind() {
        let dir = tempdir().unwrap();
        let fs = sample_fs(dir.path());
        let id = fs.resolve_path(Path::new("/hello.txt")).await.unwrap();
        let err = fs.resolve_symlink(id).await.unwrap_err();
        assert!(matches!(
            err,
            FsError::WrongKind {
                expected: "symlink",
                actual: "file"
            }
        ));
    }

    #[test]
    fn label_and_deterministic_id() {
        let dir = tempdir().unwrap();
        let path = sample_zip(dir.path());
        let fs1 = ZipFs::with_label("left", &path).unwrap();
        let fs2 = ZipFs::open(&path).unwrap();
        assert_eq!(fs1.label(), "left");
        assert_eq!(fs1.id(), fs2.id());
        assert_eq!(fs1.archive_path(), fs_err::canonicalize(&path).unwrap());
    }

    #[test]
    fn open_missing_archive_fails() {
        let err = ZipFs::open("/nonexistent/archive.zip").unwrap_err();
        assert!(matches!(err, FsError::NotFound { .. }));
    }
}
